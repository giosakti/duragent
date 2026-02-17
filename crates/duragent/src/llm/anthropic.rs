//! Anthropic LLM provider with native API format.

use std::pin::Pin;
use std::task::{Context, Poll};

use async_trait::async_trait;
use futures::Stream;
use reqwest::Client;

use super::{
    ChatRequest, ChatResponse, ChatStream, Choice, FunctionCall, LLMError, LLMProvider, Message,
    Role, StreamEvent, ToolCall, ToolDefinition, Usage, check_response_error,
};
use crate::sse_parser::SseEventStream;

/// Authentication mode for the Anthropic provider.
pub enum AnthropicAuth {
    /// Standard API key authentication.
    ApiKey(String),
    /// OAuth access token.
    OAuth(String),
}

/// Anthropic provider with native API format.
pub struct AnthropicProvider {
    client: Client,
    base_url: String,
    auth: AnthropicAuth,
    api_version: String,
}

impl AnthropicProvider {
    pub const DEFAULT_API_VERSION: &'static str = "2023-06-01";

    #[must_use]
    pub fn new(client: Client, auth: AnthropicAuth, base_url: String) -> Self {
        Self {
            client,
            base_url,
            auth,
            api_version: Self::DEFAULT_API_VERSION.to_string(),
        }
    }

    /// Build a POST request with appropriate auth headers.
    fn build_request(&self, url: &str, body: &Request) -> reqwest::RequestBuilder {
        let mut builder = self
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .header("anthropic-version", &self.api_version);

        builder = builder
            .header("accept", "application/json")
            .header("anthropic-dangerous-direct-browser-access", "true");

        match &self.auth {
            AnthropicAuth::ApiKey(key) => {
                builder = builder.header("x-api-key", key).header(
                    "anthropic-beta",
                    "fine-grained-tool-streaming-2025-05-14,interleaved-thinking-2025-05-14",
                );
            }
            AnthropicAuth::OAuth(token) => {
                builder = builder
                    .header("Authorization", format!("Bearer {}", token))
                    .header(
                        "anthropic-beta",
                        "claude-code-20250219,oauth-2025-04-20,fine-grained-tool-streaming-2025-05-14,interleaved-thinking-2025-05-14",
                    )
                    .header("user-agent", "claude-cli/2.1.2 (external, cli)")
                    .header("x-app", "cli")
                    .header("x-stainless-lang", "js")
                    .header("x-stainless-package-version", "0.70.0")
                    .header("x-stainless-os", "Linux")
                    .header("x-stainless-arch", "x64")
                    .header("x-stainless-runtime", "node")
                    .header("x-stainless-runtime-version", "v22.13.0")
                    .header("x-stainless-retry-count", "0");
            }
        }

        builder.json(body)
    }

    fn is_oauth(&self) -> bool {
        matches!(self.auth, AnthropicAuth::OAuth(_))
    }
}

#[async_trait]
impl LLMProvider for AnthropicProvider {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LLMError> {
        let url = format!("{}/v1/messages", self.base_url);
        let anthropic_request = to_request(&request, None, self.is_oauth());

        let response = self.build_request(&url, &anthropic_request).send().await?;

        if let Some(err) = check_response_error(&response) {
            return Err(err);
        }
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response.text().await.unwrap_or_default();
            return Err(LLMError::Api { status, message });
        }

        let anthropic_response: Response = response.json().await?;
        Ok(from_response(anthropic_response))
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, LLMError> {
        let url = format!("{}/v1/messages", self.base_url);
        let anthropic_request = to_request(&request, Some(true), self.is_oauth());

        let response = self.build_request(&url, &anthropic_request).send().await?;

        if let Some(err) = check_response_error(&response) {
            return Err(err);
        }
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response.text().await.unwrap_or_default();
            return Err(LLMError::Api { status, message });
        }

        let byte_stream = response.bytes_stream();
        let sse_stream = SseEventStream::new(byte_stream);
        let event_stream = AnthropicStreamAdapter::new(sse_stream);

        Ok(Box::pin(event_stream))
    }
}

