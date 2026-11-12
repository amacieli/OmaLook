//! Email provider trait for pluggable auth backends
//!
//! Implementations: Graph API, Gmail API (IMAP future)

use crate::errors::Result;
use crate::models::{DeltaEnd, DeltaPage, EmailMessage, MailFolder};
use async_trait::async_trait;

/// Trait for email provider implementations (Graph, IMAP, etc.)
#[async_trait]
pub trait EmailProvider: Send + Sync {
    /// Fetch emails from inbox (convenience — calls fetch_folder_messages("inbox", limit))
    async fn fetch_inbox(&self, limit: usize) -> Result<Vec<EmailMessage>>;

    /// Fetch emails from a specific folder by ID or well-known name
    async fn fetch_folder_messages(&self, folder_id: &str, limit: usize) -> Result<Vec<EmailMessage>>;

    /// Fetch ALL messages in a folder with pagination, streaming batches to a channel.
    /// Default implementation calls fetch_folder_messages repeatedly (non-paginated fallback).
    async fn fetch_all_folder_messages(
        &self,
        folder_id: &str,
        tx: tokio::sync::mpsc::Sender<Vec<EmailMessage>>,
    ) -> Result<usize> {
        // Default: one page of 50 (providers that support full pagination override this)
        let emails = self.fetch_folder_messages(folder_id, 50).await?;
        let count = emails.len();
        if !emails.is_empty() {
            let _ = tx.send(emails).await;
        }
        Ok(count)
    }

    /// Search message bodies at the provider: ids (in the form the local cache stores them) of
    /// messages in `folder_id` (None = whole mailbox) whose full text matches `text`, at most
    /// `limit`; the flag says there were more. Used by `fullbody:`.
    async fn search_message_ids(&self, _folder_id: Option<&str>, _text: &str, _limit: usize) -> Result<(Vec<String>, bool)> {
        Err(crate::errors::OmarchyError::HttpError("provider search not supported".into()))
    }

    /// Ids of every unread message in a folder. Err if the list could not be read in full
    /// (callers must not treat a partial list as authoritative).
    async fn fetch_unread_ids(&self, _folder_id: &str) -> Result<Vec<String>> {
        Err(crate::errors::OmarchyError::HttpError("fetch_unread_ids not supported".into()))
    }

    /// Ids (scoped, as stored locally) of EVERY message in a folder. Err if the list could not
    /// be read in full: callers delete local rows absent from it, so a partial list must fail.
    async fn fetch_all_ids(&self, _folder_id: &str) -> Result<Vec<String>> {
        Err(crate::errors::OmarchyError::HttpError("fetch_all_ids not supported".into()))
    }

    /// Set a message's read flag on the provider.
    async fn set_message_read(&self, _id: &str, _is_read: bool) -> Result<()> {
        Err(crate::errors::OmarchyError::HttpError("set_message_read not supported".into()))
    }

    /// Carry out an archive / trash / delete / move on the provider. `from_folder` is the
    /// (scoped) id of the folder the message is in. Err(transient) is retried later (see
    /// `compose::is_transient`); anything else makes the daemon put the message back.
    async fn apply_message_action(
        &self,
        _id: &str,
        _from_folder: &str,
        _action: &crate::models::MessageAction,
    ) -> Result<()> {
        Err(crate::errors::OmarchyError::HttpError("message actions not supported".into()))
    }

    /// Categories (Exchange) / user labels (Gmail) of the account with their colours.
    /// Err when the account may not read them (Exchange before it re-signs-in with the
    /// MailboxSettings permission).
    async fn fetch_categories(&self) -> Result<Vec<crate::models::CategoryDef>> {
        Err(crate::errors::OmarchyError::HttpError("fetch_categories not supported".into()))
    }

    /// Create a category / label with a palette colour on the provider.
    async fn create_category(&self, _name: &str, _color: &crate::models::PaletteColor) -> Result<crate::models::CategoryDef> {
        Err(crate::errors::OmarchyError::HttpError("create_category not supported".into()))
    }

