//! Tool lifecycle hooks for guards and steering.
//!
//! Before-tool hooks can reject a tool call (e.g., missing dependency, duplicate).
//! After-tool hooks inject steering messages into the conversation.

use std::collections::HashMap;

use tracing::warn;

use crate::agent::{AfterToolHook, BeforeToolHook, BeforeToolType, HooksConfig};
use crate::llm::{Message, Role};
use crate::tools::ToolResult;

// ============================================================================
// Types
// ============================================================================

/// Context passed to hook evaluators.
pub struct HookContext<'a> {
    /// Tool name (e.g. "background_process").
    pub tool_name: &'a str,
    /// Action extracted from arguments (e.g. "send_keys").
    pub action: Option<&'a str>,
    /// Parsed tool arguments.
    pub arguments: &'a serde_json::Value,
    /// Current conversation messages (for history scanning).
    pub messages: &'a [Message],
}

/// Result of running before-tool hooks.
pub enum GuardVerdict {
    Allow,
    Reject(String),
}

// ============================================================================
// Default Hooks
// ============================================================================

/// Returns default hooks for the given set of enabled builtin tool names.
///
/// Each tool declares its own hygiene hooks — if you enable `memory`, you get
/// dedup guards; if you enable `background_process`, you get `depends_on` and
/// steering. No explicit config needed.
pub fn default_hooks(tool_names: &[&str]) -> HooksConfig {
    let mut before_tool = Vec::new();
    let mut after_tool = Vec::new();

    for name in tool_names {
        match *name {
            "memory" => {
                before_tool.push(BeforeToolHook {
                    tool_match: "memory:recall".to_string(),
                    hook_type: BeforeToolType::SkipDuplicate,
                    prior: None,
                    match_arg: None,
                    match_args: vec!["days".to_string()],
                });
            }
            "background_process" => {
                before_tool.push(BeforeToolHook {
                    tool_match: "background_process:send_keys".to_string(),
                    hook_type: BeforeToolType::DependsOn,
                    prior: Some("background_process:capture".to_string()),
                    match_arg: Some("handle".to_string()),
                    match_args: vec![],
                });
                after_tool.push(AfterToolHook {
                    tool_match: "background_process:capture".to_string(),
                    message: "You now have the current screen state. Base your next action on what you see above.".to_string(),
                    unless: HashMap::new(),
                });
                after_tool.push(AfterToolHook {
                    tool_match: "background_process:spawn".to_string(),
                    message:
                        "Remember to start a watcher for this process if you need to monitor it."
                            .to_string(),
                    unless: HashMap::from([("wait".to_string(), serde_json::json!(true))]),
                });
            }
            _ => {}
        }
    }

    HooksConfig {
        before_tool,
        after_tool,
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Run all matching before-tool hooks. First rejection wins.
pub fn run_before_tool(config: &HooksConfig, ctx: &HookContext) -> GuardVerdict {
    let invocation = build_invocation(ctx.tool_name, ctx.action);

    for hook in &config.before_tool {
        if !matches_hook_pattern(&hook.tool_match, &invocation) {
            continue;
        }

        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dispatch_before_hook(hook, ctx)
        })) {
            Ok(GuardVerdict::Reject(reason)) => return GuardVerdict::Reject(reason),
            Ok(GuardVerdict::Allow) => {}
            Err(e) => {
                warn!(
                    hook_match = %hook.tool_match,
                    error = ?e,
                    "Before-tool hook panicked, skipping"
                );
            }
        }
    }

    GuardVerdict::Allow
}

