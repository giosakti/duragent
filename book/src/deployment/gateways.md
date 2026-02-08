# Gateway Setup

This guide covers setting up Telegram and Discord gateways for Duragent.

## Telegram

### 1. Create a Bot

1. Message [@BotFather](https://t.me/BotFather) on Telegram
2. Send `/newbot` and follow the prompts
3. Copy the bot token

### 2. Install the Gateway

```bash
cargo install --git https://github.com/giosakti/duragent.git duragent-gateway-telegram
```

### 3. Configure

```yaml
# duragent.yaml
gateways:
  telegram:
    enabled: true
    bot_token: ${TELEGRAM_BOT_TOKEN}

routes:
  - match:
      gateway: telegram
    agent: my-assistant
```

### 4. Start

```bash
export TELEGRAM_BOT_TOKEN=your-token
export OPENROUTER_API_KEY=your-key
duragent serve
```

Your bot is now live. Send it a message on Telegram.

### Telegram Features

- Long polling (no webhook setup needed)
- Inline keyboard buttons for tool approval
- Typing indicator while processing

## Discord

### 1. Create a Bot

1. Go to [Discord Developer Portal](https://discord.com/developers/applications)
2. Create a New Application
3. Go to **Bot** tab, click **Add Bot**
4. Copy the bot token
5. Under **Privileged Gateway Intents**, enable **Message Content Intent**
6. Go to **OAuth2 > URL Generator**, select `bot` scope with `Send Messages` and `Read Message History` permissions
7. Use the generated URL to invite the bot to your server

### 2. Install the Gateway

```bash
cargo install --git https://github.com/giosakti/duragent.git duragent-gateway-discord
```

### 3. Configure

```yaml
# duragent.yaml
gateways:
  external:
    - name: discord
      command: duragent-gateway-discord
      env:
        DISCORD_TOKEN: ${DISCORD_TOKEN}
      restart: on_failure

routes:
  - match:
      gateway: discord
    agent: my-assistant
```

### 4. Start

```bash
export DISCORD_TOKEN=your-token
export OPENROUTER_API_KEY=your-key
duragent serve
```

### Discord Features

- DM and server channel support
- Button components for tool approval
- 2000-character message chunking
- Reply threading
- Typing indicator while processing

## Multiple Gateways

You can run multiple gateways simultaneously with different routing:

```yaml
gateways:
  telegram:
    enabled: true
    bot_token: ${TELEGRAM_BOT_TOKEN}
  external:
    - name: discord
      command: duragent-gateway-discord
      env:
        DISCORD_TOKEN: ${DISCORD_TOKEN}

routes:
  - match:
      gateway: telegram
      chat_type: group
    agent: group-assistant

  - match:
      gateway: discord
    agent: discord-assistant

  - match: {}
    agent: default-assistant
```

## Custom Gateways

You can write custom gateways in any language. They communicate with Duragent via JSON Lines over stdio. See [Gateway Plugins](../guides/gateway-plugins.md) for the protocol specification.

```yaml
gateways:
  external:
    - name: my-custom-gateway
      command: ./my-gateway-binary
      restart: always
```
