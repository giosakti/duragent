use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use tokio::fs;

use serde::Deserialize;
use thiserror::Error;

// ============================================================================
// Config (root)
// ============================================================================

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub agents_dir: Option<PathBuf>,
    #[serde(default)]
    pub services: ServicesConfig,
    #[serde(default)]
    pub world_memory: WorldMemoryConfig,
    #[serde(default)]
    pub gateways: GatewaysConfig,
    /// Global routing rules for agent selection (evaluated in order, first match wins).
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
    #[serde(default)]
    pub sandbox: SandboxConfig,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Io(#[from] std::io::Error),

    #[error("failed to parse config file: {0}")]
    Yaml(#[from] serde_saphyr::Error),

    #[error("environment variable '{0}' is not set")]
    MissingEnvVar(String),

    #[error("unclosed variable reference '${{' (missing '}}')")]
    UnclosedVarReference,
}

impl Config {
    pub async fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let contents = match fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(ConfigError::Io(e)),
        };
        let expanded = expand_env_vars(&contents)?;
        Ok(serde_saphyr::from_str(&expanded)?)
    }
}

/// Resolve a path relative to the config file directory.
///
/// If the path is absolute, it is returned as-is.
/// If the path is relative, it is joined with the config file's parent directory.
///
/// This ensures consistent behavior regardless of the current working directory.
pub fn resolve_path(config_path: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }

    let config_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    config_dir.join(path)
}

// ============================================================================
// Default Paths
// ============================================================================

/// Default workspace directory (relative to config file).
pub const DEFAULT_WORKSPACE: &str = ".duragent";
/// Default agents directory (relative to workspace).
pub const DEFAULT_AGENTS_DIR: &str = "agents";
/// Default sessions directory (relative to workspace).
pub const DEFAULT_SESSIONS_DIR: &str = "sessions";
/// Default world memory directory (relative to workspace).
pub const DEFAULT_WORLD_MEMORY_DIR: &str = "memory/world";
/// Default directives directory (relative to workspace).
pub const DEFAULT_DIRECTIVES_DIR: &str = "directives";

// ============================================================================
// Private Helpers (Serde Defaults)
// ============================================================================

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_request_timeout() -> u64 {
    300
}

fn default_idle_timeout() -> u64 {
    60
}

fn default_keep_alive_interval() -> u64 {
    15
}

/// Serde default for bool fields that should be `true` (serde's default is `false`).
fn default_true() -> bool {
    true
}

// ============================================================================
// Environment Variable Expansion
// ============================================================================

/// Expand environment variables in a string.
///
/// Supports the following syntax (shell-compatible):
/// - `${VAR}` - Required variable, errors if not set
/// - `${VAR:-default}` - Optional variable with default value
/// - `${VAR:-}` - Optional variable, empty string if not set
/// - `$$` - Escaped `$` (only needed before `{` to prevent expansion)
///
/// # Limitations
///
/// - No nested/recursive expansion: `${VAR:-${DEFAULT}}` is not supported
/// - Unclosed `${` (missing `}`) returns an error
///
/// # Examples
///
/// ```yaml
/// # Required - errors if TELEGRAM_BOT_TOKEN is not set
/// bot_token: ${TELEGRAM_BOT_TOKEN}
///
/// # Optional with default
/// host: ${HOST:-0.0.0.0}
/// port: ${PORT:-8080}
///
/// # Optional, empty if not set
/// api_key: ${OPTIONAL_KEY:-}
///
/// # Plain $ doesn't need escaping
/// price: $100
/// ```
fn expand_env_vars(input: &str) -> Result<String, ConfigError> {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '$' {
            match chars.peek() {
                // Escaped $ -> literal $
                Some('$') => {
                    chars.next();
                    result.push('$');
                }
                // Start of variable reference
                Some('{') => {
                    chars.next(); // consume '{'
                    let expanded = parse_var_reference(&mut chars)?;
                    result.push_str(&expanded);
                }
                // Not a variable reference, keep literal $
                _ => {
                    result.push('$');
                }
            }
        } else {
            result.push(c);
        }
    }

    Ok(result)
}

