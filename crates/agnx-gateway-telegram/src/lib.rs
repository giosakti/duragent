//! Telegram gateway for Agnx using teloxide.
//!
//! This crate provides a Telegram gateway that can be used:
//! - As a library (built-in mode): Import and call `TelegramGateway::start()`
//! - As a subprocess: Run the `agnx-telegram` binary
//!
//! Both modes use the same Gateway Protocol for communication.

use std::collections::HashMap;
use std::time::Instant;

use agnx_gateway_protocol::{
    GatewayCommand, GatewayEvent, MessageContent, MessageReceivedData, RoutingContext, Sender,
    capabilities,
};
use teloxide::prelude::*;
use teloxide::types::{MediaKind, MessageKind};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for the Telegram gateway.
#[derive(Debug, Clone)]
pub struct TelegramConfig {
    /// Telegram bot token from BotFather.
    pub bot_token: String,
}

impl TelegramConfig {
    /// Create a new config with the given bot token.
    pub fn new(bot_token: impl Into<String>) -> Self {
        Self {
            bot_token: bot_token.into(),
        }
    }
}

// ============================================================================
// Telegram Gateway
// ============================================================================

/// Telegram gateway that bridges Telegram Bot API with Agnx.
pub struct TelegramGateway {
    config: TelegramConfig,
    started_at: Instant,
}

