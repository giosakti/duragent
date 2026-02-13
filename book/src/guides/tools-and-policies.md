# Tools and Policies

Duragent agents can use tools to interact with the outside world. A policy system controls which tools are allowed, with optional human-in-the-loop approval.

## Tool Types

| Type | Description | Best For |
|------|------------|----------|
| **Built-in** | Bundled with Duragent | Core operations (e.g., `bash`) |
| **CLI** | Custom scripts with optional README | Simple extensions, any language |
| **MCP** | Model Context Protocol servers | Complex integrations *(planned)* |

CLI tools can be declared explicitly in `agent.yaml` or [auto-discovered](#convention-based-tool-discovery) from `tools/` directories.

## Configuration

```yaml
# agent.yaml
spec:
  tools:
    # Built-in tool
    - type: builtin
      name: bash

    # CLI tool
    - type: cli
      name: code-search
      command: ./tools/code-search.sh
      description: Search codebase for patterns
      readme: ./tools/code-search/README.md
```

### Built-in Tools

| Name | Description |
|------|-------------|
| `bash` | Execute shell commands in sandbox |
| `reload_tools` | Re-scan tool directories and register newly discovered tools |
| `web_search` | Search the web (requires `BRAVE_API_KEY`) |
| `web_fetch` | Fetch a URL and convert to Markdown |
| `schedule_task` | Create a scheduled task |
| `list_schedules` | List active schedules |
| `cancel_schedule` | Cancel a schedule by ID |

Memory tools (`recall`, `remember`, `reflect`, `update_world`) are automatically registered when memory is configured. See [Memory](./memory.md).

#### reload_tools

Re-scans tool directories and makes newly discovered tools available. Call this after writing a new tool script to disk so the agent can use it immediately.

- **Parameters:** none
- **Scans:** agent `tools/` directory and workspace `tools/` directory
- **Returns:** JSON summary of all discovered tools

```yaml
spec:
  tools:
    - type: builtin
      name: reload_tools
```

#### web_search

Searches the web using the [Brave Search API](https://brave.com/search/api/).

- **Parameters:** `query` (string, required), `count` (integer, 1–20, default 5)
- **Requires:** `BRAVE_API_KEY` environment variable. If not set, the tool is silently skipped when registering.
- **Timeout:** 30 seconds

```yaml
spec:
  tools:
    - type: builtin
      name: web_search
```

#### web_fetch

Fetches a web page and converts HTML to Markdown.

- **Parameters:** `url` (string, required — `http` and `https` only)
- **Download limit:** 1 MB response body
- **Output limit:** 50 KB sent to the LLM (truncated with notice if larger)
- **Timeout:** 30 seconds
- **No API key required** — always available as a built-in tool

```yaml
spec:
  tools:
    - type: builtin
      name: web_fetch
```

### CLI Tools

CLI tools are scripts or binaries that the agent can call. They're more token-efficient than MCP because the agent reads the README only when it needs the tool (no upfront schema exchange).

| Field | Required | Description |
|-------|----------|-------------|
| `type` | Yes | `cli` |
| `name` | Yes | Tool identifier |
| `command` | Yes | Script path (relative to agent directory) |
| `description` | No | Short description shown to LLM |
| `readme` | No | Path to README (loaded on demand) |

### Convention-Based Tool Discovery

Tools can be auto-discovered from directories without declaring them in `agent.yaml`. Place a subdirectory with a `run` script inside a `tools/` directory, and Duragent picks it up automatically.

#### Directory Convention

```
tools/
  my-tool/
    run.sh          # Any executable named "run" or "run.*" (run.sh, run.py, etc.)
    README.md       # Optional — loaded as the tool description
```

- **Tool name** = directory name (e.g., `tools/code-search/` becomes a tool named `code-search`)
- **Executable** = any file named `run` or `run.*` inside the subdirectory
- **Description** = content of `README.md` if present; otherwise a default description is used

#### Discovery Directories

Duragent scans two directories for tools:

| Directory | Scope | Path |
|-----------|-------|------|
| Agent tools | Per-agent | `agents/<name>/tools/` |
| Workspace tools | Shared across agents | `.duragent/tools/` |

#### Precedence Rules

1. **Explicit tools win** — tools declared in `agent.yaml` take precedence over discovered tools with the same name
2. **First directory wins** — if both agent and workspace directories contain a tool with the same name, the agent-level tool is used
3. **Policy applies** — discovered tools are subject to the same [tool policy](#tool-policy) as all other tools (matched as `cli:<tool-name>`)

#### Dynamic Discovery with `reload_tools`

Agents can create new tools at runtime and make them available immediately:

1. Agent writes a new tool script to disk (e.g., `tools/my-new-tool/run.sh`)
2. Agent calls `reload_tools`
3. Duragent re-scans the tool directories and registers any new tools
4. The new tool is available for use in the same session

This enables agents to extend their own capabilities during a conversation.

## Tool Policy

The policy system controls which tools agents can execute. It supports three modes with a deny list safety net.

### File Layout

```
agents/my-agent/
  agent.yaml
  policy.yaml           # Base policy (version controlled)
  policy.local.yaml     # User overrides (gitignored, auto-created)
```

### Policy Modes

| Mode | Deny List | Allow List | Unknown Commands |
|------|-----------|------------|------------------|
| `dangerous` | Blocks (always) | Ignored | Allowed |
| `ask` | Blocks (always) | Auto-approved | Requires human approval |
| `restrict` | Blocks (always) | Allowed | Denied |

The deny list is always checked first regardless of mode — it acts as an air-gap safety mechanism.

### Example Policy

```yaml
# policy.yaml
apiVersion: duragent/v1alpha1
kind: Policy

mode: ask

deny:
  - "bash:rm -rf /*"
  - "bash:*sudo*"
  - "*:*password*"

allow:
  - "bash:cargo *"
  - "bash:git *"
  - "mcp:github:*"

notify:
  enabled: true
  patterns:
    - "bash:git push*"
  deliveries:
    - type: log
    - type: webhook
      url: https://hooks.slack.com/services/...
```

### Pattern Format

Patterns use `tool_type:pattern` with glob-style matching.

#### Available Tool Types

| Tool type | Matches | Invocation string |
|-----------|---------|-------------------|
| `bash` | The `bash` built-in tool | The shell command (e.g., `cargo test`) |
| `builtin` | Built-in tools (e.g., `web_search`, `reload_tools`, memory tools) | The tool name (e.g., `web_search`) |
| `cli` | CLI tools and auto-discovered tools | The tool name (e.g., `code-search`) |
| `mcp` | MCP server tools *(planned)* | — |
| `*` | Any tool type | — |

#### Examples

| Pattern | Matches |
|---------|---------|
| `bash:cargo *` | Bash commands starting with "cargo" |
| `cli:code-search` | A CLI/discovered tool named "code-search" |
| `cli:deploy*` | Any CLI tool starting with "deploy" |
| `builtin:web_*` | Built-in tools starting with "web_" |
| `*:*secret*` | "secret" in any tool type |

### Approval Flow (Ask Mode)

When a command isn't in the allow or deny list:

1. Agent requests tool invocation
2. User sees an approval prompt
3. User chooses: **Allow Once**, **Allow Always**, or **Deny**
4. "Allow Always" saves the pattern to `policy.local.yaml` for future auto-approval

### Merge Behavior

When both `policy.yaml` and `policy.local.yaml` exist:

| Field | Strategy |
|-------|----------|
| `mode` | Local overrides base (unless `dangerous`) |
| `deny` | Lists merged (union) |
| `allow` | Lists merged (union) |
| `notify` | Lists merged |

### Default Behavior

When no policy files exist, the default is `dangerous` mode with no filtering — matching pre-policy behavior.

## Notifications

You can configure notifications for specific tool patterns:

```yaml
notify:
  enabled: true
  patterns:
    - "bash:rm *"
    - "bash:git push*"
  deliveries:
    - type: log
    - type: webhook
      url: https://hooks.slack.com/services/...
```

Notifications don't block execution — they fire after the command runs.
