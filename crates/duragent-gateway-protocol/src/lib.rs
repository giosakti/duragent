//! Gateway Protocol types for communication between Duragent and gateway plugins.
//!
//! This crate defines the protocol for gateway plugins to communicate with Duragent.
//! Use this crate to build custom gateways in Rust.
//!
//! # Protocol Overview
//!
//! The protocol is bidirectional with JSON Lines (newline-delimited JSON) over stdio:
//!
//! - **Commands** (Duragent → Gateway): Instructions from Duragent to the gateway
//! - **Events** (Gateway → Duragent): Notifications from the gateway to Duragent
//!
//! # Example: Minimal Gateway
//!
//! ```ignore
//! use duragent_gateway_protocol::{GatewayCommand, GatewayEvent};
//!
//! // Read commands from stdin
//! let line = read_line_from_stdin();
//! let command: GatewayCommand = serde_json::from_str(&line)?;
//!
//! // Send events to stdout
//! let event = GatewayEvent::Ready {
//!     gateway: "my-gateway".to_string(),
//!     version: "1.0.0".to_string(),
//!     capabilities: vec![],
//! };
//! println!("{}", serde_json::to_string(&event)?);
//! ```

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ============================================================================
// Commands (Duragent → Gateway)
// ============================================================================

/// Commands sent from Duragent to a gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GatewayCommand {
    /// Send a text message to a chat.
    SendMessage {
        request_id: String,
        chat_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to: Option<String>,
        /// Optional inline keyboard for approval prompts.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        inline_keyboard: Option<InlineKeyboard>,
    },

    /// Send media (image, video, audio, document) to a chat.
    SendMedia {
        request_id: String,
        chat_id: String,
        media: MediaPayload,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
    },

    /// Show typing indicator in a chat.
    SendTyping {
        chat_id: String,
        /// Duration in seconds (0 = stop typing indicator).
        #[serde(default)]
        duration: u32,
    },

    /// Edit a previously sent message.
    EditMessage {
        request_id: String,
        chat_id: String,
        message_id: String,
        content: String,
    },

    /// Delete a message.
    DeleteMessage {
        request_id: String,
        chat_id: String,
        message_id: String,
    },

    /// Health check / ping.
    Ping { request_id: String },

    /// Answer a callback query (dismiss loading indicator on button press).
    AnswerCallbackQuery {
        request_id: String,
        callback_query_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },

    /// Request graceful shutdown.
    Shutdown,
}

/// Inline keyboard for interactive buttons in messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineKeyboard {
    /// Rows of buttons (each row is a Vec of buttons).
    pub rows: Vec<Vec<InlineButton>>,
}

/// A button in an inline keyboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineButton {
    /// Button text displayed to user.
    pub text: String,
    /// Callback data sent when button is pressed.
    pub callback_data: String,
}

impl InlineKeyboard {
    /// Create a single-row keyboard with the given buttons.
    pub fn single_row(buttons: Vec<InlineButton>) -> Self {
        Self {
            rows: vec![buttons],
        }
    }
}

impl InlineButton {
    /// Create a new button.
    pub fn new(text: impl Into<String>, callback_data: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            callback_data: callback_data.into(),
        }
    }
}

/// Media payload for SendMedia command.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum MediaPayload {
    /// Fetch media from URL.
    Url {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
    /// Base64-encoded media data.
    Base64 {
        data: String,
        mime_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
    },
}

// ============================================================================
// Events (Gateway → Duragent)
// ============================================================================

/// Events sent from a gateway to Duragent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GatewayEvent {
    /// Gateway is ready to receive commands.
    Ready {
        gateway: String,
        version: String,
        #[serde(default)]
        capabilities: Vec<String>,
    },

    /// Incoming message from a user.
    MessageReceived(Box<MessageReceivedData>),

    /// Callback query from inline keyboard button press.
    CallbackQuery(Box<CallbackQueryData>),

    /// Command completed successfully.
    CommandOk {
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
    },

    /// Command failed.
    CommandError {
        request_id: String,
        code: String,
        message: String,
    },

    /// Response to Ping command.
    Pong {
        request_id: String,
        uptime_seconds: u64,
        connected: bool,
    },

    /// Gateway-level error (not tied to a specific command).
    Error {
        code: String,
        message: String,
        /// Whether this error is fatal (gateway will shut down).
        #[serde(default)]
        fatal: bool,
    },

    /// Gateway is shutting down.
    Shutdown { reason: String },

    /// Authentication required (e.g., WhatsApp QR code).
    AuthRequired { method: AuthMethod },

    /// Authentication successful.
    AuthSuccess,
}

/// Data for a callback query event (inline keyboard button press).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallbackQueryData {
    /// Unique identifier for this callback query.
    pub callback_query_id: String,
    /// Chat where the callback originated.
    pub chat_id: String,
    /// User who pressed the button.
    pub sender: Sender,
    /// Message ID that contained the inline keyboard.
    pub message_id: String,
    /// Data from the pressed button.
    pub data: String,
}

