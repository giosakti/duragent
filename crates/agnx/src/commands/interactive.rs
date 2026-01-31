//! Shared interactive CLI helpers for chat and attach commands.

use std::io::{Write, stdout};

use anyhow::Result;
use futures::StreamExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use agnx::client::{AgentClient, ApprovalDecision, ApproveCommandResponse, ClientStreamEvent};

/// Run the interactive chat loop using the HTTP client.
pub async fn run_interactive_loop(client: &AgentClient, session_id: &str) -> Result<()> {
    let stdin = BufReader::new(tokio::io::stdin());
    let mut async_stdout = tokio::io::stdout();
    let mut lines = stdin.lines();

    loop {
        async_stdout.write_all(b"> ").await?;
        async_stdout.flush().await?;

        let Some(input) = lines.next_line().await? else {
            println!();
            break;
        };

        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        if input == "/exit" || input == "/quit" {
            break;
        }

        // Stream the response
        match client.stream_message(session_id, input).await {
            Ok(stream) => {
                println!();
                match stream_response(stream).await? {
                    StreamResult::Done => {}
                    StreamResult::ApprovalRequired { call_id, command } => {
                        // Handle approval flow (may loop if multiple tools need approval)
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

    Ok(())
}

/// Result of streaming a response.
pub enum StreamResult {
    /// Stream completed normally.
    Done,
    /// Approval is required for a command.
    ApprovalRequired { call_id: String, command: String },
}

/// Stream response tokens to stdout.
pub async fn stream_response(
    mut stream: impl futures::Stream<Item = agnx::client::Result<ClientStreamEvent>> + Unpin,
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
    print!("Your choice: ");
    stdout().flush()?;

    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    if let Some(input) = lines.next_line().await? {
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
    } else {
        Ok(ApprovalDecision::Deny)
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
                    // Display the agent's response
                    println!();
                    println!("{}", content);
                    return Ok(());
                }
                ApproveCommandResponse::PendingApproval {
                    call_id: new_call_id,
                    command: new_command,
                } => {
                    // Another tool needs approval, loop again
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

/// Truncate a string to max length with ellipsis.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}
