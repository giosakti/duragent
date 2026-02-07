# Contributing to Duragent

Thank you for your interest in contributing to Duragent! This document provides guidelines and instructions for contributing.

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
```

## How to Contribute

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
3. **Run the test suite** to ensure nothing is broken: `make test`
4. **Run the linter**: `make lint`
5. **Update documentation** if needed
6. **Submit a pull request**

#### PR Guidelines

- Keep PRs focused — one feature or fix per PR
- Write clear commit messages
- Update tests and documentation as needed
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

## Code Style

### Rust

- Follow standard Rust conventions
- Run `cargo fmt` (enforced by CI)
- Run `cargo clippy` for lints
- Keep functions focused and small
- Write descriptive variable names
- Add comments for non-obvious logic (explain "why", not "what")

### Documentation

- Use clear, concise language
- Include code examples where helpful
- Keep markdown files under 80 characters per line when possible

## Project Structure

```
duragent/
├── src/                # Source code
│   ├── main.rs         # CLI entrypoint
│   ├── config.rs       # Configuration loading
│   ├── handlers/       # HTTP handlers
│   └── ...
├── docs/               # Documentation
│   ├── specs/          # Design documents
│   └── examples/       # Example agents and skills
├── target/             # Build artifacts (gitignored)
└── Cargo.toml          # Dependencies
```

## Testing

```bash
# Run all tests
make test

# Run tests with output
make test-nocapture

# Run tests with coverage
make coverage
```

## Questions?

- Open an issue for bugs or feature requests
- Check existing documentation in `docs/`

Thank you for contributing!
