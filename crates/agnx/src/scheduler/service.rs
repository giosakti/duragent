//! Scheduler service for executing scheduled tasks.
//!
//! Runs as a background task, managing timers for all active schedules
//! and executing them when they fire.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{RwLock, mpsc, oneshot};
use tokio::time::Instant;
use tracing::{debug, error, info, warn};

use crate::agent::AgentStore;
use crate::context::ContextBuilder;
use crate::gateway::GatewayManager;
use crate::llm::{Message, ProviderRegistry, Role};
use crate::sandbox::Sandbox;
use crate::session::{
    AgenticResult, ChatSessionCache, EventContext, SessionEventPayload, SessionLocks, SessionStore,
    persist_assistant_message, record_event, run_agentic_loop,
};
use crate::tools::ToolExecutor;

use super::error::{Result, SchedulerError};
use super::run_log::RunLog;
use super::schedule::{
    RunLogEntry, RunStatus, Schedule, ScheduleId, SchedulePayload, ScheduleState, ScheduleStatus,
    ScheduleTiming,
};
use super::store::ScheduleStore;

/// Timeout for stuck run detection (2 hours).
const STUCK_RUN_TIMEOUT_SECS: i64 = 2 * 60 * 60;

/// Command to the scheduler service.
enum SchedulerCommand {
    /// Add a new schedule.
    Add(Box<Schedule>),
    /// Cancel a schedule.
    Cancel(ScheduleId),
    /// Shutdown the service.
    Shutdown,
}

/// Handle for interacting with the scheduler service.
#[derive(Clone)]
pub struct SchedulerHandle {
    command_tx: mpsc::Sender<SchedulerCommand>,
    store: ScheduleStore,
}

impl SchedulerHandle {
    /// Create a new schedule.
    pub async fn create_schedule(&self, schedule: Schedule) -> Result<ScheduleId> {
        let id = schedule.id.clone();

        // Validate timing
        validate_timing(&schedule.timing)?;

        // Store first
        self.store.create(schedule.clone()).await?;

        // Notify service
        let _ = self
            .command_tx
            .send(SchedulerCommand::Add(Box::new(schedule)))
            .await;

        Ok(id)
    }

    /// Cancel a schedule.
    pub async fn cancel_schedule(&self, id: &str, agent: &str) -> Result<()> {
        // Verify ownership
        let schedule = self
            .store
            .get(id)
            .await
            .ok_or_else(|| SchedulerError::NotFound(id.to_string()))?;

        if schedule.agent != agent {
            return Err(SchedulerError::NotAuthorized(schedule.agent.clone()));
        }

        // Update status
        self.store
            .update_status(id, ScheduleStatus::Cancelled)
            .await?;

        // Notify service
        let _ = self
            .command_tx
            .send(SchedulerCommand::Cancel(id.to_string()))
            .await;

        Ok(())
    }

    /// List schedules for an agent.
    pub async fn list_schedules(&self, agent: &str) -> Vec<Schedule> {
        self.store.list_by_agent(agent).await
    }

    /// Shutdown the scheduler.
    pub async fn shutdown(&self) {
        let _ = self.command_tx.send(SchedulerCommand::Shutdown).await;
    }
}

/// Configuration for the scheduler service.
pub struct SchedulerConfig {
    pub schedules_path: PathBuf,
    pub sessions_path: PathBuf,
    pub agents: AgentStore,
    pub providers: ProviderRegistry,
    pub sessions: SessionStore,
    pub session_locks: SessionLocks,
    pub gateways: GatewayManager,
    pub sandbox: Arc<dyn Sandbox>,
    pub chat_session_cache: ChatSessionCache,
}

/// The scheduler service.
pub struct SchedulerService {
    store: ScheduleStore,
    run_log: RunLog,
    config: SchedulerConfig,
    /// Active timers by schedule ID.
    timers: Arc<RwLock<HashMap<ScheduleId, oneshot::Sender<()>>>>,
}

