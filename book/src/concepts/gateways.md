# Gateways

Gateways handle communication between users and agents. Duragent separates **core gateways** (built-in) from **platform gateways** (plugins).

## Core Gateways (Built-in)

These protocols ship with Duragent:

| Gateway | Use Case |
|---------|----------|
| **CLI** | Local development, `duragent chat`, `duragent attach` |
| **HTTP REST** | Programmatic access, webhooks, Admin API |
| **SSE** | Real-time LLM token streaming |

## Platform Gateways (Plugins)

Platform-specific integrations (Telegram, Discord, etc.) run as separate processes communicating via the **Gateway Protocol** — JSON Lines over stdio.

```
┌──────────────────────────────────┐
│           Duragent               │
│  ┌────────────────────────────┐  │
│  │   Core Gateways (built-in) │  │
│  │   CLI │ HTTP REST │ SSE    │  │
│  └────────────────────────────┘  │
│              │                   │
│       Gateway Protocol           │
│      (JSON Lines / stdio)        │
│              │                   │
└──────────────┼───────────────────┘
               │
    ┌──────────┼──────────┐
    ▼          ▼          ▼
[Telegram]  [Discord]  [others]
 (plugin)   (plugin)   (plugin)
```

This design means:

- **Gateway crashes don't affect core** — plugins are isolated processes
- **Any language** — plugins can be written in Rust, Python, Go, etc.
- **Independent releases** — plugins ship separately from Duragent core
- **Lightweight core** — platform SDKs aren't bundled in the main binary

## Gateway Protocol

Plugins communicate with Duragent using JSON Lines over stdin/stdout:

| Direction | Message Types |
|-----------|--------------|
| Duragent to plugin | `send_message`, `send_typing`, `edit_message`, `ping`, `shutdown` |
| Plugin to Duragent | `ready`, `message_received`, `command_ok`, `command_error`, `shutdown` |

First-party plugins (Telegram, Discord) are written in Rust and ship as standalone binaries. Third-party plugins can use any language.

## Available Gateways

| Gateway | Status | Crate |
|---------|--------|-------|
| CLI | Built-in | `duragent` |
| HTTP REST | Built-in | `duragent` |
| SSE | Built-in | `duragent` |
| Telegram | Plugin | `duragent-gateway-telegram` |
| Discord | Plugin | `duragent-gateway-discord` |

For setup instructions, see [Gateway Setup](../deployment/gateways.md). For configuration details, see [Gateway Plugins](../guides/gateway-plugins.md).
