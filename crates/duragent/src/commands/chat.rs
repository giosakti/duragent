//! Interactive chat command implementation.

use std::path::Path;

use anyhow::{Context, Result};

use duragent::config::Config;
use duragent::launcher::{LaunchOptions, ensure_server_running};

use super::interactive::run_interactive_loop;

pub async fn run(
    agent_name: &str,
    config_path: &str,
    agents_dir_override: Option<&Path>,
    server_url: Option<&str>,
) -> Result<()> {
    super::check_workspace(config_path)?;
    let config = Config::load(config_path).await?;

    // Get client (auto-starts server if needed)
    let client = ensure_server_running(LaunchOptions {
        server_url,
        config_path: Path::new(config_path),
        config: &config,
        agents_dir: agents_dir_override,
    })
    .await
    .context("Failed to connect to server")?;

    // Get agent info to display model details
    let agent = client
        .get_agent(agent_name)
        .await
        .with_context(|| format!("Failed to get agent '{}'", agent_name))?;

    // Create a new session
    let session = client
        .create_session(agent_name)
        .await
        .context("Failed to create session")?;

    println!("Chat with {} (Ctrl+C or /exit to detach)", agent_name);
    println!(
        "Model: {} via {}",
        agent.spec.model.name, agent.spec.model.provider
    );
    println!("Session: {}", session.session_id);
    println!();

    run_interactive_loop(&client, &session.session_id).await?;

    println!(
        "Session saved. Reattach with: duragent attach {}",
        session.session_id
    );

    Ok(())
}
