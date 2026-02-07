//! Notification delivery for tool execution events.

use serde::Serialize;
use tracing::{debug, error, info, warn};

use crate::agent::{Delivery, NotifyConfig};

/// Send a notification about command execution to all configured deliveries.
pub async fn send_notification(
    config: &NotifyConfig,
    session_id: &str,
    agent: &str,
    command: &str,
    success: bool,
) {
    if !config.enabled {
        return;
    }

    // If no deliveries configured, default to log
    if config.deliveries.is_empty() {
        send_log(session_id, agent, command, success);
        return;
    }

    // Send to all configured deliveries
    for delivery in &config.deliveries {
        match delivery {
            Delivery::Log => {
                send_log(session_id, agent, command, success);
            }
            Delivery::Webhook { url } => {
                send_webhook(url, session_id, agent, command, success).await;
            }
        }
    }
}

// ============================================================================
// Private Helpers
// ============================================================================

/// Send a log notification.
fn send_log(session_id: &str, agent: &str, command: &str, success: bool) {
    if success {
        info!(
            session_id = %session_id,
            agent = %agent,
            command = %command,
            "Command executed successfully"
        );
    } else {
        warn!(
            session_id = %session_id,
            agent = %agent,
            command = %command,
            "Command execution failed"
        );
    }
}

/// Payload for webhook notifications.
#[derive(Serialize)]
struct WebhookPayload<'a> {
    event: &'static str,
    session_id: &'a str,
    agent: &'a str,
    command: &'a str,
    success: bool,
}

/// Send a webhook notification (fire and forget).
async fn send_webhook(url: &str, session_id: &str, agent: &str, command: &str, success: bool) {
    let payload = WebhookPayload {
        event: "command_notification",
        session_id,
        agent,
        command,
        success,
    };

    let client = reqwest::Client::new();
    match client.post(url).json(&payload).send().await {
        Ok(response) => {
            if response.status().is_success() {
                debug!(url = %url, "Webhook notification sent");
            } else {
                warn!(
                    url = %url,
                    status = %response.status(),
                    "Webhook notification failed"
                );
            }
        }
        Err(e) => {
            error!(url = %url, error = %e, "Failed to send webhook notification");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn notification_respects_enabled_flag() {
        let config = NotifyConfig {
            enabled: false,
            ..Default::default()
        };

        // Should not panic or error when disabled
        send_notification(&config, "session_1", "test-agent", "echo hello", true).await;
    }

    #[tokio::test]
    async fn log_notification_doesnt_panic() {
        let config = NotifyConfig {
            enabled: true,
            deliveries: vec![Delivery::Log],
            ..Default::default()
        };

        send_notification(&config, "session_1", "test-agent", "echo hello", true).await;
        send_notification(&config, "session_1", "test-agent", "rm -rf /", false).await;
    }

    #[tokio::test]
    async fn empty_deliveries_defaults_to_log() {
        let config = NotifyConfig {
            enabled: true,
            deliveries: vec![],
            ..Default::default()
        };

        // Should default to log and not panic
        send_notification(&config, "session_1", "test-agent", "echo hello", true).await;
    }

    #[tokio::test]
    async fn multiple_deliveries_all_called() {
        let config = NotifyConfig {
            enabled: true,
            deliveries: vec![
                Delivery::Log,
                // Webhook with invalid URL will fail but shouldn't panic
                Delivery::Webhook {
                    url: "http://localhost:99999/invalid".to_string(),
                },
            ],
            ..Default::default()
        };

        // Should send to both (log succeeds, webhook fails gracefully)
        send_notification(&config, "session_1", "test-agent", "echo hello", true).await;
    }
}
