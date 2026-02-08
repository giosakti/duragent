# Simple Mode (Self-Hosted)

For self-hosted power users, Duragent runs as a single binary with all state stored as files. Zero external dependencies.

## Architecture

```
duragent serve
├── Core Gateways (CLI, HTTP, SSE)
├── Gateway Protocol → [Telegram] [Discord] (plugins)
└── File-Based State
    └── .duragent/
        ├── agents/my-agent/
        │   ├── agent.yaml
        │   ├── SOUL.md
        │   ├── SYSTEM_PROMPT.md
        │   ├── INSTRUCTIONS.md
        │   ├── policy.yaml
        │   ├── skills/
        │   └── memory/
        ├── sessions/
        │   └── session_abc/
        │       ├── events.jsonl
        │       └── state.yaml
        ├── schedules/
        └── memory/world/
```

## Setup

### 1. Initialize a Workspace

```bash
duragent init
# Follow the interactive setup
```

### 2. Set Up Your API Key and Start the Server

```bash
duragent login anthropic  # or: export OPENROUTER_API_KEY=your-key
duragent serve
```

### 3. Chat

```bash
duragent chat --agent <YOUR_AGENT_NAME>
```

## Properties

- **Zero external dependencies** — single binary, no database, no Docker
- **All state is files** — git-friendly, inspectable, editable
- **Memory is markdown** — human-readable, easy to export
- **Core gateways built-in** — CLI, HTTP, SSE
- **Platform gateways via plugins** — optional, separate binaries

## Storage Configuration

By default, all state lives under `.duragent/`. You can customize paths:

```yaml
# duragent.yaml
workspace: .duragent

services:
  session:
    path: .duragent/sessions

world_memory:
  path: .duragent/memory/world
```

## File Formats

| Format | Use Case |
|--------|----------|
| **JSONL** | Event streams (append-only, fast writes) |
| **YAML** | Structured state (snapshots, schedules) |
| **Markdown** | Prose content (prompts, memory) |

All files are human-readable and designed for version control.
