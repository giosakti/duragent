//! `duragent doctor` — diagnose installation and configuration issues.

use std::collections::HashSet;
use std::net::IpAddr;
use std::path::Path;

use anyhow::{Result, bail};
use serde::Serialize;

use duragent::auth::AuthStorage;
use duragent::config::{self, Config, ConfigError};
use duragent::llm::Provider;
use duragent::store::file::FileAgentCatalog;
use duragent::store::{AgentCatalog, ScanWarning};

// ============================================================================
// Report Types
// ============================================================================

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum CheckStatus {
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Serialize)]
struct CheckResult {
    status: CheckStatus,
    message: String,
}

#[derive(Debug, Serialize)]
struct Section {
    name: String,
    checks: Vec<CheckResult>,
}

#[derive(Debug, Serialize)]
struct Summary {
    ok: usize,
    warn: usize,
    error: usize,
}

#[derive(Debug, Serialize)]
struct Report {
    status: CheckStatus,
    sections: Vec<Section>,
    summary: Summary,
}

impl Report {
    fn from_sections(sections: Vec<Section>) -> Self {
        let mut ok = 0;
        let mut warn = 0;
        let mut error = 0;
        for section in &sections {
            for check in &section.checks {
                match check.status {
                    CheckStatus::Ok => ok += 1,
                    CheckStatus::Warn => warn += 1,
                    CheckStatus::Error => error += 1,
                }
            }
        }
        let status = if error > 0 {
            CheckStatus::Error
        } else if warn > 0 {
            CheckStatus::Warn
        } else {
            CheckStatus::Ok
        };
        Report {
            status,
            sections,
            summary: Summary { ok, warn, error },
        }
    }

    fn render(&self, format: &str) -> Result<()> {
        match format {
            "json" => {
                println!("{}", serde_json::to_string_pretty(self)?);
            }
            _ => self.render_text(),
        }
        Ok(())
    }

    fn render_text(&self) {
        println!("Duragent Doctor");
        println!("{}", "=".repeat(50));

        for section in &self.sections {
            if section.checks.is_empty() {
                continue;
            }
            println!();
            println!("{}", section.name);
            println!("{}", "-".repeat(section.name.len()));
            for check in &section.checks {
                let label = match check.status {
                    CheckStatus::Ok => "  OK   ",
                    CheckStatus::Warn => "  WARN ",
                    CheckStatus::Error => "  ERROR",
                };
                println!("{} {}", label, check.message);
            }
        }

        println!();
        let status_label = match self.status {
            CheckStatus::Ok => "PASS",
            CheckStatus::Warn => "PASS (with warnings)",
            CheckStatus::Error => "FAIL",
        };
        println!(
            "{}: {} ok, {} warning(s), {} error(s)",
            status_label, self.summary.ok, self.summary.warn, self.summary.error,
        );
    }
}

// ============================================================================
// Entry Point
// ============================================================================

pub async fn run(config_path: &str, format: &str) -> Result<()> {
    let mut sections = Vec::new();

    let config = check_config(&mut sections, config_path).await;
    if let Some(config) = config {
        check_agents(&mut sections, &config, config_path).await;
        check_gateways(&mut sections, &config);
        check_security(&mut sections, &config);
    }

    let report = Report::from_sections(sections);
    report.render(format)?;

    if report.summary.error > 0 {
        bail!("{} error(s) found", report.summary.error);
    }
    Ok(())
}

// ============================================================================
// Check: Configuration
// ============================================================================

