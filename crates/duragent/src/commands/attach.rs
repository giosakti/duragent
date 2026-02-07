//! Attach command for reconnecting to existing sessions.

use std::path::Path;

use anyhow::{Context, Result};

use duragent::client::SessionStatus;
use duragent::config::Config;
use duragent::launcher::{LaunchOptions, ensure_server_running};

use super::interactive::run_interactive_loop;

/// List attachable sessions from the server.
pub async fn list(
    config_path: &str,
    agents_dir_override: Option<&Path>,
    server_url: Option<&str>,
) -> Result<()> {
    let config = Config::load(config_path).await?;

    let client = ensure_server_running(LaunchOptions {
        server_url,
        config_path: Path::new(config_path),
        config: &config,
        agents_dir: agents_dir_override,
    })
    .await
    .context("Failed to connect to server")?;

    let sessions = client.list_sessions().await?;

    // Filter to only show attachable sessions (not completed)
    let attachable: Vec<_> = sessions
        .into_iter()
        .filter(|s| s.status != SessionStatus::Completed)
        .collect();

    if attachable.is_empty() {
        println!("No attachable sessions found.");
        return Ok(());
    }

    println!("Attachable sessions:");
    println!();
    println!("{:<40} {:<20} {:<10}", "SESSION ID", "AGENT", "STATUS");
    println!("{:-<40} {:-<20} {:-<10}", "", "", "");

    for session in &attachable {
        println!(
            "{:<40} {:<20} {:<10}",
            session.session_id, session.agent, session.status
        );
    }

    Ok(())
}

/// Attach to an existing session and resume interactive chat.
pub async fn run(
    session_id: &str,
    config_path: &str,
    agents_dir_override: Option<&Path>,
    server_url: Option<&str>,
) -> Result<()> {
    let config = Config::load(config_path).await?;

    let client = ensure_server_running(LaunchOptions {
        server_url,
        config_path: Path::new(config_path),
        config: &config,
        agents_dir: agents_dir_override,
    })
    .await
    .context("Failed to connect to server")?;

    // Get session info
    let session = client
        .get_session(session_id)
        .await
        .with_context(|| format!("Failed to get session '{}'", session_id))?;

    // Check if session is attachable
    if session.status == SessionStatus::Completed {
        anyhow::bail!("Session '{}' has already completed", session_id);
    }

    // Get agent info for display
    let agent = client.get_agent(&session.agent).await?;

    // Get conversation history
    let messages = client.get_messages(session_id, None).await?;

    println!("Attached to session {} (Ctrl+C to exit)", session_id);
    println!(
        "Agent: {} | Model: {} via {}",
        session.agent, agent.spec.model.name, agent.spec.model.provider
    );
    println!(
        "Status: {} | Messages in history: {}",
        session.status,
        messages.len()
    );
    println!();

    // Show recent conversation context
    if !messages.is_empty() {
        println!("--- Recent conversation ---");
        let show_count = messages.len().min(4);
        for msg in messages
            .iter()
            .skip(messages.len().saturating_sub(show_count))
        {
            let prefix = match msg.role.as_str() {
                "user" => ">",
                "assistant" => "<",
                "system" => "[sys]",
                _ => "?",
            };
            // Show first 100 chars of each message
            let content = truncate_str(&msg.content, 100);
            println!("{} {}", prefix, content.replace('\n', " "));
        }
        println!("---------------------------");
        println!();
    }

    run_interactive_loop(&client, session_id).await?;

    Ok(())
}

/// Safely truncate a string to at most `max_chars` characters.
fn truncate_str(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max_chars).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_str_exact() {
        assert_eq!(truncate_str("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_str_long() {
        assert_eq!(truncate_str("hello world", 5), "hello...");
    }
}