impl SchedulerService {
    /// Create a new scheduler service.
    pub fn new(config: SchedulerConfig) -> Self {
        let store = ScheduleStore::new(config.schedules_path.clone());
        let run_log = RunLog::new(config.schedules_path.join("runs"));

        Self {
            store,
            run_log,
            config,
            timers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Start the scheduler service.
    ///
    /// Returns a handle for interacting with the service.
    pub async fn start(self) -> SchedulerHandle {
        let (command_tx, command_rx) = mpsc::channel(100);
        let handle = SchedulerHandle {
            command_tx,
            store: self.store.clone(),
        };

        // Load existing schedules
        if let Err(e) = self.store.load().await {
            error!(error = %e, "Failed to load schedules");
        }

        // Start all active schedule timers
        let schedules = self.store.list_active().await;
        for schedule in schedules {
            self.start_timer(&schedule).await;
        }

        // Clear any stuck runs
        self.clear_stuck_runs().await;

        // Spawn the main loop
        tokio::spawn(self.run(command_rx));

        handle
    }

    /// Main service loop.
    async fn run(self, mut command_rx: mpsc::Receiver<SchedulerCommand>) {
        info!("Scheduler service started");

        while let Some(cmd) = command_rx.recv().await {
            match cmd {
                SchedulerCommand::Add(schedule) => {
                    self.start_timer(&schedule).await;
                }
                SchedulerCommand::Cancel(id) => {
                    self.cancel_timer(&id).await;
                }
                SchedulerCommand::Shutdown => {
                    info!("Scheduler service shutting down");
                    // Cancel all timers
                    let mut timers = self.timers.write().await;
                    for (_, cancel) in timers.drain() {
                        let _ = cancel.send(());
                    }
                    break;
                }
            }
        }

        info!("Scheduler service stopped");
    }

    /// Start a timer for a schedule.
    async fn start_timer(&self, schedule: &Schedule) {
        let next_run = match calculate_next_run(&schedule.timing, None) {
            Some(t) => t,
            None => {
                warn!(
                    schedule_id = %schedule.id,
                    "Could not calculate next run time"
                );
                return;
            }
        };

        // Update state with next run time
        let mut state = self.store.get_state(&schedule.id).await.unwrap_or_default();
        state.next_run_at = Some(next_run);
        self.store.update_state(&schedule.id, state).await;

        let delay = next_run
            .signed_duration_since(Utc::now())
            .to_std()
            .unwrap_or(Duration::ZERO);

        debug!(
            schedule_id = %schedule.id,
            next_run = %next_run,
            delay_secs = delay.as_secs(),
            "Starting timer"
        );

        let (cancel_tx, cancel_rx) = oneshot::channel();

        // Store cancel handle
        {
            let mut timers = self.timers.write().await;
            timers.insert(schedule.id.clone(), cancel_tx);
        }

        // Clone what we need for the spawned task
        let schedule_id = schedule.id.clone();
        let store = self.store.clone();
        let run_log = self.run_log.clone();
        let timers = self.timers.clone();
        let config = SchedulerConfigRef {
            sessions_path: self.config.sessions_path.clone(),
            agents: self.config.agents.clone(),
            providers: self.config.providers.clone(),
            sessions: self.config.sessions.clone(),
            session_locks: self.config.session_locks.clone(),
            gateways: self.config.gateways.clone(),
            sandbox: self.config.sandbox.clone(),
            chat_session_cache: self.config.chat_session_cache.clone(),
        };

        tokio::spawn(async move {
            // Wait for timer or cancellation
            let deadline = Instant::now() + delay;
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => {
                    // Timer fired - execute
                    execute_schedule(schedule_id, store, run_log, config, timers).await;
                }
                _ = cancel_rx => {
                    debug!(schedule_id = %schedule_id, "Timer cancelled");
                }
            }
        });
    }

    /// Cancel a timer for a schedule.
    async fn cancel_timer(&self, id: &ScheduleId) {
        let mut timers = self.timers.write().await;
        if let Some(cancel) = timers.remove(id) {
            let _ = cancel.send(());
            debug!(schedule_id = %id, "Timer cancelled");
        }
    }

    /// Clear runs that appear stuck (running_since > 2 hours).
    async fn clear_stuck_runs(&self) {
        let schedules = self.store.list_active().await;
        let cutoff = Utc::now() - chrono::Duration::seconds(STUCK_RUN_TIMEOUT_SECS);

        for schedule in schedules {
            if let Some(state) = self.store.get_state(&schedule.id).await
                && let Some(running_since) = state.running_since
                && running_since < cutoff
            {
                warn!(
                    schedule_id = %schedule.id,
                    running_since = %running_since,
                    "Clearing stuck run"
                );

                let new_state = ScheduleState {
                    running_since: None,
                    last_status: Some(RunStatus::Error),
                    last_error: Some("Run stuck (timeout)".to_string()),
                    ..state
                };
                self.store.update_state(&schedule.id, new_state).await;

                // Log the failure
                let entry = RunLogEntry {
                    ts: Utc::now().timestamp_millis(),
                    status: RunStatus::Error,
                    duration_ms: None,
                    error: Some("Run stuck (timeout)".to_string()),
                    next_run_at: None,
                };
                let _ = self.run_log.append(&schedule.id, &entry).await;
            }
        }
    }
}

/// Clone-friendly config reference for spawned tasks.
#[derive(Clone)]
struct SchedulerConfigRef {
    sessions_path: PathBuf,
    agents: AgentStore,
    providers: ProviderRegistry,
    sessions: SessionStore,
    session_locks: SessionLocks,
    gateways: GatewayManager,
    sandbox: Arc<dyn Sandbox>,
    chat_session_cache: ChatSessionCache,
}

/// Execute a schedule.
fn execute_schedule(
    schedule_id: ScheduleId,
    store: ScheduleStore,
    run_log: RunLog,
    config: SchedulerConfigRef,
    timers: Arc<RwLock<HashMap<ScheduleId, oneshot::Sender<()>>>>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    Box::pin(async move {
        let start = Utc::now();
        let start_instant = std::time::Instant::now();

        debug!(schedule_id = %schedule_id, "Executing schedule");

        // Load schedule
        let schedule = match store.get(&schedule_id).await {
            Some(s) if s.status == ScheduleStatus::Active => s,
            Some(_) => {
                debug!(schedule_id = %schedule_id, "Schedule no longer active");
                return;
            }
            None => {
                warn!(schedule_id = %schedule_id, "Schedule not found");
                return;
            }
        };

        // Mark as running
        let state = store.get_state(&schedule_id).await.unwrap_or_default();
        let running_state = ScheduleState {
            running_since: Some(start),
            ..state
        };
        store.update_state(&schedule_id, running_state).await;

        // Execute with retry logic
        let max_attempts = schedule
            .retry
            .as_ref()
            .map(|r| r.max_retries + 1)
            .unwrap_or(1);

        let mut result = Err(SchedulerError::ExecutionFailed("not executed".into()));
        let mut attempts_made = 0u8;

        for attempt in 0..max_attempts {
            // Wait before retry (skip for first attempt)
            if attempt > 0
                && let Some(ref retry_config) = schedule.retry
            {
                let delay = retry_config.delay_for_attempt(attempt - 1);
                debug!(
                    schedule_id = %schedule_id,
                    attempt = attempt + 1,
                    max_attempts = max_attempts,
                    delay_ms = delay.as_millis(),
                    "Retrying scheduled task"
                );
                tokio::time::sleep(delay).await;

                // Check if schedule was cancelled during wait
                if store
                    .get(&schedule_id)
                    .await
                    .is_none_or(|s| s.status != ScheduleStatus::Active)
                {
                    debug!(schedule_id = %schedule_id, "Schedule cancelled during retry wait");
                    return;
                }
            }

            attempts_made = attempt + 1;

            result = match &schedule.payload {
                SchedulePayload::Message { message } => {
                    execute_message_payload(&config, &schedule, message).await
                }
                SchedulePayload::Task { task } => {
                    execute_task_payload(&config, &schedule, task).await
                }
            };

            if result.is_ok() {
                break;
            }

            // Log retry attempt failure
            if attempt + 1 < max_attempts {
                warn!(
                    schedule_id = %schedule_id,
                    attempt = attempt + 1,
                    max_attempts = max_attempts,
                    error = ?result.as_ref().err(),
                    "Scheduled task attempt failed, will retry"
                );
            }
        }

        let duration_ms = start_instant.elapsed().as_millis() as u64;

        // Update state and log
        let (status, error) = match &result {
            Ok(_) => (RunStatus::Ok, None),
            Err(e) => {
                let error_msg = if attempts_made > 1 {
                    format!("{} (after {} attempts)", e, attempts_made)
                } else {
                    e.to_string()
                };
                (RunStatus::Error, Some(error_msg))
            }
        };

        // Calculate next run for recurring schedules
        let next_run = if schedule.is_recurring() && result.is_ok() {
            calculate_next_run(&schedule.timing, Some(start))
        } else {
            None
        };

        let final_state = ScheduleState {
            next_run_at: next_run,
            running_since: None,
            last_run_at: Some(start),
            last_status: Some(status),
            last_error: error.clone(),
            last_duration_ms: Some(duration_ms),
        };
        store.update_state(&schedule_id, final_state).await;

        // Log the run
        let entry = RunLogEntry {
            ts: start.timestamp_millis(),
            status,
            duration_ms: Some(duration_ms),
            error,
            next_run_at: next_run.map(|t| t.timestamp_millis()),
        };
        let _ = run_log.append(&schedule_id, &entry).await;

        // Handle completion or rescheduling
        if schedule.is_one_shot() {
            // Mark as completed
            if let Err(e) = store
                .update_status(&schedule_id, ScheduleStatus::Completed)
                .await
            {
                error!(schedule_id = %schedule_id, error = %e, "Failed to mark schedule complete");
            }
            // Remove timer entry
            let mut timers = timers.write().await;
            timers.remove(&schedule_id);

            info!(
                schedule_id = %schedule_id,
                duration_ms = duration_ms,
                "Schedule executed successfully"
            );
        } else if let Some(next) = next_run {
            // Reschedule recurring
            let delay = next
                .signed_duration_since(Utc::now())
                .to_std()
                .unwrap_or(Duration::ZERO);

            let (cancel_tx, cancel_rx) = oneshot::channel();

            {
                let mut timers_lock = timers.write().await;
                timers_lock.insert(schedule_id.clone(), cancel_tx);
            }

            // Clone for the debug statement after spawn
            let schedule_id_for_log = schedule_id.clone();

            tokio::spawn(async move {
                let deadline = Instant::now() + delay;
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => {
                        execute_schedule(schedule_id, store, run_log, config, timers).await;
                    }
                    _ = cancel_rx => {
                        debug!(schedule_id = %schedule_id, "Recurring timer cancelled");
                    }
                }
            });

            if result.is_ok() {
                debug!(
                    schedule_id = %schedule_id_for_log,
                    next_run = %next,
                    duration_ms = duration_ms,
                    "Recurring schedule executed and rescheduled"
                );
            } else {
                error!(
                    schedule_id = %schedule_id_for_log,
                    error = ?result.err(),
                    "Recurring schedule execution failed"
                );
            }
        } else if result.is_err() {
            // Recurring schedule failed and not rescheduled
            error!(
                schedule_id = %schedule_id,
                error = ?result.err(),
                "Schedule execution failed"
            );
        }
    })
}

