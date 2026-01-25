use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Deserialize;

// -----------------------------------------------------------------------------
// Config (root)
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default = "default_agents_dir")]
    pub agents_dir: PathBuf,
    #[serde(default)]
    pub services: ServicesConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            agents_dir: default_agents_dir(),
            services: ServicesConfig::default(),
        }
    }
}

impl Config {
    pub fn load(path: &str) -> Result<Self, ConfigError> {
        let path = Path::new(path);
        let contents = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(ConfigError::Io(e)),
        };
        serde_saphyr::from_str(&contents).map_err(ConfigError::Yaml)
    }
}

fn default_agents_dir() -> PathBuf {
    PathBuf::from(".agnx/agents")
}

// -----------------------------------------------------------------------------
// ServerConfig
// -----------------------------------------------------------------------------

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
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            request_timeout_seconds: default_request_timeout(),
            idle_timeout_seconds: default_idle_timeout(),
            keep_alive_interval_seconds: default_keep_alive_interval(),
        }
    }
}

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

// -----------------------------------------------------------------------------
// ServicesConfig
// -----------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ServicesConfig {
    #[serde(default)]
    pub session: SessionServiceConfig,
}

// -----------------------------------------------------------------------------
// SessionServiceConfig
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SessionServiceConfig {
    #[serde(default = "default_session_path")]
    pub path: PathBuf,
}

impl Default for SessionServiceConfig {
    fn default() -> Self {
        Self {
            path: default_session_path(),
        }
    }
}

fn default_session_path() -> PathBuf {
    PathBuf::from(".agnx/sessions")
}

// -----------------------------------------------------------------------------
// ConfigError
// -----------------------------------------------------------------------------

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Yaml(serde_saphyr::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "failed to read config file: {e}"),
            ConfigError::Yaml(e) => write!(f, "failed to parse config file: {e}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io(e) => Some(e),
            ConfigError::Yaml(e) => Some(e),
        }
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempDir};

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.server.request_timeout_seconds, 300);
        assert_eq!(config.server.idle_timeout_seconds, 60);
        assert_eq!(config.server.keep_alive_interval_seconds, 15);
        assert_eq!(config.agents_dir, PathBuf::from(".agnx/agents"));
        assert_eq!(
            config.services.session.path,
            PathBuf::from(".agnx/sessions")
        );
    }

    #[test]
    fn test_load_missing_file_returns_defaults() {
        let tmp_dir = TempDir::new().unwrap();
        let missing_path = tmp_dir.path().join("missing-config.yaml");
        let config = Config::load(missing_path.to_str().unwrap()).unwrap();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 8080);
    }

    #[test]
    fn test_load_valid_yaml() {
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
agents_dir: ".agnx/agents-custom"
"#
        )
        .unwrap();

        let config = Config::load(file.path().to_str().unwrap()).unwrap();
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.server.request_timeout_seconds, 60);
        assert_eq!(config.server.idle_timeout_seconds, 120);
        assert_eq!(config.server.keep_alive_interval_seconds, 30);
        assert_eq!(config.agents_dir, PathBuf::from(".agnx/agents-custom"));
    }

    #[test]
    fn test_load_partial_yaml_uses_defaults() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
server:
  port: 9000
"#
        )
        .unwrap();

        let config = Config::load(file.path().to_str().unwrap()).unwrap();
        assert_eq!(config.server.host, "0.0.0.0"); // default
        assert_eq!(config.server.port, 9000);
        assert_eq!(config.server.request_timeout_seconds, 300); // default
        assert_eq!(config.server.idle_timeout_seconds, 60); // default
        assert_eq!(config.server.keep_alive_interval_seconds, 15); // default
        assert_eq!(config.agents_dir, PathBuf::from(".agnx/agents")); // default
    }

    #[test]
    fn test_load_invalid_yaml() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "invalid: yaml: content: [").unwrap();

        let result = Config::load(file.path().to_str().unwrap());
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
}