/// Parse a variable reference after seeing `${`.
///
/// Handles:
/// - `VAR}` - Required variable
/// - `VAR:-default}` - Variable with default
///
/// Returns error if closing `}` is missing.
fn parse_var_reference(
    chars: &mut std::iter::Peekable<std::str::Chars>,
) -> Result<String, ConfigError> {
    let mut var_name = String::new();
    let mut default_value: Option<String> = None;
    let mut in_default = false;
    let mut found_closing_brace = false;

    while let Some(&c) = chars.peek() {
        match c {
            '}' => {
                chars.next(); // consume '}'
                found_closing_brace = true;
                break;
            }
            ':' if !in_default => {
                chars.next(); // consume ':'
                // Check for '-' (default value syntax)
                if chars.peek() == Some(&'-') {
                    chars.next(); // consume '-'
                    in_default = true;
                    default_value = Some(String::new());
                } else {
                    // ':' without '-' is part of var name (unusual but valid)
                    var_name.push(':');
                }
            }
            _ => {
                chars.next();
                if in_default {
                    default_value.as_mut().unwrap().push(c);
                } else {
                    var_name.push(c);
                }
            }
        }
    }

    if !found_closing_brace {
        return Err(ConfigError::UnclosedVarReference);
    }

    // Look up the environment variable
    match std::env::var(&var_name) {
        Ok(value) => Ok(value),
        Err(_) => match default_value {
            Some(default) => Ok(default),
            None => Err(ConfigError::MissingEnvVar(var_name)),
        },
    }
}

// ============================================================================
// ServerConfig
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_request_timeout")]
    pub request_timeout_seconds: u64,
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_seconds: u64,
    #[serde(default = "default_keep_alive_interval")]
    pub keep_alive_interval_seconds: u64,
    /// Optional admin API token. If set, admin endpoints require this token.
    /// If not set, admin endpoints only accept requests from localhost.
    #[serde(default)]
    pub admin_token: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            request_timeout_seconds: default_request_timeout(),
            idle_timeout_seconds: default_idle_timeout(),
            keep_alive_interval_seconds: default_keep_alive_interval(),
            admin_token: None,
        }
    }
}

// ============================================================================
// ServicesConfig
// ============================================================================

#[derive(Debug, Default, Deserialize)]
pub struct ServicesConfig {
    #[serde(default)]
    pub session: SessionServiceConfig,
}

// ============================================================================
// SessionServiceConfig
// ============================================================================

#[derive(Debug, Default, Deserialize)]
pub struct SessionServiceConfig {
    #[serde(default)]
    pub path: Option<PathBuf>,
}

// ============================================================================
// WorldMemoryConfig
// ============================================================================

/// Configuration for shared world memory.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WorldMemoryConfig {
    #[serde(default)]
    pub path: Option<PathBuf>,
}

// ============================================================================
// GatewaysConfig
// ============================================================================

/// Configuration for all gateways.
#[derive(Debug, Default, Deserialize)]
pub struct GatewaysConfig {
    /// Discord gateway configuration.
    #[serde(default)]
    pub discord: Option<DiscordGatewayConfig>,

    /// Telegram gateway configuration.
    #[serde(default)]
    pub telegram: Option<TelegramGatewayConfig>,

    /// External gateway configurations.
    #[serde(default)]
    pub external: Vec<ExternalGatewayConfig>,
}

/// Configuration for the Discord gateway.
#[derive(Debug, Clone, Deserialize)]
pub struct DiscordGatewayConfig {
    /// Whether the gateway is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Discord bot token.
    pub bot_token: String,
}

/// Configuration for the Telegram gateway.
#[derive(Debug, Clone, Deserialize)]
pub struct TelegramGatewayConfig {
    /// Whether the gateway is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Telegram bot token from @BotFather.
    pub bot_token: String,
}

/// A routing rule that maps message context to an agent.
#[derive(Debug, Clone, Deserialize)]
pub struct RoutingRule {
    /// Conditions that must all match (AND logic).
    /// If empty/omitted, acts as a catch-all rule.
    #[serde(rename = "match", default)]
    pub match_conditions: RoutingMatch,

    /// Agent to use when this rule matches.
    pub agent: String,
}

/// Alias for RoutingRule used in global routes config.
pub type RouteConfig = RoutingRule;

