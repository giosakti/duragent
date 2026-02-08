# Contributing to Duragent

Thank you for your interest in contributing to Duragent! This document provides guidelines and instructions for contributing.

To make the best use of your time, consider opening an issue to discuss your idea before starting a PR — this helps ensure your contribution aligns with the project's direction.

## Development Setup

### Prerequisites

- Rust 1.85+ (stable toolchain)
- Git

### Getting Started

```bash
# Clone the repository
git clone https://github.com/giosakti/duragent.git
cd duragent

# Build
make build

# Run tests
make test

# Run linter
make lint

# Run tests with output
make test-nocapture

# Run tests with coverage
make coverage
```

## Architecture Overview

Duragent is a Rust workspace with four crates:

```
duragent/
├── crates/
│   ├── duragent/                     # Core runtime (binary + library)
│   │   └── src/
│   │       ├── main.rs               # CLI entrypoint (clap)
│   │       ├── config.rs             # Configuration loading (duragent.yaml)
│   │       ├── session/              # Session actor, persistence, lifecycle
│   │       ├── gateway/              # Gateway manager, handler, subprocess
│   │       ├── handlers/             # HTTP handlers (Axum)
│   │       ├── llm/                  # LLM provider clients
│   │       ├── tools/                # Tool execution, policy, scheduling
│   │       ├── memory/               # Memory system (recall, remember, reflect)
│   │       ├── context/              # Context building, directives, truncation
│   │       ├── scheduler/            # Scheduled task service
│   │       └── agent/                # Agent spec loading, policy
│   ├── duragent-gateway-protocol/    # Shared protocol types (JSON Lines over stdio)
│   ├── duragent-gateway-telegram/    # Telegram gateway plugin
│   └── duragent-gateway-discord/     # Discord gateway plugin
├── book/                             # mdBook user guide (source)
├── docs/
│   ├── PROJECT_STATUS.md             # Roadmap and current focus
│   ├── internal/                     # Internal design documents (maintainer reference)
│   └── examples/                     # Example agents and skills
├── .github/workflows/                # CI/CD pipelines
├── Cargo.toml                        # Workspace root
├── Makefile
└── LICENSE                           # MIT
```

| Crate | Purpose |
|-------|---------|
| `duragent` | Core runtime: HTTP server, session management, LLM calls, tools, memory, scheduling |
| `duragent-gateway-protocol` | Shared types for the Gateway Protocol (used by both core and plugins) |
| `duragent-gateway-telegram` | Telegram bot gateway — compiles as library (built-in) or binary (subprocess) |
| `duragent-gateway-discord` | Discord bot gateway — compiles as library (built-in) or binary (subprocess) |

For deeper architectural details, see `docs/internal/` (internal design documents for maintainers).

## Code Style

- Follow standard Rust conventions
- Run `make fmt` (enforced by CI)
- Keep functions focused and small
- Write descriptive variable names
- Add comments for non-obvious logic (explain "why", not "what")

### Error Handling

- Use `anyhow` for application errors (in the `duragent` crate)
- Use `thiserror` for library errors (in protocol/gateway crates)
- Prefer `?` propagation over manual matching where appropriate

### Async

- Tokio runtime throughout
- `async_trait` for async trait definitions
- Avoid holding locks across `.await` points

## Submitting Changes

### Reporting Bugs

Before creating a bug report, please check existing issues to avoid duplicates.

When filing a bug report, include:
- A clear, descriptive title
- Steps to reproduce the issue
- Expected vs actual behavior
- Rust version (`rustc --version`)
- OS and version

### Suggesting Features

Feature requests are welcome! Please:
- Check existing issues first
- Describe the use case clearly
- Explain why this feature would be useful

### Pull Requests

1. **Fork the repository** and create your branch from `main`
2. **Write tests** for any new functionality
3. **Run `make test`** to ensure nothing is broken
4. **Run `make lint`** to check for lints
5. **Update documentation** if needed
6. **Submit a pull request**

#### PR Guidelines

- Keep PRs focused — one feature or fix per PR
- Ensure CI passes before requesting review

### Commit Messages

We follow conventional commit style:

```
<type>: <description>

[optional body]
```

Types:
- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation only
- `refactor`: Code change that neither fixes a bug nor adds a feature
- `test`: Adding or updating tests
- `chore`: Maintenance tasks

Examples:
```
feat: add streaming support to chat endpoint
fix: handle empty agent config gracefully
docs: update API reference with new endpoints
```

## Questions?

- Open an issue for bugs or feature requests
- Check existing documentation in `docs/`

Thank you for contributing!
