//! Shared interactive CLI helpers for chat and attach commands.

use std::io::{Write, stdout};

use anyhow::Result;
use futures::StreamExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use agnx::client::{AgentClient, ClientStreamEvent};

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
                stream_response(stream).await?;
                println!();
            }
            Err(e) => {
                eprintln!("Error: {}", e);
            }
        }
    }

    Ok(())
}

/// Stream response tokens to stdout.
pub async fn stream_response(
    mut stream: impl futures::Stream<Item = agnx::client::Result<ClientStreamEvent>> + Unpin,
) -> Result<()> {
    let mut sync_stdout = stdout();

    while let Some(event) = stream.next().await {
        match event {
            Ok(ClientStreamEvent::Token { content }) => {
                write!(sync_stdout, "{}", content)?;
                sync_stdout.flush()?;
            }
            Ok(ClientStreamEvent::Done { .. }) => {
                break;
            }
            Ok(ClientStreamEvent::Cancelled) => {
                println!("\n[cancelled]");
                break;
            }
            Ok(ClientStreamEvent::Error { message }) => {
                eprintln!("\nError: {}", message);
                break;
            }
            Ok(ClientStreamEvent::Start) => {}
            Err(e) => {
                eprintln!("\nStream error: {}", e);
                break;
            }
        }
    }

    Ok(())
}