async fn check_config(sections: &mut Vec<Section>, config_path: &str) -> Option<Config> {
    let mut checks = Vec::new();
    let path = Path::new(config_path);

    // Check config file exists
    if !path.exists() {
        checks.push(CheckResult {
            status: CheckStatus::Warn,
            message: format!("Config file '{}' not found, using defaults", config_path),
        });
    } else {
        checks.push(CheckResult {
            status: CheckStatus::Ok,
            message: format!("Config file '{}' found", config_path),
        });
    }

    // Try loading config
    let config = match Config::load(config_path).await {
        Ok(c) => c,
        Err(e) => {
            let message = match &e {
                ConfigError::Yaml(_) => format!("Invalid YAML: {e}"),
                ConfigError::MissingEnvVar(var) => {
                    format!("Environment variable '{var}' is not set")
                }
                _ => format!("Failed to load config: {e}"),
            };
            checks.push(CheckResult {
                status: CheckStatus::Error,
                message,
            });
            sections.push(Section {
                name: "Configuration".to_string(),
                checks,
            });
            return None;
        }
    };

    // Resolve and check workspace directory
    let config_path_ref = Path::new(config_path);
    let workspace_raw = config
        .workspace
        .as_deref()
        .unwrap_or(Path::new(config::DEFAULT_WORKSPACE));
    let workspace = config::resolve_path(config_path_ref, workspace_raw);

    if workspace.exists() {
        checks.push(CheckResult {
            status: CheckStatus::Ok,
            message: format!("Workspace directory '{}'", workspace.display()),
        });
    } else {
        checks.push(CheckResult {
            status: CheckStatus::Error,
            message: format!(
                "Workspace directory '{}' not found (run `duragent init`)",
                workspace.display(),
            ),
        });
    }

    // Resolve and check agents directory
    let agents_dir = config
        .agents_dir
        .as_ref()
        .map(|p| config::resolve_path(config_path_ref, p))
        .unwrap_or_else(|| workspace.join(config::DEFAULT_AGENTS_DIR));

    if agents_dir.exists() {
        checks.push(CheckResult {
            status: CheckStatus::Ok,
            message: format!("Agents directory '{}'", agents_dir.display()),
        });
    } else {
        checks.push(CheckResult {
            status: CheckStatus::Warn,
            message: format!("Agents directory '{}' not found", agents_dir.display()),
        });
    }

    // Parse server host
    if config.server.host.parse::<IpAddr>().is_err() {
        checks.push(CheckResult {
            status: CheckStatus::Error,
            message: format!(
                "Invalid server host '{}' (must be a valid IP address)",
                config.server.host,
            ),
        });
    }

    sections.push(Section {
        name: "Configuration".to_string(),
        checks,
    });
    Some(config)
}

// ============================================================================
// Check: Agents
// ============================================================================

async fn check_agents(sections: &mut Vec<Section>, config: &Config, config_path: &str) {
    let mut checks = Vec::new();

    let config_path_ref = Path::new(config_path);
    let workspace_raw = config
        .workspace
        .as_deref()
        .unwrap_or(Path::new(config::DEFAULT_WORKSPACE));
    let workspace = config::resolve_path(config_path_ref, workspace_raw);

    let agents_dir = config
        .agents_dir
        .as_ref()
        .map(|p| config::resolve_path(config_path_ref, p))
        .unwrap_or_else(|| workspace.join(config::DEFAULT_AGENTS_DIR));

    let catalog = FileAgentCatalog::new(&agents_dir, Some(workspace));
    let scan = match catalog.load_all().await {
        Ok(s) => s,
        Err(e) => {
            checks.push(CheckResult {
                status: CheckStatus::Error,
                message: format!("Failed to scan agents: {e}"),
            });
            sections.push(Section {
                name: "Agents".to_string(),
                checks,
            });
            return;
        }
    };

    // Map scan warnings
    for warning in &scan.warnings {
        match warning {
            ScanWarning::AgentsDirMissing { path } => {
                checks.push(CheckResult {
                    status: CheckStatus::Error,
                    message: format!("Agents directory missing: {path}"),
                });
            }
            ScanWarning::InvalidAgent { name, error } => {
                checks.push(CheckResult {
                    status: CheckStatus::Error,
                    message: format!("Invalid agent '{name}': {error}"),
                });
            }
            ScanWarning::MissingResource { agent, resource } => {
                checks.push(CheckResult {
                    status: CheckStatus::Warn,
                    message: format!("Agent '{agent}' missing resource: {resource}"),
                });
            }
        }
    }

    // Check each loaded agent
    let mut checked_providers: HashSet<Provider> = HashSet::new();
    for agent in &scan.agents {
        let provider = &agent.model.provider;
        checks.push(CheckResult {
            status: CheckStatus::Ok,
            message: format!(
                "Agent '{}' (provider: {})",
                agent.metadata.name,
                provider.as_str(),
            ),
        });

        // Check provider credentials
        if checked_providers.insert(provider.clone())
            && let Some(cred_check) = check_provider_credentials(provider)
        {
            checks.push(cred_check);
        }
    }

    if scan.agents.is_empty() && scan.warnings.is_empty() {
        checks.push(CheckResult {
            status: CheckStatus::Warn,
            message: "No agents found".to_string(),
        });
    }

    sections.push(Section {
        name: "Agents".to_string(),
        checks,
    });
}

