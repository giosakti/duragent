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
/// Persists to `~/.agnx/auth.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthStorage {
    #[serde(flatten)]
    providers: HashMap<String, AuthCredential>,
}

impl AuthStorage {
    /// Default path for the auth credentials file.
    pub fn default_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(home).join(".agnx").join("auth.json")
    }

    /// Load credentials from disk. Returns empty storage if file doesn't exist.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("reading auth file: {}", path.display()))?;
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
        std::fs::write(path, &contents)
            .with_context(|| format!("writing auth file: {}", path.display()))?;

        // Set file permissions to 0600 (owner read/write only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
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
        let path = Path::new("/tmp/nonexistent-agnx-auth.json");
        let storage = AuthStorage::load(path).unwrap();
        assert!(storage.get_anthropic().is_none());
    }
}