impl TelegramGateway {
    /// Create a new Telegram gateway.
    pub fn new(config: TelegramConfig) -> Self {
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
        // Configure HTTP client with timeout longer than polling timeout
        let client = teloxide::net::default_reqwest_settings()
            .timeout(std::time::Duration::from_secs(60))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to build HTTP client");

        let bot = Bot::with_client(&self.config.bot_token, client);

        // Send ready event
        let ready_event = GatewayEvent::Ready {
            gateway: "telegram".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities: vec![
                capabilities::MEDIA.to_string(),
                capabilities::EDIT.to_string(),
                capabilities::DELETE.to_string(),
                capabilities::TYPING.to_string(),
                capabilities::REPLY.to_string(),
            ],
        };
        if event_tx.send(ready_event).await.is_err() {
            error!("Failed to send ready event");
            return;
        }

        info!("Telegram gateway starting");

        // Build dispatcher and get shutdown token for graceful shutdown
        let handler = Update::filter_message().endpoint({
            let event_tx = event_tx.clone();
            move |bot: Bot, msg: Message| {
                let event_tx = event_tx.clone();
                async move {
                    if let Err(e) = handle_message(&bot, &msg, &event_tx).await {
                        warn!(error = %e, "Failed to handle message");
                    }
                    respond(())
                }
            }
        });

        let mut dispatcher = Dispatcher::builder(bot.clone(), handler).build();
        let shutdown_token = dispatcher.shutdown_token();

        // Clone for command handler
        let bot_for_commands = bot.clone();
        let event_tx_for_commands = event_tx.clone();
        let started_at = self.started_at;

        // Spawn command handler
        let command_handle = tokio::spawn(async move {
            while let Some(command) = command_rx.recv().await {
                match command {
                    GatewayCommand::SendMessage {
                        request_id,
                        chat_id,
                        content,
                        reply_to,
                    } => {
                        let result = send_message(
                            &bot_for_commands,
                            &chat_id,
                            &content,
                            reply_to.as_deref(),
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
                        let chat_id: i64 = match chat_id.parse() {
                            Ok(id) => id,
                            Err(_) => continue,
                        };
                        let _ = bot_for_commands
                            .send_chat_action(ChatId(chat_id), teloxide::types::ChatAction::Typing)
                            .await;
                    }

                    GatewayCommand::EditMessage {
                        request_id,
                        chat_id,
                        message_id,
                        content,
                    } => {
                        let result =
                            edit_message(&bot_for_commands, &chat_id, &message_id, &content).await;

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
                        let result = delete_message(&bot_for_commands, &chat_id, &message_id).await;

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

                    GatewayCommand::Shutdown => {
                        info!("Telegram gateway received shutdown command");
                        // Trigger graceful shutdown of the dispatcher
                        drop(
                            shutdown_token
                                .shutdown()
                                .expect("Failed to shutdown dispatcher"),
                        );
                        let _ = event_tx_for_commands
                            .send(GatewayEvent::Shutdown {
                                reason: "shutdown requested".to_string(),
                            })
                            .await;
                        break;
                    }

                    GatewayCommand::SendMedia { request_id, .. } => {
                        // TODO: Implement media sending
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

        // Configure long polling with appropriate timeout
        let polling = teloxide::update_listeners::Polling::builder(bot)
            .timeout(std::time::Duration::from_secs(30))
            .build();

        // Start the dispatcher (this blocks until shutdown)
        dispatcher
            .dispatch_with_listener(
                polling,
                teloxide::error_handlers::LoggingErrorHandler::with_custom_text(
                    "Telegram polling error (will retry)",
                ),
            )
            .await;

        // Clean up
        command_handle.abort();
        info!("Telegram gateway stopped");
    }
}

// ============================================================================
// Message Handling
// ============================================================================

async fn handle_message(
    _bot: &Bot,
    msg: &Message,
    event_tx: &mpsc::Sender<GatewayEvent>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let content = extract_content(msg);
    let Some(content) = content else {
        debug!("Ignoring message without extractable content");
        return Ok(());
    };

    let sender = extract_sender(msg);
    let routing = extract_routing(msg);

    let event = GatewayEvent::MessageReceived(Box::new(MessageReceivedData {
        message_id: msg.id.0.to_string(),
        chat_id: msg.chat.id.0.to_string(),
        sender,
        content,
        routing,
        reply_to: msg.reply_to_message().map(|m| m.id.0.to_string()),
        timestamp: Some(msg.date),
        metadata: serde_json::json!({}),
    }));

    event_tx.send(event).await?;
    Ok(())
}

fn extract_content(msg: &Message) -> Option<MessageContent> {
    match &msg.kind {
        MessageKind::Common(common) => match &common.media_kind {
            MediaKind::Text(text) => Some(MessageContent::Text {
                text: text.text.clone(),
            }),
            MediaKind::Photo(photo) => Some(MessageContent::Media {
                media_type: "image".to_string(),
                url: None, // Would need to call getFile API
                caption: photo.caption.clone(),
            }),
            MediaKind::Video(video) => Some(MessageContent::Media {
                media_type: "video".to_string(),
                url: None,
                caption: video.caption.clone(),
            }),
            MediaKind::Audio(audio) => Some(MessageContent::Media {
                media_type: "audio".to_string(),
                url: None,
                caption: audio.caption.clone(),
            }),
            MediaKind::Document(doc) => Some(MessageContent::Media {
                media_type: "document".to_string(),
                url: None,
                caption: doc.caption.clone(),
            }),
            MediaKind::Voice(voice) => Some(MessageContent::Media {
                media_type: "voice".to_string(),
                url: None,
                caption: voice.caption.clone(),
            }),
            MediaKind::Location(loc) => Some(MessageContent::Location {
                latitude: loc.location.latitude,
                longitude: loc.location.longitude,
            }),
            MediaKind::Contact(contact) => Some(MessageContent::Contact {
                name: format!(
                    "{} {}",
                    contact.contact.first_name,
                    contact.contact.last_name.as_deref().unwrap_or("")
                )
                .trim()
                .to_string(),
                phone: contact.contact.phone_number.clone(),
            }),
            _ => None,
        },
        _ => None,
    }
}

fn extract_sender(msg: &Message) -> Sender {
    msg.from
        .as_ref()
        .map(|user| Sender {
            id: user.id.0.to_string(),
            username: user.username.clone(),
            display_name: Some(
                format!(
                    "{} {}",
                    user.first_name,
                    user.last_name.as_deref().unwrap_or("")
                )
                .trim()
                .to_string(),
            ),
        })
        .unwrap_or_else(|| Sender {
            id: "unknown".to_string(),
            username: None,
            display_name: None,
        })
}

fn extract_routing(msg: &Message) -> RoutingContext {
    let chat_type = match &msg.chat.kind {
        teloxide::types::ChatKind::Private(_) => "dm",
        teloxide::types::ChatKind::Public(public) => match &public.kind {
            teloxide::types::PublicChatKind::Group => "group",
            teloxide::types::PublicChatKind::Supergroup(_) => "group",
            teloxide::types::PublicChatKind::Channel(_) => "channel",
        },
    };

    let mut extra = HashMap::new();

    // Add thread ID if present
    if let Some(thread_id) = msg.thread_id {
        extra.insert("thread_id".to_string(), thread_id.0.to_string());
    }

    RoutingContext {
        channel: "telegram".to_string(),
        chat_type: chat_type.to_string(),
        chat_id: msg.chat.id.0.to_string(),
        sender_id: msg
            .from
            .as_ref()
            .map(|u| u.id.0.to_string())
            .unwrap_or_default(),
        extra,
    }
}

// ============================================================================
// Command Execution
// ============================================================================

async fn send_message(
    bot: &Bot,
    chat_id: &str,
    content: &str,
    reply_to: Option<&str>,
) -> Result<String, String> {
    let chat_id: i64 = chat_id.parse().map_err(|_| "invalid chat_id".to_string())?;

    let mut request = bot.send_message(ChatId(chat_id), content);

    if let Some(reply_to) = reply_to
        && let Ok(msg_id) = reply_to.parse::<i32>()
    {
        request = request.reply_parameters(teloxide::types::ReplyParameters::new(
            teloxide::types::MessageId(msg_id),
        ));
    }

    let msg = request.await.map_err(|e| e.to_string())?;
    Ok(msg.id.0.to_string())
}

async fn edit_message(
    bot: &Bot,
    chat_id: &str,
    message_id: &str,
    content: &str,
) -> Result<(), String> {
    let chat_id: i64 = chat_id.parse().map_err(|_| "invalid chat_id".to_string())?;
    let message_id: i32 = message_id
        .parse()
        .map_err(|_| "invalid message_id".to_string())?;

    bot.edit_message_text(
        ChatId(chat_id),
        teloxide::types::MessageId(message_id),
        content,
    )
    .await
    .map_err(|e| e.to_string())?;

    Ok(())
}

async fn delete_message(bot: &Bot, chat_id: &str, message_id: &str) -> Result<(), String> {
    let chat_id: i64 = chat_id.parse().map_err(|_| "invalid chat_id".to_string())?;
    let message_id: i32 = message_id
        .parse()
        .map_err(|_| "invalid message_id".to_string())?;

    bot.delete_message(ChatId(chat_id), teloxide::types::MessageId(message_id))
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}
