# Project Status - Duragent

> **Purpose:** Living status + session context. The stable vision and principles live in the [Project Charter](./202601111100.project-charter.md).

## Last Updated
2026-02-06

## Strategic Direction

See [Project Charter](./202601111100.project-charter.md) for vision, goals, and guiding principles.

Key specs and design docs:
- [Architecture](./202601111101.architecture.md)
- [API Reference](./202601111102.api-reference.md)
- [Deployment](./202601111103.deployment.md)
- [Duragent Format](./202601111200.duragent-format.md)

## Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Architecture | Crash-resilient core, durable sessions | Immediate persistence ensures recovery; sessions survive disconnects |
| Deployment philosophy | Persistent-first, pluggable backends | Simple mode (files) for self-hosted, complex mode (external) for SaaS |
| Storage default | File-based (`./.duragent/` files) | Zero dependencies, git-friendly, human-readable |
| Storage pluggable | PostgreSQL, Redis, S3 | Upgrade path for production deployments |
| Storage formats | JSONL (events), YAML (state), Markdown (prose) | JSONL for high-frequency append-only writes; YAML for structured snapshots; Markdown for human content |
| Session persistence | Append-only event log + periodic YAML snapshot | Fast writes (append), fast reads (snapshot), human-readable, survives disconnects |
| Session disconnect | Configurable: `continue` or `pause` | `continue` for async workflows (agent keeps working); `pause` for interactive chat |
| Sandbox default | bubblewrap (Linux); Docker or trust mode (macOS/Windows) | Lightweight on Linux; heavier isolation or trust mode elsewhere |
| Sandbox pluggable | Docker, None (trust mode) | Heavier isolation when needed |
| Terminal attachment | `duragent attach` for remote sessions | "SSH to your agent" — connect/disconnect/resume, session survives |
| Agent orchestration | Duragent supervises external agents (Claude Code, OpenCode) | Supervisor agent delegates to specialized workers |
| Worker agents | Claude Code headless, OpenCode, custom | Leverage existing tools via their CLI/SDK interfaces |
| Orchestration pattern | Supervisor reviews worker output | LLM-based review loop; approve/reject/retry before committing |
| Core gateways | CLI/TUI, HTTP REST, SSE (built-in) | Protocols that ship with Duragent; SSE for streaming (simpler than WebSocket) |
| Platform gateways | Subprocess plugins (Telegram, Discord, Slack) | JSON-over-stdio protocol; crash independently; any language |
| Runtime pattern | Simple event loop | Minimal complexity; add features only when needed |
| Services layer | Session, Memory, Artifact | Clean separation of concerns, pluggable backends |
| Multi-tenant | Server: thousands; Edge: optimized for few | Lazy loading + LRU cache; edge prioritizes efficiency |
| Agent deployment | Admin API + file-based | Supports both GitOps and dynamic |
| Agent definition | YAML + Markdown (Duragent Format; no code) | Portability, inspectable, git-friendly |
| Discovery | A2A Agent Card | Standards-compliant agent discovery |
| Tool results | Content + Details separation | LLM sees minimal text/JSON; clients get structured metadata |
| Security | Sandboxed by default (Linux); trust mode available | Sandbox for isolation; trust mode fallback for trusted setups |
| Agent routing | Routing table (first match wins) | Flexible, extensible; user controls specificity via ordering |
| Session compaction | Auto-compact after N events; archive old events | Prevents unbounded storage growth |
| Tool approval | Optional approval workflow for dangerous tools | Safety without sacrificing autonomy |
| Scheduled tasks | Agent-initiated via tools; YAML persistence | Agents can create reminders and recurring checks autonomously |
| Observability | Structured logging (JSON); metrics + tracing planned | Debug + monitor from day one |

## Roadmap / Milestones

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

### v0.5.0 — Context & Observability
- [ ] Context window management
- [ ] Structured logging improvements
- [ ] Metrics (OpenTelemetry)
- [ ] Tracing

### v0.6.0 — Sandbox
- [ ] Sandbox interface + auto-selection
- [ ] bubblewrap backend (Linux)
- [ ] Docker backend (cross-platform fallback)

### v0.7.0 — External Backends & Gateways
- [ ] Services: PostgreSQL backend
- [ ] Services: Redis backend
- [ ] Services: S3 backend
- [ ] First-party plugin: duragent-gateway-whatsapp

