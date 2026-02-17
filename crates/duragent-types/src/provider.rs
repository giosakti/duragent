//! LLM provider enum.

use std::str::FromStr;

use serde::Deserialize;

/// Supported model providers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(try_from = "String")]
pub enum Provider {
    Anthropic,
    Ollama,
    OpenAI,
    OpenRouter,
    Other(String),
}

impl Provider {
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::Ollama => "ollama",
            Provider::OpenAI => "openai",
            Provider::OpenRouter => "openrouter",
            Provider::Other(s) => s.as_str(),
        }
    }
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for Provider {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "anthropic" => Provider::Anthropic,
            "ollama" => Provider::Ollama,
            "openai" => Provider::OpenAI,
            "openrouter" => Provider::OpenRouter,
            other => Provider::Other(other.to_string()),
        })
    }
}

impl From<String> for Provider {
    fn from(s: String) -> Self {
        s.parse().unwrap()
    }
}