/// Match conditions for routing rules.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RoutingMatch {
    /// Match by gateway name (e.g., "telegram", "discord").
    #[serde(default)]
    pub gateway: Option<String>,

    /// Match by chat type (e.g., "dm", "group", "channel").
    #[serde(default)]
    pub chat_type: Option<String>,

    /// Match by chat ID.
    #[serde(default)]
    pub chat_id: Option<String>,

    /// Match by sender ID.
    #[serde(default)]
    pub sender_id: Option<String>,
}

/// Configuration for an external (subprocess) gateway.
#[derive(Debug, Clone, Deserialize)]
pub struct ExternalGatewayConfig {
    /// Gateway name (used for routing and logging).
    pub name: String,

    /// Command to execute (path to binary).
    pub command: String,

    /// Arguments to pass to the command.
    #[serde(default)]
    pub args: Vec<String>,

    /// Environment variables to set.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,

    /// Restart policy.
    #[serde(default)]
    pub restart: RestartPolicy,
}

/// Restart policy for external gateways.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    /// Always restart on exit.
    Always,
    /// Restart only on non-zero exit.
    #[default]
    OnFailure,
    /// Never restart.
    Never,
}

// ============================================================================
// SandboxConfig
// ============================================================================

/// Sandbox configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SandboxConfig {
    /// Sandbox mode: "trust", "bubblewrap", "docker"
    pub mode: String,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            mode: "trust".to_string(),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempDir};

    // ========================================================================
    // Config Tests
    // ========================================================================

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.server.request_timeout_seconds, 300);
        assert_eq!(config.server.idle_timeout_seconds, 60);
        assert_eq!(config.server.keep_alive_interval_seconds, 15);
        assert!(config.workspace.is_none());
        assert!(config.agents_dir.is_none());
        assert!(config.services.session.path.is_none());
        assert!(config.world_memory.path.is_none());
        assert_eq!(config.sandbox.mode, "trust");
    }

    #[tokio::test]
    async fn test_load_missing_file_returns_defaults() {
        let tmp_dir = TempDir::new().unwrap();
        let missing_path = tmp_dir.path().join("missing-config.yaml");
        let config = Config::load(missing_path.to_str().unwrap()).await.unwrap();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 8080);
    }

    #[tokio::test]
    async fn test_load_valid_yaml() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
server:
  host: "127.0.0.1"
  port: 3000
  request_timeout_seconds: 60
  idle_timeout_seconds: 120
  keep_alive_interval_seconds: 30
agents_dir: ".duragent/agents-custom"
"#
        )
        .unwrap();

        let config = Config::load(file.path().to_str().unwrap()).await.unwrap();
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.server.request_timeout_seconds, 60);
        assert_eq!(config.server.idle_timeout_seconds, 120);
        assert_eq!(config.server.keep_alive_interval_seconds, 30);
        assert_eq!(
            config.agents_dir,
            Some(PathBuf::from(".duragent/agents-custom"))
        );
    }

    #[tokio::test]
    async fn test_load_partial_yaml_uses_defaults() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
server:
  port: 9000