/// Execute a message payload (simple send, no LLM).
async fn execute_message_payload(
    config: &SchedulerConfigRef,
    schedule: &Schedule,
    message: &str,
) -> Result<()> {
    config
        .gateways
        .send_message(
            &schedule.destination.gateway,
            &schedule.destination.chat_id,
            message,
            None,
        )
        .await
        .map_err(|e| SchedulerError::GatewayUnavailable(e.to_string()))?;

    Ok(())
}

/// Execute a task payload (run agentic loop, send result).
async fn execute_task_payload(
    config: &SchedulerConfigRef,
    schedule: &Schedule,
    task: &str,
) -> Result<()> {
    // Get agent
    let agent = config
        .agents
        .get(&schedule.agent)
        .ok_or_else(|| SchedulerError::AgentNotFound(schedule.agent.clone()))?;

    // Get provider
    let provider = config
        .providers
        .get(&agent.model.provider, agent.model.base_url.as_deref())
        .ok_or_else(|| {
            SchedulerError::ExecutionFailed(format!("Provider not found: {}", agent.model.provider))
        })?;

    // Find or create session for this chat
    let session_id = get_or_create_session(
        config,
        &schedule.destination.gateway,
        &schedule.destination.chat_id,
        &schedule.agent,
    )
    .await?;

    // Build task message
    let task_message = Message::text(
        Role::User,
        format!(
            "[Scheduled Task]\n\n{}\n\n(This is an automated task. Send the result to this chat.)",
            task
        ),
    );

    // Add to session
    if config
        .sessions
        .add_message(&session_id, task_message.clone())
        .await
        .is_err()
    {
        return Err(SchedulerError::ExecutionFailed(
            "Failed to add task message".to_string(),
        ));
    }

    // Record user message event
    let _ = record_event(
        &config.sessions,
        &config.sessions_path,
        &session_id,
        SessionEventPayload::UserMessage {
            content: task_message.content.clone().unwrap_or_default(),
        },
        &config.session_locks,
    )
    .await;

    // Create tool executor (without execution context - schedules don't create nested schedules)
    let executor = ToolExecutor::new(
        agent.tools.clone(),
        config.sandbox.clone(),
        agent.agent_dir.clone(),
        agent.policy.clone(),
        schedule.agent.clone(),
    )
    .with_session_id(session_id.clone());

    // Build messages using StructuredContext
    let history = config
        .sessions
        .get_messages(&session_id)
        .await
        .unwrap_or_default();
    let messages = ContextBuilder::new()
        .from_agent_spec(agent)
        .with_messages(history)
        .build()
        .render(
            &agent.model.name,
            agent.model.temperature,
            agent.model.max_output_tokens,
        )
        .messages;

    // Run agentic loop
    let event_ctx = EventContext {
        sessions: config.sessions.clone(),
        sessions_path: config.sessions_path.clone(),
        session_id: session_id.clone(),
        session_locks: config.session_locks.clone(),
    };

    let result = run_agentic_loop(provider, &executor, agent, messages, &event_ctx)
        .await
        .map_err(|e| SchedulerError::ExecutionFailed(e.to_string()))?;

    let response = match result {
        AgenticResult::Complete { content, usage, .. } => {
            // Persist assistant message
            let _ = persist_assistant_message(
                &config.sessions,
                &config.sessions_path,
                &session_id,
                &schedule.agent,
                content.clone(),
                usage,
                &config.session_locks,
            )
            .await;
            content
        }
        AgenticResult::AwaitingApproval { pending, .. } => {
            // For scheduled tasks, we can't prompt for approval
            // Return a message indicating this
            format!(
                "⏰ Scheduled task requires approval for: `{}`\n\nPlease run this task manually to approve.",
                pending.command
            )
        }
    };

    // Send response via gateway
    config
        .gateways
        .send_message(
            &schedule.destination.gateway,
            &schedule.destination.chat_id,
            &response,
            None,
        )
        .await
        .map_err(|e| SchedulerError::GatewayUnavailable(e.to_string()))?;

    Ok(())
}

