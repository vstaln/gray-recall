//! gray-recall: search past gray sessions for prior work.

use std::path::PathBuf;

pub mod catchup;
pub mod cli;
pub mod extract;
pub mod index;
pub mod project;
pub mod query;
pub mod redact;
pub mod render;
pub mod session;
#[cfg(test)]
pub mod testkit;
pub mod text;

/// Manifest name: the host forwards `gray recall ...` to this binary.
pub const PLUGIN_NAME: &str = "recall";
/// Wire protocol version.
pub const PROTOCOL: &str = "1.1";
/// Slash commands claimed in the TUI.
pub const COMMANDS: &[&str] = &["/recall"];

/// `~/.gray`, overridable with `GRAY_HOME` (same resolution as gray-memory).
pub fn gray_home() -> anyhow::Result<PathBuf> {
    std::env::var_os("GRAY_HOME")
        .filter(|v| !v.to_string_lossy().trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".gray"))
        })
        .ok_or_else(|| anyhow::anyhow!("cannot resolve home: set GRAY_HOME or HOME"))
}