"#
        )
        .unwrap();

        let config = Config::load(file.path().to_str().unwrap()).await.unwrap();
        assert_eq!(config.server.host, "0.0.0.0"); // default
        assert_eq!(config.server.port, 9000);
        assert_eq!(config.server.request_timeout_seconds, 300); // default
        assert_eq!(config.server.idle_timeout_seconds, 60); // default
        assert_eq!(config.server.keep_alive_interval_seconds, 15); // default
        assert!(config.workspace.is_none()); // default
        assert!(config.agents_dir.is_none()); // default
    }

    #[tokio::test]
    async fn test_load_invalid_yaml() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "invalid: yaml: content: [").unwrap();

        let result = Config::load(file.path().to_str().unwrap()).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_config_error_display() {
        let io_error = ConfigError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "test",
        ));
        assert!(io_error.to_string().contains("failed to read config file"));
    }

    // ========================================================================
    // resolve_path Tests
    // ========================================================================

    #[test]
    fn test_resolve_path_absolute() {
        let config_path = Path::new("/etc/duragent/duragent.yaml");
        let absolute_path = Path::new("/var/data/sessions");
        let result = resolve_path(config_path, absolute_path);
        assert_eq!(result, PathBuf::from("/var/data/sessions"));
    }

    #[test]
    fn test_resolve_path_relative() {
        let config_path = Path::new("/etc/duragent/duragent.yaml");
        let relative_path = Path::new(".duragent/sessions");
        let result = resolve_path(config_path, relative_path);
        assert_eq!(result, PathBuf::from("/etc/duragent/.duragent/sessions"));
    }

    #[test]
    fn test_resolve_path_relative_with_dotslash() {
        let config_path = Path::new("/opt/project/config/duragent.yaml");
        let relative_path = Path::new("./my-gateway");
        let result = resolve_path(config_path, relative_path);
        assert_eq!(result, PathBuf::from("/opt/project/config/./my-gateway"));
    }

    #[test]
    fn test_resolve_path_config_in_current_dir() {
        let config_path = Path::new("duragent.yaml");
        let relative_path = Path::new(".duragent/agents");
        let result = resolve_path(config_path, relative_path);
        // When config has no parent dir, uses "." which joins to just the relative path
        assert_eq!(result, PathBuf::from(".duragent/agents"));
    }

    #[test]
    fn test_resolve_path_config_with_subdir() {
        let config_path = Path::new("config/duragent.yaml");
        let relative_path = Path::new(".duragent/agents");
        let result = resolve_path(config_path, relative_path);
        assert_eq!(result, PathBuf::from("config/.duragent/agents"));
    }

    // ========================================================================
    // Environment Variable Expansion Tests
    // ========================================================================

    #[test]
    fn test_expand_env_vars_unclosed_brace() {
        let input = "value: ${UNCLOSED_VAR";
        let result = expand_env_vars(input);
        assert!(result.is_err());
        match result {
            Err(ConfigError::UnclosedVarReference) => {}
            _ => panic!("expected UnclosedVarReference error"),
        }
    }

    #[test]
    fn test_expand_env_vars_unclosed_brace_with_default() {
        let input = "value: ${VAR:-default";
        let result = expand_env_vars(input);
        assert!(result.is_err());
        match result {
            Err(ConfigError::UnclosedVarReference) => {}
            _ => panic!("expected UnclosedVarReference error"),
        }
    }

    #[tokio::test]
    async fn test_global_routing_config() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
gateways:
  telegram:
    enabled: true
    bot_token: "test_token"

routes:
  - match:
      gateway: "telegram"
      sender_id: "123456789"
    agent: "personal-assistant"
  - match:
      gateway: "telegram"
      chat_id: "-1001234567890"
    agent: "work-assistant"
  - match:
      chat_type: "group"
    agent: "group-moderator"
  - match: {{}}
    agent: "general-assistant"
"#
        )
        .unwrap();

        let config = Config::load(file.path().to_str().unwrap()).await.unwrap();
        let telegram = config
            .gateways
            .telegram
            .expect("telegram config should exist");

        assert!(telegram.enabled);
        assert_eq!(telegram.bot_token, "test_token");

        // Routes are now global
        assert_eq!(config.routes.len(), 4);

        // First rule: telegram + sender_id match
        assert_eq!(
            config.routes[0].match_conditions.gateway,
            Some("telegram".to_string())
        );
        assert_eq!(
            config.routes[0].match_conditions.sender_id,
            Some("123456789".to_string())
        );
        assert_eq!(config.routes[0].agent, "personal-assistant");

        // Second rule: telegram + chat_id match
        assert_eq!(
            config.routes[1].match_conditions.gateway,
            Some("telegram".to_string())
        );
        assert_eq!(
            config.routes[1].match_conditions.chat_id,
            Some("-1001234567890".to_string())
        );
        assert_eq!(config.routes[1].agent, "work-assistant");

        // Third rule: chat_type match (any gateway)
        assert_eq!(config.routes[2].match_conditions.gateway, None);
        assert_eq!(
            config.routes[2].match_conditions.chat_type,
            Some("group".to_string())
        );
        assert_eq!(config.routes[2].agent, "group-moderator");

        // Fourth rule: catch-all
        assert_eq!(config.routes[3].match_conditions.gateway, None);
        assert_eq!(config.routes[3].match_conditions.chat_type, None);
        assert_eq!(config.routes[3].agent, "general-assistant");
    }

    #[tokio::test]
    async fn test_telegram_gateway_minimal() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
gateways:
  telegram:
    bot_token: "test_token"
