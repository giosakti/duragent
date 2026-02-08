# Installation

## From Source

```bash
git clone https://github.com/giosakti/duragent.git
cd duragent
make build
./target/release/duragent --version
```

## Cargo Install

```bash
cargo install --git https://github.com/giosakti/duragent.git
```

## Requirements

- Rust 1.85+ (stable toolchain)
- Linux, macOS, or Windows

## Gateway Plugins

Gateway plugins (Telegram, Discord) are separate binaries. To install them:

```bash
# Discord gateway
cargo install --git https://github.com/giosakti/duragent.git duragent-gateway-discord

# Telegram gateway
cargo install --git https://github.com/giosakti/duragent.git duragent-gateway-telegram
```

## Verify Installation

```bash
duragent --version
```
