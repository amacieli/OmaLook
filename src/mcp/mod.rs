//! MCP (Model Context Protocol) server for OmaLook.
//!
//! The daemon owns the policy and the tools; `omalook mcp` ([`shim`]) is only a stdio forwarder
//! for clients that spawn a command. Design: `notes/DESIGN-mcp-server.md`.
//!
//! - [`policy`]: master switch → domain level → tool mode, fail-closed.
//! - [`read`]: the read tools (cache only).
//! - [`registry`]: the tool catalogue and handlers.
//! - [`rpc`]: JSON-RPC 2.0 / MCP method handling (transport-free).
//! - [`server`]: the authenticated localhost listener (separate port from the UI's).
//! - [`token`]: the bearer token file.

pub mod policy;
pub mod read;
pub mod registry;
pub mod rpc;
pub mod server;
pub mod shim;
pub mod token;

use std::path::{Path, PathBuf};

/// Default port of the MCP listener (the UI's HTTP server is 27182). `OMALOOK_MCP_PORT` overrides.
pub const DEFAULT_MCP_PORT: u16 = 27183;

pub fn mcp_port() -> u16 {
    std::env::var("OMALOOK_MCP_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_MCP_PORT)
}

/// What a request handler needs. Holds no cached policy: it is re-read from settings.toml for
/// every request, so a change in Settings applies to the next call.
#[derive(Clone)]
pub struct Ctx {
    pub config_dir: PathBuf,
}

impl Ctx {
    pub fn new(config_dir: &Path) -> Ctx { Ctx { config_dir: config_dir.to_path_buf() } }

    pub fn settings_path(&self) -> PathBuf { self.config_dir.join("settings.toml") }

    /// The current effective policy. A settings file that cannot be read gives a locked policy.
    pub fn policy(&self) -> policy::Policy {
        let path = self.settings_path();
        match crate::settings::read_mcp_settings(&path) {
            Some(s) => policy::Policy::new(s, crate::settings::read_run_in_background(&path)),
            None => policy::Policy::locked(),
        }
    }
}
