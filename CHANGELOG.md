# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/giosakti/agnx/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/giosakti/agnx/compare/v0.0.1...v0.1.0
[0.0.1]: https://github.com/giosakti/agnx/releases/tag/v0.0.1
