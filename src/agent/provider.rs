use serde::Deserialize;

/// Supported model providers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    Ollama,
    OpenAI,
    OpenRouter,
    Other(String),
}

impl Provider {
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

impl<'de> Deserialize<'de> for Provider {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "anthropic" => Provider::Anthropic,
            "ollama" => Provider::Ollama,
            "openai" => Provider::OpenAI,
            "openrouter" => Provider::OpenRouter,
            other => Provider::Other(other.to_string()),
        })
    }
}
