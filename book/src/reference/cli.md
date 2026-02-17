# CLI Commands

## Global Flags

These flags can be used with any command:

```bash
Flags:
  -v, --verbose     Increase log verbosity (-v = debug, -vv = trace)
  -q, --quiet       Suppress all log output (errors only)
      --version     Print version info (includes variant, commit, build date)
  -h, --help        Print help
```

**Examples:**
```bash
duragent -v serve              # Debug-level logging
duragent -vv serve             # Trace-level logging
duragent -q serve              # Only error output
duragent --version             # e.g. "0.5.3 (core, commit: abc1234, built: 2026-02-17)"
```

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

### `duragent serve status`

Check if a server is running and show its status.

```bash
duragent serve status [flags]

Flags:
  -c, --config string     Path to config file (default duragent.yaml)
  -p, --port int          Port override
```

**Example:**
```bash
duragent serve status
duragent serve status --port 9090
```

## Agents

### `duragent agent create`

Create a new agent in an existing workspace. Like `duragent init` but adds a single agent without touching the rest of the workspace.

```bash
duragent agent create <name> [flags]

Flags:
      --provider string     LLM provider (anthropic, openrouter, openai, ollama)
      --model string        Model name
  -c, --config string       Path to config file (default duragent.yaml)
      --no-interactive      Skip interactive prompts; use defaults
```

**Examples:**
```bash
duragent agent create research-bot
duragent agent create my-bot --provider anthropic --model claude-sonnet-4-20250514
duragent agent create quick-bot --no-interactive
```

### `duragent agent list`

List all available agents on a running server.

```bash
duragent agent list [flags]

Flags:
  -c, --config string       Path to config file (default duragent.yaml)
      --agents-dir string   Path to agents directory (overrides config)
  -s, --server string       Connect to a specific server URL
```

**Example:**
```bash
duragent agent list
duragent agent list --server http://localhost:9090
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

#### Interactive Commands

Within `duragent chat`, these commands are available:

| Command | Description |
|---------|-------------|
| `/quit` or `/exit` | End session |
| `/reset` | End current session; next message starts fresh |
| `/status` | Show current session info |
| `Ctrl+D` | Detach from session (EOF) |

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

### `duragent session list`

List all sessions on a running server.

```bash
duragent session list [flags]

Flags:
  -c, --config string       Path to config file (default duragent.yaml)
      --agents-dir string   Path to agents directory (overrides config)
  -s, --server string       Connect to a specific server URL
```

**Example:**
```bash
duragent session list
```

### `duragent session delete`

Delete a session.

```bash
duragent session delete <SESSION_ID> [flags]

Flags:
  -c, --config string       Path to config file (default duragent.yaml)
      --agents-dir string   Path to agents directory (overrides config)
  -s, --server string       Connect to a specific server URL
```

**Example:**
```bash
duragent session delete 01JMABCD1234
```

## Maintenance

### `duragent doctor`

Diagnose installation and configuration issues. Checks config files, agents, gateways, provider credentials, and security settings.

```bash
duragent doctor [flags]

Flags:
  -c, --config string     Path to config file (default duragent.yaml)
      --format string     Output format: text or json (default text)
```

**Examples:**
```bash
duragent doctor
duragent doctor --format json
```

### `duragent upgrade`

Upgrade duragent to the latest version (or a specific version) by downloading the platform binary from GitHub Releases. Verifies checksums when available and replaces the binary atomically.

```bash
duragent upgrade [flags]

Flags:
      --check             Only check for updates, don't install
      --version string    Target version (e.g., "0.6.0" or "v0.6.0")
      --full              Download full variant (includes gateway binaries)
      --core              Download core variant only (no gateway binaries)
      --restart           Restart a running server after upgrade
  -c, --config string     Path to config file (default duragent.yaml, used with --restart)
  -p, --port int          Port override (used with --restart)
      --format string     Output format: text or json (default text)
```

**Examples:**
```bash
duragent upgrade --check
duragent upgrade
duragent upgrade --version 0.4.0
duragent upgrade --restart
duragent upgrade --format json
```

With `--restart`, the command shuts down the running server gracefully, then execs the new binary with the same serve arguments. Sessions are flushed to disk before shutdown and recovered on startup.

## Utilities

### `duragent completions`

Generate shell completions for your shell.

```bash
duragent completions <SHELL>
```

Supported shells: `bash`, `zsh`, `fish`, `elvish`, `powershell`

**Examples:**
```bash
# Bash
duragent completions bash > ~/.local/share/bash-completion/completions/duragent

# Zsh
duragent completions zsh > ~/.zfunc/_duragent

# Fish
duragent completions fish > ~/.config/fish/completions/duragent.fish
```
