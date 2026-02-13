//! Web fetch tool for retrieving web pages as markdown.

use async_trait::async_trait;

use crate::llm::{FunctionDefinition, ToolDefinition};

use crate::tools::error::ToolError;
use crate::tools::executor::ToolResult;
use crate::tools::tool::Tool;

/// Maximum bytes to read from the response body (1 MB).
const MAX_BODY_BYTES: usize = 1_048_576;

/// Maximum bytes in the output sent to the LLM (50 KB).
const MAX_OUTPUT_BYTES: usize = 51_200;

/// Web fetch tool that retrieves a URL and converts HTML to markdown.
pub struct WebFetchTool {
    client: reqwest::Client,
}

impl WebFetchTool {
    /// Create a new web fetch tool.
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .user_agent(format!("Duragent/{}", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Self { client }
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn definition(&self) -> ToolDefinition {
        definition()
    }

    async fn execute(&self, arguments: &str) -> Result<ToolResult, ToolError> {
        execute(&self.client, arguments).await
    }
}

/// Execute the web fetch tool.
pub async fn execute(client: &reqwest::Client, arguments: &str) -> Result<ToolResult, ToolError> {
    let args: WebFetchArgs =
        serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

    // Validate URL scheme
    let parsed = url::Url::parse(&args.url)
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

    let response = client
        .get(&args.url)
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
    let markdown = html_to_markdown_rs::convert(&html, None).unwrap_or_else(|_| html.into_owned());

    let mut content = format!("Source: {}\n\n{}", args.url, markdown);
    if content.len() > MAX_OUTPUT_BYTES {
        truncate_at_char_boundary(&mut content, MAX_OUTPUT_BYTES);
        content.push_str("\n\n[content truncated at 50 KB]");
    }

    Ok(ToolResult {
        success: true,
        content,
    })
}

/// Generate the tool definition for web fetch.
pub fn definition() -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "web_fetch".to_string(),
            description: "Fetch a web page and extract its content as markdown.".to_string(),
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch (http or https)"
                    }
                },
                "required": ["url"]
            })),
        },
    }
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
struct WebFetchArgs {
    url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_has_required_url() {
        let def = definition();
        assert_eq!(def.function.name, "web_fetch");

        let params = def.function.parameters.unwrap();
        assert!(
            params["required"]
                .as_array()
                .unwrap()
                .contains(&"url".into())
        );
    }

    #[test]
    fn parse_args() {
        let args: WebFetchArgs = serde_json::from_str(r#"{"url": "https://example.com"}"#).unwrap();
        assert_eq!(args.url, "https://example.com");
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
}
