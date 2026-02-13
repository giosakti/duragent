//! Credential storage for LLM provider authentication.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Stored authentication credentials for LLM providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthCredential {
    ApiKey {
        key: String,
    },
    OAuth {
        access: String,
        refresh: String,
        /// Unix timestamp in seconds when the access token expires.
        expires: i64,
    },
}

/// Storage for provider authentication credentials.
///
/// Persists to `~/.duragent/auth.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthStorage {
    #[serde(flatten)]
    providers: HashMap<String, AuthCredential>,
}

impl AuthStorage {
    /// Default path for the auth credentials file.
    pub fn default_path() -> PathBuf {
        let home = match std::env::var("HOME") {
            Ok(h) => h,
            Err(_) => {
                tracing::warn!("HOME not set, using /tmp for auth credentials");
                "/tmp".to_string()
            }
        };
        PathBuf::from(home)
            .join(crate::config::DEFAULT_WORKSPACE)
            .join("auth.json")
    }

    /// Load credentials from disk. Returns empty storage if file doesn't exist.
    pub fn load(path: &Path) -> Result<Self> {
        let contents = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => {
                return Err(
                    anyhow::Error::new(e).context(format!("reading auth file: {}", path.display()))
                );
            }
        };
        let storage: Self = serde_json::from_str(&contents)
            .with_context(|| format!("parsing auth file: {}", path.display()))?;
        Ok(storage)
    }

    /// Save credentials to disk with restricted permissions.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating directory: {}", parent.display()))?;
        }

        let contents = serde_json::to_string_pretty(self)?;

        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)
                .with_context(|| format!("writing auth file: {}", path.display()))?;
            let mut writer = std::io::BufWriter::new(file);
            writer
                .write_all(contents.as_bytes())
                .with_context(|| format!("writing auth file: {}", path.display()))?;
            let file = writer
                .into_inner()
                .map_err(|e| anyhow::anyhow!("flushing auth file: {}", e))?;
            file.sync_all()
                .with_context(|| format!("syncing auth file: {}", path.display()))?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(path, &contents)
                .with_context(|| format!("writing auth file: {}", path.display()))?;
        }

        Ok(())
    }

    /// Get the Anthropic credential, if any.
    pub fn get_anthropic(&self) -> Option<&AuthCredential> {
        self.providers.get("anthropic")
    }

    /// Set the Anthropic credential.
    pub fn set_anthropic(&mut self, credential: AuthCredential) {
        self.providers.insert("anthropic".to_string(), credential);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_oauth_credential() {
        let mut storage = AuthStorage::default();
        storage.set_anthropic(AuthCredential::OAuth {
            access: "sk-ant-oat-test".to_string(),
            refresh: "refresh-token".to_string(),
            expires: 1707825600,
        });

        let json = serde_json::to_string_pretty(&storage).unwrap();
        let loaded: AuthStorage = serde_json::from_str(&json).unwrap();

        match loaded.get_anthropic().unwrap() {
            AuthCredential::OAuth {
                access,
                refresh,
                expires,
            } => {
                assert_eq!(access, "sk-ant-oat-test");
                assert_eq!(refresh, "refresh-token");
                assert_eq!(*expires, 1707825600);
            }
            _ => panic!("expected OAuth credential"),
        }
    }

    #[test]
    fn empty_storage_returns_none() {
        let storage = AuthStorage::default();
        assert!(storage.get_anthropic().is_none());
    }

    #[test]
    fn load_nonexistent_file_returns_empty() {
        let path = Path::new("/tmp/nonexistent-duragent-auth.json");
        let storage = AuthStorage::load(path).unwrap();
        assert!(storage.get_anthropic().is_none());
    }
}