"#
        )
        .unwrap();

        let config = Config::load(file.path().to_str().unwrap()).await.unwrap();
        let telegram = config
            .gateways
            .telegram
            .expect("telegram config should exist");

        assert!(telegram.enabled);
        assert_eq!(telegram.bot_token, "test_token");
        assert!(config.routes.is_empty());
    }

    #[tokio::test]
    async fn test_sandbox_config() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
sandbox:
  mode: "bubblewrap"
"#
        )
        .unwrap();

        let config = Config::load(file.path().to_str().unwrap()).await.unwrap();
        assert_eq!(config.sandbox.mode, "bubblewrap");
    }

    #[tokio::test]
    async fn test_sandbox_config_default() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "# empty config").unwrap();

        let config = Config::load(file.path().to_str().unwrap()).await.unwrap();
        assert_eq!(config.sandbox.mode, "trust");
    }

    #[tokio::test]
    async fn test_routing_multiple_match_conditions() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
routes:
  - match:
      gateway: "telegram"
      chat_type: "group"
      chat_id: "-100123"
      sender_id: "456"
    agent: "specific-agent"
"#
        )
        .unwrap();

        let config = Config::load(file.path().to_str().unwrap()).await.unwrap();

        assert_eq!(config.routes.len(), 1);
        let rule = &config.routes[0];
        assert_eq!(rule.match_conditions.gateway, Some("telegram".to_string()));
        assert_eq!(rule.match_conditions.chat_type, Some("group".to_string()));
        assert_eq!(rule.match_conditions.chat_id, Some("-100123".to_string()));
        assert_eq!(rule.match_conditions.sender_id, Some("456".to_string()));
        assert_eq!(rule.agent, "specific-agent");
    }

    #[tokio::test]
    async fn test_external_gateway_config() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
gateways:
  external:
    - name: discord
      command: /usr/local/bin/duragent-discord
      args: ["--verbose"]
      restart: always

routes:
  - match:
      gateway: "discord"
      chat_type: "dm"
    agent: personal-assistant
  - match:
      gateway: "discord"
      chat_id: "server-123"
    agent: server-bot
