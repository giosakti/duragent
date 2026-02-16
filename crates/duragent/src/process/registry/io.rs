use std::io::SeekFrom;
use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader};

use crate::process::{ProcessError, ProcessMeta, ProcessRegistryHandle};

use super::STDIN_WRITE_TIMEOUT_SECS;

impl ProcessRegistryHandle {
    /// List all processes for a given session.
    pub fn list_by_session(&self, session_id: &str) -> Vec<ProcessMeta> {
        self.entries
            .iter()
            .filter(|entry| entry.meta.session_id == session_id)
            .map(|entry| entry.meta.clone())
            .collect()
    }

    /// Get status of a specific process.
    pub fn get_status(
        &self,
        handle_id: &str,
        session_id: &str,
    ) -> Result<ProcessMeta, ProcessError> {
        let entry = self
            .entries
            .get(handle_id)
            .ok_or_else(|| ProcessError::NotFound(handle_id.to_string()))?;
        if entry.meta.session_id != session_id {
            return Err(ProcessError::WrongSession);
        }
        Ok(entry.meta.clone())
    }

    /// Read log output for a process.
    pub async fn read_log(
        &self,
        handle_id: &str,
        session_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<String, ProcessError> {
        let entry = self
            .entries
            .get(handle_id)
            .ok_or_else(|| ProcessError::NotFound(handle_id.to_string()))?;
        if entry.meta.session_id != session_id {
            return Err(ProcessError::WrongSession);
        }
        let log_path = entry.meta.log_path.clone();
        drop(entry);

        let file = tokio::fs::File::open(&log_path)
            .await
            .map_err(ProcessError::Io)?;
        let mut lines = BufReader::new(file).lines();
        let mut index = 0usize;
        let mut collected = 0usize;
        let mut selected = String::new();

        while let Some(line) = lines.next_line().await.map_err(ProcessError::Io)? {
            if index >= offset {
                if collected >= limit {
                    break;
                }
                if !selected.is_empty() {
                    selected.push('\n');
                }
                selected.push_str(&line);
                collected += 1;
            }
            index += 1;
        }

        Ok(selected)
    }

    /// Capture interactive (tmux) pane content.
    pub async fn capture(&self, handle_id: &str, session_id: &str) -> Result<String, ProcessError> {
        let entry = self
            .entries
            .get(handle_id)
            .ok_or_else(|| ProcessError::NotFound(handle_id.to_string()))?;
        if entry.meta.session_id != session_id {
            return Err(ProcessError::WrongSession);
        }
        let backend = self
            .backends
            .get(entry.meta.backend)
            .ok_or(ProcessError::TmuxUnavailable)?;
        let tmux_session = entry.meta.tmux_session.clone();
        drop(entry);

        backend.capture(tmux_session.as_deref()).await
    }

    /// Send keystrokes to an interactive (tmux) process.
    pub async fn send_keys(
        &self,
        handle_id: &str,
        session_id: &str,
        keys: &str,
        press_enter: bool,
    ) -> Result<(), ProcessError> {
        let entry = self
            .entries
            .get(handle_id)
            .ok_or_else(|| ProcessError::NotFound(handle_id.to_string()))?;
        if entry.meta.session_id != session_id {
            return Err(ProcessError::WrongSession);
        }
        if entry.meta.status.is_terminal() {
            return Err(ProcessError::NotRunning);
        }
        let backend = self
            .backends
            .get(entry.meta.backend)
            .ok_or(ProcessError::TmuxUnavailable)?;
        let tmux_session = entry.meta.tmux_session.clone();
        drop(entry);

        backend
            .send_keys(tmux_session.as_deref(), keys, press_enter)
            .await
    }

    /// Write text to a process.
    ///
    /// For tmux processes: uses `send_literal` to type the text into the pane
    /// (preserves spaces, sends Enter if the input ends with newline).
    /// For non-tmux processes: writes directly to stdin.
    pub async fn write_input(
        &self,
        handle_id: &str,
        session_id: &str,
        input: &str,
    ) -> Result<(), ProcessError> {
        let entry = self
            .entries
            .get(handle_id)
            .ok_or_else(|| ProcessError::NotFound(handle_id.to_string()))?;
        if entry.meta.session_id != session_id {
            return Err(ProcessError::WrongSession);
        }
        if entry.meta.status.is_terminal() {
            return Err(ProcessError::NotRunning);
        }
        let is_tmux = entry.meta.tmux_session.clone();
        drop(entry);

        // Interpret escape sequences so LLM-generated \n works as documented
        let input = input.replace("\\n", "\n");

        if let Some(tmux_session) = is_tmux {
            // For tmux processes, type the text literally.
            // Enter is only pressed when the input explicitly ends with '\n'.
            let press_enter = input.ends_with('\n');
            let text = input.trim_end_matches('\n');
            super::tmux::send_literal(&tmux_session, text, press_enter)
                .await
                .map_err(ProcessError::Io)
        } else {
            self.write_stdin_inner(handle_id, &input).await
        }
    }

    /// Write directly to a non-tmux process's stdin pipe.
    async fn write_stdin_inner(&self, handle_id: &str, input: &str) -> Result<(), ProcessError> {
        // Serialize concurrent writes to the same process stdin
        let lock = self.stdin_locks.get(handle_id);
        let _guard = lock.lock().await;

        // Take stdin out of the entry to avoid holding DashMap lock across await
        let mut stdin_handle = {
            let mut entry = self
                .entries
                .get_mut(handle_id)
                .ok_or_else(|| ProcessError::NotFound(handle_id.to_string()))?;
            if entry.meta.status.is_terminal() {
                return Err(ProcessError::NotRunning);
            }
            entry.stdin.take().ok_or(ProcessError::NoStdin)?
            // DashMap RefMut dropped here
        };

        use tokio::io::AsyncWriteExt;
        let write_result =
            tokio::time::timeout(Duration::from_secs(STDIN_WRITE_TIMEOUT_SECS), async {
                stdin_handle
                    .write_all(input.as_bytes())
                    .await
                    .map_err(ProcessError::Io)?;
                stdin_handle.flush().await.map_err(ProcessError::Io)?;
                Ok::<(), ProcessError>(())
            })
            .await;

        let result = match write_result {
            Ok(res) => res,
            Err(_) => Err(ProcessError::StdinTimeout),
        };

        // Return stdin handle to the entry
        if let Some(mut entry) = self.entries.get_mut(handle_id) {
            entry.stdin = Some(stdin_handle);
        }

        result
    }

    pub(super) async fn read_log_tail(
        path: &Path,
        max_bytes: usize,
    ) -> std::io::Result<(String, bool)> {
        let mut file = tokio::fs::File::open(path).await?;
        let metadata = file.metadata().await?;
        let len = metadata.len();
        let start = len.saturating_sub(max_bytes as u64);
        let truncated = start > 0;

        file.seek(SeekFrom::Start(start)).await?;
        let mut buf = Vec::with_capacity((len - start) as usize);
        file.read_to_end(&mut buf).await?;
        let mut content = String::from_utf8_lossy(&buf).to_string();

        if truncated && let Some(pos) = content.find('\n') {
            content = content[(pos + 1)..].to_string();
        }

        Ok((content, truncated))
    }
}