/// Get or create a session for the scheduled task.
///
/// Uses the shared `ChatSessionCache` to reuse existing sessions for the same
/// (gateway, chat_id, agent) tuple. This allows scheduled task results to appear
/// in the same conversation as interactive messages.
async fn get_or_create_session(
    config: &SchedulerConfigRef,
    gateway: &str,
    chat_id: &str,
    agent: &str,
) -> Result<String> {
    // Check cache for existing session
    if let Some(session_id) = config.chat_session_cache.get(gateway, chat_id, agent).await {
        // Verify session still exists
        if config.sessions.get(&session_id).await.is_some() {
            debug!(
                session_id = %session_id,
                gateway = %gateway,
                chat_id = %chat_id,
                agent = %agent,
                "Reusing existing session for scheduled task"
            );
            return Ok(session_id);
        }
    }

    // Create new session
    let session = config.sessions.create(agent).await;
    let session_id = session.id.clone();

    // Record session start
    let _ = record_event(
        &config.sessions,
        &config.sessions_path,
        &session_id,
        SessionEventPayload::SessionStart {
            session_id: session_id.clone(),
            agent: agent.to_string(),
        },
        &config.session_locks,
    )
    .await;

    // Store in shared cache
    config
        .chat_session_cache
        .insert(gateway, chat_id, agent, &session_id)
        .await;

    debug!(
        session_id = %session_id,
        gateway = %gateway,
        chat_id = %chat_id,
        agent = %agent,
        "Created session for scheduled task"
    );

    Ok(session_id)
}

