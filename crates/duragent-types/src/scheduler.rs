//! Schedule data structures.
//!
//! Defines the core types for scheduled tasks including timing options,
//! payloads, and runtime state.
//!
//! Evaluation methods (`generate_schedule_id`, `delay_for_attempt`) live in
//! `duragent::scheduler::schedule_eval`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ============================================================================
// Schedule - Main Type
// ============================================================================

/// Unique identifier for a schedule.
pub type ScheduleId = String;

/// A scheduled task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    /// Unique identifier.
    pub id: ScheduleId,
    /// Agent that owns this schedule.
    pub agent: String,
    /// Session that created this schedule.
    pub created_by_session: String,
    /// Where to deliver results.
    pub destination: ScheduleDestination,
    /// When to fire.
    #[serde(rename = "schedule")]
    pub timing: ScheduleTiming,
    /// What to do when fired.
    pub payload: SchedulePayload,
    /// When the schedule was created.
    pub created_at: DateTime<Utc>,
    /// Schedule lifecycle status.
    pub status: ScheduleStatus,
    /// Optional retry configuration for transient failures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryConfig>,
    /// Optional linked process handle. When set, the schedule is auto-cancelled
    /// when the linked process exits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_handle: Option<String>,
}

impl Schedule {
    /// Check if this schedule is a one-shot (fires once).
    pub fn is_one_shot(&self) -> bool {
        matches!(self.timing, ScheduleTiming::At { .. })
    }

    /// Check if this schedule is recurring.
    pub fn is_recurring(&self) -> bool {
        !self.is_one_shot()
    }
}

// ============================================================================
// Schedule Component Types
// ============================================================================

/// Where to deliver schedule results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleDestination {
    /// Gateway name (e.g., "telegram").
    pub gateway: String,
    /// Chat ID to deliver to.
    pub chat_id: String,
}

/// Timing configuration for a schedule.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleTiming {
    /// One-shot at a specific time.
    At {
        /// When to fire (UTC).
        at: DateTime<Utc>,
    },
    /// Recurring at a fixed interval.
    Every {
        /// Interval in seconds.
        every_seconds: u64,
        /// Optional anchor time for alignment.
        #[serde(skip_serializing_if = "Option::is_none")]
        anchor: Option<DateTime<Utc>>,
    },
    /// Standard cron expression.
    Cron {
        /// Cron expression (e.g., "0 9 * * MON-FRI").
        expr: String,
        /// Optional timezone (defaults to UTC).
        #[serde(skip_serializing_if = "Option::is_none")]
        tz: Option<String>,
    },
}

/// What to do when the schedule fires.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulePayload {
    /// Send a pre-defined message directly (no LLM call).
    Message {
        /// The message to send.
        message: String,
    },
    /// Execute a task with agent tools, summarize results, and report.
    Task {
        /// The task description for the agent.
        task: String,
    },
}

/// Schedule lifecycle status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleStatus {
    /// Schedule is active and will fire.
    #[default]
    Active,
    /// Schedule completed (one-shot fired).
    Completed,
    /// Schedule was cancelled.
    Cancelled,
}

// ============================================================================
// Retry Configuration
// ============================================================================

/// Retry configuration for handling transient failures.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (not including the initial attempt).
    #[serde(default = "RetryConfig::default_max_retries")]
    pub max_retries: u8,
    /// Initial delay before first retry in milliseconds.
    #[serde(default = "RetryConfig::default_initial_delay_ms")]
    pub initial_delay_ms: u64,
    /// Maximum delay between retries in milliseconds.
    #[serde(default = "RetryConfig::default_max_delay_ms")]
    pub max_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: Self::default_max_retries(),
            initial_delay_ms: Self::default_initial_delay_ms(),
            max_delay_ms: Self::default_max_delay_ms(),
        }
    }
}

impl RetryConfig {
    fn default_max_retries() -> u8 {
        3
    }

    fn default_initial_delay_ms() -> u64 {
        1000
    }

    fn default_max_delay_ms() -> u64 {
        30000
    }
}

// ============================================================================
// Runtime State & Logging
// ============================================================================

/// Runtime state for a schedule.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScheduleState {
    /// When the schedule is next due to fire.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<DateTime<Utc>>,
    /// When the current run started (for stuck detection).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running_since: Option<DateTime<Utc>>,
    /// When the last run completed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<DateTime<Utc>>,
    /// Status of the last run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_status: Option<RunStatus>,
    /// Error message from last run (if failed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Duration of last run in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_duration_ms: Option<u64>,
}

/// Status of a schedule run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Run completed successfully.
    Ok,
    /// Run failed with an error.
    Error,
    /// Run was skipped (e.g., previous still running).
    Skipped,
}

/// Entry for run log (JSONL format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunLogEntry {
    /// Unix timestamp in milliseconds.
    pub ts: i64,
    /// Run status.
    pub status: RunStatus,
    /// Duration in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Error message (if status is Error).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Next scheduled run time (Unix timestamp ms).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<i64>,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_one_shot_returns_true_for_at() {
        let schedule = Schedule {
            id: "test".to_string(),
            agent: "agent".to_string(),
            created_by_session: "session".to_string(),
            destination: ScheduleDestination {
                gateway: "telegram".to_string(),
                chat_id: "123".to_string(),
            },
            timing: ScheduleTiming::At { at: Utc::now() },
            payload: SchedulePayload::Message {
                message: "test".to_string(),
            },
            created_at: Utc::now(),
            status: ScheduleStatus::Active,
            retry: None,
            process_handle: None,
        };
        assert!(schedule.is_one_shot());
        assert!(!schedule.is_recurring());
    }

    #[test]
    fn is_recurring_returns_true_for_every() {
        let schedule = Schedule {
            id: "test".to_string(),
            agent: "agent".to_string(),
            created_by_session: "session".to_string(),
            destination: ScheduleDestination {
                gateway: "telegram".to_string(),
                chat_id: "123".to_string(),
            },
            timing: ScheduleTiming::Every {
                every_seconds: 3600,
                anchor: None,
            },
            payload: SchedulePayload::Task {
                task: "check status".to_string(),
            },
            created_at: Utc::now(),
            status: ScheduleStatus::Active,
            retry: None,
            process_handle: None,
        };
        assert!(schedule.is_recurring());
        assert!(!schedule.is_one_shot());
    }

    #[test]
    fn run_log_entry_serializes_to_json() {
        let entry = RunLogEntry {
            ts: 1706630400000,
            status: RunStatus::Ok,
            duration_ms: Some(1234),
            error: None,
            next_run_at: Some(1706716800000),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"ts\":1706630400000"));
        assert!(json.contains("\"status\":\"ok\""));
        assert!(json.contains("\"duration_ms\":1234"));
    }

    #[test]
    fn retry_config_default_values() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.initial_delay_ms, 1000);
        assert_eq!(config.max_delay_ms, 30000);
    }
}
