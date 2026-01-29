# Project Status - Agnx

> **Purpose:** Living status + session context. The stable vision and principles live in the [Project Charter](./202601111100.project-charter.md).

## Last Updated
2026-01-29

## Strategic Direction

See [Project Charter](./202601111100.project-charter.md) for vision, goals, and guiding principles.

Key specs and design docs:
- [Architecture](./202601111101.architecture.md)
- [API Reference](./202601111102.api-reference.md)
- [Deployment](./202601111103.deployment.md)
- [Agnx Agent Format](./202601111200.agnx-agent-format.md)

## Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Architecture | Crash-resilient core, durable sessions | Immediate persistence ensures recovery; sessions survive disconnects |
| Deployment philosophy | Persistent-first, pluggable backends | Simple mode (files) for self-hosted, complex mode (external) for SaaS |
| Storage default | File-based (`./.agnx/` files) | Zero dependencies, git-friendly, human-readable |
| Storage pluggable | PostgreSQL, Redis, S3 | Upgrade path for production deployments |
| Storage formats | JSONL (events), YAML (state), Markdown (prose) | JSONL for high-frequency append-only writes; YAML for structured snapshots; Markdown for human content |
| Session persistence | Append-only event log + periodic YAML snapshot | Fast writes (append), fast reads (snapshot), human-readable, survives disconnects |
| Session disconnect | Configurable: `continue` or `pause` | `continue` for async workflows (agent keeps working); `pause` for interactive chat |
| Sandbox default | bubblewrap (Linux); Docker or trust mode (macOS/Windows) | Lightweight on Linux; heavier isolation or trust mode elsewhere |
| Sandbox pluggable | Docker, None (trust mode) | Heavier isolation when needed |
| Terminal attachment | `agnx attach` for remote sessions | "SSH to your agent" — connect/disconnect/resume, session survives |
| Agent orchestration | Agnx supervises external agents (Claude Code, OpenCode) | Supervisor agent delegates to specialized workers |
| Worker agents | Claude Code headless, OpenCode, custom | Leverage existing tools via their CLI/SDK interfaces |
| Orchestration pattern | Supervisor reviews worker output | LLM-based review loop; approve/reject/retry before committing |
| Core gateways | CLI/TUI, HTTP REST, SSE (built-in) | Protocols that ship with Agnx; SSE for streaming (simpler than WebSocket) |
| Platform gateways | Subprocess plugins (Telegram, Discord, Slack) | JSON-over-stdio protocol; crash independently; any language |
| Runtime pattern | Simple event loop | Minimal complexity; add features only when needed |
| Services layer | Session, Memory, Artifact | Clean separation of concerns, pluggable backends |
| Multi-tenant | Server: thousands; Edge: optimized for few | Lazy loading + LRU cache; edge prioritizes efficiency |
| Agent deployment | Admin API + file-based | Supports both GitOps and dynamic |
| Agent definition | YAML + Markdown (AAF; no code) | Portability, inspectable, git-friendly |
| Discovery | A2A Agent Card | Standards-compliant agent discovery |
| Tool results | Content + Details separation | LLM sees minimal text/JSON; clients get structured metadata |
| Security | Sandboxed by default (Linux); trust mode available | Sandbox for isolation; trust mode fallback for trusted setups |
| Agent routing | Routing table (first match wins) | Flexible, extensible; user controls specificity via ordering |
| Session compaction | Auto-compact after N events; archive old events | Prevents unbounded storage growth |
| Tool approval | Optional approval workflow for dangerous tools | Safety without sacrificing autonomy |
| Observability | Structured logging (JSON); metrics + tracing planned | Debug + monitor from day one |

## Roadmap / Milestones

### v0.1.0 — Foundation ✓
- [x] Agent spec loader (AAF: YAML + Markdown)
- [x] LLM providers (OpenRouter, OpenAI, Anthropic, Ollama)
- [x] Basic agent executor (prompt → response)
- [x] Core gateways: CLI, HTTP REST
- [x] CLI: `agnx serve`, `agnx chat`
- [x] Sessions API (in-memory)
- [x] Integration tests