    /// The account's server-side mailbox rules as the provider's raw JSON objects (Graph `messageRule`).
    /// Read-only in OL-040 phase 2. Err("... not supported") for providers without server rules here.
    async fn fetch_message_rules(&self) -> Result<Vec<serde_json::Value>> {
        Err(crate::errors::OmarchyError::HttpError("fetch_message_rules not supported".into()))
    }

    /// Create a mailbox rule (`body` is a Graph `messageRule`); returns what the server stored.
    async fn create_message_rule(&self, _body: &serde_json::Value) -> Result<serde_json::Value> {
        Err(crate::errors::OmarchyError::HttpError("create_message_rule not supported".into()))
    }

    /// PATCH one mailbox rule (`sequence`, `isEnabled`, ...); returns what the server stored.
    async fn update_message_rule(&self, _id: &str, _patch: &serde_json::Value) -> Result<serde_json::Value> {
        Err(crate::errors::OmarchyError::HttpError("update_message_rule not supported".into()))
    }

    async fn delete_message_rule(&self, _id: &str) -> Result<()> {
        Err(crate::errors::OmarchyError::HttpError("delete_message_rule not supported".into()))
    }

    /// The account's server-side mail filters as the provider's raw JSON (Gmail `users.settings.filters`).
    /// OL-040 phase 5. Err for providers without filters; Err mentioning "sign in" when the scope is missing.
    async fn fetch_mail_filters(&self) -> Result<Vec<serde_json::Value>> {
        Err(crate::errors::OmarchyError::HttpError("fetch_mail_filters not supported".into()))
    }

    /// Create a filter (`body` = `criteria` + `action`); returns what the server stored.
    async fn create_mail_filter(&self, _body: &serde_json::Value) -> Result<serde_json::Value> {
        Err(crate::errors::OmarchyError::HttpError("create_mail_filter not supported".into()))
    }

    /// Delete a filter by id. Already gone counts as done. (Gmail filters cannot be edited.)
    async fn delete_mail_filter(&self, _id: &str) -> Result<()> {
        Err(crate::errors::OmarchyError::HttpError("delete_mail_filter not supported".into()))
    }

    /// User labels of the account, id -> name (Gmail). Empty for providers without label ids.
    async fn label_names(&self) -> Result<std::collections::HashMap<String, String>> {
        Ok(Default::default())
    }

    /// Id of the user label named `name` (case-insensitive), None when there is no such label.
    async fn label_id_for_tag(&self, _name: &str) -> Result<Option<String>> {
        Ok(None)
    }

    /// Change a category / label's colour on the provider. Implementations re-read it afterwards and
    /// fail (not transiently) when the provider silently kept the old colour.
    async fn update_category_color(&self, _name: &str, _color: &crate::models::PaletteColor) -> Result<crate::models::CategoryDef> {
        Err(crate::errors::OmarchyError::HttpError("update_category_color not supported".into()))
    }

    /// Delete a category / label from the provider's list. Already gone counts as done. The caller has
    /// stripped it from the account's messages first.
    async fn delete_category(&self, _name: &str) -> Result<()> {
        Err(crate::errors::OmarchyError::HttpError("delete_category not supported".into()))
    }

    /// Make a message's categories `wanted` (`had` is what the provider last reported; Gmail
    /// adds and removes the difference, Exchange just sets the list).
    async fn set_message_categories(&self, _id: &str, _wanted: &[String], _had: &[String]) -> Result<()> {
        Err(crate::errors::OmarchyError::HttpError("set_message_categories not supported".into()))
    }

    /// True when the provider has no change feed to repopulate message details with, so the
    /// daemon fetches them message by message (`fetch_message_meta`).
    fn repopulates_meta_per_message(&self) -> bool {
        false
    }

    /// Details of these (scoped) message ids; ones that fail are left out and retried later.
    async fn fetch_message_meta(&self, _ids: &[String]) -> Vec<EmailMessage> {
        Vec::new()
    }

    /// Full body of one (scoped) message: (content type \"html\"|\"text\", content). The daemon
    /// uses this to fill the cache ahead of the user; Err leaves the message for a later round.
    async fn fetch_body(&self, _id: &str) -> Result<(String, String)> {
        Err(crate::errors::OmarchyError::HttpError("fetch_body not supported".into()))
    }

