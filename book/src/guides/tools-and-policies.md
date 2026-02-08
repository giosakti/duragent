# Tools and Policies

Duragent agents can use tools to interact with the outside world. A policy system controls which tools are allowed, with optional human-in-the-loop approval.

## Tool Types

| Type | Description | Best For |
|------|------------|----------|
| **Built-in** | Bundled with Duragent | Core operations (e.g., `bash`) |
| **CLI** | Custom scripts with optional README | Simple extensions, any language |
| **MCP** | Model Context Protocol servers | Complex integrations *(planned)* |

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
| `schedule_task` | Create a scheduled task |
| `list_schedules` | List active schedules |
| `cancel_schedule` | Cancel a schedule by ID |

Memory tools (`recall`, `remember`, `reflect`, `update_world`) are automatically registered when memory is configured. See [Memory](./memory.md).

### CLI Tools

CLI tools are scripts or binaries that the agent can call. They're more token-efficient than MCP because the agent reads the README only when it needs the tool (no upfront schema exchange).

| Field | Required | Description |
|-------|----------|-------------|
| `type` | Yes | `cli` |
| `name` | Yes | Tool identifier |
| `command` | Yes | Script path (relative to agent directory) |
| `description` | No | Short description shown to LLM |
| `readme` | No | Path to README (loaded on demand) |

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

Patterns use `tool_type:pattern` with glob-style matching:

| Pattern | Matches |
|---------|---------|
| `bash:cargo *` | Bash commands starting with "cargo" |
| `mcp:github:*` | Any tool from github MCP server |
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
