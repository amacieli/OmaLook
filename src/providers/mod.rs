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

    /// Send a composed message. `from` is the account's own address (providers that sign the
    /// From line themselves may ignore it). Err(transient) failures are retried by the outbox
    /// (see `compose::is_transient`); everything else is reported to the user.
    async fn send_message(&self, _msg: &crate::compose::OutgoingMessage, _from: &str) -> Result<()> {
        Err(crate::errors::OmarchyError::HttpError("send_message not supported".into()))
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

pub mod graph;
pub use graph::GraphEmailProvider;

pub mod calendar;
pub use calendar::{CalendarProvider, GraphCalendarProvider};

pub mod contacts;
pub use contacts::{ContactsProvider, GraphContactsProvider};

pub mod google_api;
pub mod gmail;
pub use gmail::GmailProvider;
pub mod google_calendar;
pub use google_calendar::GoogleCalendarProvider;
pub mod google_contacts;
pub use google_contacts::GoogleContactsProvider;
