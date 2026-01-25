//! Interactive chat command implementation.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use agnx::agent;
use agnx::config::Config;
use agnx::llm::{self, ChatRequest, Message, Role};

pub async fn run(
    agent_name: String,
    config_path: String,
    agents_dir_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = Config::load(&config_path)?;

    if let Some(dir) = agents_dir_override {
        config.agents_dir = dir;
    }

    // Load agents
    let agents_dir = agent::resolve_agents_dir(Path::new(&config_path), &config.agents_dir);
    let scan = agent::AgentStore::scan(&agents_dir);

    agent::log_scan_warnings(&scan.warnings);

    // Get the specified agent
    let Some(agent_spec) = scan.store.get(&agent_name) else {
        let available: Vec<_> = scan.store.iter().map(|(name, _)| name.as_str()).collect();
        let available_str = if available.is_empty() {
            "none".to_string()
        } else {
            available.join(", ")
        };
        return Err(format!(
            "Agent '{}' not found. Available agents: {}",
            agent_name, available_str
        )
        .into());
    };

    // Initialize LLM providers
    let providers = llm::ProviderRegistry::from_env();

    // Get the provider for this agent
    let Some(provider) = providers.get(&agent_spec.model.provider) else {
        return Err(format!(
            "Provider '{}' not configured. Set the appropriate API key environment variable.",
            agent_spec.model.provider
        )
        .into());
    };

    // Build system message
    let mut system_content = String::new();
    if let Some(ref prompt) = agent_spec.system_prompt {
        system_content.push_str(prompt);
    }
    if let Some(ref instructions) = agent_spec.instructions {
        if !system_content.is_empty() {
            system_content.push_str("\n\n");
        }
        system_content.push_str(instructions);
    }

    // Conversation history
    let mut messages: Vec<Message> = Vec::new();
    if !system_content.is_empty() {
        messages.push(Message {
            role: Role::System,
            content: system_content,
        });
    }

    println!("Chat with {} (Ctrl+C to exit)", agent_name);
    println!(
        "Model: {} via {}",
        agent_spec.model.name, agent_spec.model.provider
    );
    println!();

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        // Print prompt
        print!("> ");
        stdout.flush()?;

        // Read user input
        let mut input = String::new();
        if stdin.read_line(&mut input)? == 0 {
            // EOF
            println!();
            break;
        }

        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        // Handle exit commands
        if input == "/exit" || input == "/quit" {
            break;
        }

        // Add user message
        messages.push(Message {
            role: Role::User,
            content: input.to_string(),
        });

        // Build request
        let request = ChatRequest {
            model: agent_spec.model.name.clone(),
            messages: messages.clone(),
            temperature: agent_spec.model.temperature,
            max_tokens: agent_spec.model.max_output_tokens,
        };

        // Call LLM
        match provider.chat(request).await {
            Ok(response) => {
                let assistant_content = response
                    .choices
                    .first()
                    .map(|c| c.message.content.clone())
                    .unwrap_or_default();

                // Print response
                println!();
                println!("{}", assistant_content);
                println!();

                // Add to history
                messages.push(Message {
                    role: Role::Assistant,
                    content: assistant_content,
                });
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                // Remove the failed user message from history
                messages.pop();
            }
        }
    }

    Ok(())
}