/// Run all matching after-tool hooks. Returns concatenated steering message, or None.
pub fn run_after_tool(
    config: &HooksConfig,
    ctx: &HookContext,
    result: &ToolResult,
) -> Option<String> {
    if !result.success {
        return None;
    }

    let invocation = build_invocation(ctx.tool_name, ctx.action);
    let mut messages = Vec::new();

    for hook in &config.after_tool {
        if !matches_hook_pattern(&hook.tool_match, &invocation) {
            continue;
        }

        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            evaluate_after_hook(hook, ctx)
        })) {
            Ok(Some(msg)) => messages.push(msg),
            Ok(None) => {}
            Err(e) => {
                warn!(
                    hook_match = %hook.tool_match,
                    error = ?e,
                    "After-tool hook panicked, skipping"
                );
            }
        }
    }

    if messages.is_empty() {
        None
    } else {
        Some(messages.join("\n"))
    }
}

// ============================================================================
// Before-Tool Implementations
// ============================================================================

fn dispatch_before_hook(hook: &BeforeToolHook, ctx: &HookContext) -> GuardVerdict {
    match hook.hook_type {
        BeforeToolType::DependsOn => check_depends_on(hook, ctx),
        BeforeToolType::SkipDuplicate => check_skip_duplicate(hook, ctx),
    }
}

/// Reject unless a prior tool call matching `hook.prior` exists for the same resource.
fn check_depends_on(hook: &BeforeToolHook, ctx: &HookContext) -> GuardVerdict {
    let Some(ref prior_pattern) = hook.prior else {
        return GuardVerdict::Allow;
    };

    // Extract the match_arg value from the current call
    let current_arg_value = hook
        .match_arg
        .as_ref()
        .and_then(|key| ctx.arguments.get(key));

    // Walk messages backwards looking for a matching prior tool call
    for msg in ctx.messages.iter().rev() {
        let Some(ref tool_calls) = msg.tool_calls else {
            continue;
        };
        if msg.role != Role::Assistant {
            continue;
        }

        for tc in tool_calls {
            let tc_invocation =
                build_invocation_from_tool_call(&tc.function.name, &tc.function.arguments);
            if !matches_hook_pattern(prior_pattern, &tc_invocation) {
                continue;
            }

            // If match_arg is specified, check that the value matches
            if let Some(ref key) = hook.match_arg {
                let prior_args: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                let prior_value = prior_args.get(key);
                if prior_value != current_arg_value {
                    continue;
                }
            }

            return GuardVerdict::Allow;
        }
    }

    let prior_display = prior_pattern;
    GuardVerdict::Reject(format!(
        "Hook guard: this tool requires a prior '{}' call first.{}",
        prior_display,
        hook.match_arg
            .as_ref()
            .map(|k| format!(" The '{}' argument must match.", k))
            .unwrap_or_default()
    ))
}

/// Reject if an identical call (matching `match_args` fields) already has a non-masked result.
fn check_skip_duplicate(hook: &BeforeToolHook, ctx: &HookContext) -> GuardVerdict {
    if hook.match_args.is_empty() {
        return GuardVerdict::Allow;
    }

    // Build identity from current call's match_args
    let current_identity = extract_identity(ctx.arguments, &hook.match_args);

    // Scan messages for prior matching tool calls with non-masked results
    let invocation = build_invocation(ctx.tool_name, ctx.action);

    for (i, msg) in ctx.messages.iter().enumerate() {
        let Some(ref tool_calls) = msg.tool_calls else {
            continue;
        };
        if msg.role != Role::Assistant {
            continue;
        }

        for tc in tool_calls {
            let tc_invocation =
                build_invocation_from_tool_call(&tc.function.name, &tc.function.arguments);
            if !matches_hook_pattern(&invocation, &tc_invocation) {
                continue;
            }

            let prior_args: serde_json::Value =
                serde_json::from_str(&tc.function.arguments).unwrap_or_default();
            let prior_identity = extract_identity(&prior_args, &hook.match_args);

            if prior_identity != current_identity {
                continue;
            }

            // Check if the result is non-masked (look for Tool message with matching call_id)
            if let Some(result_msg) = ctx.messages[i + 1..]
                .iter()
                .find(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some(&tc.id))
                && !result_msg.content_str().starts_with("[result masked")
            {
                return GuardVerdict::Reject(format!(
                    "Hook guard: duplicate call skipped. An identical '{}' call with the same arguments already has a result in this conversation.",
                    invocation
                ));
            }
        }
    }

    GuardVerdict::Allow
}

