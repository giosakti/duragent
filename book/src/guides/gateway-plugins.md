# Gateway Plugins

Gateway plugins enable Duragent to communicate with messaging platforms. They run as separate processes and communicate via the Gateway Protocol (JSON Lines over stdio).

## Configuration

Gateway plugins are configured in `duragent.yaml`:

```yaml
gateways:
  # Built-in Telegram gateway
  telegram:
    enabled: true
    bot_token: ${TELEGRAM_BOT_TOKEN}

  # External gateways (subprocess plugins)
  external:
    - name: discord
      command: /usr/local/bin/duragent-discord
      args: ["--verbose"]
      env:
        DISCORD_TOKEN: ${DISCORD_TOKEN}
      restart: on_failure

    - name: custom-gateway
      command: ./my-gateway
      restart: always
```

### External Gateway Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Gateway identifier |
| `command` | string | required | Path to gateway binary |
| `args` | array | `[]` | Command arguments |
| `env` | map | `{}` | Environment variables |
| `restart` | enum | `on_failure` | `always`, `on_failure`, or `never` |

## Agent Routing

Routing rules determine which agent handles messages from each gateway. Rules are global and evaluated in order:

```yaml
routes:
  - match:
      gateway: telegram
      sender_id: "123456789"
    agent: personal-assistant

  - match:
      gateway: telegram
      chat_type: group
    agent: group-moderator

  - match: {}    # Catch-all
    agent: default-assistant
```

### Match Conditions

| Field | Description | Examples |
|-------|-------------|---------|
| `gateway` | Gateway name | `telegram`, `discord` |
| `chat_type` | Conversation type | `dm`, `group`, `channel` |
| `chat_id` | Specific chat ID | `-1001234567890` |
| `sender_id` | Specific user ID | `123456789` |

All conditions in a rule must match (AND logic). First match wins. An empty `match: {}` acts as a catch-all.

## Process Management

Duragent manages plugin lifecycle:

- **Startup** — Plugins are spawned when the server starts
- **Health checks** — Periodic pings to verify plugin is responsive
- **Restart** — Configurable restart policy on crash
- **Shutdown** — Graceful shutdown signal, then force kill
- **Orphan prevention** — On Linux, plugins die when Duragent dies (via `prctl`)

## Writing a Custom Gateway

Custom gateways implement the Gateway Protocol — JSON Lines over stdin/stdout. You can write them in any language.

### Protocol Messages

**Events (gateway to Duragent):**

| Event | Purpose |
|-------|---------|
| `ready` | Gateway initialized |
| `message_received` | Incoming user message |
| `command_ok` | Command succeeded |
| `command_error` | Command failed |
| `shutdown` | Gateway terminating |

**Commands (Duragent to gateway):**

| Command | Purpose |
|---------|---------|
| `send_message` | Send text to a chat |
| `send_typing` | Show typing indicator |
| `edit_message` | Edit a previously sent message |
| `ping` | Health check |
| `shutdown` | Graceful termination request |

### Message Metadata

Gateways provide metadata with incoming messages:

- `chat_id` — Platform-specific chat identifier
- `chat_type` — `dm`, `group`, or `channel`
- `sender_id` — User identifier
- `mentions_bot` — Whether the bot was @mentioned
- `reply_to_bot` — Whether this is a reply to the bot's message

See the `duragent-gateway-protocol` crate for the full type definitions.