// ============================================================================
// Request/Response Types
// ============================================================================

#[derive(serde::Serialize)]
struct Request {
    model: String,
    max_tokens: u32,
    /// System prompt: plain string for API key mode, array of content blocks for OAuth.
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<serde_json::Value>,
    messages: Vec<RequestMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

/// Anthropic tool definition format.
#[derive(serde::Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_schema: Option<serde_json::Value>,
}

#[derive(serde::Serialize)]
#[serde(untagged)]
enum RequestMessage {
    /// Simple text message.
    Text { role: String, content: String },
    /// Message with content blocks (for tool results).
    ContentBlocks {
        role: String,
        content: Vec<ContentBlock>,
    },
}

/// Content block for Anthropic messages.
#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlock {
    /// Text content.
    Text { text: String },
    /// Tool use by the assistant.
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tool result from the user.
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(serde::Deserialize)]
struct Response {
    id: String,
    content: Vec<ResponseContent>,
    stop_reason: Option<String>,
    usage: Option<ResponseUsage>,
}

#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponseContent {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(serde::Deserialize)]
struct ResponseUsage {
    input_tokens: u32,
    output_tokens: u32,
}

// ============================================================================
// Conversions
// ============================================================================

/// Anthropic canonical tool names.
/// Tools matching these (case-insensitive) are renamed to this casing.
const ANTHROPIC_TOOLS: &[&str] = &[
    "Read",
    "Write",
    "Edit",
    "Bash",
    "Grep",
    "Glob",
    "AskUserQuestion",
    "EnterPlanMode",
    "ExitPlanMode",
    "KillShell",
    "NotebookEdit",
    "Skill",
    "Task",
    "TaskOutput",
    "TodoWrite",
    "WebFetch",
    "WebSearch",
];

/// Map a tool name to the canonical casing if it matches.
fn to_canonical_name(name: &str) -> String {
    let lower = name.to_lowercase();
    for cc_name in ANTHROPIC_TOOLS {
        if cc_name.to_lowercase() == lower {
            return (*cc_name).to_string();
        }
    }
    name.to_string()
}

/// Convert OpenAI-style tool definitions to Anthropic format.
fn convert_tools(tools: Option<&Vec<ToolDefinition>>, oauth: bool) -> Option<Vec<AnthropicTool>> {
    tools.map(|ts| {
        ts.iter()
            .map(|t| AnthropicTool {
                name: if oauth {
                    to_canonical_name(&t.function.name)
                } else {
                    t.function.name.clone()
                },
                description: t.function.description.clone(),
                input_schema: t.function.parameters.clone(),
            })
            .collect()
    })
}

/// Identity string required by Anthropic's OAuth endpoint.
const ANTHROPIC_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

