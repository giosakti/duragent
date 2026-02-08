# CLAUDE.md — Project Context for AI Agents

> This file provides context for AI coding agents (Claude Code, Cursor, etc.) working on this codebase.

## Project Overview

> **Duragent** is a durable, portable runtime for AI agents.

- **Sessions that survive** — crash, restart, reconnect from any terminal: duragent is durable by design
- **Transparent state** — agent specs, memories, and sessions are human-readable files you can inspect, version, and export
- **Sandboxed by default** — bubblewrap on Linux; Docker or trust mode on other platforms
- **Pluggable everything** — file-based by default; bring your own storage or messaging backends
- **Edge-ready** — single-binary core (~10-15MB), gateway plugins are optional separate binaries

Core protocols built-in (CLI, HTTP, SSE). Platform integrations via plugins.

Self-host it directly, or use it as the foundation for agent-powered products.

### Key Concepts

| Concept | Description |
|---------|-------------|
| **Session** | Durable conversation context; survives crashes/disconnects; attach/detach like tmux |
| **on_disconnect** | `continue` (agent keeps working) or `pause` (waits for reconnect) |
| **Core gateways** | CLI/TUI, HTTP REST, SSE — built into Duragent |
| **Platform gateways** | Telegram, Discord, Slack — subprocess plugins via Gateway Protocol |
| **Sandbox** | bubblewrap (Linux), Docker — configurable isolation |
| **Duragent Format** | YAML + Markdown agent definitions |

## Strategic Documents

- **[README](README.md)**
- **[Project status / roadmap](./docs/PROJECT_STATUS.md)**
- **[Project Charter](./docs/specs/202601111100.project-charter.md)**
- **[Architecture](./docs/specs/202601111101.architecture.md)**
- **[API Reference](./docs/specs/202601111102.api-reference.md)**
- **[Deployment](./docs/specs/202601111103.deployment.md)**
- **[Duragent Format](./docs/specs/202601111200.duragent-format.md)**
- **[Example skill](./docs/examples/skills/task-extraction/)**

## Tech Stack

- Rust 2024 Edition
- HTTP: Axum
- Streaming: SSE
- Config/spec: YAML (structured) + Markdown (prose)
- Tool ecosystem: built-in, CLI tools, MCP
- Discovery: A2A Agent Card
- Sandbox: bubblewrap, Docker

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
