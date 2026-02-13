# Authentication

Duragent supports multiple LLM providers. Each provider requires credentials — either an API key set as an environment variable or, for Anthropic, an OAuth login.

## Methods

| Method | Providers | Storage | Auto-Refresh |
|--------|-----------|---------|-------------|
| **OAuth login** | Anthropic only | `~/.duragent/auth.json` | Yes |
| **Environment variable** | All providers | Shell environment | No |

When both are configured for the same provider, OAuth credentials take precedence over the environment variable.

## OAuth Login (Anthropic)

```bash
duragent login anthropic
```

This starts a browser-based OAuth flow (PKCE):

1. A URL is printed — open it in your browser
2. Authorize the application on Anthropic's site
3. Copy the returned code and paste it back into the terminal
4. Tokens are saved to `~/.duragent/auth.json` (mode 0600, user-only read/write)

Tokens are automatically refreshed when they expire. You do not need to run `duragent login` again unless the refresh token is revoked.

> **Security:** Never commit `~/.duragent/auth.json` to version control.

## Environment Variables

For providers that don't support OAuth login, set the API key as an environment variable:

| Variable | Provider |
|----------|----------|
| `ANTHROPIC_API_KEY` | Anthropic (Claude) |
| `OPENROUTER_API_KEY` | OpenRouter |
| `OPENAI_API_KEY` | OpenAI-compatible providers |

At least one provider must be configured for agents to function. Ollama does not require an API key (local inference).

```bash
# Example: OpenRouter
export OPENROUTER_API_KEY=your-key
duragent serve
```

## Per-Provider Setup

### Anthropic

**Recommended:** Use OAuth login for automatic token refresh.

```bash
duragent login anthropic
```

**Alternative:** Set the API key manually.

```bash
export ANTHROPIC_API_KEY=sk-ant-...
```

Agent configuration:

```yaml
spec:
  model:
    provider: anthropic
    name: claude-sonnet-4-20250514
```

### OpenRouter

```bash
export OPENROUTER_API_KEY=your-key
```

```yaml
spec:
  model:
    provider: openrouter
    name: anthropic/claude-sonnet-4
```

### OpenAI

```bash
export OPENAI_API_KEY=sk-...
```

```yaml
spec:
  model:
    provider: openai
    name: gpt-4o
```

### Ollama (Local)

No API key needed. Ollama must be running locally.

```yaml
spec:
  model:
    provider: ollama
    name: llama3
    base_url: http://localhost:11434
```

## Credential Precedence

For Anthropic, credentials are resolved in this order:

1. OAuth token from `~/.duragent/auth.json` (if present and valid)
2. `ANTHROPIC_API_KEY` environment variable

For all other providers, only the environment variable is checked.

## Troubleshooting

| Problem | Solution |
|---------|----------|
| "Unsupported provider" from `duragent login` | Only `anthropic` supports OAuth. Use environment variables for other providers. |
| OAuth token expired | Tokens auto-refresh. If the refresh token is revoked, run `duragent login anthropic` again. |
| "No LLM provider configured" | Set at least one API key or run `duragent login anthropic`. |
| Credentials not taking effect | Restart the server after changing environment variables or running `duragent login`. |
