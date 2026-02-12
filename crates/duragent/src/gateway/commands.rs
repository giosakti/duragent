//! Slash command handling for gateway messages.

use crate::api::SessionStatus;

use super::handler::GatewayMessageHandler;

// ============================================================================
// Command Handlers
// ============================================================================

impl GatewayMessageHandler {
    /// Dispatch a slash command from a gateway chat.
    ///
    /// Returns Some(response) if the command was handled, None if unknown
    /// (unknown commands fall through to regular message processing).
    pub(super) async fn handle_command(
        &self,
        command: &str,
        gateway: &str,
        chat_id: &str,
    ) -> Option<String> {
        match command {
            "reset" => Some(self.handle_reset_command(gateway, chat_id).await),
            "status" => Some(self.handle_status_command(gateway, chat_id).await),
            _ => None,
        }
    }

    async fn handle_reset_command(&self, gateway: &str, chat_id: &str) -> String {
        let Some(handle) = self.find_session_for_chat(gateway, chat_id).await else {
            return "No active session to reset.".to_string();
        };
        let _ = handle.set_status(SessionStatus::Completed).await;
        self.chat_session_cache
            .remove_by_session_id(handle.id())
            .await;
        self.services.session_registry.remove(handle.id());
        "Session reset. Send a message to start a new conversation.".to_string()
    }

    async fn handle_status_command(&self, gateway: &str, chat_id: &str) -> String {
        let Some(handle) = self.find_session_for_chat(gateway, chat_id).await else {
            return "No active session.".to_string();
        };
        let Ok(metadata) = handle.get_metadata().await else {
            return "Failed to get session status.".to_string();
        };
        format!(
            "Session: {}\nAgent: {}\nStatus: {:?}\nCreated: {}",
            metadata.id,
            metadata.agent,
            metadata.status,
            metadata.created_at.format("%Y-%m-%d %H:%M UTC"),
        )
    }
}