/// Validate schedule timing.
fn validate_timing(timing: &ScheduleTiming) -> Result<()> {
    match timing {
        ScheduleTiming::At { at } => {
            if *at <= Utc::now() {
                return Err(SchedulerError::PastTimestamp);
            }
        }
        ScheduleTiming::Every { every_seconds, .. } => {
            if *every_seconds == 0 {
                return Err(SchedulerError::InvalidSchedule(
                    "every_seconds must be > 0".to_string(),
                ));
            }
        }
        ScheduleTiming::Cron { expr, .. } => {
            // Validate cron expression
            cron::Schedule::from_str(expr)
                .map_err(|e| SchedulerError::InvalidCron(e.to_string()))?;
        }
    }
    Ok(())
}

/// Calculate the next run time for a schedule.
fn calculate_next_run(
    timing: &ScheduleTiming,
    after: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    let now = Utc::now();
    let after = after.unwrap_or(now);

    match timing {
        ScheduleTiming::At { at } => {
            if *at > now {
                Some(*at)
            } else {
                None
            }
        }
        ScheduleTiming::Every {
            every_seconds,
            anchor,
        } => {
            let interval_secs = *every_seconds as i64;
            let base = anchor.unwrap_or(after);

            // Calculate next aligned time using modulo arithmetic (O(1))
            let elapsed = (after - base).num_seconds().max(0);
            let intervals_passed = elapsed / interval_secs;
            let next = base + chrono::Duration::seconds((intervals_passed + 1) * interval_secs);
            Some(next)
        }
        ScheduleTiming::Cron { expr, .. } => {
            // Parse cron and find next occurrence
            let schedule = cron::Schedule::from_str(expr).ok()?;
            schedule.after(&after).next()
        }
    }
}

