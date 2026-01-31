//! Gateway system for platform integrations (Telegram, WhatsApp, etc.).
//!
//! Gateways enable Agnx to communicate with messaging platforms. The system supports:
//!
//! - **Built-in gateways**: Compiled into Agnx, communicate via Rust channels
//! - **External gateways**: Subprocess plugins, communicate via JSON over stdio
//!
//! Both types implement the same Gateway Protocol, allowing uniform handling.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                         Agnx Core                                │
//! │                                                                  │
//! │  ┌─────────────────────────────────────────────────────────────┐ │
//! │  │                    Gateway Manager                          │ │
//! │  │   Routes messages between sessions and gateways             │ │
//! │  └──────────────────────────┬──────────────────────────────────┘ │
//! │                             │                                    │
//! │    ┌────────────────────────┴────────────────────────────────┐   │
//! │    │ Built-in Gateways (feature flags)                       │   │
//! │    │  Communication: Rust mpsc channels                      │   │
//! │    └────────────────────────┬────────────────────────────────┘   │
//! │                             │                                    │
//! └─────────────────────────────┼────────────────────────────────────┘
//!                               │ JSON Lines over stdio
//!                     ┌─────────┴─────────┐
//!                     │ External Gateways │
//!                     │ (subprocess)      │
//!                     └───────────────────┘
//! ```
//!
//! # Protocol
//!
//! The Gateway Protocol defines two message types:
//!
//! - [`GatewayCommand`]: Messages from Agnx to gateway (send message, typing, etc.)
//! - [`GatewayEvent`]: Messages from gateway to Agnx (message received, errors, etc.)
//!
//! For external gateways, these are serialized as JSON Lines (newline-delimited JSON).

pub mod handler;
pub mod manager;
pub mod subprocess;

// Re-export protocol types from the protocol crate
pub use agnx_gateway_protocol::{
    AuthMethod, GatewayCommand, GatewayEvent, MediaPayload, MessageContent, MessageReceivedData,
    RoutingContext, Sender, capabilities, error_codes,
};

pub use handler::{GatewayHandlerConfig, GatewayMessageHandler, RoutingConfig};
pub use manager::{
    GatewayHandle, GatewayManager, MessageHandler, SendError, build_approval_keyboard,
};
pub use subprocess::SubprocessGateway;

// Re-export Telegram gateway from the telegram crate
#[cfg(feature = "gateway-telegram")]
pub use agnx_gateway_telegram::{TelegramConfig, TelegramGateway};
