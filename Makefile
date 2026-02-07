.PHONY: build test test-nocapture lint coverage clean run help

# Build variables
BINARY_NAME := duragent
BUILD_DIR := target
VERSION := $(shell git describe --tags --always --dirty 2>/dev/null || echo "dev")
COMMIT := $(shell git rev-parse --short HEAD 2>/dev/null || echo "none")
DATE := $(shell date -u +%Y-%m-%dT%H:%M:%SZ)

## help: Show this help message
help:
	@echo "Usage: make [target]"
	@echo ""
	@echo "Targets:"
	@sed -n 's/^##//p' $(MAKEFILE_LIST) | column -t -s ':' | sed 's/^/ /'

## build: Build the binary
build:
	cargo build --release

## test: Run tests
test:
	cargo test

## test-nocapture: Run tests with output (don't capture stdout/stderr)
test-nocapture:
	cargo test -- --nocapture

## lint: Run linter
lint:
	cargo clippy -- -D warnings

## coverage: Run tests with coverage report
coverage:
	cargo tarpaulin --out Html
	@echo "Coverage report: tarpaulin-report.html"

## clean: Remove build artifacts
clean:
	cargo clean
	rm -f tarpaulin-report.html

## run: Build and run the server
run:
	cargo run -- serve

## fmt: Format code
fmt:
	cargo fmt

## check: Run all checks (lint + test)
check: lint test
