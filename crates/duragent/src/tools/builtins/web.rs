//! Web tool for searching and fetching web content.
//!
//! Consolidated tool with actions: search, fetch.

use std::fmt::Write;

use async_trait::async_trait;

use crate::llm::{FunctionDefinition, ToolDefinition};

use crate::tools::error::ToolError;
use crate::tools::executor::ToolResult;
use crate::tools::tool::Tool;

/// Maximum bytes to read from the response body (1 MB).
const MAX_BODY_BYTES: usize = 1_048_576;

/// Maximum bytes in the output sent to the LLM (50 KB).
const MAX_OUTPUT_BYTES: usize = 51_200;

// ============================================================================
// Tool struct
// ============================================================================

/// Consolidated web tool with search and fetch actions.
pub struct WebTool {
    client: reqwest::Client,
    api_key: Option<String>,
}

impl WebTool {
    /// Create a new web tool.
    ///
    /// Always created. Search action requires `BRAVE_API_KEY` env var;
    /// fetch action works without it.
    pub fn new() -> Self {
        let api_key = std::env::var("BRAVE_API_KEY")
            .ok()
            .filter(|k| !k.is_empty());
        let client = reqwest::Client::builder()
            .user_agent(format!("Duragent/{}", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Self { client, api_key }
    }
}

// ============================================================================
// Tool trait implementation
// ============================================================================

#[async_trait]
impl Tool for WebTool {
    fn name(&self) -> &str {
        "web"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "web".to_string(),
                description: "Web tools. Actions: 'search' the web using Brave Search, 'fetch' a web page as markdown.".to_string(),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["search", "fetch"],
                            "description": "Action to perform"
                        },
                        "query": {
                            "type": "string",
                            "description": "(search) The search query"
                        },
                        "count": {
                            "type": "integer",
                            "description": "(search) Number of results to return (1-20, default 5)"
                        },
                        "url": {
                            "type": "string",
                            "description": "(fetch) The URL to fetch (http or https)"
                        }
                    },
                    "required": ["action"]
                })),
            },
        }
    }

    async fn execute(&self, arguments: &str) -> Result<ToolResult, ToolError> {
        let args: WebArgs = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        match args.action.as_str() {
            "search" => {
                let query = args.query.ok_or_else(|| {
                    ToolError::InvalidArguments("'query' is required for search action".to_string())
                })?;
                self.action_search(&query, args.count).await
            }
            "fetch" => {
                let url = args.url.ok_or_else(|| {
                    ToolError::InvalidArguments("'url' is required for fetch action".to_string())
                })?;
                self.action_fetch(&url).await
            }
            other => Ok(ToolResult {
                success: false,
                content: format!("Unknown action: '{}'. Use: search, fetch", other),
            }),
        }
    }
}

// ============================================================================
// Action implementations
// ============================================================================

impl WebTool {
    async fn action_search(
        &self,
        query: &str,
        count: Option<u32>,
    ) -> Result<ToolResult, ToolError> {
        let api_key = self.api_key.as_deref().ok_or_else(|| {
            ToolError::ExecutionFailed("BRAVE_API_KEY environment variable is not set".to_string())
        })?;

        let count = count.unwrap_or(5).clamp(1, 20);

        let mut url = url::Url::parse("https://api.search.brave.com/res/v1/web/search")
            .expect("hardcoded URL is valid");
        url.query_pairs_mut()
            .append_pair("q", query)
            .append_pair("count", &count.to_string());

        let response = self
            .client
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

    async fn action_fetch(&self, url_str: &str) -> Result<ToolResult, ToolError> {
        // Validate URL scheme
        let parsed = url::Url::parse(url_str)
            .map_err(|e| ToolError::InvalidArguments(format!("Invalid URL: {e}")))?;
        match parsed.scheme() {
            "http" | "https" => {}
            scheme => {
                return Ok(ToolResult {
                    success: false,
                    content: format!(
                        "Unsupported URL scheme: {scheme}. Only http and https are allowed."
                    ),
                });
            }
        }

        let response = self
            .client
            .get(url_str)
            .send()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("HTTP request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            return Ok(ToolResult {
                success: false,
                content: format!("HTTP error: {status}"),
            });
        }

        // Read body with size limit
        let body_bytes = read_limited_body(response, MAX_BODY_BYTES)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read response: {e}")))?;

        let html = String::from_utf8_lossy(&body_bytes);
        let markdown =
            html_to_markdown_rs::convert(&html, None).unwrap_or_else(|_| html.into_owned());

        let mut content = format!("Source: {}\n\n{}", url_str, markdown);
        if content.len() > MAX_OUTPUT_BYTES {
            truncate_at_char_boundary(&mut content, MAX_OUTPUT_BYTES);
            content.push_str("\n\n[content truncated at 50 KB]");
        }

        Ok(ToolResult {
            success: true,
            content,
        })
    }
}

// ============================================================================
// Private Helpers
// ============================================================================

fn format_results(results: &[SearchResult]) -> String {
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

/// Read response body up to a byte limit.
async fn read_limited_body(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, reqwest::Error> {
    use futures::StreamExt;

    let mut stream = response.bytes_stream();
    let mut body = Vec::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let remaining = limit.saturating_sub(body.len());
        if remaining == 0 {
            break;
        }
        let take = chunk.len().min(remaining);
        body.extend_from_slice(&chunk[..take]);
    }

    Ok(body)
}

/// Truncate a string at a char boundary, in place.
fn truncate_at_char_boundary(s: &mut String, max_chars: usize) {
    let mut end = max_chars;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

// ============================================================================
// Private Types
// ============================================================================

#[derive(serde::Deserialize)]
struct WebArgs {
    action: String,
    query: Option<String>,
    count: Option<u32>,
    url: Option<String>,
}

#[derive(serde::Deserialize)]
struct BraveSearchResponse {
    web: Option<WebResults>,
}

#[derive(serde::Deserialize)]
struct WebResults {
    results: Vec<SearchResult>,
}

#[derive(serde::Deserialize)]
struct SearchResult {
    title: String,
    url: String,
    description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_search_args() {
        let args: WebArgs =
            serde_json::from_str(r#"{"action": "search", "query": "rust programming"}"#).unwrap();
        assert_eq!(args.action, "search");
        assert_eq!(args.query.unwrap(), "rust programming");
    }

    #[test]
    fn parse_fetch_args() {
        let args: WebArgs =
            serde_json::from_str(r#"{"action": "fetch", "url": "https://example.com"}"#).unwrap();
        assert_eq!(args.action, "fetch");
        assert_eq!(args.url.unwrap(), "https://example.com");
    }

    #[test]
    fn truncate_at_char_boundary_ascii() {
        let mut s = "hello world".to_string();
        truncate_at_char_boundary(&mut s, 5);
        assert_eq!(s, "hello");
    }

    #[test]
    fn truncate_at_char_boundary_multibyte() {
        // Each emoji is 4 bytes
        let mut s = "ab\u{1F600}cd".to_string();
        // Truncate at byte 3, which is in the middle of the emoji
        truncate_at_char_boundary(&mut s, 3);
        assert_eq!(s, "ab");
    }

    #[test]
    fn truncate_noop_when_under_limit() {
        let mut s = "short".to_string();
        truncate_at_char_boundary(&mut s, 100);
        assert_eq!(s, "short");
    }

    #[test]
    fn format_results_numbered_list() {
        let results = vec![
            SearchResult {
                title: "First Result".to_string(),
                url: "https://example.com/1".to_string(),
                description: Some("First description".to_string()),
            },
            SearchResult {
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
