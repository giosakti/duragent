//! HTTP request handlers.

mod admin;
pub(crate) mod api_auth;
mod health;
pub(crate) mod problem_details;
pub mod v1;
mod version;

pub use admin::{reload_agents, shutdown};
pub use health::{livez, readyz};
pub use version::version;