fn check_provider_credentials(provider: &Provider) -> Option<CheckResult> {
    match provider {
        Provider::Anthropic => {
            if std::env::var("ANTHROPIC_API_KEY").is_ok() {
                return None; // env var present, all good
            }
            // Check OAuth credentials file
            let auth_path = AuthStorage::default_path();
            if let Ok(storage) = AuthStorage::load(&auth_path)
                && storage.get_anthropic().is_some()
            {
                return None; // OAuth present
            }
            Some(CheckResult {
                status: CheckStatus::Error,
                message: "Anthropic: ANTHROPIC_API_KEY not set and no OAuth credentials found"
                    .to_string(),
            })
        }
        Provider::OpenAI => {
            if std::env::var("OPENAI_API_KEY").is_ok() {
                return None;
            }
            Some(CheckResult {
                status: CheckStatus::Error,
                message: "OpenAI: OPENAI_API_KEY not set".to_string(),
            })
        }
        Provider::OpenRouter => {
            if std::env::var("OPENROUTER_API_KEY").is_ok() {
                return None;
            }
            Some(CheckResult {
                status: CheckStatus::Error,
                message: "OpenRouter: OPENROUTER_API_KEY not set".to_string(),
            })
        }
        Provider::Ollama => None, // No credentials needed
        Provider::Other(name) => Some(CheckResult {
            status: CheckStatus::Warn,
            message: format!("Unknown provider '{}': cannot verify credentials", name,),
        }),
    }
}

// ============================================================================
// Check: Gateways
// ============================================================================

fn check_gateways(sections: &mut Vec<Section>, config: &Config) {
    let gw = &config.gateways;
    if gw.discord.is_none() && gw.telegram.is_none() && gw.external.is_empty() {
        return; // No gateways configured, skip section
    }

    let mut checks = Vec::new();

    if let Some(discord) = &gw.discord {
        if discord.enabled {
            checks.push(CheckResult {
                status: CheckStatus::Ok,
                message: "Discord gateway configured".to_string(),
            });
        } else {
            checks.push(CheckResult {
                status: CheckStatus::Ok,
                message: "Discord gateway disabled".to_string(),
            });
        }
    }

    if let Some(telegram) = &gw.telegram {
        if telegram.enabled {
            checks.push(CheckResult {
                status: CheckStatus::Ok,
                message: "Telegram gateway configured".to_string(),
            });
        } else {
            checks.push(CheckResult {
                status: CheckStatus::Ok,
                message: "Telegram gateway disabled".to_string(),
            });
        }
    }

    for ext in &gw.external {
        let command_exists = which_command(&ext.command);
        if command_exists {
            checks.push(CheckResult {
                status: CheckStatus::Ok,
                message: format!(
                    "External gateway '{}': command '{}' found",
                    ext.name, ext.command
                ),
            });
        } else {
            checks.push(CheckResult {
                status: CheckStatus::Error,
                message: format!(
                    "External gateway '{}': command '{}' not found in PATH",
                    ext.name, ext.command,
                ),
            });
        }
    }

    sections.push(Section {
        name: "Gateways".to_string(),
        checks,
    });
}