// ============================================================================
// After-Tool Implementation
// ============================================================================

/// Evaluate an after-tool hook. Returns the message if it should fire.
fn evaluate_after_hook(hook: &AfterToolHook, ctx: &HookContext) -> Option<String> {
    // Check unless conditions
    if !hook.unless.is_empty() {
        for (key, expected) in &hook.unless {
            if let Some(actual) = ctx.arguments.get(key)
                && actual == expected
            {
                return None;
            }
        }
    }

    Some(hook.message.clone())
}

// ============================================================================
// Helpers
// ============================================================================

/// Build "tool_name:action" invocation string.
fn build_invocation(tool_name: &str, action: Option<&str>) -> String {
    match action {
        Some(a) => format!("{}:{}", tool_name, a),
        None => tool_name.to_string(),
    }
}

/// Build invocation from a tool call's name and raw arguments.
fn build_invocation_from_tool_call(name: &str, arguments: &str) -> String {
    #[derive(serde::Deserialize)]
    struct ActionArgs {
        action: Option<String>,
    }

    let action = serde_json::from_str::<ActionArgs>(arguments)
        .ok()
        .and_then(|a| a.action);

    match action {
        Some(a) => format!("{}:{}", name, a),
        None => name.to_string(),
    }
}

/// Extract identity values for the given argument keys.
fn extract_identity(
    args: &serde_json::Value,
    keys: &[String],
) -> HashMap<String, serde_json::Value> {
    let mut identity = HashMap::new();
    for key in keys {
        if let Some(val) = args.get(key) {
            identity.insert(key.clone(), val.clone());
        }
    }
    identity
}

