# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/giosakti/agnx/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/giosakti/agnx/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/giosakti/agnx/compare/v0.0.1...v0.1.0
[0.0.1]: https://github.com/giosakti/agnx/releases/tag/v0.0.1
