# Background Processes

Agents can spawn and manage long-running external commands. Processes run in the background, survive across conversation turns, and optionally use tmux for human observation and interactive input.

## Overview

Background processes let agents handle use cases like:

- "Run the test suite and tell me when it's done"
- "Start the dev server so I can test against it"
- "Compile the project in the background while we keep talking"

## Process Tool

A single built-in `background_process` tool handles all process operations via an `action` parameter:

```yaml
spec:
  tools:
    - type: builtin
      name: background_process
```

### Actions

| Action | Description | Required Parameters |
|--------|-------------|---------------------|
| `spawn` | Start a background command | `command` |
| `list` | List all processes for the current session | — |
| `status` | Get status of a specific process | `handle` |
| `log` | Read process output | `handle`, optional `offset`/`limit` |
| `capture` | Capture screen content | `handle` (interactive only) |
| `send_keys` | Send keystrokes | `handle`, `keys`, optional `press_enter` (interactive only) |
| `write` | Write to process stdin | `handle`, `input` (non-interactive only) |
| `kill` | Terminate a process | `handle` |
| `watch` | Start screen watcher (fires callback when screen stops changing) | `handle`, optional `interval_seconds` (interactive only) |
| `unwatch` | Stop screen watcher | `handle` |

### Spawn Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `command` | string | Yes | — | Shell command (passed to `bash -c`) |
| `workdir` | string | No | — | Working directory |
| `wait` | boolean | No | `false` | Block until the process completes |
| `interactive` | boolean | No | `false` | Run in interactive mode (terminal multiplexer) for human observation and agent interaction |
| `label` | string | No | — | Human-readable label |
| `timeout_seconds` | integer | No | `1800` | Max runtime before the process is killed |

**Returns:** A process handle (e.g., `01hqxyz...`) for use with subsequent actions.

## Execution Modes

### Async (default)

The process runs in the background. The agent gets a handle immediately and can check on it later with the `status` or `log` actions.

When the process finishes, a **completion callback** is injected into the session so the agent can report results back to the user.

### Synchronous (`wait: true`)

The tool call blocks until the process exits (or times out) and returns the full output directly. Useful for short commands where the agent needs the result before continuing.

## Interactive Mode

When `interactive: true` is set and a terminal multiplexer (currently tmux) is available, the process runs inside an interactive session.

| Feature | Plain subprocess | Interactive |
|---------|-----------------|-------------|
| Human can observe | No | `tmux attach -t <session>` |
| Agent reads output | `log` action | `log` or `capture` action |
| Agent sends input | `write` action (stdin) | `send_keys` action |
| Interactive programs | Limited (stdin only) | Full terminal emulation |

tmux availability is detected once at startup. If tmux is not installed, `interactive: true` silently falls back to a plain subprocess.

## Example Scenarios

### Run Tests in Background

1. User: "Run the full test suite and let me know the results"
2. Agent calls `background_process` with `action: "spawn"`, `command: "cargo test"`, `label: "test suite"`
3. Agent continues the conversation
4. When tests finish, the completion callback fires and the agent reports results

### Interactive Dev Server

1. User: "Start the dev server"
2. Agent calls `background_process` with `action: "spawn"`, `command: "npm run dev"`, `interactive: true`
3. Agent (or user via `tmux attach`) can observe the server output
4. Agent calls `background_process` with `action: "capture"` to check server status
5. Agent calls `background_process` with `action: "send_keys"` to interact if needed

### Supervised Process (Watch Pattern)

For processes that need periodic monitoring — like a coding agent that may prompt for input — combine `background_process` with `schedule` to create a watch loop:

1. Agent spawns the process: `background_process` with `action: "spawn"`, `interactive: true`
2. Agent creates a watch schedule: `schedule` with `action: "create"`, `every_seconds: 30`, `process_handle: "{handle}"`, `task: "Check process {handle}. If it needs input, provide it. Otherwise report current status."`
3. Every 30 seconds, the schedule fires a task that inspects the process and acts
4. When the process exits, the schedule is **automatically cancelled** via the `process_handle` link — no manual cleanup needed

The `process_handle` parameter links the schedule to the process lifecycle. When the process completes (or fails/times out/is killed), any schedules with a matching `process_handle` are cancelled before the completion callback fires. This eliminates wasted LLM calls from watch schedules firing after the process is already done.

## Process Lifecycle

Processes can be in one of these states:

| Status | Description |
|--------|-------------|
| `running` | Process is active |
| `completed` | Exited with code 0 |
| `failed` | Exited with non-zero code |
| `timed_out` | Killed after exceeding `timeout_seconds` |
| `killed` | Terminated by agent via `kill` action |
| `lost` | Was running when Duragent restarted, but couldn't be recovered |

## Recovery

Process state is persisted to disk. On restart:

- **Interactive processes** (tmux) that are still running are re-adopted and monitored
- **Plain subprocesses** cannot be re-adopted and are marked as `lost`
- **Already-completed processes** are loaded for query access

Lost processes trigger a completion callback so the agent can inform the user.

## Storage

Process metadata and logs are stored in the workspace:

```
.duragent/
├── processes/
│   ├── proc-01hqxyz.meta.json   # Process metadata
│   ├── proc-01hqxyz.log         # stdout/stderr output
```

Completed processes are automatically cleaned up after 30 minutes.
