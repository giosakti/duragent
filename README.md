# Agnx

[![CI](https://github.com/AgnxAI/agnx/actions/workflows/ci.yml/badge.svg)](https://github.com/AgnxAI/agnx/actions/workflows/ci.yml)
[![Rust](https://img.shields.io/badge/rust-1.85+-orange?logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

> **Agnx** is a durable, portable runtime for AI agents.

- **Sessions that survive** — crash, restart, reconnect from any terminal: agnx is durable by design
- **Transparent state** — agent specs, memories, and sessions are human-readable files you can inspect, version, and export
- **Sandboxed by default** — bubblewrap on Linux; Docker or trust mode on other platforms
- **Pluggable everything** — file-based by default; bring your own storage or messaging backends
- **Edge-ready** — single-binary core (~10MB), gateway plugins are optional separate binaries

Core protocols built-in (CLI, HTTP, SSE). Platform integrations via plugins.

Self-host it directly, or use it as the foundation for agent-powered products.

## Why Agnx?

| Problem | Agnx Solution |
|---------|---------------|
| Sessions lost on crash or disconnect | **Durable sessions** — attach/detach like tmux, resume where you left off |
| Agent definitions locked in proprietary formats | **Portable YAML + Markdown** — version control, inspect, migrate anywhere |
| Hidden runtime state you can't access | **File-based by default** — sessions, memory, artifacts are just files |
| Heavy frameworks with complex setup | **Single binary, <5MB** — starts in milliseconds |
| Vendor lock-in to specific LLM providers | **LLM-agnostic** — OpenRouter, OpenAI, Anthropic, Ollama |

## Installation

### From Source

```bash
git clone https://github.com/AgnxAI/agnx.git
cd agnx
make build
./target/release/agnx --version
```

### Cargo Install

```bash
cargo install --git https://github.com/AgnxAI/agnx.git
```

## Quick Start

### 1. Create an agent

```bash
mkdir -p .agnx/agents/my-assistant

cat > .agnx/agents/my-assistant/agent.yaml << 'EOF'
apiVersion: agnx/v1alpha1
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

cat > .agnx/agents/my-assistant/SYSTEM_PROMPT.md << 'EOF'
You are a helpful assistant. Be concise and actionable.
EOF
```

### 2. Start the server

```bash
export OPENROUTER_API_KEY=your-key
agnx serve --port 8080
```

### 3. Chat with your agent

```bash
# Interactive CLI
agnx chat --agent my-assistant

# Or via HTTP
curl -X POST http://localhost:8080/api/v1/agents/my-assistant/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "Hello!"}'
```

### 4. Attach to a session later

```bash
# List sessions
agnx sessions

# Attach to existing session
agnx attach session_abc123
```

## Workspace Layout

```
./.agnx/
├── agents/<agent-name>/
│   ├── agent.yaml           # Agent definition (AAF)
│   ├── SYSTEM_PROMPT.md     # Agent personality
│   ├── INSTRUCTIONS.md      # Behavioral rules
│   └── skills/              # Local skills
├── memory/                  # Durable memory (Markdown)
├── sessions/
│   └── <session_id>/
│       ├── events.jsonl     # Append-only event log
│       └── state.yaml       # Snapshot for fast resume
└── artifacts/               # Generated outputs
```

## Documentation

| Document | Description |
|----------|-------------|
| [Project Status](./docs/PROJECT_STATUS.md) | Roadmap and current focus |
| [Project Charter](./docs/specs/202601111100.project-charter.md) | Vision, goals, and principles |
| [Architecture](./docs/specs/202601111101.architecture.md) | System design and components |
| [API Reference](./docs/specs/202601111102.api-reference.md) | HTTP API and CLI commands |
| [Deployment](./docs/specs/202601111103.deployment.md) | Deployment modes and configuration |
| [Agent Format (AAF)](./docs/specs/202601111200.agnx-agent-format.md) | Agent definition specification |
| [Example Skill](./docs/examples/skills/task-extraction/) | Sample skill implementation |

## Tech Stack

- Rust 2024 Edition
- HTTP: Axum
- Streaming: SSE
- Config/spec: YAML (structured) + Markdown (prose)
- Tool ecosystem: built-in, CLI tools, MCP
- Discovery: A2A Agent Card
- Sandbox: bubblewrap, Docker

## Contributing

We welcome contributions! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

[MIT LICENSE](LICENSE)
