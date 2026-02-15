# Agent Format

The Duragent Format defines a portable, human-readable specification for AI agents. Agents are defined with YAML for structure and Markdown for prose — no code required.

## Directory Layout

```
.duragent/agents/my-agent/
├── agent.yaml              # Agent definition (required)
├── SOUL.md                 # "Who the agent IS" (identity and personality)
├── SYSTEM_PROMPT.md        # "What the agent DOES" (core system prompt)
├── INSTRUCTIONS.md         # Additional runtime instructions (optional)
├── policy.yaml             # Tool policy (optional, version controlled)
├── policy.local.yaml       # User policy overrides (gitignored)
├── skills/                 # Skills directory (optional)
│   └── task-extraction/
│       └── SKILL.md
├── memory/                 # Agent's long-term memory
│   ├── MEMORY.md
│   └── daily/
└── tools/                  # Auto-discovered tools (optional)
    └── my-tool/
        ├── run.sh          # Executable (run, run.sh, run.py, etc.)
        └── README.md       # Tool description (optional)
```

## Agent Definition

### Minimal Example

```yaml
apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: simple-assistant
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
  system_prompt: ./SYSTEM_PROMPT.md
```

### Full Example

```yaml
apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: productivity-assistant
  description: Personal productivity assistant with task management
  version: 1.0.0
  labels:
    domain: productivity

spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
    temperature: 0.7
    max_input_tokens: 100000
    max_output_tokens: 4096
    # base_url: https://custom-endpoint.example.com/v1

  soul: ./SOUL.md
  system_prompt: ./SYSTEM_PROMPT.md
  instructions: ./INSTRUCTIONS.md

  session:
    on_disconnect: continue
    max_tool_iterations: 10
    llm_timeout_seconds: 300
    ttl_hours: 48
    compaction: archive
    context:
      max_history_tokens: 20000
      max_tool_result_tokens: 8000
      tool_result_truncation: head
      tool_result_keep_first: 2
      tool_result_keep_last: 5

  access:
    dm:
      policy: allowlist
      allowlist: ["123456789"]
    groups:
      policy: allowlist
      allowlist: ["telegram:-100123456"]
      activation: mention
      sender_default: allow
      sender_overrides:
        "999999": block
        "888888": passive
      context_buffer:
        mode: silent
        max_messages: 100
        max_age_hours: 24
      queue:
        mode: batch
        max_pending: 10
        overflow: drop_old
        # reject_message: "I'm busy, please wait."
        debounce:
          enabled: true
          window_ms: 1500

  memory:
    backend: filesystem

  tools:
    - type: builtin
      name: bash
    - type: builtin
      name: web
    - type: builtin
      name: schedule
    - type: builtin
      name: background_process
    - type: builtin
      name: session
    - type: builtin
      name: reload_tools
    - type: cli
      name: git-helper
      command: ./tools/git-helper/script.sh
      description: Run git operations
      readme: ./tools/git-helper/README.md

  skills_dir: ./skills/
```

## Fields Reference

### metadata

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Unique identifier (alphanumeric + hyphens) |
| `description` | string | No | Human-readable description |
| `version` | string | No | Semantic version |
| `labels` | map | No | Key-value labels for filtering |

### spec.model

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `provider` | string | Yes | `openrouter`, `openai`, `anthropic`, or `ollama` |
| `name` | string | Yes | Model name/identifier |
| `temperature` | float | No | Sampling temperature (0-2, default 0.7) |
| `max_input_tokens` | int | No | Cap input tokens (for cost control) |
| `max_output_tokens` | int | No | Max response tokens |
| `base_url` | string | No | Override provider's base URL |

### Prompt Files

| Field | Points To | Purpose |
|-------|-----------|---------|
| `soul` | Markdown file | "Who the agent IS" — identity and personality |
| `system_prompt` | Markdown file | "What the agent DOES" — core system prompt |
| `instructions` | Markdown file | Additional runtime instructions |

**SOUL.md example:**
```markdown
Communication style rules:
- Be concise and concrete; prefer bullet points over paragraphs
- Ask one clarifying question when requirements are ambiguous
- Match the user's tone; only use emojis if the user uses them first
```

### spec.session

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `on_disconnect` | string | `pause` | `continue` or `pause` |
| `max_tool_iterations` | int | `10` | Max tool call iterations per request |
| `llm_timeout_seconds` | int | `300` | Timeout for LLM requests in seconds |
| `ttl_hours` | int | (global) | Per-agent session TTL override |
| `compaction` | string | (global) | Per-agent compaction override |

### spec.session.context

Controls how conversation history and tool results are truncated to fit within the model's context window.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_history_tokens` | int | `20000` | Max tokens for conversation history (`0` = no cap) |
| `max_tool_result_tokens` | int | `8000` | Max tokens per tool result |
| `tool_result_truncation` | string | `head` | `head`, `tail`, or `both` |
| `tool_result_keep_first` | int | `2` | First N tool results kept visible |
| `tool_result_keep_last` | int | `5` | Last M tool results kept visible |

### spec.tools

See [Tools and Policies](./tools-and-policies.md) for full details.

### spec.access

See [Group Chat](./group-chat.md) for full details.

### spec.memory

See [Memory](./memory.md) for full details.

## Versioning

The format uses API versions:

| Version | Stability |
|---------|-----------|
| `duragent/v1alpha1` | Alpha (current, breaking changes allowed) |
| `duragent/v1beta1` | Beta (mostly stable) |
| `duragent/v1` | Stable (future) |

## Examples

### Q&A Bot

```yaml
apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: qa-bot
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
  system_prompt: ./SYSTEM_PROMPT.md
```

### Code Review Agent

```yaml
apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: code-reviewer
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
    temperature: 0.3
  system_prompt: ./SYSTEM_PROMPT.md
  instructions: ./INSTRUCTIONS.md
  session:
    on_disconnect: pause
  tools:
    - type: builtin
      name: bash
    - type: cli
      name: git-diff
      command: ./tools/git-diff.sh
      description: Show git diff for review
```

### Autonomous Supervisor

```yaml
apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: code-supervisor
spec:
  model:
    provider: anthropic
    name: claude-sonnet-4-20250514
  system_prompt: ./SYSTEM_PROMPT.md
  session:
    on_disconnect: continue
    max_tool_iterations: 50
  tools:
    - type: builtin
      name: bash
```