fn to_request(request: &ChatRequest, stream: Option<bool>, oauth: bool) -> Request {
    let mut system_parts: Vec<String> = Vec::new();
    let mut messages = Vec::new();

    for msg in &request.messages {
        match msg.role {
            Role::System => {
                if let Some(content) = msg.content.as_ref()
                    && !content.is_empty()
                {
                    system_parts.push(content.clone());
                }
            }
            Role::User | Role::Steering => {
                let content = msg.content.clone().unwrap_or_default();
                messages.push(RequestMessage::Text {
                    role: "user".to_string(),
                    content,
                });
            }
            Role::Assistant => {
                // Check if this is a tool call response
                if let Some(ref tool_calls) = msg.tool_calls {
                    let mut blocks = Vec::new();
                    // Include text content if present
                    if let Some(ref content) = msg.content
                        && !content.is_empty()
                    {
                        blocks.push(ContentBlock::Text {
                            text: content.clone(),
                        });
                    }
                    // Add tool use blocks
                    for tc in tool_calls {
                        let input: serde_json::Value = if tc.function.arguments.trim().is_empty() {
                            serde_json::Value::Object(Default::default())
                        } else {
                            serde_json::from_str(&tc.function.arguments).unwrap_or_else(|e| {
                                tracing::warn!(
                                    tool_call_id = %tc.id,
                                    tool_name = %tc.function.name,
                                    error = %e,
                                    "Failed to parse tool call arguments, using empty object"
                                );
                                serde_json::Value::Object(Default::default())
                            })
                        };
                        blocks.push(ContentBlock::ToolUse {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            input,
                        });
                    }
                    messages.push(RequestMessage::ContentBlocks {
                        role: "assistant".to_string(),
                        content: blocks,
                    });
                } else {
                    let content = msg.content.clone().unwrap_or_default();
                    messages.push(RequestMessage::Text {
                        role: "assistant".to_string(),
                        content,
                    });
                }
            }
            Role::Tool => {
                // Tool results in Anthropic are sent as user messages with tool_result content
                if let Some(ref tool_call_id) = msg.tool_call_id {
                    let content = msg.content.clone().unwrap_or_default();
                    messages.push(RequestMessage::ContentBlocks {
                        role: "user".to_string(),
                        content: vec![ContentBlock::ToolResult {
                            tool_use_id: tool_call_id.clone(),
                            content,
                        }],
                    });
                }
            }
        }
    }

    // Merge consecutive messages with the same role.
    // The Anthropic API requires strict role alternation. Event replay can
    // produce adjacent messages of the same role (e.g. tool_call + partial
    // content recorded as separate assistant events, or user text arriving
    // between a tool_use and its tool_result).
    merge_consecutive_messages(&mut messages);

    // Build system prompt: array format for OAuth, string for API key
    let system = if oauth {
        let cache_control = serde_json::json!({"type": "ephemeral"});
        let mut blocks = vec![serde_json::json!({
            "type": "text",
            "text": ANTHROPIC_IDENTITY,
            "cache_control": cache_control,
        })];
        for text in system_parts {
            blocks.push(serde_json::json!({
                "type": "text",
                "text": text,
                "cache_control": cache_control,
            }));
        }
        Some(serde_json::Value::Array(blocks))
    } else if system_parts.is_empty() {
        None
    } else {
        Some(serde_json::Value::String(system_parts.join("\n\n")))
    };

    Request {
        model: request.model.clone(),
        max_tokens: request.max_tokens.unwrap_or(4096),
        system,
        messages,
        temperature: request.temperature,
        tools: convert_tools(request.tools.as_ref(), oauth),
        stream,
    }
}

/// Merge consecutive messages with the same role into single messages.
///
/// The Anthropic API requires strict user/assistant alternation. Event replay
/// can produce adjacent messages of the same role that must be combined.
fn merge_consecutive_messages(messages: &mut Vec<RequestMessage>) {
    if messages.len() < 2 {
        return;
    }

    let mut merged: Vec<RequestMessage> = Vec::with_capacity(messages.len());

    for msg in messages.drain(..) {
        if let Some(last) = merged.last_mut()
            && message_role(last) == message_role(&msg)
        {
            let insert_separator = is_plain_text_message(last) && is_plain_text_message(&msg);
            merge_into(last, msg, insert_separator);
        } else {
            merged.push(msg);
        }
    }

    *messages = merged;
}

fn message_role(msg: &RequestMessage) -> &str {
    match msg {
        RequestMessage::Text { role, .. } | RequestMessage::ContentBlocks { role, .. } => role,
    }
}

fn is_plain_text_message(msg: &RequestMessage) -> bool {
    match msg {
        RequestMessage::Text { content, .. } => !content.is_empty(),
        RequestMessage::ContentBlocks { .. } => false,
    }
}

