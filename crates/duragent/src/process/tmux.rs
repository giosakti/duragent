//! tmux integration helpers.
//!
//! All functions shell out via `tokio::process::Command`.

use std::path::Path;

use tokio::process::Command;
use tracing::{debug, warn};

/// Check if tmux is available on the system.
pub async fn detect_tmux() -> bool {
    match Command::new("tmux").arg("-V").output().await {
        Ok(output) => {
            if output.status.success() {
                let version = String::from_utf8_lossy(&output.stdout);
                debug!(version = %version.trim(), "tmux detected");
                true
            } else {
                debug!("tmux -V returned non-zero");
                false
            }
        }
        Err(_) => {
            debug!("tmux not found in PATH");
            false
        }
    }
}

/// Create a new tmux session running the given command.
///
/// The command is wrapped to tee output to a log file and record the exit code.
pub async fn create_session(
    name: &str,
    command: &str,
    log_path: &Path,
    cwd: Option<&str>,
) -> std::io::Result<()> {
    let log_str = log_path.to_string_lossy();
    // Escape characters special inside double quotes ($, `, \, ") so that
    // workspace paths containing these chars aren't interpreted by the shell.
    let escaped_log = log_str
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`");
    // Wrap command to capture output and exit code.
    // pipefail ensures $? reflects the command's exit code, not tee's.
    let wrapped = format!(
        "bash -c 'set -o pipefail; {} 2>&1 | tee \"{}\"; echo EXIT_CODE:$? >> \"{}\"'",
        command.replace('\'', "'\\''"),
        escaped_log,
        escaped_log,
    );

    let mut cmd = Command::new("tmux");
    cmd.args(["new-session", "-d", "-s", name, &wrapped]);

    if let Some(dir) = cwd {
        cmd.args(["-c", dir]);
    }

    let output = cmd.output().await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(std::io::Error::other(format!(
            "tmux new-session failed: {}",
            stderr.trim()
        )));
    }

    Ok(())
}

/// Check if a tmux session exists.
pub async fn has_session(name: &str) -> bool {
    match Command::new("tmux")
        .args(["has-session", "-t", name])
        .output()
        .await
    {
        Ok(output) => output.status.success(),
        Err(_) => false,
    }
}

/// Capture the current content of a tmux pane.
pub async fn capture_pane(name: &str) -> std::io::Result<String> {
    let output = Command::new("tmux")
        .args(["capture-pane", "-t", name, "-p"])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(std::io::Error::other(format!(
            "tmux capture-pane failed: {}",
            stderr.trim()
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Send keystrokes to a tmux session.
///
/// If `press_enter` is true, an Enter keystroke is appended after `keys`.
pub async fn send_keys(name: &str, keys: &str, press_enter: bool) -> std::io::Result<()> {
    let mut args = vec!["send-keys", "-t", name, keys];
    if press_enter {
        args.push("Enter");
    }
    let output = Command::new("tmux").args(&args).output().await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(std::io::Error::other(format!(
            "tmux send-keys failed: {}",
            stderr.trim()
        )));
    }

    Ok(())
}

/// Kill a tmux session.
pub async fn kill_session(name: &str) -> std::io::Result<()> {
    let output = Command::new("tmux")
        .args(["kill-session", "-t", name])
        .output()
        .await;

    match output {
        Ok(o) if !o.status.success() => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            warn!(session = %name, error = %stderr.trim(), "tmux kill-session failed");
        }
        Err(e) => {
            warn!(session = %name, error = %e, "tmux kill-session failed");
        }
        _ => {}
    }

    Ok(())
}

/// Parse exit code from the last line of a log file.
///
/// Looks for `EXIT_CODE:<n>` pattern written by our tmux wrapper.
pub fn parse_exit_code_from_log(content: &str) -> Option<i32> {
    for line in content.lines().rev() {
        if let Some(code_str) = line.strip_prefix("EXIT_CODE:")
            && let Ok(code) = code_str.trim().parse::<i32>()
        {
            return Some(code);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_exit_code_success() {
        let log = "some output\nmore output\nEXIT_CODE:0\n";
        assert_eq!(parse_exit_code_from_log(log), Some(0));
    }

    #[test]
    fn parse_exit_code_failure() {
        let log = "error output\nEXIT_CODE:1\n";
        assert_eq!(parse_exit_code_from_log(log), Some(1));
    }

    #[test]
    fn parse_exit_code_missing() {
        let log = "some output\nno exit code here\n";
        assert_eq!(parse_exit_code_from_log(log), None);
    }

    #[test]
    fn parse_exit_code_multiple_takes_last() {
        let log = "EXIT_CODE:0\nmore output\nEXIT_CODE:1\n";
        assert_eq!(parse_exit_code_from_log(log), Some(1));
    }
}
