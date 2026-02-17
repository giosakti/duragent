//! Policy extension: persistence and concurrency helpers.
//!
//! Domain types live in `duragent-types`. Evaluation logic in `policy_eval.rs`.
//! This module provides only the PolicyStore-dependent operations.

use super::policy_eval::ToolPolicyEval;
use crate::agent::ToolType;
use crate::store::PolicyStore;
use crate::sync::KeyedLocks;

/// Per-agent locks for policy file writes.
///
/// Prevents concurrent writes from overwriting each other.
/// Different agents can write concurrently without contention.
pub type PolicyLocks = KeyedLocks;

/// Atomically add a pattern to the allow list and save.
///
/// This is the recommended way to modify policies as it:
/// 1. Acquires a per-agent lock to prevent concurrent writes
/// 2. Reloads the latest policy from storage
/// 3. Adds the new pattern
/// 4. Saves atomically
///
/// This prevents race conditions where concurrent calls would overwrite
/// each other's changes.
pub async fn add_policy_pattern_and_save(
    policy_store: &dyn PolicyStore,
    agent_name: &str,
    tool_type: ToolType,
    pattern: &str,
    policy_locks: &PolicyLocks,
) -> Result<(), crate::store::StorageError> {
    // Hold lock while reading, modifying, and writing
    let lock = policy_locks.get(agent_name);
    let _guard = lock.lock().await;

    // Load current policy (includes any concurrent changes)
    let mut policy = policy_store.load(agent_name).await;
    policy.add_allow_pattern(tool_type, pattern);
    policy_store.save(agent_name, &policy).await
}