/// Merge `source` into `target` (both same role). Converts Text to ContentBlocks as needed.
fn merge_into(target: &mut RequestMessage, source: RequestMessage, insert_separator: bool) {
    // Convert target to ContentBlocks if it's currently Text
    let target_blocks = match target {
        RequestMessage::ContentBlocks { content, .. } => content,
        RequestMessage::Text { role, content } => {
            let blocks = if content.is_empty() {
                vec![]
            } else {
                vec![ContentBlock::Text {
                    text: std::mem::take(content),
                }]
            };
            let role_val = std::mem::take(role);
            *target = RequestMessage::ContentBlocks {
                role: role_val,
                content: blocks,
            };
            match target {
                RequestMessage::ContentBlocks { content, .. } => content,
                _ => unreachable!(),
            }
        }
    };

    // Extract blocks from source
    match source {
        RequestMessage::Text { content, .. } => {
            if !content.is_empty() {
                if insert_separator && !target_blocks.is_empty() {
                    target_blocks.push(ContentBlock::Text {
                        text: "\n\n".to_string(),
                    });
                }
                target_blocks.push(ContentBlock::Text { text: content });
            }
        }
        RequestMessage::ContentBlocks { content, .. } => {
            target_blocks.extend(content);
        }
    }
}

fn from_response(response: Response) -> ChatResponse {
    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();

    for block in response.content {
        match block {
            ResponseContent::Text { text } => text_parts.push(text),
            ResponseContent::ToolUse { id, name, input } => {
                tool_calls.push(ToolCall {
                    id,
                    tool_type: "function".to_string(),
                    function: FunctionCall {
                        name,
                        arguments: input.to_string(),
                    },
                });
            }
        }
    }

    let content = text_parts.join("");
    let mut message = Message::text(Role::Assistant, content);
    if !tool_calls.is_empty() {
        message.tool_calls = Some(tool_calls);
    }

    ChatResponse {
        id: response.id,
        choices: vec![Choice {
            index: 0,
            message,
            finish_reason: response.stop_reason,
        }],
        usage: response.usage.map(|u| Usage {
            prompt_tokens: u.input_tokens,
            completion_tokens: u.output_tokens,
            total_tokens: u.input_tokens + u.output_tokens,
        }),
    }
}

// ============================================================================
// Streaming
// ============================================================================

/// Adapter that converts SSE lines into Anthropic StreamEvents.
struct AnthropicStreamAdapter<S> {
    inner: SseEventStream<S>,
    done: bool,
    usage: Option<Usage>,
    /// Input tokens from message_start event.
    input_tokens: u32,
    /// Accumulated tool calls from streaming.
    tool_calls: Vec<ToolCallAccumulator>,
    /// Current tool call index being accumulated.
    current_tool_index: Option<usize>,
}

/// Accumulates tool call data from streaming chunks.
#[derive(Default)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    input_json: String,
}

impl<S> AnthropicStreamAdapter<S> {
    fn new(inner: SseEventStream<S>) -> Self {
        Self {
            inner,
            done: false,
            usage: None,
            input_tokens: 0,
            tool_calls: Vec::new(),
            current_tool_index: None,
        }
    }

    /// Convert accumulated tool calls into final ToolCall structs.
    fn finalize_tool_calls(&mut self) -> Vec<ToolCall> {
        std::mem::take(&mut self.tool_calls)
            .into_iter()
            .filter(|tc| !tc.id.is_empty())
            .map(|tc| ToolCall {
                id: tc.id,
                tool_type: "function".to_string(),
                function: FunctionCall {
                    name: tc.name,
                    arguments: tc.input_json,
                },
            })
            .collect()
    }
}

