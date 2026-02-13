# CLI Commands

## Setup

### `duragent init`

Initialize a new Duragent workspace.

```bash
duragent init [path] [flags]

Flags:
      --agent-name string   Name for the starter agent
      --provider string     LLM provider (anthropic, openrouter, openai, ollama)
      --model string        Model name
      --no-interactive      Skip interactive prompts; use defaults
```

**Example:**
```bash
duragent init
duragent init --agent-name my-bot --provider anthropic
```

### `duragent login`

Authenticate with an LLM provider via OAuth. Currently only `anthropic` is supported.

Credentials are stored at `~/.duragent/auth.json` (mode 0600) and take precedence over the corresponding environment variable (e.g. `ANTHROPIC_API_KEY`). Tokens are automatically refreshed when they expire.

```bash
duragent login <provider>
```

**Example:**
```bash
duragent login anthropic
```

## Server

### `duragent serve`

Start the Duragent server.

```bash
duragent serve [flags]

Flags:
      --host string           Host to bind to (overrides config)
  -p, --port int              HTTP port (overrides config)
      --agents-dir string     Path to agents directory (overrides config)
  -c, --config string         Path to config file (default duragent.yaml)
      --ephemeral <SECONDS>   Auto-shutdown after N seconds with no active sessions
```

**Example:**
```bash
duragent serve
duragent serve --port 9090
```

### `duragent serve stop`

Stop a running server.

```bash
duragent serve stop
```

### `duragent serve reload-agents`

Reload agent configurations from disk without restarting the server.

```bash
duragent serve reload-agents
```

## Sessions

### `duragent chat`

Start an interactive chat session with an agent.

```bash
duragent chat [flags]

Flags:
  -a, --agent string      Agent name (required)
      --agents-dir string  Path to agents directory (overrides config)
  -c, --config string     Path to config file (default duragent.yaml)
  -s, --server string     Connect to a specific server URL
```

**Examples:**
```bash
duragent chat --agent my-assistant
```

### `duragent attach`

Attach to an existing session (like tmux attach).

```bash
duragent attach [SESSION_ID] [flags]

Flags:
  -l, --list              List all attachable sessions
      --agents-dir string  Path to agents directory (overrides config)
  -c, --config string     Path to config file (default duragent.yaml)
  -s, --server string     Connect to a specific server URL
```

**Examples:**
```bash
duragent attach --list
duragent attach SESSION_ID
```

When attaching to a session with `on_disconnect: continue`, you'll see any output that was buffered while you were away.

## Interactive Commands

Within `duragent chat`, these commands are available:

| Command | Description |
|---------|-------------|
| `/quit` or `/exit` | End session |
| `Ctrl+D` | Detach from session (EOF) |
