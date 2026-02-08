//! Discord gateway for Duragent using serenity.
//!
//! This crate provides a Discord gateway that can be used:
//! - As a library (built-in mode): Import and call `DiscordGateway::start()`
//! - As a subprocess: Run the `duragent-discord` binary
//!
//! Both modes use the same Gateway Protocol for communication.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use duragent_gateway_protocol::{
    CallbackQueryData, GatewayCommand, GatewayEvent, InlineKeyboard, MessageContent,
    MessageReceivedData, RoutingContext, Sender, capabilities,
};
use serenity::all::{
    ChannelId, CreateActionRow, CreateButton, CreateInteractionResponse, CreateMessage,
    EditMessage, GatewayIntents, MessageId,
};
use serenity::async_trait;
use serenity::model::application::{ButtonStyle, ComponentInteraction, Interaction};
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::prelude::*;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Discord message character limit.
const MAX_MESSAGE_LENGTH: usize = 2000;

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for the Discord gateway.
#[derive(Debug, Clone)]
pub struct DiscordConfig {
    /// Discord bot token.
    pub bot_token: String,
}

impl DiscordConfig {
    /// Create a new config with the given bot token.
    pub fn new(bot_token: impl Into<String>) -> Self {
        Self {
            bot_token: bot_token.into(),
        }
    }
}

// ============================================================================
// Discord Gateway
// ============================================================================

/// Discord gateway that bridges the Discord Bot API with Duragent.
pub struct DiscordGateway {
    config: DiscordConfig,
    started_at: Instant,
}

impl DiscordGateway {
    /// Create a new Discord gateway.
    pub fn new(config: DiscordConfig) -> Self {
        Self {
            config,
            started_at: Instant::now(),
        }
    }

    /// Start the gateway and communicate via the provided channels.
    ///
    /// This method blocks until shutdown is requested.
    pub async fn start(
        self,
        event_tx: mpsc::Sender<GatewayEvent>,
        mut command_rx: mpsc::Receiver<GatewayCommand>,
    ) {
        let intents = GatewayIntents::GUILD_MESSAGES
            | GatewayIntents::DIRECT_MESSAGES
            | GatewayIntents::MESSAGE_CONTENT;

        let bot_user_id = Arc::new(tokio::sync::OnceCell::new());
        let handler = Handler {
            event_tx: event_tx.clone(),
            bot_user_id: bot_user_id.clone(),
        };

        let mut client = match Client::builder(&self.config.bot_token, intents)
            .event_handler(handler)
            .await
        {
            Ok(client) => client,
            Err(e) => {
                error!(error = %e, "Failed to create Discord client");
                let _ = event_tx
                    .send(GatewayEvent::Error {
                        code: "client_error".to_string(),
                        message: e.to_string(),
                        fatal: true,
                    })
                    .await;
                return;
            }
        };

        // Send ready event after client is created successfully
        let ready_event = GatewayEvent::Ready {
            gateway: "discord".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities: vec![
                capabilities::EDIT.to_string(),
                capabilities::DELETE.to_string(),
                capabilities::TYPING.to_string(),
                capabilities::REPLY.to_string(),
                capabilities::INLINE_KEYBOARD.to_string(),
            ],
        };
        if event_tx.send(ready_event).await.is_err() {
            error!("failed to send ready event");
            return;
        }

        info!("Discord gateway started");

        // Clone http for command handler and shard_manager for shutdown
        let http = client.http.clone();
        let shard_manager = client.shard_manager.clone();
        let event_tx_for_commands = event_tx.clone();
        let started_at = self.started_at;

        // Spawn command handler
        let command_handle = tokio::spawn(async move {
            let shard_manager = shard_manager;
            while let Some(command) = command_rx.recv().await {
                match command {
                    GatewayCommand::SendMessage {
                        request_id,
                        chat_id,
                        content,
                        reply_to,
                        inline_keyboard,
                    } => {
                        let result = send_message(
                            &http,
                            &chat_id,
                            &content,
                            reply_to.as_deref(),
                            inline_keyboard.as_ref(),
                        )
                        .await;

                        let event = match result {
                            Ok(msg_id) => GatewayEvent::CommandOk {
                                request_id,
                                message_id: Some(msg_id),
                            },
                            Err(e) => GatewayEvent::CommandError {
                                request_id,
                                code: "send_failed".to_string(),
                                message: e,
                            },
                        };

                        if event_tx_for_commands.send(event).await.is_err() {
                            break;
                        }
                    }

                    GatewayCommand::SendTyping { chat_id, .. } => {
                        let channel_id: u64 = match chat_id.parse() {
                            Ok(id) => id,
                            Err(_) => continue,
                        };
                        let _ = ChannelId::new(channel_id).broadcast_typing(&http).await;
                    }

                    GatewayCommand::EditMessage {
                        request_id,
                        chat_id,
                        message_id,
                        content,
                    } => {
                        let result = edit_message(&http, &chat_id, &message_id, &content).await;

                        let event = match result {
                            Ok(_) => GatewayEvent::CommandOk {
                                request_id,
                                message_id: Some(message_id),
                            },
                            Err(e) => GatewayEvent::CommandError {
                                request_id,
                                code: "edit_failed".to_string(),
                                message: e,
                            },
                        };

                        if event_tx_for_commands.send(event).await.is_err() {
                            break;
                        }
                    }

                    GatewayCommand::DeleteMessage {
                        request_id,
                        chat_id,
                        message_id,
                    } => {
                        let result = delete_message(&http, &chat_id, &message_id).await;

                        let event = match result {
                            Ok(_) => GatewayEvent::CommandOk {
                                request_id,
                                message_id: None,
                            },
                            Err(e) => GatewayEvent::CommandError {
                                request_id,
                                code: "delete_failed".to_string(),
                                message: e,
                            },
                        };

                        if event_tx_for_commands.send(event).await.is_err() {
                            break;
                        }
                    }

                    GatewayCommand::Ping { request_id } => {
                        let event = GatewayEvent::Pong {
                            request_id,
                            uptime_seconds: started_at.elapsed().as_secs(),
                            connected: true,
                        };
                        if event_tx_for_commands.send(event).await.is_err() {
                            break;
                        }
                    }

                    GatewayCommand::AnswerCallbackQuery { request_id, .. } => {
                        // Discord interactions are acknowledged immediately in interaction_create,
                        // so this is a no-op. Just confirm success.
                        let event = GatewayEvent::CommandOk {
                            request_id,
                            message_id: None,
                        };
                        if event_tx_for_commands.send(event).await.is_err() {
                            break;
                        }
                    }

                    GatewayCommand::Shutdown => {
                        info!("Discord gateway received shutdown command");
                        shard_manager.shutdown_all().await;
                        let _ = event_tx_for_commands
                            .send(GatewayEvent::Shutdown {
                                reason: "shutdown requested".to_string(),
                            })
                            .await;
                        break;
                    }

                    GatewayCommand::SendMedia { request_id, .. } => {
                        let event = GatewayEvent::CommandError {
                            request_id,
                            code: "not_implemented".to_string(),
                            message: "Media sending not yet implemented".to_string(),
                        };
                        if event_tx_for_commands.send(event).await.is_err() {
                            break;
                        }
                    }
                }
            }
            debug!("Command handler stopped");
        });

        // Start the Discord client (this blocks until shutdown)
        if let Err(e) = client.start().await {
            error!(error = %e, "Discord client error");
        }

        // Clean up
        command_handle.abort();
        info!("Discord gateway stopped");
    }
}

