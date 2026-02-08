//! Truncation utilities for context window management.
//!
//! Three layers of truncation for the agentic loop:
//! - Layer 3a: Individual tool result truncation
//! - Layer 3b: Observation masking (replace middle tool results with placeholders)
//! - Layer 3c: Iteration group dropping (remove oldest iteration groups)

use crate::agent::ToolResultTruncation;
use crate::llm::{Message, Role};

use super::tokens::{estimate_message_tokens, estimate_tokens};

// ============================================================================
// Layer 3a: Tool Result Truncation
// ============================================================================

/// Truncate a tool result string to fit within a token budget.
///
/// Returns the original string unchanged if it fits within `max_tokens`.
/// Otherwise applies the specified truncation strategy and appends an indicator.
pub fn truncate_tool_result(
    content: &str,
    max_tokens: u32,
    strategy: ToolResultTruncation,
) -> String {
    let current_tokens = estimate_tokens(content);
    if current_tokens <= max_tokens {
        return content.to_string();
    }

    let max_bytes = (max_tokens as usize) * 4;

    match strategy {
        ToolResultTruncation::Head => {
            let truncated = safe_truncate(content, max_bytes);
            format!(
                "{}\n\n[truncated: kept first ~{} of ~{} tokens (head)]",
                truncated, max_tokens, current_tokens
            )
        }
        ToolResultTruncation::Tail => {
            let truncated = safe_truncate_tail(content, max_bytes);
            format!(
                "[truncated: kept last ~{} of ~{} tokens (tail)]\n\n{}",
                max_tokens, current_tokens, truncated
            )
        }
        ToolResultTruncation::Both => {
            let half = max_bytes / 2;
            let head = safe_truncate(content, half);
            let tail = safe_truncate_tail(content, half);
            format!(
                "{}\n\n[truncated: kept first+last ~{} of ~{} tokens (both)]\n\n{}",
                head, max_tokens, current_tokens, tail
            )
        }
    }
}

