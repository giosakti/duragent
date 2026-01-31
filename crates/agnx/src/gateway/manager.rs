//! Gateway Manager for managing built-in and external gateway plugins.
//!
//! The Gateway Manager provides a unified interface for:
//! - Registering and starting gateways (built-in or external)
//! - Routing messages from gateways to sessions
//! - Sending responses back through gateways
//! - Managing gateway lifecycle (start, stop, restart)

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{RwLock, mpsc};
use tracing::{debug, error, info, warn};

use agnx_gateway_protocol::{
    GatewayCommand, GatewayEvent, InlineButton, InlineKeyboard, MessageContent, RoutingContext,
};

// ============================================================================
// Gateway Manager
// ============================================================================

/// Manager for all gateway plugins.
///
/// Handles registration, lifecycle, and message routing for both
/// built-in and external gateways.
#[derive(Clone)]
pub struct GatewayManager {
    inner: Arc<RwLock<GatewayManagerInner>>,
}

struct GatewayManagerInner {
    /// Registered gateways by name.
    gateways: HashMap<String, GatewayHandle>,

    /// Message handler for incoming messages.
    handler: Option<Arc<dyn MessageHandler>>,
}

impl GatewayManager {
    /// Create a new gateway manager.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(GatewayManagerInner {
                gateways: HashMap::new(),
                handler: None,
            })),
        }
    }

    /// Set the message handler for incoming gateway messages.
    pub async fn set_handler(&self, handler: Arc<dyn MessageHandler>) {
        let mut inner = self.inner.write().await;
        inner.handler = Some(handler);
    }

    /// Register a gateway and get channels for communication.
    ///
    /// Returns:
    /// - `Receiver<GatewayCommand>`: Gateway receives commands from Agnx
    /// - `Sender<GatewayEvent>`: Gateway sends events to Agnx
    pub async fn register(
        &self,
        name: impl Into<String>,
        capabilities: Vec<String>,
    ) -> (mpsc::Receiver<GatewayCommand>, mpsc::Sender<GatewayEvent>) {
        let name = name.into();
        let (cmd_tx, cmd_rx) = mpsc::channel(100);
        let (evt_tx, evt_rx) = mpsc::channel(100);

        let handle = GatewayHandle {
            name: name.clone(),
            command_tx: cmd_tx,
            capabilities,
        };

        {
            let mut inner = self.inner.write().await;
            inner.gateways.insert(name.clone(), handle);
        }

        // Spawn event handler task
        let manager = self.clone();
        let gateway_name = name.clone();
        tokio::spawn(async move {
            manager.handle_events(gateway_name, evt_rx).await;
        });

        info!(gateway = %name, "Gateway registered");
        (cmd_rx, evt_tx)
    }

    /// Unregister a gateway.
    pub async fn unregister(&self, name: &str) {
        let mut inner = self.inner.write().await;
        if inner.gateways.remove(name).is_some() {
            info!(gateway = %name, "Gateway unregistered");
        }
    }

    /// Get a gateway handle by name.
    pub async fn get(&self, name: &str) -> Option<GatewayHandle> {
        let inner = self.inner.read().await;
        inner.gateways.get(name).map(|h| GatewayHandle {
            name: h.name.clone(),
            command_tx: h.command_tx.clone(),
            capabilities: h.capabilities.clone(),
        })
    }

    /// List all registered gateways.
    pub async fn list(&self) -> Vec<String> {
        let inner = self.inner.read().await;
        inner.gateways.keys().cloned().collect()
    }

    /// Send a message through a gateway.
    pub async fn send_message(
        &self,
        gateway: &str,
        chat_id: &str,
        content: &str,
        reply_to: Option<String>,
    ) -> Result<(), SendError> {
        self.send_message_with_keyboard(gateway, chat_id, content, reply_to, None)
            .await
    }

    /// Send a message through a gateway with an optional inline keyboard.
    pub async fn send_message_with_keyboard(
        &self,
        gateway: &str,
        chat_id: &str,
        content: &str,
        reply_to: Option<String>,
        inline_keyboard: Option<InlineKeyboard>,
    ) -> Result<(), SendError> {
        let handle = {
            let inner = self.inner.read().await;
            inner.gateways.get(gateway).map(|h| h.command_tx.clone())
        };

        let Some(tx) = handle else {
            warn!(gateway = %gateway, "Gateway not found");
            return Err(SendError::ChannelClosed);
        };

        let request_id = uuid::Uuid::new_v4().to_string();
        let command = GatewayCommand::SendMessage {
            request_id,
            chat_id: chat_id.to_string(),
            content: content.to_string(),
            reply_to,
            inline_keyboard,
        };

        tx.send(command).await.map_err(|_| SendError::ChannelClosed)
    }

    /// Send typing indicator through a gateway.
    pub async fn send_typing(&self, gateway: &str, chat_id: &str) -> Result<(), SendError> {
        let handle = {
            let inner = self.inner.read().await;
            inner.gateways.get(gateway).map(|h| h.command_tx.clone())
        };

        let Some(tx) = handle else {
            return Err(SendError::ChannelClosed);
        };

        let command = GatewayCommand::SendTyping {
            chat_id: chat_id.to_string(),
            duration: 5,
        };

        tx.send(command).await.map_err(|_| SendError::ChannelClosed)
    }

    /// Answer a callback query with an optional notification text.
    ///
    /// This dismisses the loading indicator on the button and optionally
    /// shows a toast notification to the user.
    pub async fn answer_callback_query(
        &self,
        gateway: &str,
        callback_query_id: &str,
        text: Option<String>,
    ) -> Result<(), SendError> {
        let handle = {
            let inner = self.inner.read().await;
            inner.gateways.get(gateway).map(|h| h.command_tx.clone())
        };

        let Some(tx) = handle else {
            return Err(SendError::ChannelClosed);
        };

        let request_id = uuid::Uuid::new_v4().to_string();
        let command = GatewayCommand::AnswerCallbackQuery {
            request_id,
            callback_query_id: callback_query_id.to_string(),
            text,
        };

        tx.send(command).await.map_err(|_| SendError::ChannelClosed)
    }

    /// Shutdown all gateways gracefully.
    pub async fn shutdown(&self) {
        let gateways = {
            let inner = self.inner.read().await;
            inner
                .gateways
                .iter()
                .map(|(k, v)| (k.clone(), v.command_tx.clone()))
                .collect::<Vec<_>>()
        };

        for (name, tx) in gateways {
            debug!(gateway = %name, "Sending shutdown to gateway");
            let _ = tx.send(GatewayCommand::Shutdown).await;
        }
    }

    /// Handle events from a gateway.
    async fn handle_events(&self, gateway: String, mut rx: mpsc::Receiver<GatewayEvent>) {
        while let Some(event) = rx.recv().await {
            match event {
                GatewayEvent::Ready {
                    gateway: gw_name,
                    version,
                    capabilities,
                } => {
                    info!(
                        gateway = %gateway,
                        reported_name = %gw_name,
                        version = %version,
                        capabilities = ?capabilities,
                        "Gateway ready"
                    );
                }

                GatewayEvent::MessageReceived(data) => {
                    debug!(
                        gateway = %gateway,
                        message_id = %data.message_id,
                        chat_id = %data.chat_id,
                        sender_id = %data.sender.id,
                        "Message received from gateway"
                    );

                    // Get handler and process message
                    let handler = {
                        let inner = self.inner.read().await;
                        inner.handler.clone()
                    };

                    if let Some(handler) = handler {
                        if let Some(response) = handler
                            .handle_message(&gateway, &data.routing, &data.content)
                            .await
                        {
                            // Send response back through gateway
                            if let Err(e) = self
                                .send_message(
                                    &gateway,
                                    &data.chat_id,
                                    &response,
                                    Some(data.message_id.clone()),
                                )
                                .await
                            {
                                error!(
                                    gateway = %gateway,
                                    chat_id = %data.chat_id,
                                    error = %e,
                                    "Failed to send response"
                                );
                            }
                        }
                    } else {
                        warn!(gateway = %gateway, "No message handler registered");
                    }
                }

                GatewayEvent::CommandOk {
                    request_id,
                    message_id,
                } => {
                    debug!(
                        gateway = %gateway,
                        request_id = %request_id,
                        message_id = ?message_id,
                        "Command completed"
                    );
                }

                GatewayEvent::CommandError {
                    request_id,
                    code,
                    message,
                } => {
                    error!(
                        gateway = %gateway,
                        request_id = %request_id,
                        code = %code,
                        message = %message,
                        "Command failed"
                    );
                }

                GatewayEvent::Error {
                    code,
                    message,
                    fatal,
                } => {
                    if fatal {
                        error!(
                            gateway = %gateway,
                            code = %code,
                            message = %message,
                            "Fatal gateway error"
                        );
                        self.unregister(&gateway).await;
                    } else {
                        warn!(
                            gateway = %gateway,
                            code = %code,
                            message = %message,
                            "Gateway error"
                        );
                    }
                }

                GatewayEvent::Shutdown { reason } => {
                    info!(gateway = %gateway, reason = %reason, "Gateway shutdown");
                    self.unregister(&gateway).await;
                    break;
                }

                GatewayEvent::AuthRequired { method } => {
                    info!(
                        gateway = %gateway,
                        method = ?method,
                        "Gateway requires authentication"
                    );
                    // TODO: Expose auth state via API for UI to display QR code
                }

                GatewayEvent::AuthSuccess => {
                    info!(gateway = %gateway, "Gateway authentication successful");
                }

                GatewayEvent::Pong {
                    request_id,
                    uptime_seconds,
                    connected,
                } => {
                    debug!(
                        gateway = %gateway,
                        request_id = %request_id,
                        uptime_seconds = %uptime_seconds,
                        connected = %connected,
                        "Gateway pong"
                    );
                }

                GatewayEvent::CallbackQuery(data) => {
                    debug!(
                        gateway = %gateway,
                        callback_query_id = %data.callback_query_id,
                        chat_id = %data.chat_id,
                        data = %data.data,
                        "Callback query received"
                    );

                    // Get handler and process callback
                    let handler = {
                        let inner = self.inner.read().await;
                        inner.handler.clone()
                    };

                    if let Some(handler) = handler {
                        let response = handler.handle_callback_query(&gateway, &data).await;

                        // Answer the callback query with toast notification
                        if let Err(e) = self
                            .answer_callback_query(&gateway, &data.callback_query_id, response)
                            .await
                        {
                            warn!(
                                gateway = %gateway,
                                callback_query_id = %data.callback_query_id,
                                error = %e,
                                "Failed to answer callback query"
                            );
                        }
                    }
                }
            }
        }

        debug!(gateway = %gateway, "Gateway event handler stopped");
    }
}

