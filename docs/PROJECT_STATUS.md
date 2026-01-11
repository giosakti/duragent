# Project Status - Pluto

> **Purpose:** Living status + session context. The stable vision and principles live in the [Project Charter](plans/202601111100.project-charter.md).

## Last Updated
2026-01-11

## Strategic Direction

See [Project Charter](plans/202601111100.project-charter.md) for vision, goals, and guiding principles.

Key specs and design docs:
- [Architecture](plans/202601111101.architecture.md)
- [API Reference](plans/202601111102.api-reference.md)
- [Deployment](plans/202601111103.deployment.md)
- [Pluto Agent Format](plans/202601111200.pluto-agent-format.md)

## Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Architecture | Stateless (nginx-like) | Simple, scalable, crash-resilient |
| Deployment philosophy | Stateless core, pluggable state | Simple mode (files) for self-hosted, complex mode (external) for SaaS |
| Storage default | File-based (`./.pluto/` files) | Zero dependencies, git-friendly, human-readable |
| Storage pluggable | PostgreSQL, Redis, S3 | Upgrade path for production deployments |
| Gateway | Embedded for simple, external for SaaS | Power users get single binary; SaaS apps own routing |
| Provider architecture | Hybrid (built-in + clean interface) | Ship common providers, keep a clean interface for extensibility |
| Runtime pattern | Simple event loop | Minimal complexity; add features only when needed |
| Services layer | Session, Memory, Artifact | Clean separation of concerns, pluggable backends |
| Multi-tenant | Yes, thousands per instance | Lazy loading + LRU cache |
| Agent deployment | Admin API + file-based | Supports both GitOps and dynamic |
| Agent definition | YAML + Markdown (PAF; no code) | Portability, inspectable, git-friendly |
| Discovery | A2A Agent Card | Standards-compliant agent discovery |
| Tool results | Content + Details separation | LLM sees minimal text/JSON; clients get structured metadata |
| Transport | HTTP + SSE default, WebSocket optional | SSE simpler; WebSocket for real-time needs |
| Security | Default permissive, optional sandbox | Low-friction for trusted setups; sandbox for multi-tenant |

## Roadmap / Milestones

### v0.1.0 — Foundation
- [ ] Agent spec loader (PAF: YAML + Markdown)
- [ ] Single LLM provider (OpenRouter)
- [ ] Basic agent executor (prompt → response)
- [ ] HTTP API (minimal, not Agent Protocol yet)
- [ ] CLI: `pluto serve`, `pluto chat`
- [ ] Docker image

### v0.2.0 — Standards
- [ ] Agent Protocol API (`/api/v1/agent/tasks`)
- [ ] Multiple LLM providers (OpenAI, Anthropic, Ollama)
- [ ] Services interfaces (Session/Memory/Artifacts) + external backends (optional)
- [ ] CLI: `pluto run`, `pluto validate`

### v0.3.0 — Tools & Memory
- [ ] MCP tool integration
- [ ] External memory backend
- [ ] Agent export/import
- [ ] CLI: `pluto export`, `pluto import`

### v0.4.0 — Production Ready
- [ ] Comprehensive test suite
- [ ] OpenAPI documentation
- [ ] Helm chart for Kubernetes
- [ ] Performance benchmarks
- [ ] Security audit

### v1.0.0 — Stable Release
- [ ] Stable API (no breaking changes)
- [ ] Full documentation
- [ ] Community contributions welcome
- [ ] Published to package managers

## Current Focus

**v0.1.0 — Foundation**: Agent spec loader + basic agent executor + minimal API/CLI.

## Recent Accomplishments

- (Add session notes here)

## Next Action

- (What’s the next concrete task?)

## Blockers / Known Issues / Decisions Needed

- (List blockers, known issues and open decisions)

## Session Notes

- (List volatile context for the next session - update as needed)
