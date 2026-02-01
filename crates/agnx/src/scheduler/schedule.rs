//! Schedule data structures.
//!
//! Defines the core types for scheduled tasks including timing options,
//! payloads, and runtime state.

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
}

impl Schedule {
    /// Generate a new schedule ID.
    pub fn generate_id() -> ScheduleId {
        format!("sched_{}", ulid::Ulid::new())
    }

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
///
/// Uses exponential backoff with jitter to avoid thundering herd.
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

    /// Calculate the delay for a given attempt using exponential backoff with jitter.
    ///
    /// Delay = min(initial * 2^attempt, max) * (0.8 + random(0, 0.4))
    pub fn delay_for_attempt(&self, attempt: u8) -> std::time::Duration {
        let base_delay = self.initial_delay_ms.saturating_mul(1 << attempt.min(10));
        let capped_delay = base_delay.min(self.max_delay_ms);

        // Add jitter: ±20% randomization
        let jitter_factor = 0.8 + (rand::random::<f64>() * 0.4);
        let jittered_delay = (capped_delay as f64 * jitter_factor) as u64;

        std::time::Duration::from_millis(jittered_delay)
    }
}

// ============================================================================
// Runtime State & Logging
// ============================================================================

/// Runtime state for a schedule.
///
/// Tracks execution timing and status. Stored separately from the
/// schedule definition for frequent updates.
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
    fn schedule_id_is_unique() {
        let id1 = Schedule::generate_id();
        let id2 = Schedule::generate_id();
        assert_ne!(id1, id2);
        assert!(id1.starts_with("sched_"));
    }

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
        };
        assert!(schedule.is_recurring());
        assert!(!schedule.is_one_shot());
    }

    #[test]
    fn is_recurring_returns_true_for_cron() {
        let schedule = Schedule {
            id: "test".to_string(),
            agent: "agent".to_string(),
            created_by_session: "session".to_string(),
            destination: ScheduleDestination {
                gateway: "telegram".to_string(),
                chat_id: "123".to_string(),
            },
            timing: ScheduleTiming::Cron {
                expr: "0 9 * * MON-FRI".to_string(),
                tz: None,
            },
            payload: SchedulePayload::Message {
                message: "Good morning!".to_string(),
            },
            created_at: Utc::now(),
            status: ScheduleStatus::Active,
            retry: None,
        };
        assert!(schedule.is_recurring());
        assert!(!schedule.is_one_shot());
    }

    #[test]
    fn serialize_schedule_at_timing() {
        let timing = ScheduleTiming::At {
            at: DateTime::parse_from_rfc3339("2026-01-30T16:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        let yaml = serde_saphyr::to_string(&timing).unwrap();
        assert!(yaml.contains("at:"));
        assert!(yaml.contains("2026-01-30"));
    }

    #[test]
    fn serialize_schedule_every_timing() {
        let timing = ScheduleTiming::Every {
            every_seconds: 1800,
            anchor: None,
        };
        let yaml = serde_saphyr::to_string(&timing).unwrap();
        assert!(yaml.contains("every_seconds: 1800"));
    }

    #[test]
    fn serialize_schedule_cron_timing() {
        let timing = ScheduleTiming::Cron {
            expr: "0 9 * * MON-FRI".to_string(),
            tz: Some("America/New_York".to_string()),
        };
        let yaml = serde_saphyr::to_string(&timing).unwrap();
        assert!(yaml.contains("expr:"));
        assert!(yaml.contains("tz:"));
    }

    #[test]
    fn serialize_message_payload() {
        let payload = SchedulePayload::Message {
            message: "Hello!".to_string(),
        };
        let yaml = serde_saphyr::to_string(&payload).unwrap();
        assert!(yaml.contains("message:"));
    }

    #[test]
    fn serialize_task_payload() {
        let payload = SchedulePayload::Task {
            task: "Check status".to_string(),
        };
        let yaml = serde_saphyr::to_string(&payload).unwrap();
        assert!(yaml.contains("task:"));
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
    fn run_log_entry_error_serializes() {
        let entry = RunLogEntry {
            ts: 1706716800000,
            status: RunStatus::Error,
            duration_ms: Some(50),
            error: Some("Gateway unavailable".to_string()),
            next_run_at: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"status\":\"error\""));
        assert!(json.contains("Gateway unavailable"));
    }

    #[test]
    fn retry_config_default_values() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.initial_delay_ms, 1000);
        assert_eq!(config.max_delay_ms, 30000);
    }

    #[test]
    fn retry_config_delay_exponential_backoff() {
        let config = RetryConfig {
            max_retries: 5,
            initial_delay_ms: 1000,
            max_delay_ms: 60000,
        };

        // First retry: ~1000ms (with jitter)
        let delay0 = config.delay_for_attempt(0);
        assert!(delay0.as_millis() >= 800 && delay0.as_millis() <= 1200);

        // Second retry: ~2000ms (with jitter)
        let delay1 = config.delay_for_attempt(1);
        assert!(delay1.as_millis() >= 1600 && delay1.as_millis() <= 2400);

        // Third retry: ~4000ms (with jitter)
        let delay2 = config.delay_for_attempt(2);
        assert!(delay2.as_millis() >= 3200 && delay2.as_millis() <= 4800);
    }

    #[test]
    fn retry_config_delay_capped_at_max() {
        let config = RetryConfig {
            max_retries: 10,
            initial_delay_ms: 1000,
            max_delay_ms: 5000,
        };

        // After many attempts, should be capped at max
        let delay = config.delay_for_attempt(10);
        // With jitter of ±20%, max is 5000 * 1.2 = 6000
        assert!(delay.as_millis() <= 6000);
        // And minimum is 5000 * 0.8 = 4000
        assert!(delay.as_millis() >= 4000);
    }

    #[test]
    fn retry_config_serialization() {
        let config = RetryConfig {
            max_retries: 5,
            initial_delay_ms: 2000,
            max_delay_ms: 60000,
        };
        let yaml = serde_saphyr::to_string(&config).unwrap();
        assert!(yaml.contains("max_retries: 5"));
        assert!(yaml.contains("initial_delay_ms: 2000"));
        assert!(yaml.contains("max_delay_ms: 60000"));
    }

    #[test]
    fn schedule_with_retry_config() {
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
            retry: Some(RetryConfig::default()),
        };

        let yaml = serde_saphyr::to_string(&schedule).unwrap();
        assert!(yaml.contains("retry:"));
        assert!(yaml.contains("max_retries: 3"));
    }

    #[test]
    fn schedule_without_retry_omits_field() {
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
        };

        let yaml = serde_saphyr::to_string(&schedule).unwrap();
        assert!(!yaml.contains("retry:"));
    }
}
