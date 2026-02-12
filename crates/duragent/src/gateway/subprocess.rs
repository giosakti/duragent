//! Subprocess gateway spawner and supervisor.
//!
//! This module handles spawning external gateway processes and supervising them
//! with restart policies, backoff, and proper cleanup when the parent dies.

use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use duragent_gateway_protocol::{GatewayCommand, GatewayEvent};

use crate::config::{ExternalGatewayConfig, RestartPolicy};

/// Supervisor for a subprocess gateway.
pub struct SubprocessGateway {
    config: ExternalGatewayConfig,
}

impl SubprocessGateway {
    /// Create a new subprocess gateway supervisor.
    pub fn new(config: ExternalGatewayConfig) -> Self {
        Self { config }
    }

    /// Run the gateway with supervision (restart on failure).
    ///
    /// This method spawns the subprocess and bridges its stdio to the provided channels.
    /// It will restart the process according to the configured restart policy.
    pub async fn run(
        self,
        evt_tx: mpsc::Sender<GatewayEvent>,
        mut cmd_rx: mpsc::Receiver<GatewayCommand>,
    ) {
        let mut attempts = 0u32;
        let mut backoff = Duration::from_secs(1);
        const MAX_ATTEMPTS: u32 = 5;
        const MAX_BACKOFF: Duration = Duration::from_secs(60);

        loop {
            attempts += 1;
            info!(
                gateway = %self.config.name,
                attempt = attempts,
                command = %self.config.command,
                "Spawning subprocess gateway"
            );

            let child = match self.spawn_child() {
                Ok(child) => child,
                Err(e) => {
                    error!(
                        gateway = %self.config.name,
                        error = %e,
                        "Failed to spawn subprocess"
                    );
                    if !self.should_restart(attempts, MAX_ATTEMPTS, false) {
                        let _ = evt_tx
                            .send(GatewayEvent::Error {
                                code: "spawn_failed".to_string(),
                                message: e.to_string(),
                                fatal: true,
                            })
                            .await;
                        return;
                    }
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
            };

            // Run the subprocess and bridge stdio
            let (exit_success, got_ready) = self.run_subprocess(child, &evt_tx, &mut cmd_rx).await;

            // Reset backoff on successful Ready event
            if got_ready {
                attempts = 0;
                backoff = Duration::from_secs(1);
            }

            // Check restart policy
            if !self.should_restart(attempts, MAX_ATTEMPTS, exit_success) {
                info!(gateway = %self.config.name, "Subprocess gateway stopped");
                let _ = evt_tx
                    .send(GatewayEvent::Shutdown {
                        reason: "subprocess exited".to_string(),
                    })
                    .await;
                return;
            }

            warn!(
                gateway = %self.config.name,
                backoff_secs = backoff.as_secs(),
                "Restarting subprocess gateway"
            );
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }

    /// Spawn the child process with proper configuration.
    fn spawn_child(&self) -> std::io::Result<Child> {
        let mut cmd = Command::new(&self.config.command);
        cmd.args(&self.config.args)
            .envs(&self.config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);

        // On Linux, set PR_SET_PDEATHSIG to ensure child dies when parent dies
        #[cfg(target_os = "linux")]
        unsafe {
            cmd.pre_exec(|| {
                // PR_SET_PDEATHSIG = 1, SIGTERM = 15
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        cmd.spawn()
    }

    /// Run the subprocess and bridge stdio to channels.
    ///
    /// Returns (exit_success, got_ready).
    async fn run_subprocess(
        &self,
        mut child: Child,
        evt_tx: &mpsc::Sender<GatewayEvent>,
        cmd_rx: &mut mpsc::Receiver<GatewayCommand>,
    ) -> (bool, bool) {
        let stdin = child.stdin.take().expect("stdin should be piped");
        let stdout = child.stdout.take().expect("stdout should be piped");

        let mut stdin = stdin;
        let mut stdout_reader = BufReader::new(stdout);

        let mut got_ready = false;
        let gateway_name = self.config.name.clone();

        loop {
            tokio::select! {
                // Read events from subprocess stdout
                line = read_bounded_line(&mut stdout_reader) => {
                    match line {
                        Ok(Some(line)) => {
                            match serde_json::from_str::<GatewayEvent>(&line) {
                                Ok(event) => {
                                    if matches!(event, GatewayEvent::Ready { .. }) {
                                        got_ready = true;
                                    }
                                    if matches!(event, GatewayEvent::Shutdown { .. }) {
                                        let _ = evt_tx.send(event).await;
                                        break;
                                    }
                                    if evt_tx.send(event).await.is_err() {
                                        debug!(gateway = %gateway_name, "Event channel closed");
                                        break;
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        gateway = %gateway_name,
                                        line = %line,
                                        error = %e,
                                        "Failed to parse gateway event"
                                    );
                                }
                            }
                        }
                        Ok(None) => {
                            // EOF - subprocess closed stdout
                            debug!(gateway = %gateway_name, "Subprocess stdout closed");
                            break;
                        }
                        Err(e) => {
                            if e.kind() == std::io::ErrorKind::InvalidData {
                                warn!(
                                    gateway = %gateway_name,
                                    error = %e,
                                    "Skipping oversized line from subprocess"
                                );
                                continue;
                            }
                            error!(gateway = %gateway_name, error = %e, "Error reading stdout");
                            break;
                        }
                    }
                }

                // Write commands to subprocess stdin
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(command) => {
                            let is_shutdown = matches!(command, GatewayCommand::Shutdown);
                            match serde_json::to_string(&command) {
                                Ok(json) => {
                                    let line = format!("{}\n", json);
                                    if let Err(e) = stdin.write_all(line.as_bytes()).await {
                                        error!(
                                            gateway = %gateway_name,
                                            error = %e,
                                            "Failed to write to subprocess stdin"
                                        );
                                        break;
                                    }
                                    if let Err(e) = stdin.flush().await {
                                        error!(
                                            gateway = %gateway_name,
                                            error = %e,
                                            "Failed to flush subprocess stdin"
                                        );
                                        break;
                                    }
                                    if is_shutdown {
                                        // Wait briefly for graceful shutdown
                                        tokio::time::sleep(Duration::from_millis(500)).await;
                                        break;
                                    }
                                }
                                Err(e) => {
                                    error!(
                                        gateway = %gateway_name,
                                        error = %e,
                                        "Failed to serialize command"
                                    );
                                }
                            }
                        }
                        None => {
                            // Command channel closed, shutdown subprocess
                            debug!(gateway = %gateway_name, "Command channel closed");
                            break;
                        }
                    }
                }

                // Check if child process exited
                status = child.wait() => {
                    match status {
                        Ok(status) => {
                            info!(
                                gateway = %gateway_name,
                                status = %status,
                                "Subprocess exited"
                            );
                            return (status.success(), got_ready);
                        }
                        Err(e) => {
                            error!(
                                gateway = %gateway_name,
                                error = %e,
                                "Error waiting for subprocess"
                            );
                            return (false, got_ready);
                        }
                    }
                }
            }
        }

        // Clean up: kill the child if still running
        let _ = child.kill().await;
        let status = child.wait().await;
        let exit_success = status.map(|s| s.success()).unwrap_or(false);

        (exit_success, got_ready)
    }

    /// Check if the subprocess should be restarted based on policy.
    fn should_restart(&self, attempts: u32, max_attempts: u32, exit_success: bool) -> bool {
        if attempts >= max_attempts {
            error!(
                gateway = %self.config.name,
                attempts = attempts,
                "Max restart attempts reached"
            );
            return false;
        }

        match self.config.restart {
            RestartPolicy::Always => true,
            RestartPolicy::OnFailure => !exit_success,
            RestartPolicy::Never => false,
        }
    }
}

/// Maximum size of a single JSON line from a gateway subprocess (1 MB).
const MAX_LINE_BYTES: usize = 1024 * 1024;

/// Read a newline-terminated line from an async buffered reader, bounded to `MAX_LINE_BYTES`.
///
/// Returns `Ok(None)` on EOF, `Ok(Some(line))` on success.
/// Lines exceeding the limit are drained and return `ErrorKind::InvalidData`.
async fn read_bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<String>> {
    let mut buf = Vec::with_capacity(4096);

    loop {
        let (found_newline, consume_n) = {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                return Ok(if buf.is_empty() {
                    None
                } else {
                    Some(String::from_utf8_lossy(&buf).into_owned())
                });
            }

            match available.iter().position(|&b| b == b'\n') {
                Some(pos) => {
                    buf.extend_from_slice(&available[..pos]);
                    (true, pos + 1)
                }
                None => {
                    buf.extend_from_slice(available);
                    (false, available.len())
                }
            }
        };

        Pin::new(&mut *reader).consume(consume_n);

        if buf.len() > MAX_LINE_BYTES {
            if !found_newline {
                drain_line(reader).await;
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("gateway line exceeds {} byte limit", MAX_LINE_BYTES),
            ));
        }

        if found_newline {
            // Strip trailing \r (Windows line endings)
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
            return Ok(Some(String::from_utf8_lossy(&buf).into_owned()));
        }
    }
}

/// Drain bytes from the reader until a newline is found or EOF.
async fn drain_line<R: AsyncBufRead + Unpin>(reader: &mut R) {
    loop {
        let (consume_n, done) = {
            let Ok(available) = reader.fill_buf().await else {
                return;
            };
            if available.is_empty() {
                return;
            }
            match available.iter().position(|&b| b == b'\n') {
                Some(pos) => (pos + 1, true),
                None => (available.len(), false),
            }
        };
        Pin::new(&mut *reader).consume(consume_n);
        if done {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_config(restart: RestartPolicy) -> ExternalGatewayConfig {
        ExternalGatewayConfig {
            name: "test".to_string(),
            command: "echo".to_string(),
            args: vec!["hello".to_string()],
            env: HashMap::new(),
            restart,
        }
    }

    #[test]
    fn test_should_restart_never() {
        let gateway = SubprocessGateway::new(test_config(RestartPolicy::Never));
        assert!(!gateway.should_restart(1, 5, true));
        assert!(!gateway.should_restart(1, 5, false));
    }

    #[test]
    fn test_should_restart_always() {
        let gateway = SubprocessGateway::new(test_config(RestartPolicy::Always));
        assert!(gateway.should_restart(1, 5, true));
        assert!(gateway.should_restart(1, 5, false));
        assert!(!gateway.should_restart(5, 5, true)); // Max attempts
    }

    #[test]
    fn test_should_restart_on_failure() {
        let gateway = SubprocessGateway::new(test_config(RestartPolicy::OnFailure));
        assert!(!gateway.should_restart(1, 5, true)); // Success = no restart
        assert!(gateway.should_restart(1, 5, false)); // Failure = restart
        assert!(!gateway.should_restart(5, 5, false)); // Max attempts
    }

    #[tokio::test]
    async fn bounded_line_reads_normal_line() {
        let data = b"hello world\n";
        let mut reader = BufReader::new(&data[..]);
        let line = read_bounded_line(&mut reader).await.unwrap();
        assert_eq!(line, Some("hello world".to_string()));
    }

    #[tokio::test]
    async fn bounded_line_returns_none_on_eof() {
        let data = b"";
        let mut reader = BufReader::new(&data[..]);
        let line = read_bounded_line(&mut reader).await.unwrap();
        assert_eq!(line, None);
    }

    #[tokio::test]
    async fn bounded_line_reads_multiple_lines() {
        let data = b"line1\nline2\nline3\n";
        let mut reader = BufReader::new(&data[..]);

        assert_eq!(
            read_bounded_line(&mut reader).await.unwrap(),
            Some("line1".to_string())
        );
        assert_eq!(
            read_bounded_line(&mut reader).await.unwrap(),
            Some("line2".to_string())
        );
        assert_eq!(
            read_bounded_line(&mut reader).await.unwrap(),
            Some("line3".to_string())
        );
        assert_eq!(read_bounded_line(&mut reader).await.unwrap(), None);
    }

    #[tokio::test]
    async fn bounded_line_strips_carriage_return() {
        let data = b"hello\r\n";
        let mut reader = BufReader::new(&data[..]);
        let line = read_bounded_line(&mut reader).await.unwrap();
        assert_eq!(line, Some("hello".to_string()));
    }

    #[tokio::test]
    async fn bounded_line_returns_unterminated_line_at_eof() {
        let data = b"no newline";
        let mut reader = BufReader::new(&data[..]);
        let line = read_bounded_line(&mut reader).await.unwrap();
        assert_eq!(line, Some("no newline".to_string()));
    }

    #[tokio::test]
    async fn bounded_line_rejects_oversized_line() {
        // Create a line that exceeds MAX_LINE_BYTES
        let mut data = vec![b'x'; MAX_LINE_BYTES + 100];
        data.push(b'\n');
        data.extend_from_slice(b"next line\n");

        let mut reader = BufReader::new(&data[..]);

        let err = read_bounded_line(&mut reader).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        // The oversized line was drained — next read gets the following line
        let line = read_bounded_line(&mut reader).await.unwrap();
        assert_eq!(line, Some("next line".to_string()));
    }
}
