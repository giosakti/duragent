# Duragent

[![CI](https://github.com/giosakti/duragent/actions/workflows/ci.yml/badge.svg)](https://github.com/giosakti/duragent/actions/workflows/ci.yml)
[![Docs](https://img.shields.io/badge/docs-guide-blue)](https://giosakti.github.io/duragent/)
[![Rust](https://img.shields.io/badge/rust-1.85+-orange?logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

> **Duragent** — A durable, self-contained runtime for AI agents.

Sessions survive crashes. Agents are just files. One binary, zero dependencies.

Use it as a personal AI assistant, or as the foundation for agent-powered products.

| What you get | How |
|--------------|-----|
| Sessions that survive crashes | Append-only event log, attach/detach like tmux |
| Agents you can read and version | YAML + Markdown — no code required |
| State you can inspect | Just files on disk — `cat`, `grep`, `git diff` |
| Deploy anywhere | Single binary, ~10MB, no Python/Node/Docker |
| Your choice of parts | Swap LLM providers, gateways, and storage backends or bring your own |

## Quick Start

### 1. Install and initialize

```bash
git clone https://github.com/giosakti/duragent.git
cd duragent && make build

# Or: cargo install --git https://github.com/giosakti/duragent.git

duragent init
# Follow the interactive setup
```

### 2. Set up your API key and start the server

```bash
export OPENROUTER_API_KEY=your-key  # or: duragent login anthropic
duragent serve
```

### 3. Chat with your agent

```bash
duragent chat --agent <YOUR_AGENT_NAME>
```

### 4. Attach to a session later

```bash
duragent attach --list       # List attachable sessions
duragent attach SESSION_ID   # Reconnect to existing session
```

## Features

- **Durable sessions** — crash, restart, reconnect; your conversation survives
- **Portable agent format** — define agents in YAML + Markdown; inspect, version, and share them
- **Memory** — agents recall past conversations, remember experiences, and reflect on long-term knowledge
- **Tools** — bash execution, CLI tools, and scheduled tasks, with configurable approval policies
- **Skills** — modular capabilities defined as Markdown files ([Agent Skills](https://agentskills.io) standard)
- **Multiple LLM providers** — Anthropic, OpenAI, OpenRouter, Ollama
- **Platform gateways** — Telegram and Discord via subprocess plugins
- **HTTP API** — REST endpoints with SSE streaming

## Modular by Design

Use the built-ins, or swap in your own:

| Component | Default | Swappable |
|-----------|---------|-----------|
| Gateways | CLI, HTTP, SSE, Telegram, Discord | Any platform via [gateway plugins](./crates/duragent-gateway-protocol) |
| LLM | OpenRouter | Anthropic, OpenAI, Ollama, or any provider |
| Sandbox | Trust mode | bubblewrap, Docker *(planned)* |
| Storage | Filesystem | Postgres, S3 *(planned)* |

## Workspace Layout

```
./.duragent/
├── agents/<agent-name>/
│   ├── agent.yaml           # Agent definition (Duragent Format)
│   ├── SOUL.md              # "Who the agent IS" (identity and personality)
│   ├── SYSTEM_PROMPT.md     # "What the agent DOES" (core system prompt)
│   ├── INSTRUCTIONS.md      # Additional runtime instructions (optional)
│   ├── policy.yaml          # Tool execution policy (optional)
│   ├── skills/              # Modular capabilities (SKILL.md files)
│   ├── tools/               # Agent-specific auto-discovered tools
│   └── memory/
│       ├── MEMORY.md        # Curated long-term memory
│       └── daily/           # Daily experience logs
├── sessions/
│   └── <session_id>/
│       ├── events.jsonl     # Append-only event log
│       └── state.yaml       # Snapshot for fast resume
├── tools/                   # Workspace-level auto-discovered tools
├── schedules/               # Scheduled tasks and run logs
└── memory/
    └── world/               # Shared knowledge across all agents
```

## Documentation

**[Read the Duragent Guide](https://giosakti.github.io/duragent/)** for installation, configuration, and usage.

For contributors: [CONTRIBUTING.md](CONTRIBUTING.md) | [Project Status](./docs/PROJECT_STATUS.md) | [Internal Specs](./docs/internal/)

## License

[MIT](LICENSE)