fn which_command(command: &str) -> bool {
    // If command is an absolute/relative path, check directly
    let path = Path::new(command);
    if path.is_absolute() || command.contains('/') {
        return path.exists();
    }
    // Otherwise search PATH
    std::process::Command::new("which")
        .arg(command)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ============================================================================
// Check: Security
// ============================================================================

fn check_security(sections: &mut Vec<Section>, config: &Config) {
    let mut checks = Vec::new();

    let is_public = config.server.host == "0.0.0.0";
    let has_admin_token = config.server.admin_token.is_some();
    let has_api_token = config.server.api_token.is_some();

    if is_public && !has_admin_token && !has_api_token {
        checks.push(CheckResult {
            status: CheckStatus::Warn,
            message: "Server binds to 0.0.0.0 with no admin_token or api_token set".to_string(),
        });
    }

    if let Some(token) = &config.server.admin_token
        && token.len() < 16
    {
        checks.push(CheckResult {
            status: CheckStatus::Warn,
            message: "admin_token is shorter than 16 characters".to_string(),
        });
    }

    if let Some(token) = &config.server.api_token
        && token.len() < 16
    {
        checks.push(CheckResult {
            status: CheckStatus::Warn,
            message: "api_token is shorter than 16 characters".to_string(),
        });
    }

    sections.push(Section {
        name: "Security".to_string(),
        checks,
    });
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(msg: &str) -> CheckResult {
        CheckResult {
            status: CheckStatus::Ok,
            message: msg.to_string(),
        }
    }

    fn warn(msg: &str) -> CheckResult {
        CheckResult {
            status: CheckStatus::Warn,
            message: msg.to_string(),
        }
    }

    fn err(msg: &str) -> CheckResult {
        CheckResult {
            status: CheckStatus::Error,
            message: msg.to_string(),
        }
    }

    #[test]
    fn report_no_errors_succeeds() {
        let sections = vec![Section {
            name: "Test".to_string(),
            checks: vec![ok("all good"), warn("minor issue")],
        }];
        let report = Report::from_sections(sections);
        assert!(matches!(report.status, CheckStatus::Warn));
        assert_eq!(report.summary.ok, 1);
        assert_eq!(report.summary.warn, 1);
        assert_eq!(report.summary.error, 0);
    }

    #[test]
    fn report_with_errors_has_error_status() {
        let sections = vec![Section {
            name: "Test".to_string(),
            checks: vec![ok("fine"), err("broken")],
        }];
        let report = Report::from_sections(sections);
        assert!(matches!(report.status, CheckStatus::Error));
        assert_eq!(report.summary.error, 1);
    }

    #[test]
    fn report_all_ok() {
        let sections = vec![Section {
            name: "Test".to_string(),
            checks: vec![ok("a"), ok("b")],
        }];
        let report = Report::from_sections(sections);
        assert!(matches!(report.status, CheckStatus::Ok));
    }

    #[test]
    fn report_json_rendering() {
        let sections = vec![Section {
            name: "Config".to_string(),
            checks: vec![ok("file found"), warn("missing dir")],
        }];
        let report = Report::from_sections(sections);
        let json = serde_json::to_string_pretty(&report).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["status"], "warn");
        assert_eq!(parsed["summary"]["ok"], 1);
        assert_eq!(parsed["summary"]["warn"], 1);
        assert_eq!(parsed["summary"]["error"], 0);
        assert_eq!(parsed["sections"][0]["name"], "Config");
        assert_eq!(parsed["sections"][0]["checks"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn report_text_rendering() {
        let sections = vec![Section {
            name: "Config".to_string(),
            checks: vec![ok("file found"), err("bad host")],
        }];
        let report = Report::from_sections(sections);
        // Renders without panicking; visual check via manual run
        report.render_text();
    }

    #[test]
    fn empty_sections_omitted_in_text() {
        let sections = vec![
            Section {
                name: "Empty".to_string(),
                checks: vec![],
            },
            Section {
                name: "Has Checks".to_string(),
                checks: vec![ok("present")],
            },
        ];
        let report = Report::from_sections(sections);
        // Empty section should not cause any issues
        report.render_text();
        assert_eq!(report.summary.ok, 1);
    }
}