/// Truncate a string to at most `max_bytes` from the beginning, respecting char boundaries.
fn safe_truncate(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    // Find the last char boundary at or before max_bytes
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Truncate a string to at most `max_bytes` from the end, respecting char boundaries.
fn safe_truncate_tail(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let start = s.len() - max_bytes;
    // Find the first char boundary at or after start
    let mut actual_start = start;
    while actual_start < s.len() && !s.is_char_boundary(actual_start) {
        actual_start += 1;
    }
    &s[actual_start..]
}

// ============================================================================
// Layer 3b: Observation Masking
// ============================================================================

/// Mask tool result messages in the middle of the agentic loop.
///
/// Within messages after `conversation_end_idx`, finds all `Role::Tool` messages.
/// Keeps the first `keep_first` and last `keep_last` unmasked.
/// Replaces all others with a placeholder showing how many tokens were removed.
///
/// No-op if both keep_first and keep_last are 0 (masking disabled),
/// or if total tool results <= keep_first + keep_last.
pub fn mask_tool_results(
    messages: &mut [Message],
    conversation_end_idx: usize,
    keep_first: u32,
    keep_last: u32,
) {
    // Disabled if both are 0
    if keep_first == 0 && keep_last == 0 {
        return;
    }

    // Find indices of all Tool messages after conversation_end_idx
    let tool_indices: Vec<usize> = messages
        .iter()
        .enumerate()
        .skip(conversation_end_idx)
        .filter(|(_, m)| m.role == Role::Tool)
        .map(|(i, _)| i)
        .collect();

    let total = tool_indices.len();
    let keep_first = keep_first as usize;
    let keep_last = keep_last as usize;

    if total <= keep_first + keep_last {
        return;
    }

    // Indices to mask: everything between keep_first and total - keep_last
    let mask_start = keep_first;
    let mask_end = total - keep_last;

    for &idx in &tool_indices[mask_start..mask_end] {
        let tokens = estimate_message_tokens(&messages[idx]);
        messages[idx].content = Some(format!("[result masked — ~{} tokens removed]", tokens));
    }
}

// ============================================================================
// Layer 3c: Iteration Group Dropping
// ============================================================================

/// Drop the oldest iteration groups to fit within a token budget.
///
/// An iteration group is: one assistant message with tool_calls + its subsequent Tool messages.
/// Drops oldest groups (never the last) until under budget.
/// Preserves all messages before `conversation_end_idx`.
pub fn drop_oldest_iterations(
    messages: &mut Vec<Message>,
    conversation_end_idx: usize,
    token_budget: u32,
) {
    // Calculate total tokens
    let total_tokens: u32 = messages.iter().map(estimate_message_tokens).sum();

    if total_tokens <= token_budget {
        return;
    }

    // Find iteration groups after conversation_end_idx
    let groups = find_iteration_groups(messages, conversation_end_idx);

    if groups.len() <= 1 {
        // Never drop the last (or only) group
        return;
    }

    // Drop oldest groups until under budget (never the last)
    let mut current_tokens = total_tokens;
    let mut groups_to_drop = Vec::new();

    for group in &groups[..groups.len() - 1] {
        if current_tokens <= token_budget {
            break;
        }
        let group_tokens: u32 = messages[group.start..=group.end]
            .iter()
            .map(estimate_message_tokens)
            .sum();
        current_tokens -= group_tokens;
        groups_to_drop.push(*group);
    }

    // Remove groups in reverse order to preserve indices
    for group in groups_to_drop.iter().rev() {
        messages.drain(group.start..=group.end);
    }
}

/// An iteration group: assistant message with tool_calls + subsequent tool messages.
#[derive(Debug, Clone, Copy)]
struct IterationGroup {
    start: usize,
    end: usize,
}

/// Find iteration groups after the conversation boundary.
fn find_iteration_groups(messages: &[Message], conversation_end_idx: usize) -> Vec<IterationGroup> {
    let mut groups = Vec::new();
    let mut i = conversation_end_idx;

    while i < messages.len() {
        // Look for assistant message with tool_calls
        if messages[i].role == Role::Assistant && messages[i].tool_calls.is_some() {
            let start = i;
            i += 1;

            // Consume subsequent Tool messages
            while i < messages.len() && messages[i].role == Role::Tool {
                i += 1;
            }

            groups.push(IterationGroup { start, end: i - 1 });
        } else {
            i += 1;
        }
    }

    groups
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{FunctionCall, ToolCall};

    // --- Layer 3a: Tool Result Truncation ---

    #[test]
    fn truncate_tool_result_under_limit() {
        let content = "short result";
        let result = truncate_tool_result(content, 1000, ToolResultTruncation::Head);
        assert_eq!(result, content);
    }

    #[test]
    fn truncate_tool_result_head() {
        let content = "a".repeat(1000);
        let result = truncate_tool_result(&content, 10, ToolResultTruncation::Head);
        assert!(result.starts_with("aaaa")); // 10 tokens * 4 bytes = 40 chars
        assert!(result.contains("[truncated:"));
        assert!(result.contains("(head)"));
    }

    #[test]
    fn truncate_tool_result_tail() {
        let content = format!("{}Z", "a".repeat(999));
        let result = truncate_tool_result(&content, 10, ToolResultTruncation::Tail);
        assert!(result.contains("[truncated:"));
        assert!(result.contains("(tail)"));
        assert!(result.ends_with('Z'));
    }

    #[test]
    fn truncate_tool_result_both() {
        let content = format!("HEAD{}TAIL", "x".repeat(1000));
        let result = truncate_tool_result(&content, 10, ToolResultTruncation::Both);
        assert!(result.contains("[truncated:"));
        assert!(result.contains("(both)"));
        assert!(result.starts_with("HEAD"));
        assert!(result.ends_with("TAIL"));
    }

    #[test]
    fn truncate_tool_result_unicode_boundary() {
        // "日" is 3 bytes. Create a string of 100 kanji = 300 bytes = 75 tokens.
        // Truncate to 10 tokens = 40 bytes. 40 / 3 = 13.33, so 13 complete chars.
        let content: String = std::iter::repeat('日').take(100).collect();
        let result = truncate_tool_result(&content, 10, ToolResultTruncation::Head);
        // Should not panic or produce invalid UTF-8
        assert!(result.contains("[truncated:"));
        // All chars should be valid
        for c in result.chars() {
            assert!(c.len_utf8() > 0);
        }
    }

    #[test]
    fn safe_truncate_basic() {
        assert_eq!(safe_truncate("hello", 3), "hel");
        assert_eq!(safe_truncate("hello", 10), "hello");
        assert_eq!(safe_truncate("hello", 0), "");
    }

    #[test]
    fn safe_truncate_tail_basic() {
        assert_eq!(safe_truncate_tail("hello", 3), "llo");
        assert_eq!(safe_truncate_tail("hello", 10), "hello");
    }

    #[test]
    fn safe_truncate_unicode() {
        let s = "aé"; // 'a' = 1 byte, 'é' = 2 bytes
        assert_eq!(safe_truncate(s, 2), "a"); // Can't split 'é'
        assert_eq!(safe_truncate(s, 3), "aé");
    }

    // --- Layer 3b: Observation Masking ---

    fn tool_result_msg(content: &str) -> Message {
        Message::tool_result("call_1", content)
    }

    fn assistant_tool_call_msg() -> Message {
        Message {
            role: Role::Assistant,
            content: None,
            tool_calls: Some(vec![ToolCall {
                id: "call_1".to_string(),
                tool_type: "function".to_string(),
                function: FunctionCall {
                    name: "bash".to_string(),
                    arguments: "{}".to_string(),
                },
            }]),
            tool_call_id: None,
        }
    }

    #[test]
    fn mask_tool_results_keeps_first_and_last() {
        let mut messages = vec![
            Message::text(Role::User, "hello"), // idx 0 (conversation)
            assistant_tool_call_msg(),          // idx 1
            tool_result_msg("result 1"),        // idx 2
            assistant_tool_call_msg(),          // idx 3
            tool_result_msg("result 2"),        // idx 4
            assistant_tool_call_msg(),          // idx 5
            tool_result_msg("result 3"),        // idx 6
            assistant_tool_call_msg(),          // idx 7
            tool_result_msg("result 4"),        // idx 8
            assistant_tool_call_msg(),          // idx 9
            tool_result_msg("result 5"),        // idx 10
        ];

        // conversation_end_idx = 1, keep_first = 1, keep_last = 1
        mask_tool_results(&mut messages, 1, 1, 1);

        // First tool result (idx 2) should be unmasked
        assert_eq!(messages[2].content_str(), "result 1");
        // Last tool result (idx 10) should be unmasked
        assert_eq!(messages[10].content_str(), "result 5");
        // Middle results (idx 4, 6, 8) should be masked
        assert!(messages[4].content_str().contains("[result masked"));
        assert!(messages[6].content_str().contains("[result masked"));
        assert!(messages[8].content_str().contains("[result masked"));
    }

    #[test]
    fn mask_tool_results_no_op_when_few_results() {
        let mut messages = vec![
            Message::text(Role::User, "hello"),
            assistant_tool_call_msg(),
            tool_result_msg("result 1"),
            assistant_tool_call_msg(),
            tool_result_msg("result 2"),
        ];

        // keep_first=1 + keep_last=1 = 2, total tool results = 2 => no-op
        mask_tool_results(&mut messages, 1, 1, 1);

        assert_eq!(messages[2].content_str(), "result 1");
        assert_eq!(messages[4].content_str(), "result 2");
    }

    #[test]
    fn mask_tool_results_disabled_when_both_zero() {
        let mut messages = vec![
            Message::text(Role::User, "hello"),
            assistant_tool_call_msg(),
            tool_result_msg("result 1"),
            assistant_tool_call_msg(),
            tool_result_msg("result 2"),
            assistant_tool_call_msg(),
            tool_result_msg("result 3"),
        ];

        mask_tool_results(&mut messages, 1, 0, 0);

        // All should be unmasked
        assert_eq!(messages[2].content_str(), "result 1");
        assert_eq!(messages[4].content_str(), "result 2");
        assert_eq!(messages[6].content_str(), "result 3");
    }

    // --- Layer 3c: Iteration Group Dropping ---

    #[test]
    fn drop_oldest_iterations_under_budget() {
        let mut messages = vec![
            Message::text(Role::User, "hello"),
            assistant_tool_call_msg(),
            tool_result_msg("result"),
        ];

        let initial_len = messages.len();
        drop_oldest_iterations(&mut messages, 1, 100_000);
        assert_eq!(messages.len(), initial_len); // No change
    }

    #[test]
    fn drop_oldest_iterations_preserves_last_group() {
        let mut messages = vec![
            Message::text(Role::User, "hello"),  // conversation
            assistant_tool_call_msg(),           // group 1
            tool_result_msg(&"x".repeat(10000)), // group 1
            assistant_tool_call_msg(),           // group 2
            tool_result_msg("small result"),     // group 2
        ];

        // Set a very small budget that only the last group + conversation can fit
        drop_oldest_iterations(&mut messages, 1, 50);

        // Conversation message should still be there
        assert_eq!(messages[0].content_str(), "hello");
        // Last group should be preserved
        assert_eq!(messages.last().unwrap().content_str(), "small result");
        // First group should be dropped
        assert_eq!(messages.len(), 3); // conversation + last group (assistant + tool)
    }

    #[test]
    fn drop_oldest_iterations_preserves_conversation_history() {
        let mut messages = vec![
            Message::text(Role::System, "You are helpful"),
            Message::text(Role::User, "hello"),
            Message::text(Role::Assistant, "hi there"),
            Message::text(Role::User, "do something"), // conversation_end_idx = 4
            assistant_tool_call_msg(),                 // group 1 start
            tool_result_msg(&"x".repeat(10000)),
            assistant_tool_call_msg(), // group 2 start
            tool_result_msg("final"),
        ];

        drop_oldest_iterations(&mut messages, 4, 50);

        // All conversation messages should be preserved
        assert_eq!(messages[0].content_str(), "You are helpful");
        assert_eq!(messages[1].content_str(), "hello");
        assert_eq!(messages[2].content_str(), "hi there");
        assert_eq!(messages[3].content_str(), "do something");
    }

    #[test]
    fn drop_oldest_iterations_single_group_never_dropped() {
        let mut messages = vec![
            Message::text(Role::User, "hello"),
            assistant_tool_call_msg(),
            tool_result_msg(&"x".repeat(100000)),
        ];

        let initial_len = messages.len();
        drop_oldest_iterations(&mut messages, 1, 1); // Impossibly small budget

        // Single group should never be dropped
        assert_eq!(messages.len(), initial_len);
    }
}