    /// Send a composed message. `from` is the account's own address (providers that sign the
    /// From line themselves may ignore it). Err(transient) failures are retried by the outbox
    /// (see `compose::is_transient`); everything else is reported to the user.
    async fn send_message(&self, _msg: &crate::compose::OutgoingMessage, _from: &str) -> Result<()> {
        Err(crate::errors::OmarchyError::HttpError("send_message not supported".into()))
    }

    /// Save a draft to the provider's Drafts folder (docs/PLAN-compose.md Phase B2). `remote_id`
    /// None creates the draft (threaded for a reply or forward); Some replaces its content. The
    /// attachment set is re-sent only when `attachments_changed` (Graph) or always (Gmail, whose
    /// draft is one MIME message). Errors follow the usual rules: network / 5xx / 429 are retried
    /// by the draft worker, other failures are kept as the draft's error.
    async fn save_draft(
        &self,
        _msg: &crate::compose::OutgoingMessage,
        _remote_id: Option<&str>,
        _from: &str,
        _attachments_changed: bool,
    ) -> Result<RemoteDraft> {
        Err(crate::errors::OmarchyError::HttpError("save_draft not supported".into()))
    }

    /// The provider's current version marker of a draft (Graph `changeKey`, Gmail message id).
    /// Ok(None) when the draft no longer exists.
    async fn draft_version(&self, _remote_id: &str) -> Result<Option<String>> {
        Err(crate::errors::OmarchyError::HttpError("draft_version not supported".into()))
    }

    /// Delete a draft from the provider. Already gone counts as done.
    async fn delete_draft(&self, _remote_id: &str) -> Result<()> {
        Err(crate::errors::OmarchyError::HttpError("delete_draft not supported".into()))
    }

    /// Send an existing draft: its content is first brought up to `msg`, then it is sent, so
    /// neither Drafts nor Sent gets a second copy.
    async fn send_draft(&self, _msg: &crate::compose::OutgoingMessage, _remote_id: &str, _from: &str) -> Result<()> {
        Err(crate::errors::OmarchyError::HttpError("send_draft not supported".into()))
    }

    /// True when the provider can report changes since a stored link (Graph delta), so the
    /// daemon syncs by change feed instead of re-reading whole folders.
    fn supports_delta(&self) -> bool {
        false
    }

    /// Walk a folder's change feed. `link` None = full enumeration (newest first); Some = resume
    /// from a stored delta/next link. Each page goes to `tx`. Returns the link to store, or
    /// `DeltaEnd::Reset` when the provider rejects `link`.
    async fn fetch_delta(
        &self,
        _folder_id: &str,
        _link: Option<String>,
        _tx: tokio::sync::mpsc::Sender<DeltaPage>,
    ) -> Result<DeltaEnd> {
        Err(crate::errors::OmarchyError::HttpError("delta not supported".into()))
    }

    /// Fetch all top-level mail folders
    async fn fetch_folders(&self) -> Result<Vec<MailFolder>>;

    /// Check if the token is still valid
    async fn is_token_valid(&self) -> Result<bool>;
}

/// HTTP client with bounded waits: a connection that is up but has no route must fail in
/// seconds (the cache then serves), not hang for minutes.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

pub mod graph;
pub use graph::GraphEmailProvider;

/// A draft as the provider stores it after a save.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteDraft {
    pub id: String,
    /// Graph `changeKey` or Gmail message id: compared before the next save to spot edits made elsewhere.
    pub version: Option<String>,
}

pub mod tasks;
pub use tasks::{GraphTasksProvider, TasksProvider};

pub mod calendar;
pub use calendar::{CalendarProvider, GraphCalendarProvider};

pub mod contacts;
pub use contacts::{ContactsProvider, GraphContactsProvider};

pub mod google_api;
pub mod gmail;
pub use gmail::GmailProvider;
pub mod google_calendar;
pub use google_calendar::GoogleCalendarProvider;
pub mod google_calendar_write;
pub mod google_contacts;
pub use google_contacts::GoogleContactsProvider;
