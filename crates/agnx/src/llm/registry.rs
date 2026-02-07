//! Provider registry for managing LLM provider credentials and creation.

use std::collections::HashMap;
use std::sync::Arc;

use reqwest::Client;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::anthropic::{AnthropicAuth, AnthropicProvider};
use super::openai::OpenAICompatibleProvider;
use super::provider::{LLMProvider, Provider};
use crate::auth::anthropic_oauth;
use crate::auth::credentials::{AuthCredential, AuthStorage};

/// Default base URLs for each provider.
pub mod defaults {
    pub const ANTHROPIC: &str = "https://api.anthropic.com";
    pub const OLLAMA: &str = "http://localhost:11434/v1";
    pub const OPENAI: &str = "https://api.openai.com/v1";
    pub const OPENROUTER: &str = "https://openrouter.ai/api/v1";
}

/// Registry of LLM provider credentials.
///
/// Stores API keys from environment variables and creates provider instances
/// on-demand with optional base_url overrides from agent configuration.
///
/// The registry holds a shared `reqwest::Client` that is passed to all providers,
/// enabling connection pooling across requests.
#[derive(Clone)]
pub struct ProviderRegistry {
    api_keys: HashMap<Provider, String>,
    client: Client,
    auth_storage: Arc<RwLock<AuthStorage>>,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self {
            api_keys: HashMap::new(),
            client: Client::new(),
            auth_storage: Arc::new(RwLock::new(AuthStorage::default())),
        }
    }
}

impl ProviderRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Initialize registry with API keys from environment variables.
    pub fn from_env() -> Self {
        let mut registry = Self::new();

        if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
            registry.api_keys.insert(Provider::Anthropic, api_key);
            info!("Found Anthropic API key");
        }

        // Ollama doesn't need an API key
        registry.api_keys.insert(Provider::Ollama, String::new());
        info!("Ollama provider available (no API key required)");

        if let Ok(api_key) = std::env::var("OPENAI_API_KEY") {
            registry.api_keys.insert(Provider::OpenAI, api_key);
            info!("Found OpenAI API key");
        }

        if let Ok(api_key) = std::env::var("OPENROUTER_API_KEY") {
            registry.api_keys.insert(Provider::OpenRouter, api_key);
            info!("Found OpenRouter API key");
        }

        // Load OAuth credentials from disk
        let auth_path = AuthStorage::default_path();
        match AuthStorage::load(&auth_path) {
            Ok(storage) => {
                if storage.get_anthropic().is_some() {
                    info!("Found Anthropic OAuth credentials");
                    if registry.api_keys.contains_key(&Provider::Anthropic) {
                        debug!("Anthropic API key also found; OAuth takes precedence");
                    }
                }
                registry.auth_storage = Arc::new(RwLock::new(storage));
            }
            Err(e) => {
                debug!(error = %e, "Failed to load auth credentials");
            }
        }

        if !registry.has_cloud_provider() {
            warn!(
                "No cloud LLM providers configured. \
                Set OPENROUTER_API_KEY, OPENAI_API_KEY, or ANTHROPIC_API_KEY, \
                or run `agnx login anthropic`."
            );
        }

        registry
    }

    /// Check if any cloud provider is configured.
    fn has_cloud_provider(&self) -> bool {
        self.api_keys.contains_key(&Provider::Anthropic)
            || self.api_keys.contains_key(&Provider::OpenAI)
            || self.api_keys.contains_key(&Provider::OpenRouter)
            || self.has_oauth_credentials()
    }

    /// Check if OAuth credentials are available for any provider.
    fn has_oauth_credentials(&self) -> bool {
        // Use try_read to avoid blocking — false if locked
        self.auth_storage
            .try_read()
            .is_ok_and(|s| s.get_anthropic().is_some())
    }

    /// Create a provider instance with optional base_url override.
    ///
    /// The base_url comes from the agent's model configuration. If not specified,
    /// the default URL for that provider is used.
    ///
    /// All providers share the registry's `reqwest::Client` for connection pooling.
    pub async fn get(
        &self,
        provider: &Provider,
        base_url: Option<&str>,
    ) -> Option<Arc<dyn LLMProvider>> {
        match provider {
            Provider::Anthropic => {
                // OAuth takes precedence over API key
                if let Some(auth) = self.get_anthropic_oauth_auth().await {
                    let url = base_url.unwrap_or(defaults::ANTHROPIC);
                    return Some(Arc::new(AnthropicProvider::new(
                        self.client.clone(),
                        auth,
                        url.to_string(),
                    )));
                }

                // Fall back to API key
                let api_key = self.api_keys.get(provider)?;
                let url = base_url.unwrap_or(defaults::ANTHROPIC);
                Some(Arc::new(AnthropicProvider::new(
                    self.client.clone(),
                    AnthropicAuth::ApiKey(api_key.clone()),
                    url.to_string(),
                )))
            }
            Provider::Ollama => {
                if !self.api_keys.contains_key(provider) {
                    return None;
                }
                let url = base_url.unwrap_or(defaults::OLLAMA);
                Some(Arc::new(OpenAICompatibleProvider::new(
                    self.client.clone(),
                    url.to_string(),
                    None,
                )))
            }
            Provider::OpenAI => {
                let api_key = self.api_keys.get(provider)?;
                let url = base_url.unwrap_or(defaults::OPENAI);
                Some(Arc::new(OpenAICompatibleProvider::new(
                    self.client.clone(),
                    url.to_string(),
                    Some(api_key.clone()),
                )))
            }
            Provider::OpenRouter => {
                let api_key = self.api_keys.get(provider)?;
                let url = base_url.unwrap_or(defaults::OPENROUTER);
                Some(Arc::new(OpenAICompatibleProvider::new(
                    self.client.clone(),
                    url.to_string(),
                    Some(api_key.clone()),
                )))
            }
            Provider::Other(name) => {
                warn!("Unknown provider: {}", name);
                None
            }
        }
    }

    /// Get OAuth auth for Anthropic, refreshing the token if expired.
    async fn get_anthropic_oauth_auth(&self) -> Option<AnthropicAuth> {
        let storage = self.auth_storage.read().await;
        let credential = storage.get_anthropic()?;

        match credential {
            AuthCredential::OAuth {
                access,
                refresh,
                expires,
            } => {
                let now = chrono::Utc::now().timestamp();
                if now < *expires - 60 {
                    // Token still valid (with 60s buffer)
                    return Some(AnthropicAuth::OAuth(access.clone()));
                }

                // Token expired — need to refresh
                let refresh_token = refresh.clone();
                drop(storage);

                debug!("Anthropic OAuth token expired, refreshing");
                match anthropic_oauth::refresh_token(&self.client, &refresh_token).await {
                    Ok(tokens) => {
                        let mut storage = self.auth_storage.write().await;
                        storage.set_anthropic(AuthCredential::OAuth {
                            access: tokens.access_token.clone(),
                            refresh: tokens.refresh_token,
                            expires: tokens.expires_at,
                        });

                        // Save to disk (best-effort)
                        let path = AuthStorage::default_path();
                        if let Err(e) = storage.save(&path) {
                            warn!(error = %e, "Failed to save refreshed OAuth tokens");
                        }

                        Some(AnthropicAuth::OAuth(tokens.access_token))
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to refresh Anthropic OAuth token");
                        None
                    }
                }
            }
            AuthCredential::ApiKey { key } => Some(AnthropicAuth::ApiKey(key.clone())),
        }
    }
}