### v0.2.0 — Sessions & Durability ✓
- [x] Session persistence (JSONL events + YAML snapshots)
- [x] Session resume on reconnect
- [x] Session disconnect behavior (`continue` / `pause`)
- [x] CLI: `agnx attach` (connect to running/paused session)
- [x] Core gateways: SSE streaming

### v0.3.0 — Gateway Plugins ✓
- [x] Gateway plugin protocol (JSON over stdio)
- [x] First-party plugin: agnx-gateway-telegram
- [x] Plugin configuration in agnx.yaml
- [x] Trust mode (no isolation) — sandbox placeholder

### v0.4.0 — Tools & Memory
- [ ] CLI tool support (lightweight alternative to MCP)
- [ ] MCP tool integration
- [ ] File-based memory bank
- [ ] Agent export/import
- [ ] CLI: `agnx export`, `agnx import`

### v0.5.0 — Sandbox
- [ ] Sandbox interface + auto-selection
- [ ] bubblewrap backend (Linux)
- [ ] Docker backend (cross-platform fallback)

### v0.6.0 — External Backends & More Gateways
- [ ] Services: PostgreSQL backend
- [ ] Services: Redis backend
- [ ] Services: S3 backend
- [ ] First-party plugin: agnx-gateway-whatsapp

### v0.7.0 — Agent Orchestration
- [ ] Built-in tool: `claude_code_exec` (invoke Claude Code headless)
- [ ] Built-in tool: `opencode_exec` (invoke OpenCode headless)
- [ ] Supervisor agent pattern (review worker output before committing)
- [ ] Worker session management (track/resume worker sessions)
- [ ] Async notifications (Slack, email, webhook)
- [ ] Inbound webhooks (trigger agent from external events like GitHub, CI)

### v0.8.0 — Production Ready
- [ ] Comprehensive test suite
- [ ] Full documentation (incl. OpenAPI)
- [ ] Performance benchmarks
- [ ] Security audit

### v1.0.0 — Stable Release
- [ ] Stable API (no breaking changes)
- [ ] Published to package managers (cargo, homebrew, apt)
- [ ] Helm chart for Kubernetes
- [ ] Community contributions welcome

### Future — Edge & Embedded
- [ ] ARM64 + x86_64 builds
- [ ] Raspberry Pi testing in CI
- [ ] Performance benchmarks (x86, ARM)
- [ ] `agnx-lite` minimal build
- [ ] <50MB runtime memory validation

## Current Focus

**v0.3.0 — Gateway Plugins**: Complete. Ready for release.

## Recent Accomplishments

- **v0.3.0 complete** — Gateway Plugins
- Added trust mode sandbox (`Sandbox` trait + `TrustSandbox` implementation) as placeholder for tool execution
- Implemented Gateway Protocol (`agnx-gateway-protocol` crate) for external gateway developers
- Built Telegram gateway (`agnx-gateway-telegram`) supporting both built-in and subprocess modes
- Refactored to workspace structure (`crates/agnx`, `crates/agnx-gateway-*`)
- Added Gateway Manager with unified interface for built-in and subprocess gateways
- Implemented subprocess supervision with restart policies, exponential backoff, and parent death handling
- Added session routing (gateway + chat_id persisted in snapshots)
- Added global agent routing rules with match conditions (gateway, chat_type, chat_id, sender_id)
- Added agent tracking in AssistantMessage events for mid-session agent switching

- **v0.2.0 released** — Sessions & Durability complete
- Implemented session persistence (JSONL events + YAML snapshots) with atomic writes
- Added session resume with snapshot + event replay on reconnect
- Added session disconnect behavior (`continue` / `pause`) with background continuation
- Implemented `agnx attach` command for connecting to running/paused sessions
- Added SSE streaming endpoint with keep-alive heartbeat and idle timeout
- Added background task registry for graceful shutdown
- Added HTTP client library for CLI-to-server communication
- Comprehensive integration tests for persistence, resume, and disconnect behavior

- **v0.1.0 released** — Foundation complete
- Implemented agent spec loader (AAF: YAML + Markdown)
- Added LLM provider abstraction (OpenRouter, OpenAI, Anthropic, Ollama)
- Implemented basic agent executor (prompt → response)
- Added `agnx serve` and `agnx chat` CLI commands
- Added Sessions API (create, get, send message)
- Added integration tests for HTTP API
- Refactored codebase into library + binary structure

