# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.1] - 2026-02-06

### Added
- Discord gateway (`agnx-gateway-discord`)
  - Supports both built-in (feature flag) and subprocess modes
  - Serenity 0.12 for Discord API (websocket gateway + HTTP)
  - Button components for approval flow (inline keyboard)
  - Message chunking at 2000 char limit
  - Reply support via message references
  - Capabilities: edit, delete, typing, reply, inline keyboard
- Typing indicator during message processing (all gateways)

### Fixed
- Memory tools (`remember`, `reflect`) now handle non-standard LLM argument formats gracefully
- Schedule tool descriptions reworded to steer models toward `at` for one-shot reminders
- Debug logging added to tool executor for diagnosing argument format issues

## [0.4.0] - 2026-02-06

### Added
- File-based memory system with four agent tools
  - `recall`: Retrieve relevant memories from agent's memory store
  - `remember`: Store new daily experiences
  - `reflect`: Consolidate experiences and curate MEMORY.md
  - `update_world`: Update shared world knowledge facts
  - World memory (shared across agents) + agent memory (per-agent)
- Directives system for runtime instructions
  - Workspace-scoped directives (`{workspace}/directives/*.md`)
  - Agent-scoped directives (`{agent_dir}/directives/*.md`)
  - Auto-created `memory.md` directive when memory is enabled
  - Priority-based ordering in context builder
- Built-in bash tool with sandbox execution
- CLI tool support (lightweight alternative to MCP)
  - Script-based tool definitions in agent config
  - README-based documentation loaded on-demand
- Tool execution policies
  - Three modes: `dangerous` (trust all), `ask` (approval for unknown), `restrict` (allow-list only)
  - Typed allow/deny patterns (e.g., `bash:cargo*`, `mcp:github:*`)
  - Policy merging (`policy.yaml` base + `policy.local.yaml` overrides)
  - Notification support for tool execution (log, webhook)
- Scheduled tasks with agent-initiated creation via tools
  - One-shot (`at`), interval (`every`), and cron expression timing
  - Message payload (direct send) or task payload (execute with tools)
  - YAML persistence with JSONL run logs
  - Retry with exponential backoff and jitter
  - Schedule tools: `schedule_task`, `list_schedules`, `cancel_schedule`
- Shared `ChatSessionCache` for gateway/scheduler session coordination
- Checkpoint-based snapshots for session persistence
  - Snapshots store only checkpointed messages (not full history)
  - Pending messages rebuilt from events since checkpoint
  - Reduces O(N²) snapshot I/O to O(N)
- Soul field for agent personality in agent spec

### Changed
- Session management refactored to actor model
  - Per-session actor with serialized state mutations via message passing
  - Batched event writes + periodic snapshots
  - Trait-based storage abstraction for pluggable backends
- Tool executor refactored to trait-based design for extensibility
- Context builder now uses structured blocks with provenance tracking and priority ordering

### Fixed
- Atomic get-or-insert for gateway session creation (race condition)
- Per-session serialization with concurrent gateway event processing
- Semaphore-limited concurrent scheduled task executions (prevents LLM call storms)

## [0.3.0] - 2026-01-29

### Added
- Gateway plugin architecture for platform integrations
  - Gateway Protocol (`agnx-gateway-protocol` crate) with JSON-over-stdio communication
  - Gateway Manager with unified interface for built-in and subprocess gateways
  - Subprocess supervision with restart policies (`always`, `on_failure`, `never`)
  - Exponential backoff and parent-death handling for subprocess gateways
- Telegram gateway (`agnx-gateway-telegram`)
  - Supports both built-in (feature flag) and subprocess modes
  - DM and group chat support with bot commands
- Global agent routing rules in `agnx.yaml`
  - Match conditions: `gateway`, `chat_type`, `chat_id`, `sender_id`
  - First-match-wins evaluation order
  - Session routing persisted in snapshots (gateway + chat_id)
- Sandbox module as placeholder for tool execution
  - `Sandbox` trait with `exec()` method for command execution
  - `TrustSandbox` implementation (no isolation, direct host execution)
  - `SandboxConfig` with configurable mode (defaults to `trust`)
- Environment variable interpolation in config files
  - `${VAR}` for required variables
  - `${VAR:-default}` for optional with defaults

### Changed
- Refactored to workspace structure (`crates/agnx`, `crates/agnx-gateway-*`)
- AssistantMessage events now track agent name for mid-session agent switching

## [0.2.0] - 2026-01-27

### Added
- Session persistence with JSONL event log and YAML snapshots
  - Atomic writes with temp-file-rename pattern
  - Event types: SessionStart, UserMessage, AssistantMessage, ToolCall, ToolResult, StatusChange, Error, SessionEnd
  - Peek/commit pattern prevents sequence drift on write failures
- Session resume on reconnect
  - Snapshot loading with event replay after `last_event_seq`
  - Server startup recovery scans `.agnx/sessions/` directory
- Session disconnect behavior (`on_disconnect` config)
  - `pause` mode: cancels LLM, saves partial content, pauses session
  - `continue` mode: transfers stream to background task, continues execution
- CLI `agnx attach` command
  - `agnx attach --list` shows attachable sessions
  - `agnx attach SESSION_ID` reconnects with conversation history
- SSE streaming endpoint (`POST /api/v1/sessions/{id}/stream`)
  - Events: `start`, `token`, `done`, `cancelled`, `error`, `keep-alive`
  - Configurable idle timeout and keep-alive heartbeat
- Background task registry for graceful shutdown
- HTTP client library for CLI-to-server communication
- Server launcher with auto-start and health checks
- Integration tests for persistence, resume, and disconnect behavior

## [0.1.0] - 2026-01-25

### Added
- Agent spec loader for Agnx Agent Format (AAF: YAML + Markdown)
- LLM provider abstraction with support for OpenRouter, OpenAI, Anthropic, and Ollama
- Basic agent executor (prompt → response)
- HTTP API endpoints:
  - Health checks: `/livez`, `/readyz`, `/version`
  - Agents API: `GET /api/v1/agents`, `GET /api/v1/agents/{name}`
  - Sessions API: `POST /api/v1/sessions`, `GET /api/v1/sessions/{id}`, `POST /api/v1/sessions/{id}/messages`
- CLI commands: `agnx serve`, `agnx chat`
- RFC 7807 Problem Details for error responses
- Configuration loading from YAML (`agnx.yaml`)
- In-memory session store
- Integration tests for HTTP API
- CI pipeline with linting, testing, and build verification

### Changed
- Project renamed from Pluto to Agnx
- Refactored into library + binary crate structure

## [0.0.1] - 2026-01-11

### Added
- Initial repository setup
- Project documentation (architecture, API reference, deployment guide)
- Agnx Agent Format (AAF) specification

[Unreleased]: https://github.com/AgnxAI/agnx/compare/v0.4.1...HEAD
[0.4.1]: https://github.com/AgnxAI/agnx/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/AgnxAI/agnx/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/AgnxAI/agnx/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/AgnxAI/agnx/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/AgnxAI/agnx/compare/v0.0.1...v0.1.0
[0.0.1]: https://github.com/AgnxAI/agnx/releases/tag/v0.0.1
