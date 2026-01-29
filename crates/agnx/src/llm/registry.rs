//! Provider registry for managing LLM provider credentials and creation.

use std::collections::HashMap;
use std::sync::Arc;

use reqwest::Client;
use tracing::{info, warn};

use super::anthropic::AnthropicProvider;
use super::openai::OpenAICompatibleProvider;
use super::provider::{LLMProvider, Provider};

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
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self {
            api_keys: HashMap::new(),
            client: Client::new(),
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

        if !registry.has_cloud_provider() {
            warn!(
                "No cloud LLM providers configured. \
                Set OPENROUTER_API_KEY, OPENAI_API_KEY, or ANTHROPIC_API_KEY."
            );
        }

        registry
    }

    /// Check if any cloud provider is configured.
    fn has_cloud_provider(&self) -> bool {
        self.api_keys.contains_key(&Provider::Anthropic)
            || self.api_keys.contains_key(&Provider::OpenAI)
            || self.api_keys.contains_key(&Provider::OpenRouter)
    }

    /// Create a provider instance with optional base_url override.
    ///
    /// The base_url comes from the agent's model configuration. If not specified,
    /// the default URL for that provider is used.
    ///
    /// All providers share the registry's `reqwest::Client` for connection pooling.
    pub fn get(&self, provider: &Provider, base_url: Option<&str>) -> Option<Arc<dyn LLMProvider>> {
        match provider {
            Provider::Anthropic => {
                let api_key = self.api_keys.get(provider)?;
                let url = base_url.unwrap_or(defaults::ANTHROPIC);
                Some(Arc::new(AnthropicProvider::new(
                    self.client.clone(),
                    api_key.clone(),
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
}
