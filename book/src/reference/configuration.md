# Configuration

Duragent is configured via `duragent.yaml` (server-level) and `agent.yaml` (per-agent). This page covers server configuration; for agent configuration see [Agent Format](../guides/agent-format.md).

## Environment Variable Interpolation

Configuration files support shell-style environment variable expansion:

| Syntax | Behavior |
|--------|----------|
| `${VAR}` | Required — errors if not set |
| `${VAR:-default}` | Optional — uses default if not set |
| `${VAR:-}` | Optional — empty string if not set |

## Environment Variables

Duragent reads certain environment variables directly at startup, independent of config file interpolation.

### LLM Provider Keys

These are read directly from the environment — there is no YAML config equivalent. Anthropic also supports OAuth login via `duragent login anthropic`, which stores credentials at `~/.duragent/auth.json` and takes precedence over the environment variable. See [Authentication](../guides/authentication.md) for details.

| Variable | Required | Description |
|----------|----------|-------------|
| `ANTHROPIC_API_KEY` | No | API key for Anthropic (Claude). Not needed if using OAuth login. |
| `OPENAI_API_KEY` | No | API key for OpenAI-compatible providers |
| `OPENROUTER_API_KEY` | No | API key for OpenRouter |

At least one LLM provider must be configured for agents to function.

### Tool Keys

These are read directly from the environment — there is no YAML config equivalent.

| Variable | Required | Description |
|----------|----------|-------------|
| `BRAVE_API_KEY` | No | Brave Search API key. Enables the `web_search` built-in tool. |

### Gateway Tokens

Gateway tokens are handled differently depending on how you run the gateway:

- **In-process** (compiled-in feature): the token comes from the `bot_token` field in `duragent.yaml`, which supports `${VAR}` interpolation.
- **Standalone binary**: the token is read directly from the environment variable below.

These are separate code paths — they do not conflict.

| Variable | Binary | Description |
|----------|--------|-------------|
| `DISCORD_BOT_TOKEN` | `duragent-discord` | Discord bot token |
| `TELEGRAM_BOT_TOKEN` | `duragent-telegram` | Telegram bot token |

> **Tip:** When running gateways as external plugins (via `gateways.external[]`), you can forward these through the `env` field in `duragent.yaml` using `${VAR}` interpolation instead of relying on the host environment.

## duragent.yaml

### Full Example

```yaml
# Workspace root (default: .duragent)
# workspace: .duragent

# Server
server:
  host: 0.0.0.0
  port: 8080
  request_timeout_seconds: 300
  idle_timeout_seconds: 60
  keep_alive_interval_seconds: 15
  admin_token: ${ADMIN_TOKEN:-}
  api_token: ${API_TOKEN:-}

# Agent directory (optional, defaults to {workspace}/agents)
# agents_dir: .duragent/agents

# Services
services:
  session:
    # path: .duragent/sessions

# World memory
world_memory:
  # path: .duragent/memory/world

# Sessions
sessions:
  ttl_hours: 168
  compaction: discard

# Gateways
gateways:
  telegram:
    enabled: true
    bot_token: ${TELEGRAM_BOT_TOKEN}

  external:
    - name: discord
      command: /usr/local/bin/duragent-discord
      args: ["--verbose"]
      env:
        DISCORD_BOT_TOKEN: ${DISCORD_BOT_TOKEN}
      restart: on_failure

# Routes
routes:
  - match:
      gateway: telegram
      sender_id: "123456789"
    agent: personal-assistant

  - match:
      gateway: telegram
      chat_type: group
    agent: group-moderator

  - match: {}
    agent: default-assistant

# Sandbox
sandbox:
  mode: trust
```

## Fields Reference

### Server

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `server.host` | string | `127.0.0.1` | Bind address |
| `server.port` | u16 | `8080` | HTTP port |
| `server.request_timeout_seconds` | u64 | `300` | Non-streaming request timeout |
| `server.idle_timeout_seconds` | u64 | `60` | SSE idle timeout |
| `server.keep_alive_interval_seconds` | u64 | `15` | SSE keep-alive interval |
| `server.admin_token` | string? | none | Admin API token |
| `server.api_token` | string? | none | API token. If set, API endpoints require this token. If not set, only localhost requests are accepted. |
| `server.max_connections` | usize | `1024` | Maximum concurrent connections |

### Workspace

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `workspace` | path? | `.duragent` | Workspace root directory |
| `agents_dir` | path? | `{workspace}/agents` | Agent definitions directory |
| `services.session.path` | path? | `{workspace}/sessions` | Session storage directory |
| `world_memory.path` | path? | `{workspace}/memory/world` | Shared world memory directory |

### Sessions

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `sessions.ttl_hours` | u64 | `168` | Hours of inactivity before session expiry. `0` disables. |
| `sessions.compaction` | enum | `discard` | `discard`, `archive`, or `disabled` |

### Gateways

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `gateways.telegram.enabled` | bool | `true` | Enable Telegram gateway (requires `gateway-telegram` feature) |
| `gateways.telegram.bot_token` | string | required | Telegram bot token |
| `gateways.discord.enabled` | bool | `true` | Enable Discord gateway (requires `gateway-discord` feature) |
| `gateways.discord.bot_token` | string | required | Discord bot token |
| `gateways.external[].name` | string | required | Gateway identifier |
| `gateways.external[].command` | string | required | Path to gateway binary |
| `gateways.external[].args` | array | `[]` | Command arguments |
| `gateways.external[].env` | map | `{}` | Environment variables |
| `gateways.external[].restart` | enum | `on_failure` | `always`, `on_failure`, or `never` |

### Routes

| Field | Type | Description |
|-------|------|-------------|
| `routes[].match` | object | Match conditions (all must match, AND logic) |
| `routes[].agent` | string | Agent to route to |

**Match conditions:**

| Field | Description |
|-------|-------------|
| `gateway` | Gateway name (`telegram`, `discord`) |
| `chat_type` | `dm`, `group`, or `channel` |
| `chat_id` | Specific chat ID |
| `sender_id` | Specific user ID |

Routes are evaluated top-to-bottom; first match wins. An empty `match: {}` acts as a catch-all.

### Sandbox

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `sandbox.mode` | string | `trust` | `trust` (only supported mode; `bubblewrap` and `docker` are planned) |

## Path Resolution

All relative paths are resolved relative to the config file directory, not the current working directory. When optional path fields are omitted, they default to subdirectories of the workspace.

## Context Window Management

Context window settings are configured per-agent in `agent.yaml` under `spec.session.context`. See [Agent Format > session.context](../guides/agent-format.md) for details.

Duragent automatically detects context window sizes from model names when `max_input_tokens` is not set. Supported model families include Claude, GPT-4/5, Gemini, Grok, DeepSeek, Qwen, Llama, and Mistral.
