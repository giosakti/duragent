# Group Chat

Duragent provides layered access control for DMs and groups, with mention gating, message queuing, and per-sender policies.

## Access Policies

### DM Policies

Control who can send direct messages to your agent:

| Policy | Behavior | Use Case |
|--------|----------|----------|
| `open` | Accept all DMs (default) | Public bot |
| `disabled` | Reject all DMs | Group-only bot |
| `allowlist` | Only listed user IDs | Private bot |

```yaml
spec:
  access:
    dm:
      policy: allowlist
      allowlist: ["12345", "67890"]
```

Allowlists support trailing `*` wildcards (e.g., `user_*`).

### Group Policies

Control which groups the agent participates in:

| Policy | Behavior | Use Case |
|--------|----------|----------|
| `open` | Participate in any group (default) | Public bot |
| `disabled` | Ignore all group messages | DM-only bot |
| `allowlist` | Only listed groups | Controlled deployment |

```yaml
spec:
  access:
    groups:
      policy: allowlist
      allowlist: ["telegram:-100123456", "discord:987654321"]
```

Group allowlists use `{channel}:{chat_id}` composite format. Wildcards are supported (e.g., `telegram:*` for all Telegram groups).

## Mention Gating

In groups, agents use mention gating by default — they only respond when triggered.

| Mode | Behavior |
|------|----------|
| `mention` (default) | Only respond when @mentioned or replied to |
| `always` | Respond to every allowed message |

```yaml
spec:
  access:
    groups:
      activation: mention
```

Mention detection supports:
- Explicit `@bot_username` in message text
- Reply to bot's previous message

## Per-Sender Policies

Within groups, you can assign different dispositions to different senders:

| Disposition | LLM Sees It? | Triggers Response? | Use Case |
|-------------|-------------|-------------------|----------|
| `allow` | Yes | Yes | Normal interaction |
| `passive` | Yes (future turns) | No | Context without triggering |
| `silent` | No | No | Audit trail only |
| `block` | No | No | Spam, bad actors |

```yaml
spec:
  access:
    groups:
      sender_default: silent
      sender_overrides:
        "67890": allow
        "99999": block
        "admin_*": allow    # Wildcard support
```

Resolution order: exact sender ID match, then wildcard pattern match, then `sender_default`.

## Context Buffer

When not mentioned in `mention` mode, messages from `allow` senders are buffered. When the agent is triggered, recent context is injected so it understands the conversation.

```yaml
spec:
  access:
    groups:
      context_buffer:
        mode: silent          # silent (default) | passive
        max_messages: 100     # Max buffered messages
        max_age_hours: 24     # Max age for buffer messages
```

| Mode | Storage | Survives Crash? | Token Cost |
|------|---------|----------------|------------|
| `silent` | In-memory buffer | No | Low (only on trigger) |
| `passive` | Conversation history | Yes | Accumulates over time |

## Message Queue

When messages arrive while the agent is processing, the queue handles them:

```yaml
spec:
  access:
    groups:
      queue:
        mode: batch           # batch (default) | sequential | drop
        max_pending: 10
        overflow: drop_old    # drop_old | drop_new | reject
        debounce:
          enabled: true
          window_ms: 1500
```

### Queue Modes

| Mode | Behavior | Use Case |
|------|----------|----------|
| `batch` | Combine all pending into one request | Rapid message senders |
| `sequential` | Process one at a time until empty | Preserve individual context |
| `drop` | Discard pending messages | Simple, predictable |

### Overflow Strategies

| Strategy | Behavior |
|----------|----------|
| `drop_old` | Evict oldest message to make room |
| `drop_new` | Silently discard the new message |
| `reject` | Discard and send `reject_message` back |

### Debouncing

Per-sender debouncing batches rapid messages before they enter the queue (e.g., user sends 3 messages in 2 seconds — they get combined into one). The first message from an idle sender bypasses debouncing for zero latency.

## Full Example

```yaml
spec:
  access:
    dm:
      policy: allowlist
      allowlist: ["12345"]
    groups:
      policy: allowlist
      allowlist: ["telegram:-100123456"]
      sender_default: passive
      sender_overrides:
        "67890": allow
        "99999": block
      activation: mention
      context_buffer:
        mode: silent
        max_messages: 50
        max_age_hours: 12
      queue:
        mode: sequential
        max_pending: 5
        overflow: reject
        reject_message: "I'm busy, please wait."
```

## Cost Impact

With mention gating, 1000 group messages/day at a 5% mention rate results in only ~50 LLM calls — a 95% reduction compared to responding to every message.
