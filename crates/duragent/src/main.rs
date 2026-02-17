mod commands;

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::LazyLock;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

static LONG_VERSION: LazyLock<String> = LazyLock::new(duragent::build_info::version_string);

// ============================================================================
// CLI Types
// ============================================================================

/// Duragent - A minimal and fast self-hosted runtime for durable and portable AI agents
#[derive(Parser, Debug)]
#[command(version = LONG_VERSION.as_str(), about, long_about = None)]
struct Cli {
    /// Increase log verbosity (-v = debug, -vv = trace)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Suppress all log output (errors only)
    #[arg(short, long, global = true)]
    quiet: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Manage agents
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },

    /// Attach to an existing session
    #[cfg(feature = "cli")]
    Attach {
        /// Session ID to attach to (omit to list attachable sessions)
        #[arg(value_name = "SESSION_ID")]
        session_id: Option<String>,

        /// List all attachable sessions
        #[arg(short, long)]
        list: bool,

        /// Path to configuration file
        #[arg(short, long, default_value = "duragent.yaml")]
        config: String,

        /// Agents directory (overrides config file)
        #[arg(long)]
        agents_dir: Option<PathBuf>,

        /// Connect to a specific server URL instead of auto-starting
        #[arg(short, long)]
        server: Option<String>,
    },

    /// Start an interactive chat session with an agent
    #[cfg(feature = "cli")]
    Chat {
        /// Name of the agent to chat with
        #[arg(short, long)]
        agent: String,

        /// Path to configuration file
        #[arg(short, long, default_value = "duragent.yaml")]
        config: String,

        /// Agents directory (overrides config file)
        #[arg(long)]
        agents_dir: Option<PathBuf>,

        /// Connect to a specific server URL instead of auto-starting
        #[arg(short, long)]
        server: Option<String>,
    },

    /// Diagnose installation and configuration issues
    Doctor {
        /// Path to configuration file
        #[arg(short, long, default_value = "duragent.yaml")]
        config: String,

        /// Output format (text or json)
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Initialize a new Duragent workspace
    Init {
        /// Directory to initialize (defaults to current directory)
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Name for the starter agent
        #[arg(long)]
        agent_name: Option<String>,

        /// LLM provider [anthropic, openrouter, openai, ollama]
        #[arg(long)]
        provider: Option<String>,

        /// Model name — format depends on provider (e.g., "claude-sonnet-4-20250514"
        /// for anthropic, "anthropic/claude-sonnet-4" for openrouter)
        #[arg(long)]
        model: Option<String>,

        /// Skip interactive prompts; use defaults for missing flags
        #[arg(long)]
        no_interactive: bool,
    },

    /// Authenticate with an LLM provider
    Login {
        /// Provider to authenticate with (e.g., "anthropic")
        provider: String,
    },

    /// Upgrade duragent to the latest version
    Upgrade {
        /// Only check for updates, don't install
        #[arg(long)]
        check: bool,

        /// Target version (e.g., "0.6.0" or "v0.6.0")
        #[arg(long)]
        version: Option<String>,

        /// Download full variant (includes gateway binaries)
        #[arg(long, conflicts_with = "core")]
        full: bool,

        /// Download core variant only (no gateway binaries)
        #[arg(long, conflicts_with = "full")]
        core: bool,

        /// Restart a running server after upgrade
        #[arg(long)]
        restart: bool,

        /// Path to configuration file (used with --restart)
        #[arg(short, long, default_value = "duragent.yaml")]
        config: String,

        /// Port override (used with --restart)
        #[arg(short, long)]
        port: Option<u16>,

        /// Output format (text or json)
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Manage the HTTP server
    Serve {
        #[command(subcommand)]
        action: Option<ServeAction>,

        /// Path to configuration file
        #[arg(short, long, default_value = "duragent.yaml", global = true)]
        config: String,

        /// Host to bind to (overrides config file)
        #[arg(long, global = true)]
        host: Option<IpAddr>,

        /// Port to listen on (overrides config file)
        #[arg(short, long, global = true)]
        port: Option<u16>,

        /// Agents directory (overrides config file). If relative, it is resolved relative to the config file directory.
        #[arg(long, global = true)]
        agents_dir: Option<PathBuf>,

        /// Auto-shutdown after N seconds with no active sessions (used by launcher)
        #[arg(long, value_name = "SECONDS")]
        ephemeral: Option<u64>,
    },
}

// Sub-enums for nested subcommands (kept adjacent to Commands)

#[derive(Subcommand, Debug)]
enum AgentAction {
    /// Create a new agent in the current workspace
    Create {
        /// Name for the new agent
        name: String,

        /// LLM provider [anthropic, openrouter, openai, ollama]
        #[arg(long)]
        provider: Option<String>,

        /// Model name
        #[arg(long)]
        model: Option<String>,

        /// Path to configuration file
        #[arg(short, long, default_value = "duragent.yaml")]
        config: String,

        /// Skip interactive prompts; use defaults for missing flags
        #[arg(long)]
        no_interactive: bool,
    },
}

#[derive(Subcommand, Debug)]
enum ServeAction {
    /// Stop a running server
    Stop,
    /// Reload agent configurations from disk
    ReloadAgents,
}

// ============================================================================
// Entry Point
// ============================================================================

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose, cli.quiet);

    match run(&cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(cli: &Cli) -> Result<()> {
    match &cli.command {
        Commands::Agent { action } => match action {
            AgentAction::Create {
                name,
                provider,
                model,
                config,
                no_interactive,
            } => {
                commands::agent::create(
                    config,
                    name,
                    provider.clone(),
                    model.clone(),
                    *no_interactive,
                )
                .await
            }
        },
        #[cfg(feature = "cli")]
        Commands::Attach {
            session_id,
            list,
            config,
            agents_dir,
            server,
        } => match session_id {
            Some(id) if !list => {
                commands::attach::run(id, config, agents_dir.as_deref(), server.as_deref()).await
            }
            _ => commands::attach::list(config, agents_dir.as_deref(), server.as_deref()).await,
        },
        #[cfg(feature = "cli")]
        Commands::Chat {
            agent,
            config,
            agents_dir,
            server,
        } => commands::chat::run(agent, config, agents_dir.as_deref(), server.as_deref()).await,
        Commands::Doctor { config, format } => commands::doctor::run(config, format).await,
        Commands::Init {
            path,
            agent_name,
            provider,
            model,
            no_interactive,
        } => {
            commands::init::run(
                path,
                agent_name.clone(),
                provider.clone(),
                model.clone(),
                *no_interactive,
            )
            .await
        }
        Commands::Login { provider } => commands::login::run(provider).await,
        Commands::Upgrade {
            check,
            version,
            full,
            core,
            restart,
            config,
            port,
            format,
        } => {
            commands::upgrade::run(commands::upgrade::UpgradeOpts {
                check_only: *check,
                target_version: version.clone(),
                full: *full,
                core: *core,
                restart: *restart,
                config_path: config,
                port: *port,
                format,
            })
            .await
        }
        Commands::Serve {
            action,
            config,
            host,
            port,
            agents_dir,
            ephemeral,
        } => match action {
            Some(ServeAction::Stop) => commands::serve::stop(config, *port).await,
            Some(ServeAction::ReloadAgents) => commands::serve::reload_agents(config, *port).await,
            None => {
                commands::serve::run(config, *host, *port, agents_dir.as_deref(), *ephemeral).await
            }
        },
    }
}

// ============================================================================
// Initialization
// ============================================================================

fn init_tracing(verbose: u8, quiet: bool) {
    let default_level = if quiet {
        "error"
    } else {
        match verbose {
            0 => "info",
            1 => "debug",
            _ => "trace",
        }
    };

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}
