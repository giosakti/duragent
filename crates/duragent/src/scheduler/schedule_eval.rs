//! Evaluation methods for scheduler types.
//!
//! Extends `RetryConfig` with backoff logic and provides `generate_schedule_id`.
//! The data definitions live in `duragent-types`; evaluation lives here.

use std::time::Duration;

use crate::scheduler::{RetryConfig, ScheduleId};

/// Generate a new unique schedule ID.
pub fn generate_schedule_id() -> ScheduleId {
    format!("sched_{}", ulid::Ulid::new())
}

/// Extension trait for `RetryConfig` evaluation logic.
pub trait RetryConfigEval {
    /// Calculate the delay for a given attempt using exponential backoff with jitter.
    fn delay_for_attempt(&self, attempt: u8) -> Duration;
}

impl RetryConfigEval for RetryConfig {
    fn delay_for_attempt(&self, attempt: u8) -> Duration {
        let base_delay = self.initial_delay_ms.saturating_mul(1 << attempt.min(10));
        let capped_delay = base_delay.min(self.max_delay_ms);

        // Add jitter: +/-20% randomization
        let jitter_factor = 0.8 + (rand::random::<f64>() * 0.4);
        let jittered_delay = (capped_delay as f64 * jitter_factor) as u64;

        Duration::from_millis(jittered_delay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_id_is_unique() {
        let id1 = generate_schedule_id();
        let id2 = generate_schedule_id();
        assert_ne!(id1, id2);
        assert!(id1.starts_with("sched_"));
    }

    #[test]
    fn retry_config_delay_exponential_backoff() {
        let config = RetryConfig {
            max_retries: 5,
            initial_delay_ms: 1000,
            max_delay_ms: 60000,
        };

        let delay0 = config.delay_for_attempt(0);
        assert!(delay0.as_millis() >= 800 && delay0.as_millis() <= 1200);

        let delay1 = config.delay_for_attempt(1);
        assert!(delay1.as_millis() >= 1600 && delay1.as_millis() <= 2400);

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

        let delay = config.delay_for_attempt(10);
        assert!(delay.as_millis() <= 6000);
        assert!(delay.as_millis() >= 4000);
    }
}
