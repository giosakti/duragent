# Scheduling

Agents can create time-based triggers to proactively send messages or execute tasks. Schedules persist across restarts and deliver results via any configured gateway.

## Overview

Scheduling lets agents handle use cases like:

- "Remind me to check deployment status tomorrow at 9am"
- "Send me a daily standup prompt on weekdays"
- "Follow up on this issue in 2 hours"

## Schedule Types

| Type | Description | Example |
|------|-------------|---------|
| One-shot (`at`) | Fire once at a specific time | `2026-02-01T09:00:00Z` |
| Interval (`every`) | Repeat every N seconds | `3600` (hourly) |
| Cron | Standard cron expression | `0 9 * * MON-FRI` (9am weekdays) |

## Schedule Tools

Three built-in tools are available for scheduling:

### schedule_task

Create a new schedule. The agent specifies the timing, destination, and payload.

### list_schedules

List all active schedules for the current agent.

### cancel_schedule

Cancel an active schedule by its ID.

## Payload Types

| Type | Description |
|------|-------------|
| `message` | Send text directly (no LLM call) |
| `task` | Execute with agent tools, summarize results |

## Session Continuity

Scheduled messages inject into the active session for a `(chat_id, agent)` pair if one exists. If not, a new session is created. This means scheduled messages appear in the same conversation as user messages — enabling natural follow-up.

## Storage

Schedules are persisted as YAML files:

```
.duragent/
├── schedules/
│   ├── sched_01HQXYZ.yaml      # Schedule definition
│   └── runs/
│       └── sched_01HQXYZ.jsonl  # Run history
```

### Schedule File Format

```yaml
id: reminder-abc123
agent: my-assistant
created_by_session: session_xyz
destination:
  gateway: telegram
  chat_id: "123456789"
schedule:
  at: "2026-01-30T16:00:00Z"    # one-shot
  # or: cron: "0 9 * * MON-FRI" # recurring
payload:
  message: "Reminder: review the PR"
```

## Example Scenarios

### One-Time Reminder

1. User: "Remind me to review the PR in 2 hours"
2. Agent calls `schedule_task` with `at: "2026-01-30T16:00:00Z"`
3. Two hours later, user receives: "Reminder: review the PR"

### Recurring Prompt

1. User: "Send me a daily standup prompt at 9am on weekdays"
2. Agent calls `schedule_task` with `cron: "0 9 * * MON-FRI"`
3. Every weekday at 9am, user receives the prompt
4. User replies, and the conversation continues naturally

## Retry

Schedules support optional retry with exponential backoff and jitter for transient failures (e.g., LLM provider 503).
