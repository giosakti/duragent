# Quick Start

This guide walks you through creating your first agent, starting the server, and chatting with it.

## 1. Initialize a Workspace

```bash
duragent init
# Follow the interactive setup
```

This creates a `.duragent/` directory with a starter agent and configuration.

## 2. Authenticate and Start the Server

Set up credentials for your LLM provider, then start the server:

```bash
# Option A: OAuth login (Anthropic only — tokens auto-refresh)
duragent login anthropic

# Option B: API key via environment variable
export OPENROUTER_API_KEY=your-key
```

```bash
duragent serve
```

See [Authentication](../guides/authentication.md) for details on all providers.

## 3. Chat with Your Agent

```bash
duragent chat --agent <YOUR_AGENT_NAME>
```

Type your message and press Enter. The agent will respond using the configured LLM.

## 4. Attach to a Session Later

Sessions are durable — you can disconnect and reconnect at any time:

```bash
duragent attach --list       # List attachable sessions
duragent attach SESSION_ID   # Reconnect to existing session
```

## What's Next?

- Learn about the [Agent Format](../guides/agent-format.md) to customize your agents
- Add [Tools](../guides/tools-and-policies.md) to give your agent capabilities
- Set up [Telegram or Discord](../deployment/gateways.md) to chat from messaging platforms
- Configure [Memory](../guides/memory.md) for persistent agent knowledge
