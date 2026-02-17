//! Interactive CLI REPL for Duragent.
//!
//! Provides a reedline-based interactive loop for chatting with agents,
//! including line editing, persistent history, and approval prompts.

use std::io::{Write, stdout};
use std::path::PathBuf;

use anyhow::Result;
use futures::StreamExt;
use reedline::{DefaultPrompt, DefaultPromptSegment, FileBackedHistory, Reedline, Signal};

use duragent_client::client::{
    AgentClient, ApprovalDecision, ApproveCommandResponse, ClientStreamEvent,
};

/// Maximum number of history entries persisted to disk.
const HISTORY_SIZE: usize = 1000;

/// Result of streaming a response.
pub enum StreamResult {
    /// Stream completed normally.
    Done,
    /// Approval is required for a command.
    ApprovalRequired { call_id: String, command: String },
}

/// Run the interactive chat loop using the HTTP client with reedline.
pub async fn run_interactive_loop(client: &AgentClient, session_id: &str) -> Result<()> {
    let mut editor = create_editor()?;
    let mut prompt = DefaultPrompt::new(
        DefaultPromptSegment::Basic("> ".to_string()),
        DefaultPromptSegment::Empty,
    );

    loop {
        // reedline is sync — run in a blocking task.
        // Both editor and prompt must be moved in and returned back.
        let (returned_editor, returned_prompt, signal) = tokio::task::spawn_blocking(move || {
            let sig = editor.read_line(&prompt);
            (editor, prompt, sig)
        })
        .await?;
        editor = returned_editor;
        prompt = returned_prompt;

        match signal {
            Ok(Signal::Success(input)) => {
                let input = input.trim();
                if input.is_empty() {
                    continue;
                }

                match input {
                    "/exit" | "/quit" => break,
                    "/help" => {
                        print_help();
                        continue;
                    }
                    _ => {}
                }

                match client.stream_message(session_id, input).await {
                    Ok(stream) => {
                        println!();
                        match stream_response(stream).await? {
                            StreamResult::Done => {}
                            StreamResult::ApprovalRequired { call_id, command } => {
                                handle_approval_loop(client, session_id, call_id, command).await?;
                            }
                        }
                        println!();
                    }
                    Err(e) => {
                        eprintln!("Error: {}", e);
                    }
                }
            }
            Ok(Signal::CtrlD) | Ok(Signal::CtrlC) => {
                println!();
                break;
            }
            Err(e) => {
                eprintln!("Input error: {}", e);
                break;
            }
        }
    }

    Ok(())
}

/// Stream response tokens to stdout.
pub async fn stream_response(
    mut stream: impl futures::Stream<Item = duragent_client::client::Result<ClientStreamEvent>> + Unpin,
) -> Result<StreamResult> {
    let mut sync_stdout = stdout();

    while let Some(event) = stream.next().await {
        match event {
            Ok(ClientStreamEvent::Token { content }) => {
                write!(sync_stdout, "{}", content)?;
                sync_stdout.flush()?;
            }
            Ok(ClientStreamEvent::Done { .. }) => {
                return Ok(StreamResult::Done);
            }
            Ok(ClientStreamEvent::Cancelled) => {
                println!("\n[cancelled]");
                return Ok(StreamResult::Done);
            }
            Ok(ClientStreamEvent::Error { message }) => {
                eprintln!("\nError: {}", message);
                return Ok(StreamResult::Done);
            }
            Ok(ClientStreamEvent::ApprovalRequired { call_id, command }) => {
                return Ok(StreamResult::ApprovalRequired { call_id, command });
            }
            Ok(ClientStreamEvent::Start) => {}
            Err(e) => {
                eprintln!("\nStream error: {}", e);
                return Ok(StreamResult::Done);
            }
        }
    }

    Ok(StreamResult::Done)
}