// ============================================================================
// Event Handler
// ============================================================================

struct Handler {
    event_tx: mpsc::Sender<GatewayEvent>,
    /// Bot user ID, set from the Ready event.
    bot_user_id: Arc<tokio::sync::OnceCell<u64>>,
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, _ctx: Context, msg: Message) {
        // Skip bot messages to avoid loops
        if msg.author.bot {
            return;
        }

        let content = MessageContent::Text {
            text: msg.content.clone(),
        };

        let sender = extract_sender(&msg);
        let routing = extract_routing(&msg);

        // Detect if bot was @mentioned or replied to
        let bot_id = self.bot_user_id.get().copied();
        let mentions_bot = bot_id.is_some_and(|id| msg.mentions.iter().any(|u| u.id.get() == id));
        let reply_to_bot = bot_id.is_some_and(|id| {
            msg.referenced_message
                .as_ref()
                .is_some_and(|m| m.author.id.get() == id)
        });

        let event = GatewayEvent::MessageReceived(Box::new(MessageReceivedData {
            message_id: msg.id.to_string(),
            chat_id: msg.channel_id.to_string(),
            sender,
            content,
            routing,
            reply_to: msg.referenced_message.as_ref().map(|m| m.id.to_string()),
            mentions_bot,
            reply_to_bot,
            timestamp: {
                let ts = msg.timestamp;
                chrono::DateTime::from_timestamp(ts.unix_timestamp(), ts.nanosecond())
            },
            metadata: serde_json::json!({}),
        }));

        if let Err(e) = self.event_tx.send(event).await {
            warn!(error = %e, "Failed to send message event");
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let Interaction::Component(component) = interaction else {
            return;
        };

        handle_component_interaction(&ctx, &component, &self.event_tx).await;
    }

    async fn ready(&self, _ctx: Context, ready: Ready) {
        let _ = self.bot_user_id.set(ready.user.id.get());
        info!(
            user = %ready.user.name,
            user_id = %ready.user.id,
            "Discord bot connected"
        );
    }
}

