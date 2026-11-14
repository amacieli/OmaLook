//! SQLite database with FTS5 for local mail cache

use crate::errors::{OmarchyError, Result};
use crate::models::{Account, CachedMessage, CalendarEvent, Contact, ContactAddress, ContactFolder, ContactPhone, ContactRow, EmailMessage, MailFolder, Message};
use crate::token_store::DEFAULT_ACCOUNT;
use log::{debug, info, warn};
use rusqlite::{Connection, params, OptionalExtension};
use chrono::Utc;
use crate::calendar_write::CalEventRef;

pub mod sms;
pub mod lists;
pub mod contacts;
pub mod list_link;
pub mod tags;
pub mod rules;
pub mod local_rules;
pub mod gmail_filters;
pub mod tasks;
pub mod drafts;
pub mod mail_extras;

// Split by area (R3.3). Public paths are unchanged via the re-exports below.
pub mod schema;
pub mod accounts;
pub mod messages;
pub mod outbox;
pub mod cal_outbox;
pub mod actions;
pub mod categories;
pub mod reconcile;
pub mod folders;
pub mod contact_store;
pub mod calendar;

pub use accounts::*;
pub use messages::*;
pub use categories::*;

/// Escape `\`, `%` and `_` so the value matches literally inside a LIKE pattern. Every LIKE that
/// carries user or provider data must bind a pattern built with this and say `ESCAPE '\'`
/// (omalook-secure-by-default rule 6). Constant patterns need neither.
pub fn like_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '%' | '_') {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

/// `%value%` with the value escaped, for `LIKE ? ESCAPE '\'`.
pub fn like_contains(s: &str) -> String {
    format!("%{}%", like_escape(s))
}

#[cfg(test)]
mod like_escape_tests {
    use super::{like_contains, like_escape};

    #[test]
    fn escapes_backslash_percent_and_underscore() {
        assert_eq!(like_escape("a_b%c\\d"), "a\\_b\\%c\\\\d");
        assert_eq!(like_escape("plain text"), "plain text");
        assert_eq!(like_escape(""), "");
    }

    #[test]
    fn contains_pattern_wraps_the_escaped_value() {
        assert_eq!(like_contains("100%"), "%100\\%%");
        assert_eq!(like_contains("a_b"), "%a\\_b%");
    }
}

/// Every table with an `account_id` column; `delete_account` clears them all in one transaction.
/// Guarded by `account_tables_cover_every_account_id_table` (walks `sqlite_master`): a new table
/// with `account_id` that is not listed here fails that test.
/// Not listed on purpose: `accounts` (keyed by `id`, deleted last), `sms_*` (device-scoped, not
/// account-scoped; see db/sms.rs), `messages_fts` (cleared with `messages`), and the tables in
/// `MESSAGE_KEYED_TABLES`.
pub const ACCOUNT_TABLES: &[&str] = &[
    "messages", "folders", "mail_delta", "mail_history",
    "category_defs", "account_flags", "category_state",
    "outbox", "drafts",
    "calendar_events", "calendar_outbox",
    "contacts", "contact_folders", "contact_sync", "contact_outbox",
    "identities", "identity_pref",
    "mail_rules", "rules_state", "rule_ops", "local_rules_state",
    "task_lists", "tasks",
    "dl_sync_targets", "dl_sync_base", "dl_sync_log",
];

/// Tables keyed by a message id instead of an account id: (table, column). Deleted through
/// `messages` before the messages go. Guarded by the same test (any table with a `message_id`
/// column must be here).
pub const MESSAGE_KEYED_TABLES: &[(&str, &str)] = &[
    ("attachment_cache", "message_id"),
    ("invite_info", "message_id"),
    ("list_info", "message_id"),
    ("local_rule_seen", "message_id"),
];

/// Names of the cache files of one account (see `account_ops::remove_account`).
#[derive(Debug, Default, Clone)]
pub struct AccountCacheKeys {
    pub message_ids: Vec<String>,
    pub draft_ids: Vec<String>,
    pub outbox_ids: Vec<String>,
    pub contact_ids: Vec<String>,
}

pub struct Database {
    /// Shared with the other handles on this thread for the same file (see `schema::shared_connection`).
    /// `Rc`, so a `Database` is not `Send`: open it on the thread that uses it.
    conn: std::rc::Rc<Connection>,
    /// Account that rows written through this handle belong to.
    account_id: String,
}

impl Database {
    pub fn account_id(&self) -> &str {
        &self.account_id
    }
}

#[cfg(test)]
#[path = "db/tests_calendar.rs"]
mod calendar_tests;
#[cfg(test)]
#[path = "db/tests_recurrence.rs"]
mod recurrence_tests;
#[cfg(test)]
#[path = "db/tests_account.rs"]
mod account_tests;
#[cfg(test)]
#[path = "db/tests_real_db_copy.rs"]
mod real_db_copy;
#[cfg(test)]
#[path = "db/tests_contact.rs"]
mod contact_tests;
#[cfg(test)]
#[path = "db/tests_transaction.rs"]
mod transaction_tests;
#[cfg(test)]
#[path = "db/tests_multi_account_data.rs"]
mod multi_account_data_tests;
#[cfg(test)]
#[path = "db/tests_batch_insert.rs"]
mod batch_insert_tests;
#[cfg(test)]
#[path = "db/tests_outbox.rs"]
mod outbox_tests;

#[cfg(test)]
#[path = "db/account_delete_tests.rs"]
mod account_delete_tests;
#[cfg(test)]
#[path = "db/tests_event_version.rs"]
mod event_version_tests;

/// `col IN (?n, ...)` over `accounts`, pushing the values onto `args`. The MCP read queries bind
/// their filters this way so placeholder numbers stay in step with the parameter list (R3.5).
pub(crate) fn account_in(col: &str, accounts: &[String], args: &mut Vec<rusqlite::types::Value>) -> String {
    let marks: Vec<String> = accounts.iter().map(|a| { args.push(a.clone().into()); format!("?{}", args.len()) }).collect();
    format!("{} IN ({})", col, marks.join(","))
}