/// Prompt user for approval decision.
pub async fn prompt_approval(command: &str) -> Result<ApprovalDecision> {
    println!();
    println!("┌─────────────────────────────────────────────────────────────┐");
    println!("│ Approval required for command:                              │");
    println!("│ {:<60}│", truncate_str(command, 60));
    println!("│                                                             │");
    println!("│ [y] Allow Once  [a] Allow Always  [n] Deny                  │");
    println!("└─────────────────────────────────────────────────────────────┘");

    // Use reedline for the approval prompt too
    let mut editor = Reedline::create();
    let prompt = DefaultPrompt::new(
        DefaultPromptSegment::Basic("Your choice: ".to_string()),
        DefaultPromptSegment::Empty,
    );

    let (_, signal) = tokio::task::spawn_blocking(move || {
        let sig = editor.read_line(&prompt);
        (editor, sig)
    })
    .await?;

    match signal {
        Ok(Signal::Success(input)) => {
            let input = input.trim().to_lowercase();
            match input.as_str() {
                "y" | "yes" | "once" => Ok(ApprovalDecision::AllowOnce),
                "a" | "always" | "allow" => Ok(ApprovalDecision::AllowAlways),
                "n" | "no" | "deny" => Ok(ApprovalDecision::Deny),
                _ => {
                    println!("Invalid choice, defaulting to deny.");
                    Ok(ApprovalDecision::Deny)
                }
            }
        }
        _ => Ok(ApprovalDecision::Deny),
    }
}

/// Handle the approval loop, which may require multiple approvals.
async fn handle_approval_loop(
    client: &AgentClient,
    session_id: &str,
    mut call_id: String,
    mut command: String,
) -> Result<()> {
    loop {
        let decision = prompt_approval(&command).await?;
        match client
            .approve_command(session_id, &call_id, &command, decision)
            .await
        {
            Ok(response) => match response {
                ApproveCommandResponse::Complete { content, .. } => {
                    println!();
                    println!("{}", content);
                    return Ok(());
                }
                ApproveCommandResponse::PendingApproval {
                    call_id: new_call_id,
                    command: new_command,
                } => {
                    call_id = new_call_id;
                    command = new_command;
                }
            },
            Err(e) => {
                let error_msg = e.to_string();
                if error_msg.contains("expired")
                    || error_msg.contains("no pending approval")
                    || error_msg.contains("not found")
                {
                    eprintln!("This approval request has expired. Please send a new message.");
                } else {
                    eprintln!("Failed to record approval: {}", e);
                }
                return Ok(());
            }
        }
    }
}

/// Create a reedline editor with file-backed history.
fn create_editor() -> Result<Reedline> {
    let history_path = history_file_path();

    // Ensure parent directory exists
    if let Some(parent) = history_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let history = Box::new(FileBackedHistory::with_file(HISTORY_SIZE, history_path)?);
    let editor = Reedline::create().with_history(history);

    Ok(editor)
}

/// Path to the history file (~/.duragent/history).
fn history_file_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".duragent").join("history")
}

/// Truncate a string to max character length with ellipsis.
fn truncate_str(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else if max_chars <= 3 {
        ".".repeat(max_chars)
    } else {
        let truncated: String = s.chars().take(max_chars - 3).collect();
        format!("{}...", truncated)
    }
}

/// Print available slash commands.
fn print_help() {
    println!("Available commands:");
    println!("  /help   - Show this help");
    println!("  /exit   - Detach from session");
    println!("  /quit   - Detach from session");
    println!();
    println!("Keys:");
    println!("  Ctrl+C  - Detach from session");
    println!("  Ctrl+D  - Detach from session");
    println!("  Up/Down - Navigate history");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn truncate_str_exact() {
        assert_eq!(truncate_str("hello", 5), "hello");
    }

    #[test]
    fn truncate_str_long() {
        assert_eq!(truncate_str("hello world", 5), "he...");
    }

    #[test]
    fn truncate_str_multibyte() {
        // Ensure no panic on multi-byte UTF-8
        assert_eq!(truncate_str("日本語テスト", 4), "日...");
    }
}