"#
        )
        .unwrap();

        let config = Config::load(file.path().to_str().unwrap()).await.unwrap();
        assert_eq!(config.gateways.external.len(), 1);

        let discord = &config.gateways.external[0];
        assert_eq!(discord.name, "discord");
        assert_eq!(discord.command, "/usr/local/bin/duragent-discord");
        assert_eq!(discord.args, vec!["--verbose"]);
        assert!(matches!(discord.restart, RestartPolicy::Always));

        // Routes are global
        assert_eq!(config.routes.len(), 2);

        // First rule
        assert_eq!(
            config.routes[0].match_conditions.gateway,
            Some("discord".to_string())
        );
        assert_eq!(
            config.routes[0].match_conditions.chat_type,
            Some("dm".to_string())
        );
        assert_eq!(config.routes[0].agent, "personal-assistant");

        // Second rule
        assert_eq!(
            config.routes[1].match_conditions.gateway,
            Some("discord".to_string())
        );
        assert_eq!(
            config.routes[1].match_conditions.chat_id,
            Some("server-123".to_string())
        );
        assert_eq!(config.routes[1].agent, "server-bot");
    }

    // ========================================================================
    // Environment Variable Expansion Tests
    // ========================================================================

    #[test]
    fn test_expand_env_vars_no_vars() {
        let input = "plain string without variables";
        let result = expand_env_vars(input).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_expand_env_vars_required_var() {
        // SAFETY: Single-threaded test
        unsafe { std::env::set_var("TEST_VAR_REQUIRED", "test_value") };
        let input = "prefix ${TEST_VAR_REQUIRED} suffix";
        let result = expand_env_vars(input).unwrap();
        assert_eq!(result, "prefix test_value suffix");
        unsafe { std::env::remove_var("TEST_VAR_REQUIRED") };
    }

    #[test]
    fn test_expand_env_vars_missing_required_var() {
        // SAFETY: Single-threaded test
        unsafe { std::env::remove_var("MISSING_VAR_12345") };
        let input = "value: ${MISSING_VAR_12345}";
        let result = expand_env_vars(input);
        assert!(result.is_err());
        match result {
            Err(ConfigError::MissingEnvVar(name)) => assert_eq!(name, "MISSING_VAR_12345"),
            _ => panic!("expected MissingEnvVar error"),
        }
    }

    #[test]
    fn test_expand_env_vars_with_default() {
        // SAFETY: Single-threaded test
        unsafe { std::env::remove_var("UNSET_VAR_WITH_DEFAULT") };
        let input = "value: ${UNSET_VAR_WITH_DEFAULT:-default_value}";
        let result = expand_env_vars(input).unwrap();
        assert_eq!(result, "value: default_value");
    }

    #[test]
    fn test_expand_env_vars_with_empty_default() {
        // SAFETY: Single-threaded test
        unsafe { std::env::remove_var("UNSET_VAR_EMPTY_DEFAULT") };
        let input = "value: ${UNSET_VAR_EMPTY_DEFAULT:-}";
        let result = expand_env_vars(input).unwrap();
        assert_eq!(result, "value: ");
    }

    #[test]
    fn test_expand_env_vars_set_var_ignores_default() {
        // SAFETY: Single-threaded test
        unsafe { std::env::set_var("SET_VAR_WITH_DEFAULT", "actual_value") };
        let input = "value: ${SET_VAR_WITH_DEFAULT:-ignored_default}";
        let result = expand_env_vars(input).unwrap();
        assert_eq!(result, "value: actual_value");
        unsafe { std::env::remove_var("SET_VAR_WITH_DEFAULT") };
    }

    #[test]
    fn test_expand_env_vars_escaped_dollar() {
        let input = "price: $$100 and ${TEST_ESCAPE:-value}";
        let result = expand_env_vars(input).unwrap();
        assert_eq!(result, "price: $100 and value");
    }

    #[test]
    fn test_expand_env_vars_multiple_vars() {
        // SAFETY: Single-threaded test
        unsafe {
            std::env::set_var("VAR_A", "aaa");
            std::env::set_var("VAR_B", "bbb");
        }
        let input = "${VAR_A} and ${VAR_B} and ${VAR_C:-ccc}";
        let result = expand_env_vars(input).unwrap();
        assert_eq!(result, "aaa and bbb and ccc");
        unsafe {
            std::env::remove_var("VAR_A");
            std::env::remove_var("VAR_B");
        }
    }

    #[test]
    fn test_expand_env_vars_in_yaml_context() {
        // SAFETY: Single-threaded test
        unsafe { std::env::set_var("TEST_BOT_TOKEN", "secret123") };
        let input = r#"
gateways:
  telegram:
    bot_token: ${TEST_BOT_TOKEN}

routes:
  - match: {}
    agent: ${DEFAULT_AGENT:-assistant}
"#;
        let result = expand_env_vars(input).unwrap();
        assert!(result.contains("bot_token: secret123"));
        assert!(result.contains("agent: assistant"));
        unsafe { std::env::remove_var("TEST_BOT_TOKEN") };
    }

    #[test]
    fn test_expand_env_vars_literal_dollar_without_brace() {
        let input = "cost is $50";
        let result = expand_env_vars(input).unwrap();
        assert_eq!(result, "cost is $50");
    }

    #[tokio::test]
    async fn test_config_load_with_env_var() {
        // SAFETY: Single-threaded test
        unsafe { std::env::set_var("TEST_CONFIG_TOKEN", "env_token_value") };

        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
gateways:
  telegram:
    bot_token: ${{TEST_CONFIG_TOKEN}}
"#
        )
        .unwrap();

        let config = Config::load(file.path().to_str().unwrap()).await.unwrap();
        let telegram = config.gateways.telegram.expect("telegram should exist");
        assert_eq!(telegram.bot_token, "env_token_value");

        unsafe { std::env::remove_var("TEST_CONFIG_TOKEN") };
    }

    #[tokio::test]
    async fn test_config_load_missing_env_var_errors() {
        // SAFETY: Single-threaded test
        unsafe { std::env::remove_var("DEFINITELY_MISSING_VAR_XYZ") };

        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
gateways:
  telegram:
    bot_token: ${{DEFINITELY_MISSING_VAR_XYZ}}
"#
        )
        .unwrap();

        let result = Config::load(file.path().to_str().unwrap()).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("DEFINITELY_MISSING_VAR_XYZ"));
    }
}
