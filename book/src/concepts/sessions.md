# Sessions

Sessions are the core durability primitive in Duragent. A session is a persistent conversation context that survives crashes, restarts, and disconnects.

## Why Sessions Matter

| Without Sessions | With Sessions |
|------------------|---------------|
| Crash = context lost | Crash = reconnect and continue |
| One client only | Access from CLI, HTTP, Telegram, web |
| Tied to terminal | Portable across devices |
| No history API | Query conversation history programmatically |

## Session States

| State | Description |
|-------|-------------|
| `active` | Running, client connected |
| `running` | Running, client disconnected (continue mode) |
| `paused` | Waiting for reconnect (pause mode) |
| `ended` | Completed or explicitly ended |

## Disconnect Behavior

When a client disconnects, the agent's behavior is configurable per-agent:

| Mode | Behavior | Use Case |
|------|----------|----------|
| `continue` | Agent keeps executing, buffers output | Async workflows, fire-and-forget tasks |
| `pause` | Agent pauses, waits for reconnect | Interactive chat |

```yaml
# agent.yaml
spec:
  session:
    on_disconnect: pause  # or: continue
```

## Multi-Gateway Access

Sessions are accessible from any gateway — the same session can be started from CLI, continued via Telegram, and queried via HTTP API.

## Storage

Each session is stored as files:

```
.duragent/sessions/{session_id}/
├── events.jsonl     # Append-only event log
└── state.yaml       # Snapshot for fast resume
```

- **events.jsonl** — Every message, tool call, and status change is appended here
- **state.yaml** — Periodic snapshot for fast resume (no need to replay all events)

## Session Lifecycle

### TTL and Expiry

Sessions can be configured to expire after a period of inactivity:

```yaml
# duragent.yaml
sessions:
  ttl_hours: 168    # 7 days (default). 0 disables auto-expiry.
```

Per-agent TTL overrides are also supported in `agent.yaml`.

### Event Log Compaction

To prevent unbounded growth, Duragent compacts the event log after each snapshot:

```yaml
# duragent.yaml
sessions:
  compaction: discard   # discard (default) | archive | disabled
```

| Mode | Behavior |
|------|----------|
| `discard` | Remove events covered by the snapshot |
| `archive` | Move old events to `events.archive.jsonl` before removing |
| `disabled` | No compaction (events grow unbounded) |

### Gateway Commands

In gateway chats (Telegram, Discord), you can use slash commands:

| Command | Behavior |
|---------|----------|
| `/reset` | End current session; next message starts fresh |
| `/status` | Show current session info |