impl<S> Stream for AnthropicStreamAdapter<S>
where
    S: Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
{
    type Item = Result<StreamEvent, LLMError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.done {
            return Poll::Ready(None);
        }

        loop {
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(event))) => {
                    if event.data.is_empty() {
                        continue;
                    }

                    // Parse Anthropic event JSON
                    match serde_json::from_str::<AnthropicStreamEvent>(&event.data) {
                        Ok(parsed) => match parsed {
                            AnthropicStreamEvent::MessageStart { message: Some(msg) } => {
                                if let Some(tokens) =
                                    msg.pointer("/usage/input_tokens").and_then(|v| v.as_u64())
                                {
                                    self.input_tokens = tokens as u32;
                                }
                            }
                            AnthropicStreamEvent::MessageStart { .. } => {}
                            AnthropicStreamEvent::ContentBlockStart {
                                index,
                                content_block: Some(block),
                            } if block.block_type == "tool_use" => {
                                let idx = index.unwrap_or(0) as usize;
                                while self.tool_calls.len() <= idx {
                                    self.tool_calls.push(ToolCallAccumulator::default());
                                }
                                self.tool_calls[idx].id = block.id.unwrap_or_default();
                                self.tool_calls[idx].name = block.name.unwrap_or_default();
                                self.current_tool_index = Some(idx);
                            }
                            AnthropicStreamEvent::ContentBlockStart { .. } => {
                                // Non-tool_use content blocks - ignore
                            }
                            AnthropicStreamEvent::ContentBlockDelta { index, delta } => {
                                // Handle text delta
                                if let Some(text) = delta.text
                                    && !text.is_empty()
                                {
                                    return Poll::Ready(Some(Ok(StreamEvent::Token(text))));
                                }
                                // Handle tool input delta
                                if let Some(partial_json) = delta.partial_json {
                                    let idx = index.unwrap_or_else(|| {
                                        self.current_tool_index.unwrap_or(0) as u32
                                    }) as usize;
                                    if idx < self.tool_calls.len() {
                                        self.tool_calls[idx].input_json.push_str(&partial_json);
                                    }
                                }
                            }
                            AnthropicStreamEvent::ContentBlockStop { .. } => {
                                // Block finished, nothing special to do
                            }
                            AnthropicStreamEvent::MessageDelta {
                                usage: Some(u),
                                stop_reason,
                                ..
                            } => {
                                self.usage = Some(Usage {
                                    prompt_tokens: self.input_tokens,
                                    completion_tokens: u.output_tokens,
                                    total_tokens: self.input_tokens + u.output_tokens,
                                });
                                // Check if we're ending due to tool use
                                if stop_reason.as_deref() == Some("tool_use")
                                    && !self.tool_calls.is_empty()
                                {
                                    let tool_calls = self.finalize_tool_calls();
                                    if !tool_calls.is_empty() {
                                        return Poll::Ready(Some(Ok(StreamEvent::ToolCalls(
                                            tool_calls,
                                        ))));
                                    }
                                }
                            }
                            AnthropicStreamEvent::MessageStop => {
                                self.done = true;

                                // Emit any pending tool calls
                                if !self.tool_calls.is_empty() {
                                    let tool_calls = self.finalize_tool_calls();
                                    if !tool_calls.is_empty() {
                                        return Poll::Ready(Some(Ok(StreamEvent::ToolCalls(
                                            tool_calls,
                                        ))));
                                    }
                                }

                                return Poll::Ready(Some(Ok(StreamEvent::Done {
                                    usage: self.usage.take(),
                                })));
                            }
                            _ => {}
                        },
                        Err(e) => {
                            tracing::debug!(
                                data = %event.data,
                                error = %e,
                                "failed to parse Anthropic SSE event"
                            );
                        }
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(LLMError::Request(e))));
                }
                Poll::Ready(None) => {
                    self.done = true;

                    // Emit any pending tool calls
                    if !self.tool_calls.is_empty() {
                        let tool_calls = self.finalize_tool_calls();
                        if !tool_calls.is_empty() {
                            return Poll::Ready(Some(Ok(StreamEvent::ToolCalls(tool_calls))));
                        }
                    }

                    return Poll::Ready(Some(Ok(StreamEvent::Done {
                        usage: self.usage.take(),
                    })));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Anthropic SSE stream events.
///
/// Many fields are present in the API response but only some are used for streaming.
/// The unused fields are kept for complete deserialization.
#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[expect(dead_code, reason = "fields required for serde deserialization")]
enum AnthropicStreamEvent {
    MessageStart {
        message: Option<serde_json::Value>,
    },
    ContentBlockStart {
        index: Option<u32>,
        content_block: Option<StreamContentBlock>,
    },
    ContentBlockDelta {
        index: Option<u32>,
        delta: Delta,
    },
    ContentBlockStop {
        index: Option<u32>,
    },
    MessageDelta {
        delta: Option<serde_json::Value>,
        usage: Option<StreamUsage>,
        stop_reason: Option<String>,
    },
    MessageStop,
    Ping,
    #[serde(other)]
    Unknown,
}

/// Content block in streaming events.
#[derive(serde::Deserialize)]
struct StreamContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    /// Tool use ID (for tool_use blocks).
    id: Option<String>,
    /// Tool name (for tool_use blocks).
    name: Option<String>,
}

