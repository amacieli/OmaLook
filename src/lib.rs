//! OmaLook: Rust backend for Outlook-style QML client
//! 
//! Modules:
//! - auth: Device Flow OAuth2 (Microsoft public client)
//! - google_auth: Gmail sign-in (browser + loopback redirect + PKCE)
//! - providers: Microsoft Graph mail/calendar/contacts providers
//! - db: SQLite cache with FTS5 for mail
//! - settings: TOML configuration with file watching
//! - keyring: SecretService integration for token storage

pub mod auth;
pub mod google_auth;
pub mod db;
pub mod settings;
pub mod keyring_mgr;
pub mod token_store;
pub mod accounts;
pub mod scheduler;
pub mod account_ops;
pub mod models;
pub mod errors;
pub mod perf;
pub mod sync_state;
pub mod qt_bridge;
pub mod email_daemon;
pub mod compose;
pub mod list_headers;
pub mod identities;
pub mod bounce;
pub mod addresses;
pub mod fs_browse;
pub mod outbox;
pub mod draft_sync;
pub mod calendar_daemon;
pub mod tasks_daemon;
pub mod calendar_write;
pub mod calendar_finder;
pub mod calendar_outbox;
pub mod join_link;
pub mod invites;
pub mod render_cache;
pub mod attachments;
pub mod local_rules;
pub mod gmail_filters;
pub mod calendar_tags;
pub mod attach_spool;
pub mod contacts_daemon;
pub mod contacts_api;
pub mod contacts_write;
pub mod list_sync;
pub mod contacts_io;
pub mod contacts_outbox;
pub mod sms_daemon;
pub mod sms_names;
pub mod providers;
pub mod search;
pub mod rules;
pub mod view_state;
pub mod background;
pub mod mcp;

pub use auth::AuthManager;
pub use db::Database;
pub use settings::SettingsManager;

use log::warn;

/// Initialize logging for the application
pub fn init_logging() {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "omalook=debug,info");
    }
    env_logger::init();
}
