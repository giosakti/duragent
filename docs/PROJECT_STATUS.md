# Project Status - Duragent

> **Purpose:** Living status document. The stable vision and principles live in the [Project Charter](./internal/specs/202601111100.project-charter.md).

## Last Updated
2026-02-16

## Strategic Direction

See [Project Charter](./internal/specs/202601111100.project-charter.md) for vision, goals, and guiding principles.

Key specs and design docs:
- [Architecture](./internal/specs/202601111101.architecture.md)
- [API Reference](./internal/specs/202601111102.api-reference.md)
- [Deployment](./internal/specs/202601111103.deployment.md)
- [Duragent Format](./internal/specs/202601111200.duragent-format.md)

## Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Architecture | Crash-resilient core, durable sessions | Immediate persistence ensures recovery; sessions survive disconnects |
| Runtime pattern | Simple event loop | Minimal complexity; add features only when needed |
| Deployment | Persistent-first, pluggable backends | Simple mode (files) for self-hosted, complex mode (external) for SaaS |
| Agent definition | YAML + Markdown (Duragent Format; no code) | Portability, inspectable, git-friendly |
| Agent routing | Routing table (first match wins) | Flexible; user controls specificity via ordering |
| Storage default | File-based (`./.duragent/` files) | Zero dependencies, git-friendly, human-readable |
| Storage formats | JSONL (events), YAML (state), Markdown (prose) | JSONL for append-only writes; YAML for structured snapshots; Markdown for human content |
| Session persistence | Append-only event log + periodic YAML snapshot | Fast writes (append), fast reads (snapshot), human-readable |
| Session disconnect | Configurable: `continue` or `pause` | `continue` for async workflows; `pause` for interactive chat |
| Core gateways | CLI, HTTP REST, SSE (built-in) | Protocols that ship with Duragent; SSE for streaming |
| Platform gateways | Subprocess plugins (Telegram, Discord) | JSON-over-stdio protocol; crash independently; any language |
| Tool approval | Optional approval workflow per tool | Safety without sacrificing autonomy |
| Scheduled tasks | Agent-initiated via tools; YAML persistence | Agents create reminders and recurring checks autonomously |
| Sandbox | Trust mode; bubblewrap and Docker planned (v0.6.0) | Trust mode for now; isolation coming later |

## Current Focus

**v0.6.0 — Observability & Sandbox**: Structured logging, metrics, tracing, and sandboxed tool execution.

## Roadmap

> Future milestones are directional, not commitments. Scope and priorities may change.

### v0.1.0 — Foundation ✓
- [x] Agent spec loader (Duragent Format: YAML + Markdown)
- [x] LLM providers (OpenRouter, OpenAI, Anthropic, Ollama)
- [x] Basic agent executor (prompt → response)
- [x] Core gateways: CLI, HTTP REST
- [x] CLI: `duragent serve`, `duragent chat`
- [x] Sessions API (in-memory)
- [x] Integration tests

### v0.2.0 — Sessions & Durability ✓
- [x] Session persistence (JSONL events + YAML snapshots)
- [x] Session resume on reconnect
- [x] Session disconnect behavior (`continue` / `pause`)
- [x] CLI: `duragent attach` (connect to running/paused session)
- [x] Core gateways: SSE streaming

### v0.3.0 — Gateway Plugins ✓
- [x] Gateway plugin protocol (JSON over stdio)
- [x] First-party plugin: duragent-gateway-telegram
- [x] Plugin configuration in duragent.yaml
- [x] Trust mode (no isolation) — sandbox placeholder

### v0.4.0 — Tools, Policy, Scheduling, and Memory ✓
- [x] Built-in bash tool
- [x] CLI tool support (lightweight alternative to MCP)
- [x] Tool execution policies (dangerous/ask/restrict modes, allow/deny lists)
- [x] Scheduled tasks (one-shot, interval, cron)
- [x] Schedule tools (`schedule_task`, `list_schedules`, `cancel_schedule`)
- [x] Shared session cache for gateway/scheduler coordination
- [x] File-based memory system (recall, remember, reflect, update_world)
- [x] Directives system (workspace + agent scoped, file-based)
- [x] Checkpoint-based snapshots for session persistence

### v0.4.1 — Discord Gateway ✓
- [x] First-party plugin: duragent-gateway-discord
- [x] Typing indicator during message processing
- [x] Memory tool robustness improvements

### v0.5.0 — Autonomy & Hardening ✓
- [x] Context window management (token budgeting, history truncation, priority-based context)
- [x] Session lifecycle management (idle timeout, explicit close)
- [x] Background process management (`spawn_process`, `manage_process` tools; tmux integration)
- [x] Web tools (`web_search` via Brave API, `web_fetch` with HTML-to-Markdown)
- [x] Dynamic tool discovery (convention-based auto-discovery from `tools/` directories)
- [x] `reload_tools` built-in for runtime tool registration
- [x] Skills system (modular Markdown-based capabilities, Agent Skills standard)
- [x] Tool hooks and steering queue for mid-loop message injection
- [x] Prime directives system (agent-level persistent instructions)
- [x] `duragent init` interactive setup command
- [x] `duragent doctor` workspace diagnostics
- [x] `duragent upgrade` self-update with SHA256 verification
- [x] OAuth PKCE authentication flow
- [x] Agent hot-reload via Admin API
- [x] Group chat support (mention gating, debouncing, per-requester approval)
- [x] Gateway protocol versioning
- [x] Session snapshots switched from YAML to JSON
- [x] Security hardening (constant-time token comparison, CSRF fix, connection limits, request body size limits, LLM call timeouts)
- [x] Persistence hardening (atomic writes with fsync, per-session locking, session store I/O off async lock path, memory write reliability)
- [x] Concurrency hardening (channel send timeouts, backpressure, narrowed lock scopes, stream-based callbacks, DashMap guard safety)
- [x] Stability fixes (UTF-8 panic prevention, empty messages handling, bounded line reading, actor message timeouts, KeyedLocks race fix, stdin/process cleanup race)

### v0.5.1 — Release Variants ✓
- [x] Core/full release variants (core = default features, full = all gateway features)
- [x] Variant-aware `duragent upgrade` (`--full` / `--core` flags, auto-detection)
- [x] Build variant exposed in `build_info::VARIANT` and `/version` endpoint
- [x] Checksum verification bugfix in `duragent upgrade`

### v0.6.0 — Observability & Sandbox
- [ ] Structured logging improvements
- [ ] Metrics (OpenTelemetry)
- [ ] Tracing
- [ ] Sandbox interface + auto-selection
- [ ] bubblewrap backend (Linux)
- [ ] Docker backend (cross-platform fallback)

### v0.7.0 — External Backends & Gateways
- [ ] Services: PostgreSQL backend
- [ ] Services: S3 backend
- [ ] Additional platform gateway plugins

### v0.8.0 — Agent Orchestration
- [ ] MCP tool integration
- [ ] Supervisor agent pattern
- [ ] Worker session management
- [ ] Inbound webhooks (trigger agent from external events)

### v0.9.0 — Production Ready
- [ ] Agent export/import
- [ ] Comprehensive test suite
- [ ] Full documentation (incl. OpenAPI)
- [ ] Performance benchmarks
- [ ] Security audit

### v1.0.0 — Stable Release
- [ ] Stable API (no breaking changes)
- [ ] Published to package managers (cargo, homebrew, apt)

## Known Issues

- **Windows: Auto-started server may not survive CLI exit**: The launcher (`src/launcher.rs`) uses `process_group(0)` on Unix to detach the server process, but Windows lacks equivalent handling. Workaround: use `duragent serve` in a separate terminal.