/// Match a hook pattern against a tool invocation.
///
/// Patterns use the `tool_name:action` format with optional `*` glob:
/// - `"background_process:send_keys"` — exact match
/// - `"background_process:*"` — all actions of background_process
/// - `"bash"` — matches "bash" exactly
fn matches_hook_pattern(pattern: &str, invocation: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == invocation;
    }

    // Glob matching: split on '*' and check parts appear in order
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut pos = 0;

    // First part must match from the start
    if let Some(first) = parts.first() {
        if !first.is_empty() && !invocation.starts_with(first) {
            return false;
        }
        pos = first.len();
    }

    // Last part must match at the end
    if let Some(last) = parts.last()
        && !last.is_empty()
        && !invocation.ends_with(last)
    {
        return false;
    }

    // Middle parts must appear in order
    for part in &parts[1..parts.len().saturating_sub(1)] {
        if part.is_empty() {
            continue;
        }
        match invocation[pos..].find(part) {
            Some(idx) => pos += idx + part.len(),
            None => return false,
        }
    }

    true
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{
        AfterToolHook, BeforeToolHook, BeforeToolType, HooksConfig, HooksConfigEval,
    };
    use crate::llm::{FunctionCall, Message, Role, ToolCall};
    use crate::tools::ToolResult;

    fn make_tool_call(name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: format!("call_{}", name),
            tool_type: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: args.to_string(),
            },
        }
    }

    // -----------------------------------------------------------------------
    // Pattern matching
    // -----------------------------------------------------------------------

    #[test]
    fn pattern_exact_match() {
        assert!(matches_hook_pattern("bash", "bash"));
        assert!(!matches_hook_pattern("bash", "bash:run"));
    }

    #[test]
    fn pattern_with_action_exact() {
        assert!(matches_hook_pattern(
            "background_process:send_keys",
            "background_process:send_keys"
        ));
        assert!(!matches_hook_pattern(
            "background_process:send_keys",
            "background_process:capture"
        ));
    }

    #[test]
    fn pattern_glob_all_actions() {
        assert!(matches_hook_pattern(
            "background_process:*",
            "background_process:send_keys"
        ));
        assert!(matches_hook_pattern(
            "background_process:*",
            "background_process:capture"
        ));
        assert!(!matches_hook_pattern("background_process:*", "bash"));
    }

    #[test]
    fn pattern_glob_wildcard_only() {
        assert!(matches_hook_pattern("*", "anything"));
        assert!(matches_hook_pattern("*", "background_process:capture"));
    }

    // -----------------------------------------------------------------------
    // depends_on guard
    // -----------------------------------------------------------------------

    #[test]
    fn depends_on_rejects_when_no_prior() {
        let config = HooksConfig {
            before_tool: vec![BeforeToolHook {
                tool_match: "background_process:send_keys".to_string(),
                hook_type: BeforeToolType::DependsOn,
                prior: Some("background_process:capture".to_string()),
                match_arg: Some("handle".to_string()),
                match_args: vec![],
            }],
            after_tool: vec![],
        };

        let ctx = HookContext {
            tool_name: "background_process",
            action: Some("send_keys"),
            arguments: &serde_json::json!({"handle": "proc1", "keys": "ls\n"}),
            messages: &[Message::text(Role::User, "hello")],
        };

        match run_before_tool(&config, &ctx) {
            GuardVerdict::Reject(reason) => {
                assert!(reason.contains("background_process:capture"));
            }
            GuardVerdict::Allow => panic!("Expected rejection"),
        }
    }

    #[test]
    fn depends_on_allows_when_prior_exists() {
        let config = HooksConfig {
            before_tool: vec![BeforeToolHook {
                tool_match: "background_process:send_keys".to_string(),
                hook_type: BeforeToolType::DependsOn,
                prior: Some("background_process:capture".to_string()),
                match_arg: Some("handle".to_string()),
                match_args: vec![],
            }],
            after_tool: vec![],
        };

        let messages = vec![
            Message::text(Role::User, "hello"),
            Message {
                role: Role::Assistant,
                content: None,
                tool_calls: Some(vec![make_tool_call(
                    "background_process",
                    r#"{"action": "capture", "handle": "proc1"}"#,
                )]),
                tool_call_id: None,
            },
            Message::tool_result("call_background_process", "screen output here"),
        ];

        let ctx = HookContext {
            tool_name: "background_process",
            action: Some("send_keys"),
            arguments: &serde_json::json!({"handle": "proc1", "keys": "ls\n"}),
            messages: &messages,
        };

        assert!(matches!(
            run_before_tool(&config, &ctx),
            GuardVerdict::Allow
        ));
    }

    #[test]
    fn depends_on_rejects_when_match_arg_differs() {
        let config = HooksConfig {
            before_tool: vec![BeforeToolHook {
                tool_match: "background_process:send_keys".to_string(),
                hook_type: BeforeToolType::DependsOn,
                prior: Some("background_process:capture".to_string()),
                match_arg: Some("handle".to_string()),
                match_args: vec![],
            }],
            after_tool: vec![],
        };

        let messages = vec![
            Message::text(Role::User, "hello"),
            Message {
                role: Role::Assistant,
                content: None,
                tool_calls: Some(vec![make_tool_call(
                    "background_process",
                    r#"{"action": "capture", "handle": "proc2"}"#,
                )]),
                tool_call_id: None,
            },
            Message::tool_result("call_background_process", "screen output"),
        ];

        let ctx = HookContext {
            tool_name: "background_process",
            action: Some("send_keys"),
            arguments: &serde_json::json!({"handle": "proc1", "keys": "ls\n"}),
            messages: &messages,
        };

        assert!(matches!(
            run_before_tool(&config, &ctx),
            GuardVerdict::Reject(_)
        ));
    }

    // -----------------------------------------------------------------------
    // skip_duplicate guard
    // -----------------------------------------------------------------------

    #[test]
    fn skip_duplicate_rejects_when_identical_call_exists() {
        let config = HooksConfig {
            before_tool: vec![BeforeToolHook {
                tool_match: "memory:recall".to_string(),
                hook_type: BeforeToolType::SkipDuplicate,
                prior: None,
                match_arg: None,
                match_args: vec!["days".to_string()],
            }],
            after_tool: vec![],
        };

        let messages = vec![
            Message::text(Role::User, "hello"),
            Message {
                role: Role::Assistant,
                content: None,
                tool_calls: Some(vec![make_tool_call(
                    "memory",
                    r#"{"action": "recall", "days": 7}"#,
                )]),
                tool_call_id: None,
            },
            Message::tool_result("call_memory", "some memories"),
        ];

        let ctx = HookContext {
            tool_name: "memory",
            action: Some("recall"),
            arguments: &serde_json::json!({"action": "recall", "days": 7}),
            messages: &messages,
        };

        assert!(matches!(
            run_before_tool(&config, &ctx),
            GuardVerdict::Reject(_)
        ));
    }

    #[test]
    fn skip_duplicate_allows_when_args_differ() {
        let config = HooksConfig {
            before_tool: vec![BeforeToolHook {
                tool_match: "memory:recall".to_string(),
                hook_type: BeforeToolType::SkipDuplicate,
                prior: None,
                match_arg: None,
                match_args: vec!["days".to_string()],
            }],
            after_tool: vec![],
        };

        let messages = vec![
            Message::text(Role::User, "hello"),
            Message {
                role: Role::Assistant,
                content: None,
                tool_calls: Some(vec![make_tool_call(
                    "memory",
                    r#"{"action": "recall", "days": 7}"#,
                )]),
                tool_call_id: None,
            },
            Message::tool_result("call_memory", "some memories"),
        ];

        let ctx = HookContext {
            tool_name: "memory",
            action: Some("recall"),
            arguments: &serde_json::json!({"action": "recall", "days": 30}),
            messages: &messages,
        };

        assert!(matches!(
            run_before_tool(&config, &ctx),
            GuardVerdict::Allow
        ));
    }

    #[test]
    fn skip_duplicate_allows_when_result_is_masked() {
        let config = HooksConfig {
            before_tool: vec![BeforeToolHook {
                tool_match: "memory:recall".to_string(),
                hook_type: BeforeToolType::SkipDuplicate,
                prior: None,
                match_arg: None,
                match_args: vec!["days".to_string()],
            }],
            after_tool: vec![],
        };

        let messages = vec![
            Message::text(Role::User, "hello"),
            Message {
                role: Role::Assistant,
                content: None,
                tool_calls: Some(vec![make_tool_call(
                    "memory",
                    r#"{"action": "recall", "days": 7}"#,
                )]),
                tool_call_id: None,
            },
            Message::tool_result("call_memory", "[result masked — ~100 tokens removed]"),
        ];

        let ctx = HookContext {
            tool_name: "memory",
            action: Some("recall"),
            arguments: &serde_json::json!({"action": "recall", "days": 7}),
            messages: &messages,
        };

        assert!(matches!(
            run_before_tool(&config, &ctx),
            GuardVerdict::Allow
        ));
    }

    // -----------------------------------------------------------------------
    // after_tool steering
    // -----------------------------------------------------------------------

    #[test]
    fn after_tool_returns_message_on_match() {
        let config = HooksConfig {
            before_tool: vec![],
            after_tool: vec![AfterToolHook {
                tool_match: "background_process:capture".to_string(),
                message: "Base your next action on the screen output above.".to_string(),
                unless: HashMap::new(),
            }],
        };

        let ctx = HookContext {
            tool_name: "background_process",
            action: Some("capture"),
            arguments: &serde_json::json!({"handle": "proc1"}),
            messages: &[],
        };

        let result = ToolResult {
            success: true,
            content: "screen output".to_string(),
        };

        let msg = run_after_tool(&config, &ctx, &result);
        assert_eq!(
            msg.unwrap(),
            "Base your next action on the screen output above."
        );
    }

    #[test]
    fn after_tool_returns_none_on_failure() {
        let config = HooksConfig {
            before_tool: vec![],
            after_tool: vec![AfterToolHook {
                tool_match: "background_process:capture".to_string(),
                message: "Steering message.".to_string(),
                unless: HashMap::new(),
            }],
        };

        let ctx = HookContext {
            tool_name: "background_process",
            action: Some("capture"),
            arguments: &serde_json::json!({}),
            messages: &[],
        };

        let result = ToolResult {
            success: false,
            content: "error".to_string(),
        };

        assert!(run_after_tool(&config, &ctx, &result).is_none());
    }

    #[test]
    fn after_tool_suppressed_by_unless() {
        let mut unless = HashMap::new();
        unless.insert("wait".to_string(), serde_json::json!(true));

        let config = HooksConfig {
            before_tool: vec![],
            after_tool: vec![AfterToolHook {
                tool_match: "background_process:spawn".to_string(),
                message: "Remember to start a watcher.".to_string(),
                unless,
            }],
        };

        let ctx = HookContext {
            tool_name: "background_process",
            action: Some("spawn"),
            arguments: &serde_json::json!({"command": "npm start", "wait": true}),
            messages: &[],
        };

        let result = ToolResult {
            success: true,
            content: "started".to_string(),
        };

        assert!(run_after_tool(&config, &ctx, &result).is_none());
    }

    #[test]
    fn after_tool_not_suppressed_when_unless_doesnt_match() {
        let mut unless = HashMap::new();
        unless.insert("wait".to_string(), serde_json::json!(true));

        let config = HooksConfig {
            before_tool: vec![],
            after_tool: vec![AfterToolHook {
                tool_match: "background_process:spawn".to_string(),
                message: "Remember to start a watcher.".to_string(),
                unless,
            }],
        };

        let ctx = HookContext {
            tool_name: "background_process",
            action: Some("spawn"),
            arguments: &serde_json::json!({"command": "npm start"}),
            messages: &[],
        };

        let result = ToolResult {
            success: true,
            content: "started".to_string(),
        };

        assert!(run_after_tool(&config, &ctx, &result).is_some());
    }

    #[test]
    fn after_tool_multiple_matches_concatenated() {
        let config = HooksConfig {
            before_tool: vec![],
            after_tool: vec![
                AfterToolHook {
                    tool_match: "background_process:capture".to_string(),
                    message: "Message A.".to_string(),
                    unless: HashMap::new(),
                },
                AfterToolHook {
                    tool_match: "background_process:*".to_string(),
                    message: "Message B.".to_string(),
                    unless: HashMap::new(),
                },
            ],
        };

        let ctx = HookContext {
            tool_name: "background_process",
            action: Some("capture"),
            arguments: &serde_json::json!({}),
            messages: &[],
        };

        let result = ToolResult {
            success: true,
            content: "output".to_string(),
        };

        let msg = run_after_tool(&config, &ctx, &result).unwrap();
        assert!(msg.contains("Message A."));
        assert!(msg.contains("Message B."));
    }

    // -----------------------------------------------------------------------
    // No matching hooks
    // -----------------------------------------------------------------------

    #[test]
    fn no_matching_hooks_allows() {
        let config = HooksConfig {
            before_tool: vec![BeforeToolHook {
                tool_match: "memory:recall".to_string(),
                hook_type: BeforeToolType::SkipDuplicate,
                prior: None,
                match_arg: None,
                match_args: vec!["days".to_string()],
            }],
            after_tool: vec![],
        };

        let ctx = HookContext {
            tool_name: "bash",
            action: None,
            arguments: &serde_json::json!({"command": "ls"}),
            messages: &[],
        };

        assert!(matches!(
            run_before_tool(&config, &ctx),
            GuardVerdict::Allow
        ));
    }

    #[test]
    fn empty_config_is_noop() {
        let config = HooksConfig::default();

        let ctx = HookContext {
            tool_name: "bash",
            action: None,
            arguments: &serde_json::json!({}),
            messages: &[],
        };

        assert!(matches!(
            run_before_tool(&config, &ctx),
            GuardVerdict::Allow
        ));

        let result = ToolResult {
            success: true,
            content: "ok".to_string(),
        };
        assert!(run_after_tool(&config, &ctx, &result).is_none());
    }

    // -----------------------------------------------------------------------
    // default_hooks
    // -----------------------------------------------------------------------

    #[test]
    fn default_hooks_empty_for_unknown_tools() {
        let config = default_hooks(&["bash", "schedule"]);
        assert!(config.before_tool.is_empty());
        assert!(config.after_tool.is_empty());
    }

    #[test]
    fn default_hooks_memory_produces_skip_duplicate() {
        let config = default_hooks(&["memory"]);
        assert_eq!(config.before_tool.len(), 1);
        assert_eq!(config.before_tool[0].tool_match, "memory:recall");
        assert!(matches!(
            config.before_tool[0].hook_type,
            BeforeToolType::SkipDuplicate
        ));
        assert_eq!(config.before_tool[0].match_args, vec!["days"]);
        assert!(config.after_tool.is_empty());
    }

    #[test]
    fn default_hooks_background_process_produces_all_hooks() {
        let config = default_hooks(&["background_process"]);
        assert_eq!(config.before_tool.len(), 1);
        assert_eq!(
            config.before_tool[0].tool_match,
            "background_process:send_keys"
        );
        assert!(matches!(
            config.before_tool[0].hook_type,
            BeforeToolType::DependsOn
        ));

        assert_eq!(config.after_tool.len(), 2);
        assert_eq!(
            config.after_tool[0].tool_match,
            "background_process:capture"
        );
        assert_eq!(config.after_tool[1].tool_match, "background_process:spawn");
        assert!(config.after_tool[1].unless.contains_key("wait"));
    }

    #[test]
    fn default_hooks_combines_multiple_tools() {
        let config = default_hooks(&["memory", "background_process"]);
        assert_eq!(config.before_tool.len(), 2);
        assert_eq!(config.after_tool.len(), 2);
    }

    // -----------------------------------------------------------------------
    // with_defaults merge
    // -----------------------------------------------------------------------

    #[test]
    fn with_defaults_adds_missing_hooks() {
        let agent = HooksConfig::default();
        let defaults = default_hooks(&["memory"]);
        let merged = agent.with_defaults(defaults);

        assert_eq!(merged.before_tool.len(), 1);
        assert_eq!(merged.before_tool[0].tool_match, "memory:recall");
    }

    #[test]
    fn with_defaults_agent_overrides_same_pattern() {
        let agent = HooksConfig {
            before_tool: vec![BeforeToolHook {
                tool_match: "memory:recall".to_string(),
                hook_type: BeforeToolType::SkipDuplicate,
                prior: None,
                match_arg: None,
                match_args: vec!["days".to_string(), "query".to_string()],
            }],
            after_tool: vec![],
        };
        let defaults = default_hooks(&["memory"]);
        let merged = agent.with_defaults(defaults);

        // Agent's hook kept, default's hook not added
        assert_eq!(merged.before_tool.len(), 1);
        assert_eq!(merged.before_tool[0].match_args, vec!["days", "query"]);
    }

    #[test]
    fn with_defaults_preserves_agent_hooks_for_other_patterns() {
        let agent = HooksConfig {
            before_tool: vec![BeforeToolHook {
                tool_match: "bash".to_string(),
                hook_type: BeforeToolType::SkipDuplicate,
                prior: None,
                match_arg: None,
                match_args: vec!["command".to_string()],
            }],
            after_tool: vec![],
        };
        let defaults = default_hooks(&["memory"]);
        let merged = agent.with_defaults(defaults);

        // Both agent and default hooks present
        assert_eq!(merged.before_tool.len(), 2);
    }
}
