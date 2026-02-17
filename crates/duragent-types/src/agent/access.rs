//! Access control types for agent-level message filtering.
//!
//! Types: `AccessConfig`, `DmAccessConfig`, `GroupAccessConfig`, queue/activation config.
//!
//! Evaluation functions (`check_access`, `resolve_sender_disposition`) live in
//! `duragent::agent::access_eval`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ============================================================================
// Access Control Types
// ============================================================================

/// Access control configuration for an agent.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AccessConfig {
    #[serde(default)]
    pub dm: DmAccessConfig,
    #[serde(default)]
    pub groups: GroupAccessConfig,
}

/// DM access policy configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DmAccessConfig {
    #[serde(default)]
    pub policy: DmPolicy,
    #[serde(default)]
    pub allowlist: Vec<String>,
}

impl Default for DmAccessConfig {
    fn default() -> Self {
        Self {
            policy: DmPolicy::Open,
            allowlist: Vec::new(),
        }
    }
}

/// DM access policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DmPolicy {
    /// Accept DMs from anyone (default).
    #[default]
    Open,
    /// Reject all DMs.
    Disabled,
    /// Only accept DMs from listed sender IDs.
    Allowlist,
}

/// Group access policy configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GroupAccessConfig {
    #[serde(default)]
    pub policy: GroupPolicy,
    #[serde(default)]
    pub allowlist: Vec<String>,
    #[serde(default)]
    pub sender_default: SenderDisposition,
    #[serde(default)]
    pub sender_overrides: HashMap<String, SenderDisposition>,
    /// How the agent is activated in groups.
    #[serde(default)]
    pub activation: ActivationMode,
    /// Configuration for the context buffer (silent messages injected on trigger).
    #[serde(default)]
    pub context_buffer: ContextBufferConfig,
    /// Queue configuration for handling concurrent messages.
    #[serde(default)]
    pub queue: QueueConfig,
}

impl Default for GroupAccessConfig {
    fn default() -> Self {
        Self {
            policy: GroupPolicy::Open,
            allowlist: Vec::new(),
            sender_default: SenderDisposition::Allow,
            sender_overrides: HashMap::new(),
            activation: ActivationMode::default(),
            context_buffer: ContextBufferConfig::default(),
            queue: QueueConfig::default(),
        }
    }
}

/// Group access policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupPolicy {
    /// Accept messages from any group (default).
    #[default]
    Open,
    /// Reject all group messages.
    Disabled,
    /// Only accept messages from listed group IDs.
    Allowlist,
}

/// Disposition for a sender within an allowed group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SenderDisposition {
    /// Message is visible to the LLM and triggers a response.
    #[default]
    Allow,
    /// Message is stored as a UserMessage (LLM sees it in future turns) but does not trigger a response.
    Passive,
    /// Message is stored in session history for audit but excluded from LLM conversation.
    Silent,
    /// Message is discarded entirely.
    Block,
}

/// Activation mode for group messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationMode {
    /// Only respond when @mentioned or replied to (default).
    #[default]
    Mention,
    /// Respond to every allowed message.
    Always,
}

// ============================================================================
// Context Buffer Types
// ============================================================================

/// Configuration for the context buffer (messages from non-triggering senders).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ContextBufferConfig {
    /// How non-triggering messages are stored.
    #[serde(default)]
    pub mode: ContextBufferMode,
    /// Maximum number of recent silent messages to inject as context.
    #[serde(default = "default_max_messages")]
    pub max_messages: usize,
    /// Maximum age in hours for context buffer messages.
    #[serde(default = "default_max_age_hours")]
    pub max_age_hours: u64,
}

impl Default for ContextBufferConfig {
    fn default() -> Self {
        Self {
            mode: ContextBufferMode::default(),
            max_messages: default_max_messages(),
            max_age_hours: default_max_age_hours(),
        }
    }
}

fn default_max_messages() -> usize {
    100
}

fn default_max_age_hours() -> u64 {
    24
}

/// How non-triggering messages from `Allow` senders are stored in mention mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextBufferMode {
    /// Ephemeral buffer: stored as SilentMessage, injected as a system block on trigger.
    #[default]
    Silent,
    /// Durable: stored as UserMessage in conversation history (no injection needed).
    Passive,
}

// ============================================================================
// Queue Types
// ============================================================================

/// Queue configuration for group message handling.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QueueConfig {
    /// How pending messages are processed when the session becomes idle.
    #[serde(default)]
    pub mode: QueueMode,
    /// Maximum number of pending messages in the queue.
    #[serde(default = "default_max_pending")]
    pub max_pending: usize,
    /// What happens when the queue is full.
    #[serde(default)]
    pub overflow: OverflowStrategy,
    /// Message sent back to the user when a message is rejected.
    #[serde(default)]
    pub reject_message: Option<String>,
    /// Debounce settings for batching rapid messages.
    #[serde(default)]
    pub debounce: DebounceConfig,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            mode: QueueMode::default(),
            max_pending: default_max_pending(),
            overflow: OverflowStrategy::default(),
            reject_message: None,
            debounce: DebounceConfig::default(),
        }
    }
}

fn default_max_pending() -> usize {
    10
}

/// Queue mode for group message handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueMode {
    /// Batch all pending messages into one combined message (default).
    #[default]
    Batch,
    /// Process pending messages one at a time in order.
    Sequential,
    /// Drop all pending messages when session becomes idle.
    Drop,
}

/// Overflow strategy when the queue is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverflowStrategy {
    /// Drop oldest pending messages to make room (default).
    #[default]
    DropOld,
    /// Reject the new message (queue stays unchanged).
    DropNew,
    /// Reject with a user-visible message.
    Reject,
}

/// Debounce configuration for batching rapid messages from the same sender.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DebounceConfig {
    /// Whether debouncing is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Idle window in milliseconds before flushing the debounce buffer.
    #[serde(default = "default_debounce_window_ms")]
    pub window_ms: u64,
}

impl Default for DebounceConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            window_ms: default_debounce_window_ms(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_debounce_window_ms() -> u64 {
    1500
}
