# Installation

## Prebuilt Binaries

Download the latest release from [GitHub Releases](https://github.com/giosakti/duragent/releases):

- **Full package** — built-in gateway integrations (Telegram, Discord)
- **Core package** — just the `duragent` binary

Extract the archive and place the binary in your `PATH`:

```bash
tar xzf duragent-*.tar.gz
sudo mv duragent /usr/local/bin/
duragent --version
```

## From Source

Requirements: Rust 1.91+ (stable toolchain)

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

## Verify Installation

```bash
duragent --version
```