### v0.8.0 — Agent Orchestration
- [ ] MCP tool integration
- [ ] Built-in tool: `claude_code_exec` (invoke Claude Code headless)
- [ ] Built-in tool: `opencode_exec` (invoke OpenCode headless)
- [ ] Supervisor agent pattern (review worker output before committing)
- [ ] Worker session management (track/resume worker sessions)
- [ ] Async notifications (Slack, email, webhook)
- [ ] Inbound webhooks (trigger agent from external events like GitHub, CI)

### v0.9.0 — Production Ready
- [ ] Agent export/import
- [ ] Comprehensive test suite
- [ ] Full documentation (incl. OpenAPI)
- [ ] Performance benchmarks
- [ ] Security audit

### v1.0.0 — Stable Release
- [ ] Stable API (no breaking changes)
- [ ] Published to package managers (cargo, homebrew, apt)
- [ ] Helm chart for Kubernetes

### Future — Edge & Embedded
- [ ] ARM64 + x86_64 builds
- [ ] Raspberry Pi testing in CI
- [ ] Performance benchmarks (x86, ARM)
- [ ] `duragent-lite` minimal build
- [ ] <50MB runtime memory validation

## Current Focus

**v0.4.1 — Discord Gateway**: Adding Discord as a first-party gateway plugin.

## Recent Accomplishments

- **Discord gateway** — Second platform gateway, mirroring Telegram patterns
  - Built-in (feature flag) and subprocess modes
  - Button components for approval flow
  - 2000 char message chunking, reply support
  - Typing indicator while processing (applied to all gateways)

- **Memory system** — File-based agent memory with four tools
  - `recall`: Retrieve relevant memories from agent's memory store
  - `remember`: Store new memories (daily experiences, curated MEMORY.md)
  - `reflect`: Consolidate and reorganize memories
  - `update_world`: Update shared world knowledge facts
  - World memory (shared across agents) + agent memory (per-agent)

- **Directives system** — File-based runtime instructions
  - Workspace-scoped directives (`{workspace}/directives/*.md`)
  - Agent-scoped directives (`{agent_dir}/directives/*.md`)
  - Auto-created `memory.md` directive when memory is enabled
  - Loaded and injected into context with proper priority ordering

- **Checkpoint-based snapshots** — Improved session persistence
  - Snapshots store only checkpointed messages (not full history)
  - Pending messages rebuilt from events since checkpoint
  - Reduces O(N²) snapshot I/O to O(N)

- **Session actor refactoring** — Actor model for session state
  - Per-session actor with serialized state mutations via message passing
  - Batched event writes + periodic snapshots
  - Trait-based storage abstraction for pluggable backends

- **Concurrency fixes** — Various reliability improvements
  - Atomic get-or-insert for gateway session creation
  - Per-session serialization with concurrent gateway event processing
  - Semaphore-limited concurrent scheduled task executions

- **v0.4.0 complete** — Tools, policy, scheduling, memory, directives, checkpoint snapshots, actor-based sessions, concurrency fixes (see milestone checklist above for details)

## Next Action

- Implement v0.4.1: duragent-gateway-discord plugin

## Blockers / Known Issues / Decisions Needed

- **Windows: Auto-started server may not survive CLI exit**: The launcher (`src/launcher.rs`) uses `process_group(0)` on Unix to detach the server process, but Windows lacks equivalent handling. On Windows, the auto-started server may be killed when the CLI exits. Workaround: use `duragent serve` in a separate terminal. Fix: add `CREATE_NEW_PROCESS_GROUP` via `std::os::windows::process::CommandExt`.
- **State Synchronization**: `SessionStore` maintains parallel in-memory state while persistence logic is distributed across handlers and streaming code. Risk: forgetting to persist causes state drift on crash. Options: (1) write-through cache in SessionStore, or (2) SessionService layer that wraps store + persistence together. Recommendation: SessionService keeps the store simple (sync, easy to test) while centralizing persistence calls.
- **Snapshot Data Duplication**: ~~`state.yaml` stores full conversation history, duplicating `events.jsonl`.~~ **Resolved** — checkpoint-based snapshots now store only checkpointed messages; pending messages rebuilt from events since checkpoint.

## Session Notes

*Older notes (v0.1–v0.3 design decisions) archived — see git history for details.*
