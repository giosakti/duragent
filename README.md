# Duragent

[![CI](https://github.com/giosakti/duragent/actions/workflows/ci.yml/badge.svg)](https://github.com/giosakti/duragent/actions/workflows/ci.yml)
[![Rust](https://img.shields.io/badge/rust-1.85+-orange?logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

> **Duragent** — Assemble your own agent runtime.

One binary. Modular parts. Zero dependencies.

Everything you need is self-contained, but every part is swappable.

Build a personal assistant, a support bot, or a multi-tenant platform. Same runtime, same simplicity.

### The problems we solve

| Problem | Duragent |
|---------|------|
| Sessions lost on crash | **Durable** — attach/detach like tmux |
| Agents locked in code | **Portable** — YAML + Markdown |
| State hidden in databases | **Transparent** — just files you can read |
| Heavy frameworks | **Minimal** — single binary, ~10MB, starts in milliseconds |
| Dependency hell | **Self-contained** — no Python, no Node, no containers |

### Modular by design

Use the built-ins, or we'll provide mechanism later for you to build and swap in your own:

| Component | Default | Swappable |
|-----------|---------|-----------|
| Gateways | CLI, HTTP, SSE, Telegram | Any platform via gateway plugins |
| LLM | OpenRouter | Any provider |
| Sandbox | Trust mode | bubblewrap, Docker *(planned)* |
| Storage | Filesystem | Postgres, Redis, S3 *(planned)* |

Swappability through clean interfaces. See [Gateway Protocol](./crates/duragent-gateway-protocol) for an example.

### Work-in-Progress (to be released soon)

Memory banks, tools, and orchestration.
Built the Duragent way: simple, transparent, portable, swappable.

## Installation

### From Source

```bash
git clone https://github.com/giosakti/duragent.git
cd duragent
make build
./target/release/duragent --version
```

### Cargo Install

```bash
cargo install --git https://github.com/giosakti/duragent.git
```

## Quick Start

### 1. Create an agent

```bash
mkdir -p .duragent/agents/my-assistant

cat > .duragent/agents/my-assistant/agent.yaml << 'EOF'
apiVersion: duragent/v1alpha1
kind: Agent
metadata:
  name: my-assistant
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
  system_prompt: ./SYSTEM_PROMPT.md
  session:
    on_disconnect: pause
EOF

cat > .duragent/agents/my-assistant/SYSTEM_PROMPT.md << 'EOF'
You are a helpful assistant. Be concise and actionable.
EOF
```

### 2. Start the server

```bash
export OPENROUTER_API_KEY=your-key
duragent serve --port 8080
```

### 3. Chat with your agent

```bash
duragent chat --agent my-assistant
```

### 4. Attach to a session later

```bash
# List attachable sessions
duragent attach --list

# Attach to existing session
duragent attach SESSION_ID
```

## Workspace Layout

```
./.duragent/
├── agents/<agent-name>/
│   ├── agent.yaml           # Agent definition (Duragent Format)
│   ├── SYSTEM_PROMPT.md     # Agent personality
│   └── INSTRUCTIONS.md      # Behavioral rules (optional)
└── sessions/
    └── <session_id>/
        ├── events.jsonl     # Append-only event log
        └── state.yaml       # Snapshot for fast resume
```

## Documentation

| Document | Description |
|----------|-------------|
| [Project Status](./docs/PROJECT_STATUS.md) | Roadmap and current focus |
| [Project Charter](./docs/specs/202601111100.project-charter.md) | Vision, goals, and principles |
| [Architecture](./docs/specs/202601111101.architecture.md) | System design and components |
| [API Reference](./docs/specs/202601111102.api-reference.md) | HTTP API and CLI commands |
| [Deployment](./docs/specs/202601111103.deployment.md) | Deployment modes and configuration |
| [Duragent Format](./docs/specs/202601111200.duragent-format.md) | Agent definition specification |
| [Example Skill](./docs/examples/skills/task-extraction/) | Sample skill implementation |

## Tech Stack

- Rust 2024 Edition
- HTTP: Axum
- Streaming: SSE
- Config/spec: YAML (structured) + Markdown (prose)
- LLM: OpenRouter, OpenAI, Anthropic, Ollama

## Contributing

We welcome contributions! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

[MIT LICENSE](LICENSE)