#[derive(serde::Deserialize)]
struct Delta {
    /// Text content (for text blocks).
    text: Option<String>,
    /// Partial JSON input (for tool_use blocks).
    partial_json: Option<String>,
}

#[derive(serde::Deserialize)]
struct StreamUsage {
    output_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_msg(role: &str, content: &str) -> RequestMessage {
        RequestMessage::Text {
            role: role.to_string(),
            content: content.to_string(),
        }
    }

    fn tool_use_msg(role: &str, id: &str, name: &str) -> RequestMessage {
        RequestMessage::ContentBlocks {
            role: role.to_string(),
            content: vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input: serde_json::json!({}),
            }],
        }
    }

    fn tool_result_msg(tool_use_id: &str, content: &str) -> RequestMessage {
        RequestMessage::ContentBlocks {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: content.to_string(),
            }],
        }
    }

    #[test]
    fn merge_no_op_for_alternating() {
        let mut msgs = vec![
            text_msg("user", "hello"),
            text_msg("assistant", "hi"),
            text_msg("user", "bye"),
        ];
        merge_consecutive_messages(&mut msgs);
        assert_eq!(msgs.len(), 3);
    }

    #[test]
    fn merge_consecutive_assistant_tool_use_and_text() {
        // Simulates approval flow: tool_call recorded, then partial content
        let mut msgs = vec![
            text_msg("user", "do it"),
            tool_use_msg("assistant", "tc1", "bash"),
            text_msg("assistant", "Running command..."),
            tool_result_msg("tc1", "done"),
        ];
        merge_consecutive_messages(&mut msgs);
        assert_eq!(msgs.len(), 3);

        // The merged assistant message should be ContentBlocks
        let serialized = serde_json::to_value(&msgs[1]).unwrap();
        let content = serialized["content"].as_array().unwrap();
        assert_eq!(content.len(), 2); // tool_use + text
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "Running command...");
    }

    #[test]
    fn merge_consecutive_user_text_and_tool_result() {
        // Simulates: user message arrived, then tool_result for prior tool_use
        let mut msgs = vec![
            tool_use_msg("assistant", "tc1", "bash"),
            text_msg("user", "process completed"),
            tool_result_msg("tc1", "done"),
        ];
        merge_consecutive_messages(&mut msgs);
        assert_eq!(msgs.len(), 2);

        let serialized = serde_json::to_value(&msgs[1]).unwrap();
        let content = serialized["content"].as_array().unwrap();
        assert_eq!(content.len(), 2); // text + tool_result
    }

    #[test]
    fn merge_multiple_user_messages() {
        let mut msgs = vec![
            text_msg("assistant", "done"),
            text_msg("user", "msg1"),
            text_msg("user", "msg2"),
            text_msg("user", "msg3"),
        ];
        merge_consecutive_messages(&mut msgs);
        assert_eq!(msgs.len(), 2);

        let serialized = serde_json::to_value(&msgs[1]).unwrap();
        let content = serialized["content"].as_array().unwrap();
        let texts: Vec<&str> = content
            .iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .filter(|t| !t.is_empty() && *t != "\n\n")
            .collect();
        assert_eq!(texts, vec!["msg1", "msg2", "msg3"]);
    }

    #[test]
    fn merge_empty_text_skipped() {
        let mut msgs = vec![
            text_msg("user", "hello"),
            text_msg("assistant", ""),
            text_msg("assistant", "real answer"),
        ];
        merge_consecutive_messages(&mut msgs);
        assert_eq!(msgs.len(), 2);

        let serialized = serde_json::to_value(&msgs[1]).unwrap();
        let content = serialized["content"].as_array().unwrap();
        // Empty text is skipped, only "real answer" remains
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["text"], "real answer");
    }
}
