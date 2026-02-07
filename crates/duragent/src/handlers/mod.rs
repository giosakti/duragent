//! HTTP request handlers.

mod admin;
mod health;
pub(crate) mod problem_details;
pub mod v1;
mod version;

pub use admin::shutdown;
pub use health::{livez, readyz};
pub use version::version;
