//! Web search tool using Brave Search API.

use std::fmt::Write;

use async_trait::async_trait;

use crate::llm::{FunctionDefinition, ToolDefinition};

use crate::tools::error::ToolError;
use crate::tools::executor::ToolResult;
use crate::tools::tool::Tool;

/// Web search tool backed by Brave Search API.
pub struct WebSearchTool {
    client: reqwest::Client,
    api_key: String,
}

impl WebSearchTool {
    /// Create a new web search tool.
    ///
    /// Returns `None` if `BRAVE_API_KEY` is not set.
    pub fn new() -> Option<Self> {
        let api_key = std::env::var("BRAVE_API_KEY").ok()?;
        if api_key.is_empty() {
            return None;
        }
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Some(Self { client, api_key })
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn definition(&self) -> ToolDefinition {
        definition()
    }

    async fn execute(&self, arguments: &str) -> Result<ToolResult, ToolError> {
        execute(&self.client, &self.api_key, arguments).await
    }
}

/// Execute the web search tool.
pub async fn execute(
    client: &reqwest::Client,
    api_key: &str,
    arguments: &str,
) -> Result<ToolResult, ToolError> {
    let args: WebSearchArgs =
        serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

    let count = args.count.unwrap_or(5).clamp(1, 20);

    let mut url = url::Url::parse("https://api.search.brave.com/res/v1/web/search")
        .expect("hardcoded URL is valid");
    url.query_pairs_mut()
        .append_pair("q", &args.query)
        .append_pair("count", &count.to_string());

    let response = client
        .get(url)
        .header("X-Subscription-Token", api_key)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| ToolError::ExecutionFailed(format!("HTTP request failed: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Ok(ToolResult {
            success: false,
            content: format!("Brave Search API error ({status}): {body}"),
        });
    }

    let body: BraveSearchResponse = response
        .json()
        .await
        .map_err(|e| ToolError::ExecutionFailed(format!("Failed to parse response: {e}")))?;

    let results = body.web.map(|w| w.results).unwrap_or_default();
    if results.is_empty() {
        return Ok(ToolResult {
            success: true,
            content: "No results found.".to_string(),
        });
    }

    let content = format_results(&results);
    Ok(ToolResult {
        success: true,
        content,
    })
}

/// Generate the tool definition for web search.
pub fn definition() -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "web_search".to_string(),
            description:
                "Search the web using Brave Search. Returns titles, URLs, and descriptions."
                    .to_string(),
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query"
                    },
                    "count": {
                        "type": "integer",
                        "description": "Number of results to return (1-20, default 5)"
                    }
                },
                "required": ["query"]
            })),
        },
    }
}

fn format_results(results: &[WebResult]) -> String {
    let mut output = String::new();
    for (i, result) in results.iter().enumerate() {
        let desc = result.description.as_deref().unwrap_or("No description");
        let _ = writeln!(
            output,
            "{}. **{}**\n   {}\n   {}\n",
            i + 1,
            result.title,
            result.url,
            desc
        );
    }
    output
}

// ============================================================================
// Private Types
// ============================================================================

#[derive(serde::Deserialize)]
struct WebSearchArgs {
    query: String,
    count: Option<u32>,
}

#[derive(serde::Deserialize)]
struct BraveSearchResponse {
    web: Option<WebResults>,
}

#[derive(serde::Deserialize)]
struct WebResults {
    results: Vec<WebResult>,
}

#[derive(serde::Deserialize)]
struct WebResult {
    title: String,
    url: String,
    description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_has_required_query() {
        let def = definition();
        assert_eq!(def.function.name, "web_search");

        let params = def.function.parameters.unwrap();
        assert!(
            params["required"]
                .as_array()
                .unwrap()
                .contains(&"query".into())
        );
    }

    #[test]
    fn parse_args_with_defaults() {
        let args: WebSearchArgs = serde_json::from_str(r#"{"query": "rust programming"}"#).unwrap();
        assert_eq!(args.query, "rust programming");
        assert_eq!(args.count, None);
    }

    #[test]
    fn parse_args_with_count() {
        let args: WebSearchArgs =
            serde_json::from_str(r#"{"query": "test", "count": 10}"#).unwrap();
        assert_eq!(args.query, "test");
        assert_eq!(args.count, Some(10));
    }

    #[test]
    fn format_results_numbered_list() {
        let results = vec![
            WebResult {
                title: "First Result".to_string(),
                url: "https://example.com/1".to_string(),
                description: Some("First description".to_string()),
            },
            WebResult {
                title: "Second Result".to_string(),
                url: "https://example.com/2".to_string(),
                description: None,
            },
        ];

        let output = format_results(&results);
        assert!(output.contains("1. **First Result**"));
        assert!(output.contains("https://example.com/1"));
        assert!(output.contains("First description"));
        assert!(output.contains("2. **Second Result**"));
        assert!(output.contains("No description"));
    }
}