## Next Action

- Release v0.3.0
- Begin v0.4.0: CLI tool support, MCP integration, memory

## Blockers / Known Issues / Decisions Needed

- **Windows: Auto-started server may not survive CLI exit**: The launcher (`src/launcher.rs`) uses `process_group(0)` on Unix to detach the server process, but Windows lacks equivalent handling. On Windows, the auto-started server may be killed when the CLI exits. Workaround: use `agnx serve` in a separate terminal. Fix: add `CREATE_NEW_PROCESS_GROUP` via `std::os::windows::process::CommandExt`.
- **State Synchronization**: `SessionStore` maintains parallel in-memory state while persistence logic is distributed across handlers and streaming code. Risk: forgetting to persist causes state drift on crash. Options: (1) write-through cache in SessionStore, or (2) SessionService layer that wraps store + persistence together. Recommendation: SessionService keeps the store simple (sync, easy to test) while centralizing persistence calls.
- **Snapshot Data Duplication**: `state.yaml` stores full conversation history, duplicating `events.jsonl`. For N messages, naive snapshots cause O(N²) total I/O. Options and tradeoffs:
  - *Periodic checkpointing* (snapshot every K events) — O(N²) → O(N) I/O; recovery replays events since checkpoint; **recommended first step**
  - *Sliding window* — bounds memory; loses old context
  - *Summary-based* — bounds memory + I/O; lossy compression

## Session Notes

**2026-01-24 — Additional analysis**

**Patterns to Adopt:**
- **Agent routing table**: First match wins; flat match object; extensible fields; user controls specificity via ordering
- **Hot reload with validation**: File watcher + schema validation + per-component restart (no full restart)
- **Exec approval workflow**: Interactive approval for dangerous tools (bash, file writes)
- **Tool streaming**: Chunk tool results for real-time feedback via SSE
- **Structured logging**: JSONL with subsystem hierarchy; add from day one

**Patterns to Avoid:**
- **Monolithic process**: All channels/agents in one process; crash affects everything → Agnx uses subprocess plugins
- **File lock contention**: Synchronous file locks block event loop → use non-blocking `flock` or DB advisory locks
- **Unbounded session growth**: JSONL grows forever → add compaction policy with pruning
- **Config complexity**: Large schema, scattered docs → start minimal, fail fast with clear errors
- **No observability**: No metrics/tracing/alerting → add OpenTelemetry from day one
- **Type erosion**: `Record<string, any>` for flexibility → Rust's type system prevents this

**2026-01-24 — Gateway & Session Refinement**

Finalized gateway architecture:
- **Core gateways** (built-in): CLI/TUI, HTTP REST, SSE — these are protocols, not platforms
- **Platform gateways** (plugins): Telegram, Discord, Slack run as subprocess plugins via Gateway Protocol (JSON over stdio)
- Removed WebSocket — SSE is sufficient for LLM streaming, simpler to implement and proxy

Session disconnect behavior:
- `on_disconnect: continue` — agent keeps executing, buffers output (for async/supervisor workflows)
- `on_disconnect: pause` — agent pauses at next safe point (for interactive chat)
- Configurable per-agent in `agent.yaml`, with global defaults in `agnx.yaml`

This enables the key use case: "fire and forget" tasks where user disconnects and agent continues working, notifies when done.

**2025-01-23 — Strategic Direction Update**

Killer feature direction: **Sessions that survive crashes, restarts, and disconnects**
- Terminal attachment (`agnx attach`) — SSH-like connection to agent sessions
- Session persistence — conversation + sandbox state survives disconnects
- Pluggable sandboxes — bubblewrap (light) → Docker → cloud

**Agent Orchestration Direction**

Rather than competing with Claude Code, OpenCode, Aider, etc., Agnx will **orchestrate** them:
- Supervisor agent runs on Agnx (always-on, async)
- Delegates coding tasks to Claude Code (headless mode: `claude -p "task" --output-format json`)
- Reviews worker output before approving (LLM-based review loop)
- User doesn't need to be at terminal — supervisor handles it

Key insight: Claude Code supports headless mode with `--output-format json` and `--resume` for session continuity. Agnx can invoke and supervise it programmatically.

This positions Agnx as **infrastructure that makes any agent better** — not another agent competing for users.