impl Default for GatewayManager {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Message Handler
// ============================================================================

/// Handler for incoming gateway messages.
///
/// Implement this trait to handle messages from gateways.
/// The Gateway Manager calls this when a message is received.
#[async_trait::async_trait]
pub trait MessageHandler: Send + Sync {
    /// Handle an incoming message from a gateway.
    ///
    /// Returns the response content to send back, or None if no response.
    async fn handle_message(
        &self,
        gateway: &str,
        routing: &RoutingContext,
        content: &MessageContent,
    ) -> Option<String>;

    /// Handle a callback query from an inline keyboard button press.
    ///
    /// Used for approval flow when user presses Allow/Deny buttons.
    /// Returns an optional notification text to show the user.
    async fn handle_callback_query(
        &self,
        _gateway: &str,
        _data: &agnx_gateway_protocol::CallbackQueryData,
    ) -> Option<String> {
        // Default implementation does nothing
        None
    }
}

// ============================================================================
// Gateway Handle
// ============================================================================

/// Handle for communicating with a gateway.
///
/// This abstracts over built-in gateways (direct channels) and external
/// gateways (JSON over stdio subprocess).
pub struct GatewayHandle {
    /// Gateway name (e.g., "telegram", "whatsapp").
    pub name: String,

    /// Channel to send commands to the gateway.
    pub command_tx: mpsc::Sender<GatewayCommand>,

