//! CLI command implementations.

use std::path::Path;

use anyhow::{Result, bail};

use duragent::config::DEFAULT_WORKSPACE;

pub mod attach;
pub mod chat;
pub mod init;
mod interactive;
pub mod login;
pub mod serve;

/// Check that a duragent workspace exists.
///
/// If neither the config file nor the default workspace directory (`.duragent/`)
/// exists, returns an error suggesting `duragent init`.
pub fn check_workspace(config_path: &str) -> Result<()> {
    if Path::new(config_path).exists() || Path::new(DEFAULT_WORKSPACE).exists() {
        return Ok(());
    }
    bail!(
        "No duragent workspace found (missing '{}' and '{}/' directory).\n\
         Run `duragent init` to set one up.",
        config_path,
        DEFAULT_WORKSPACE,
    )
}