use std::str::FromStr;

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use crate::scheduler::schedule::ScheduleDestination;

    #[test]
    fn validate_timing_rejects_past_timestamp() {
        let timing = ScheduleTiming::At {
            at: Utc::now() - chrono::Duration::hours(1),
        };
        assert!(matches!(
            validate_timing(&timing),
            Err(SchedulerError::PastTimestamp)
        ));
    }

    #[test]
    fn validate_timing_accepts_future_timestamp() {
        let timing = ScheduleTiming::At {
            at: Utc::now() + chrono::Duration::hours(1),
        };
        assert!(validate_timing(&timing).is_ok());
    }

    #[test]
    fn validate_timing_rejects_zero_interval() {
        let timing = ScheduleTiming::Every {
            every_seconds: 0,
            anchor: None,
        };
        assert!(matches!(
            validate_timing(&timing),
            Err(SchedulerError::InvalidSchedule(_))
        ));
    }

    #[test]
    fn validate_timing_accepts_valid_interval() {
        let timing = ScheduleTiming::Every {
            every_seconds: 3600,
            anchor: None,
        };
        assert!(validate_timing(&timing).is_ok());
    }

    #[test]
    fn validate_timing_rejects_invalid_cron() {
        let timing = ScheduleTiming::Cron {
            expr: "not a cron".to_string(),
            tz: None,
        };
        assert!(matches!(
            validate_timing(&timing),
            Err(SchedulerError::InvalidCron(_))
        ));
    }

    #[test]
    fn validate_timing_accepts_valid_cron() {
        // cron crate uses 7-field format: sec min hour day-of-month month day-of-week year
        let timing = ScheduleTiming::Cron {
            expr: "0 0 9 * * MON-FRI *".to_string(),
            tz: None,
        };
        assert!(validate_timing(&timing).is_ok());
    }

    #[test]
    fn calculate_next_run_at_future() {
        let future = Utc::now() + chrono::Duration::hours(1);
        let timing = ScheduleTiming::At { at: future };
        assert_eq!(calculate_next_run(&timing, None), Some(future));
    }

    #[test]
    fn calculate_next_run_at_past() {
        let past = Utc::now() - chrono::Duration::hours(1);
        let timing = ScheduleTiming::At { at: past };
        assert_eq!(calculate_next_run(&timing, None), None);
    }

    #[test]
    fn calculate_next_run_every() {
        let now = Utc::now();
        let timing = ScheduleTiming::Every {
            every_seconds: 3600,
            anchor: Some(now),
        };

        let next = calculate_next_run(&timing, Some(now)).unwrap();
        assert!(next > now);
        assert!((next - now).num_seconds() <= 3600);
    }

    #[test]
    fn calculate_next_run_cron() {
        // cron crate uses 7-field format: sec min hour day-of-month month day-of-week year
        let timing = ScheduleTiming::Cron {
            expr: "0 * * * * * *".to_string(), // Every minute at second 0
            tz: None,
        };

        let next = calculate_next_run(&timing, None).unwrap();
        assert!(next > Utc::now());
    }
}
