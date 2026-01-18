# Agnx

[![CI](https://github.com/giosakti/agnx/actions/workflows/ci.yml/badge.svg)](https://github.com/giosakti/agnx/actions/workflows/ci.yml)
[![Rust](https://img.shields.io/badge/rust-1.85+-orange?logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

> **Agnx is the "nginx for AI agents"** — a minimal, fast, self-hostable runtime that runs agents defined in a **transparent, portable format**, exposed through a standard API.

Agnx treats agents as durable artifacts: files you own that should outlast the runtime.

- **Transparent agent format** (human-readable, inspectable, versionable)
- **Stateless by default** (no hidden server-side state)
- **File-based state** when present (specs, memories, logs, config) — if Agnx disappears, take these and host elsewhere
- **Edge/embedded-friendly by design** (small binary, low memory, minimal dependencies)

## Why Agnx?

| Problem | Agnx Solution |
|---------|---------------|
| Agent definitions locked in proprietary formats | **Portable YAML + Markdown** — version control, inspect, migrate anywhere |
| Hidden runtime state you can't access | **File-based by default** — sessions, memory, artifacts are just files |
| Heavy frameworks with complex setup | **Single binary, <5MB** — starts in milliseconds |
| Vendor lock-in to specific LLM providers | **LLM-agnostic** — OpenRouter, OpenAI, Anthropic, Ollama |

## Installation

### From Source

```bash
# Clone the repository
git clone https://github.com/giosakti/agnx.git
cd agnx

# Build
make build

# Verify
./target/release/agnx --version
```

### Cargo Install

```bash
cargo install --git https://github.com/giosakti/agnx.git
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
curl -X POST http://localhost:8080/api/v1/agents/my-assistant/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "Hello, how are you?"}'
```

## Workspace Layout

Agnx's default workspace layout (file-based mode):

```
./.agnx/
├── agents/<agent-name>/
│   ├── agent.yaml           # Agent definition (AAF)
│   ├── SYSTEM_PROMPT.md     # Agent personality
│   ├── INSTRUCTIONS.md      # Behavioral rules
│   └── skills/              # Local skill discovery
├── memory/                  # Durable memory (files)
├── sessions/                # Chat history (files)
└── artifacts/               # Generated outputs (files)
```

## Documentation

| Document | Description |
|----------|-------------|
| [Project Status](./docs/specs/PROJECT_STATUS.md) | Roadmap and current focus |
| [Architecture](./docs/specs/202601111101.architecture.md) | System design and components |
| [API Reference](./docs/specs/202601111102.api-reference.md) | HTTP API and CLI commands |
| [Deployment](./docs/specs/202601111103.deployment.md) | Deployment modes and configuration |
| [Agent Format (AAF)](./docs/specs/202601111200.agnx-agent-format.md) | Agent definition specification |
| [Example Skill](./docs/examples/skills/task-extraction/) | Sample skill implementation |

## Tech Stack

- **Language:** Rust 2024 Edition (single binary, <5MB, no runtime dependencies)
- **HTTP:** Axum + SSE for streaming
- **Config:** YAML (structured) + Markdown (unstructured)
- **Tools:** MCP (Model Context Protocol)
- **Discovery:** A2A Agent Card
- **Targets:** x86_64, ARM64, ARM32 (edge/embedded-capable)

## Contributing

We welcome contributions! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

[MIT LICENSE](LICENSE)
