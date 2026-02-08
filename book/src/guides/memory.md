# Memory

Duragent provides a file-first memory system using plain markdown files. Agents decide when to read and write memory using four built-in tools.

## Design

- **Markdown-first** — All memory is plain text files you can read, edit, and version
- **Tools-only** — No automatic injection; the agent decides when to use memory
- **Curation over automation** — Agents promote important learnings manually

## Directory Structure

```
{workspace}/
├── memory/
│   └── world/                   # Shared facts (all agents see)
│       ├── people.md
│       ├── systems.md
│       └── {topic}.md
└── agents/
    └── {agent-name}/
        ├── agent.yaml
        └── memory/              # Agent-specific
            ├── MEMORY.md        # Curated long-term memory
            └── daily/
                └── YYYY-MM-DD.md  # Daily experiences (append-only)
```

| Path | Owner | Purpose |
|------|-------|---------|
| `memory/world/*.md` | Shared | Objective facts about the world |
| `agents/{name}/memory/MEMORY.md` | Agent | Curated learnings |
| `agents/{name}/memory/daily/*.md` | Agent | Daily experience log |

## Enabling Memory

Add a `memory` section to your agent spec:

```yaml
# agent.yaml
spec:
  memory:
    backend: filesystem
```

When the `memory` section is present, all four memory tools are automatically registered. When omitted, no memory tools are available.

## Memory Tools

| Tool | Action | When to Use |
|------|--------|-------------|
| `recall` | Read memory context | Start of conversation, when context needed |
| `remember` | Append to daily log | After learning something |
| `reflect` | Rewrite MEMORY.md | End of session, consolidate learnings |
| `update_world` | Write world knowledge topic | New shared fact discovered |

### recall

Load memory context — world knowledge, agent memory, and recent daily logs.

- **Parameters:** `days` (integer, default: 3)
- **Returns:** Concatenated content from world files, MEMORY.md, and the last N days of daily logs

### remember

Append an experience to today's daily log.

- **Parameters:** `content` (string, required)
- **Behavior:** Appends a timestamped entry to `memory/daily/YYYY-MM-DD.md`

### reflect

Rewrite the agent's long-term memory.

- **Parameters:** `content` (string, required)
- **Behavior:** Atomic write to `memory/MEMORY.md`

### update_world

Write shared world knowledge for a topic.

- **Parameters:** `topic` (string, required), `content` (string, required)
- **Behavior:** Atomic write to `world/{topic}.md` (replaces existing content)

## Configuration

### Global (duragent.yaml)

```yaml
workspace: .duragent

# Optional: override world memory location
# world_memory:
#   path: custom/memory/world
```

The world memory directory defaults to `{workspace}/memory/world`.

### Per-Agent (agent.yaml)

```yaml
spec:
  memory:
    backend: filesystem   # Enables all 4 memory tools
```

## Directives

Directives are `*.md` files that are injected into the system prompt. They're loaded from two directories:

- `{workspace}/directives/` — workspace-level, shared across all agents
- `{agent_dir}/directives/` — agent-level, per-agent

When any agent has memory configured, a default `memory.md` directive is auto-created at `{workspace}/directives/memory.md` with instructions for using memory tools.

## Growth Management

| File Type | Expected Size | Management |
|-----------|--------------|------------|
| `world/*.md` | 1-10 KB each | Agent rewrites during `update_world` |
| `MEMORY.md` | 2-10 KB | Agent rewrites during `reflect` |
| `daily/*.md` | 0.5-2 KB/day | Old files naturally become irrelevant |

For searching older memories, you can use external tools like [fmd](https://github.com/giosakti/fmd) — a fast BM25F markdown search tool.