async fn handle_component_interaction(
    ctx: &Context,
    component: &ComponentInteraction,
    event_tx: &mpsc::Sender<GatewayEvent>,
) {
    // Acknowledge immediately — Discord requires response within 3 seconds
    let ack = CreateInteractionResponse::Acknowledge;
    if let Err(e) = component.create_response(&ctx.http, ack).await {
        warn!(error = %e, "Failed to acknowledge interaction");
        return;
    }

    let sender = Sender {
        id: component.user.id.to_string(),
        username: Some(component.user.name.clone()),
        display_name: component.user.global_name.clone(),
    };

    let event = GatewayEvent::CallbackQuery(Box::new(CallbackQueryData {
        callback_query_id: component.id.to_string(),
        chat_id: component.channel_id.to_string(),
        sender,
        message_id: component.message.id.to_string(),
        data: component.data.custom_id.clone(),
    }));

    if let Err(e) = event_tx.send(event).await {
        warn!(error = %e, "Failed to send callback query event");
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn extract_sender(msg: &Message) -> Sender {
    Sender {
        id: msg.author.id.to_string(),
        username: Some(msg.author.name.clone()),
        display_name: msg.author.global_name.clone(),
    }
}

fn extract_routing(msg: &Message) -> RoutingContext {
    let chat_type = if msg.guild_id.is_some() {
        "group"
    } else {
        "dm"
    };

    let mut extra = HashMap::new();
    if let Some(guild_id) = msg.guild_id {
        extra.insert("guild_id".to_string(), guild_id.to_string());
    }

    RoutingContext {
        channel: "discord".to_string(),
        chat_type: chat_type.to_string(),
        chat_id: msg.channel_id.to_string(),
        sender_id: msg.author.id.to_string(),
        extra,
    }
}

// ============================================================================
// Command Execution
// ============================================================================

async fn send_message(
    http: &Arc<serenity::http::Http>,
    chat_id: &str,
    content: &str,
    reply_to: Option<&str>,
    inline_keyboard: Option<&InlineKeyboard>,
) -> Result<String, String> {
    let channel_id: u64 = chat_id.parse().map_err(|_| "invalid chat_id".to_string())?;
    let channel = ChannelId::new(channel_id);

    let chunks = chunk_message(content);
    let last_idx = chunks.len() - 1;
    let mut last_msg_id = String::new();

    // Parse reply_to message ID once
    let reply_to_id = reply_to
        .and_then(|id| id.parse::<u64>().ok())
        .map(MessageId::new);

    for (i, chunk) in chunks.iter().enumerate() {
        let mut builder = CreateMessage::new().content(*chunk);

        // Reply only on the first chunk
        if i == 0
            && let Some(msg_id) = reply_to_id
        {
            builder = builder.reference_message((channel, msg_id));
        }

        // Attach buttons only to the last chunk
        if i == last_idx
            && let Some(keyboard) = inline_keyboard
        {
            let rows = convert_inline_keyboard(keyboard);
            builder = builder.components(rows);
        }

        let msg = channel
            .send_message(http, builder)
            .await
            .map_err(|e| e.to_string())?;

        last_msg_id = msg.id.to_string();
    }

    Ok(last_msg_id)
}

fn convert_inline_keyboard(keyboard: &InlineKeyboard) -> Vec<CreateActionRow> {
    keyboard
        .rows
        .iter()
        .map(|row| {
            let buttons: Vec<CreateButton> = row
                .iter()
                .map(|btn| {
                    CreateButton::new(&btn.callback_data)
                        .label(&btn.text)
                        .style(ButtonStyle::Primary)
                })
                .collect();
            CreateActionRow::Buttons(buttons)
        })
        .collect()
}

fn chunk_message(content: &str) -> Vec<&str> {
    if content.len() <= MAX_MESSAGE_LENGTH {
        return vec![content];
    }

    let mut chunks = Vec::new();
    let mut remaining = content;

    while !remaining.is_empty() {
        if remaining.len() <= MAX_MESSAGE_LENGTH {
            chunks.push(remaining);
            break;
        }

        // Try to split at a newline within the limit
        let boundary = remaining.floor_char_boundary(MAX_MESSAGE_LENGTH);
        let split_at = remaining[..boundary].rfind('\n').unwrap_or(boundary);

        let (chunk, rest) = remaining.split_at(split_at);
        chunks.push(chunk);
        // Skip the newline if we split at one
        remaining = rest.strip_prefix('\n').unwrap_or(rest);
    }

    chunks
}

async fn edit_message(
    http: &Arc<serenity::http::Http>,
    chat_id: &str,
    message_id: &str,
    content: &str,
) -> Result<(), String> {
    let channel_id: u64 = chat_id.parse().map_err(|_| "invalid chat_id".to_string())?;
    let msg_id: u64 = message_id
        .parse()
        .map_err(|_| "invalid message_id".to_string())?;

    ChannelId::new(channel_id)
        .edit_message(
            http,
            MessageId::new(msg_id),
            EditMessage::new().content(content),
        )
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

async fn delete_message(
    http: &Arc<serenity::http::Http>,
    chat_id: &str,
    message_id: &str,
) -> Result<(), String> {
    let channel_id: u64 = chat_id.parse().map_err(|_| "invalid chat_id".to_string())?;
    let msg_id: u64 = message_id
        .parse()
        .map_err(|_| "invalid message_id".to_string())?;

    ChannelId::new(channel_id)
        .delete_message(http, MessageId::new(msg_id))
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}