    /// Capabilities reported by the gateway.
    pub capabilities: Vec<String>,
}

impl GatewayHandle {
    /// Send a command to the gateway.
    pub async fn send(&self, command: GatewayCommand) -> Result<(), SendError> {
        self.command_tx
            .send(command)
            .await
            .map_err(|_| SendError::ChannelClosed)
    }

    /// Check if the gateway supports a capability.
    pub fn has_capability(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }
}

/// Error sending a command to a gateway.
#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("gateway channel closed")]
    ChannelClosed,
}

/// Build an inline keyboard for approval prompts.
///
/// Creates a single row with Allow Once, Allow Always, and Deny buttons.
/// The callback data format is just `approve:{decision}` — the session is
/// looked up from the chat_id when the callback is received.
pub fn build_approval_keyboard() -> InlineKeyboard {
    InlineKeyboard::single_row(vec![
        InlineButton::new("Allow Once", "approve:allow_once"),
        InlineButton::new("Allow Always", "approve:allow_always"),
        InlineButton::new("Deny", "approve:deny"),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_register_and_list() {
        let manager = GatewayManager::new();

        let (_cmd_tx, _evt_tx) = manager
            .register("telegram", vec!["media".to_string()])
            .await;

        let gateways = manager.list().await;
        assert_eq!(gateways.len(), 1);
        assert!(gateways.contains(&"telegram".to_string()));
    }

    #[tokio::test]
    async fn test_unregister() {
        let manager = GatewayManager::new();

        let (_cmd_tx, _evt_tx) = manager.register("telegram", vec![]).await;
        assert_eq!(manager.list().await.len(), 1);

        manager.unregister("telegram").await;
        assert_eq!(manager.list().await.len(), 0);
    }

    #[tokio::test]
    async fn test_get_gateway() {
        let manager = GatewayManager::new();

        let (_cmd_tx, _evt_tx) = manager
            .register("telegram", vec!["media".to_string(), "edit".to_string()])
            .await;

        let handle = manager.get("telegram").await.unwrap();
        assert_eq!(handle.name, "telegram");
        assert!(handle.has_capability("media"));
        assert!(handle.has_capability("edit"));
        assert!(!handle.has_capability("delete"));
    }
}
