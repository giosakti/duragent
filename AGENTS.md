# AGENTS.md — Project Context for AI Agents

## Project Overview

> **Duragent** — A durable, self-contained runtime for AI agents.

Sessions survive crashes. Agents are just files. One binary, zero dependencies.

Use it as a personal AI assistant, or as the foundation for agent-powered products.

### Key Concepts

| Concept | Description |
|---------|-------------|
| **Session** | Durable conversation context; survives crashes/disconnects; attach/detach like tmux |
| **on_disconnect** | `continue` (agent keeps working) or `pause` (waits for reconnect) |
| **Core gateways** | CLI, HTTP REST, SSE — built into Duragent |
| **Platform gateways** | Telegram, Discord — subprocess plugins via Gateway Protocol |
| **Sandbox** | Trust mode (no isolation) |
| **Duragent Format** | YAML + Markdown agent definitions |

### Planned

- TUI (ratatui) for core gateways
- Slack gateway plugin
- Sandbox isolation: bubblewrap (Linux), Docker (cross-platform)
- MCP tool integration
- A2A Agent Card discovery

See [Project Status](./docs/PROJECT_STATUS.md) for the full roadmap.

## Strategic Documents

- **[README](README.md)**
- **[Duragent Guide](https://giosakti.github.io/duragent/)** — user-facing documentation (source: `book/`)
- **[Project status / roadmap](./docs/PROJECT_STATUS.md)**
- **[Project Charter](./docs/internal/specs/202601111100.project-charter.md)**
- **[Architecture](./docs/internal/specs/202601111101.architecture.md)**
- **[API Reference](./docs/internal/specs/202601111102.api-reference.md)**
- **[Deployment](./docs/internal/specs/202601111103.deployment.md)**
- **[Duragent Format](./docs/internal/specs/202601111200.duragent-format.md)**
- **[Example skill](./docs/examples/skills/task-extraction/)**

## Tech Stack

- Rust 2024 Edition
- HTTP: Axum
- Streaming: SSE
- Config/spec: YAML (structured) + Markdown (prose)
- Tool ecosystem: built-in, CLI tools
- Sandbox: trust mode

## Code Conventions

- **Error handling:** Use `anyhow` for application errors, `thiserror` for library errors
- **Async:** Tokio runtime, async traits via `async_trait`
- **Config:** `serde` for YAML/JSON, environment variables via `${VAR}` syntax
- **Logging:** `tracing` crate with structured spans
- **Tests:** Unit tests inline, integration tests in `tests/`

## Release Protocol

1. **Update `Cargo.toml`** — bump `version = "0.x.0"`
2. **Update `CHANGELOG.md`**:
   - Move items from `[Unreleased]` to new `[0.x.0] - YYYY-MM-DD` section
   - Add comparison link at bottom
3. **Update `PROJECT_STATUS.md`** — mark milestone complete, update recent accomplishments
4. **Commit** — `git commit -m "Release v0.x.0"`
5. **Tag** — `git tag v0.x.0`
6. **Push** — `git push && git push --tags`