/// Data for an incoming message event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageReceivedData {
    pub message_id: String,
    pub chat_id: String,
    pub sender: Sender,
    pub content: MessageContent,
    pub routing: RoutingContext,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// Timestamp when the message was sent (from the platform).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub metadata: serde_json::Value,
}

/// Sender information for incoming messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sender {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// Content of an incoming message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageContent {
    /// Plain text message.
    Text { text: String },

    /// Media message (image, video, audio, document).
    Media {
        media_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
    },

    /// Location message.
    Location { latitude: f64, longitude: f64 },

    /// Contact message.
    Contact { name: String, phone: String },

    /// Unknown/unsupported content type.
    Unknown {
        #[serde(default)]
        raw: serde_json::Value,
    },
}

impl MessageContent {
    /// Extract text content if this is a text message.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            MessageContent::Text { text } => Some(text),
            MessageContent::Media { caption, .. } => caption.as_deref(),
            _ => None,
        }
    }
}

/// Routing context for message routing decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingContext {
    /// Gateway/platform name (e.g., "telegram", "whatsapp").
    pub channel: String,

    /// Chat type (e.g., "dm", "group", "channel").
    pub chat_type: String,

    /// Chat identifier.
    pub chat_id: String,

    /// Sender identifier.
    pub sender_id: String,

    /// Additional platform-specific fields for routing.
    #[serde(flatten, default)]
    pub extra: HashMap<String, String>,
}

/// Authentication method for gateways requiring user authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum AuthMethod {
    /// QR code authentication (e.g., WhatsApp Web).
    QrCode {
        /// Raw QR code data (for rendering).
        qr_data: String,
        /// ASCII art representation for terminal display.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        qr_ascii: Option<String>,
        /// Seconds until QR code expires.
        expires_in: u32,
    },

    /// Pairing code authentication.
    PairCode {
        /// The pairing code to enter on the device.
        code: String,
        /// Seconds until code expires.
        expires_in: u32,
    },
}

// ============================================================================
// Gateway Capabilities
// ============================================================================

/// Well-known gateway capabilities.
pub mod capabilities {
    /// Gateway supports sending media.
    pub const MEDIA: &str = "media";
    /// Gateway supports editing messages.
    pub const EDIT: &str = "edit";
    /// Gateway supports deleting messages.
    pub const DELETE: &str = "delete";
    /// Gateway supports typing indicators.
    pub const TYPING: &str = "typing";
    /// Gateway supports reply-to (threading).
    pub const REPLY: &str = "reply";
    /// Gateway supports inline keyboards for interactive buttons.
    pub const INLINE_KEYBOARD: &str = "inline_keyboard";
}

// ============================================================================
// Error Codes
// ============================================================================

/// Well-known error codes for CommandError and Error events.
pub mod error_codes {
    /// Chat/conversation not found.
    pub const CHAT_NOT_FOUND: &str = "chat_not_found";
    /// Message not found (for edit/delete).
    pub const MESSAGE_NOT_FOUND: &str = "message_not_found";
    /// Rate limited by platform.
    pub const RATE_LIMITED: &str = "rate_limited";
    /// Not authorized to perform action.
    pub const UNAUTHORIZED: &str = "unauthorized";
    /// Platform API error.
    pub const PLATFORM_ERROR: &str = "platform_error";
    /// Invalid request from Duragent.
    pub const INVALID_REQUEST: &str = "invalid_request";
    /// Gateway not connected to platform.
    pub const NOT_CONNECTED: &str = "not_connected";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_serialization() {
        let cmd = GatewayCommand::SendMessage {
            request_id: "req_001".to_string(),
            chat_id: "123".to_string(),
            content: "Hello!".to_string(),
            reply_to: None,
            inline_keyboard: None,
        };

        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""type":"send_message""#));

        let parsed: GatewayCommand = serde_json::from_str(&json).unwrap();
        match parsed {
            GatewayCommand::SendMessage { content, .. } => {
                assert_eq!(content, "Hello!");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_event_serialization() {
        let event = GatewayEvent::Ready {
            gateway: "telegram".to_string(),
            version: "0.1.0".to_string(),
            capabilities: vec!["media".to_string(), "edit".to_string()],
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"ready""#));

        let parsed: GatewayEvent = serde_json::from_str(&json).unwrap();
        match parsed {
            GatewayEvent::Ready { gateway, .. } => {
                assert_eq!(gateway, "telegram");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_message_content_as_text() {
        let text = MessageContent::Text {
            text: "hello".to_string(),
        };
        assert_eq!(text.as_text(), Some("hello"));

        let media = MessageContent::Media {
            media_type: "image".to_string(),
            url: None,
            caption: Some("caption".to_string()),
        };
        assert_eq!(media.as_text(), Some("caption"));

        let location = MessageContent::Location {
            latitude: 0.0,
            longitude: 0.0,
        };
        assert_eq!(location.as_text(), None);
    }
}
