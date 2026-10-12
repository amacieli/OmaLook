//! SQLite database with FTS5 for local mail cache

use crate::errors::{OmarchyError, Result};
use crate::accounts;
use crate::models::{Account, CachedMessage, CalendarEvent, Contact, ContactAddress, ContactFolder, ContactPhone, ContactRow, EmailMessage, MailFolder, Message};
use crate::token_store::DEFAULT_ACCOUNT;
use log::{debug, info, warn};
use rusqlite::{Connection, params, OptionalExtension};
use chrono::Utc;

/// Folder kinds that get an "all accounts" row when two or more accounts have one.
/// Only names that mean the same thing across providers (Graph and Gmail well-known names).
pub const ALL_ACCOUNTS_KINDS: &[&str] = &["inbox", "drafts", "sentitems", "deleteditems", "junkemail", "archive"];

/// Display label of an all-accounts row, e.g. "Inbox - all accounts".
pub fn all_accounts_label(well_known: &str) -> &'static str {
    match well_known {
        "inbox" => "Inbox - all accounts",
        "drafts" => "Drafts - all accounts",
        "sentitems" => "Sent - all accounts",
        "deleteditems" => "Deleted - all accounts",
        "junkemail" => "Junk - all accounts",
        "archive" => "Archive - all accounts",
        _ => "All accounts",
    }
}

/// One all-accounts folder row: summed counts over every account's folder of that kind.
#[derive(Debug, Clone, PartialEq)]
pub struct AllAccountsFolder {
    pub well_known_name: String,
    pub accounts: i64,
    pub unread_item_count: i64,
    pub total_item_count: i64,
}

/// Kinds that exist in at least two accounts, in ALL_ACCOUNTS_KINDS order, with summed counts.
/// A kind held by only one account gets no row: it would just repeat that account's folder.
pub fn all_accounts_folders(conn: &Connection) -> rusqlite::Result<Vec<AllAccountsFolder>> {
    let mut stmt = conn.prepare(
        "SELECT well_known_name, COUNT(DISTINCT account_id),
                COALESCE(SUM(unread_item_count), 0), COALESCE(SUM(total_item_count), 0)
         FROM folders
         WHERE well_known_name IN ('inbox','drafts','sentitems','deleteditems','junkemail','archive')
         GROUP BY well_known_name
         HAVING COUNT(DISTINCT account_id) > 1",
    )?;
    let mut rows: Vec<AllAccountsFolder> = stmt.query_map([], |r| Ok(AllAccountsFolder {
        well_known_name: r.get(0)?,
        accounts: r.get(1)?,
        unread_item_count: r.get(2)?,
        total_item_count: r.get(3)?,
    }))?.collect::<rusqlite::Result<_>>()?;
    rows.sort_by_key(|f| ALL_ACCOUNTS_KINDS.iter().position(|k| *k == f.well_known_name));
    Ok(rows)
}

/// How one sender's mail should be shown. Keyed by lower-cased address.
#[derive(Debug, Clone, PartialEq)]
pub struct SenderPref {
    pub email: String,
    /// Always render this sender's mail as HTML.
    pub always_html: bool,
    /// Always load remote images in this sender's mail.
    pub always_images: bool,
}

/// Result of `add_sender`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SenderAdd {
    Added,
    AlreadyListed,
    /// Not shaped like an address (no single `@`, spaces, empty parts).
    Invalid,
}

/// `["A","B"]` as stored in `messages.categories`; `None` stays NULL (not fetched).
pub fn categories_json(c: &Option<Vec<String>>) -> Option<String> {
    c.as_ref().map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".into()))
}

/// Inverse of `categories_json` (NULL or garbage -> empty).
pub fn categories_from_json(s: Option<&str>) -> Vec<String> {
    s.and_then(|t| serde_json::from_str::<Vec<String>>(t).ok()).unwrap_or_default()
}

pub struct Database {
    conn: Connection,
    /// Account that rows written through this handle belong to.
    account_id: String,
}

impl Database {
    /// Open or create database at the given path
    pub fn open(path: &str) -> Result<Self> {
        Self::open_for_account(path, DEFAULT_ACCOUNT)
    }

    /// Open the database; rows written through this handle are stamped with `account_id`.
    /// `open()` uses the legacy single account (`exchange-primary`) until the
    /// per-account sync scheduler lands.
    pub fn open_for_account(path: &str, account_id: &str) -> Result<Self> {
        debug!("Opening database at: {} (account {})", path, account_id);
        
        let conn = Connection::open(path)
            .map_err(|e| OmarchyError::DatabaseError(e))?;
        
        // WAL: background sync writers no longer block the HTTP server's readers (and the
        // UI stays responsive during a crawl); NORMAL is the recommended pairing with WAL.
        // busy_timeout covers the brief moments two writers collide.
        conn.busy_timeout(std::time::Duration::from_secs(10)).map_err(OmarchyError::DatabaseError)?;
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.pragma_update(None, "synchronous", "NORMAL");

        let db = Self { conn, account_id: account_id.to_string() };
        db.init_schema()?;
        // Serves "newest N in folder" without scanning and sorting the whole table.
        db.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_messages_folder_received ON messages(folder_id, received_at DESC)", [])?;
        // Change-feed position per account + folder (Graph delta). `complete` = 0 while the
        // first full enumeration is still running (the link then resumes it next launch).
        db.conn.execute(
            "CREATE TABLE IF NOT EXISTS mail_delta (
                account_id TEXT NOT NULL,
                folder_id  TEXT NOT NULL,
                link       TEXT NOT NULL,
                complete   INTEGER NOT NULL DEFAULT 0,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (account_id, folder_id)
            )", [])?;
        info!("Database initialized");
        
        Ok(db)
    }

    pub fn account_id(&self) -> &str {
        &self.account_id
    }
    
    /// Initialize schema with FTS5 for full-text search
    fn init_schema(&self) -> Result<()> {
        debug!("Initializing database schema");
        
        // Messages table
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                subject TEXT NOT NULL,
                from_email TEXT NOT NULL,
                from_name TEXT,
                body TEXT NOT NULL,
                received_at DATETIME NOT NULL,
                is_read BOOLEAN DEFAULT 0,
                cached_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;
        
        // FTS5 virtual table for full-text search
        self.conn.execute(
            "CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                id UNINDEXED,
                subject,
                from_email UNINDEXED,
                from_name,
                body,
                tokenize = 'porter'
            )",
            [],
        )?;
        
        // Triggers to keep FTS in sync
        self.conn.execute(
            "CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
                INSERT INTO messages_fts(id, subject, from_email, from_name, body)
                VALUES (new.id, new.subject, new.from_email, new.from_name, new.body);
            END",
            [],
        )?;
        
        self.create_messages_au_trigger()?;
        
        self.conn.execute(
            "CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
                DELETE FROM messages_fts WHERE id = old.id;
            END",
            [],
        )?;

        // Databases created before this fix carry triggers that use the FTS5 'delete'
        // command, which is only valid for external-content tables; on this ordinary
        // table every UPDATE or DELETE on `messages` failed with "SQL logic error".
        let stale_triggers: i32 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'trigger' AND name IN ('messages_au', 'messages_ad')
               AND sql LIKE '%''delete''%'",
            [],
            |row| row.get(0),
        )?;
        if stale_triggers > 0 {
            self.conn.execute("DROP TRIGGER IF EXISTS messages_au", [])?;
            self.conn.execute("DROP TRIGGER IF EXISTS messages_ad", [])?;
            self.create_messages_au_trigger()?;
            self.conn.execute(
                "CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
                    DELETE FROM messages_fts WHERE id = old.id;
                END",
                [],
            )?;
        }

        // Folders table
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS folders (
                id TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                parent_folder_id TEXT,
                unread_item_count INTEGER DEFAULT 0,
                total_item_count INTEGER DEFAULT 0,
                well_known_name TEXT,
                sort_order INTEGER DEFAULT 999,
                cached_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;

        // folder_id column on messages (added via ALTER TABLE for existing DBs)
        let has_folder_col: bool = self.conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name='folder_id'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .unwrap_or(0) > 0;
        if !has_folder_col {
            self.conn.execute(
                "ALTER TABLE messages ADD COLUMN folder_id TEXT",
                [],
            )?;
        }

        // read_pending: a local read/unread change not yet pushed to the provider.
        let has_pending_col: bool = self.conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name='read_pending'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .unwrap_or(0) > 0;
        if !has_pending_col {
            self.conn.execute(
                "ALTER TABLE messages ADD COLUMN read_pending INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }

        // Message details beyond the list basics (To/Cc/Bcc, sender name, categories, sent time,
        // importance, attachments, conversation). `meta_ok` = 0 marks rows stored before these
        // existed: the one-time repopulate fills them and sets it to 1.
        // categories: JSON array of names. categories_synced: what the provider last said.
        // cat_pending: a local category change not yet pushed.
        let had_meta = self.conn
            .query_row("SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name='meta_ok'", [], |r| r.get::<_, i32>(0))
            .unwrap_or(0) > 0;
        for (col, decl) in [
            ("to_text", "TEXT"), ("cc_text", "TEXT"), ("bcc_text", "TEXT"),
            ("categories", "TEXT"), ("categories_synced", "TEXT"), ("cat_pending", "INTEGER NOT NULL DEFAULT 0"),
            ("sent_at", "TEXT"), ("importance", "TEXT"), ("has_attachments", "INTEGER"),
            ("conversation_id", "TEXT"), ("meta_ok", "INTEGER NOT NULL DEFAULT 0"),
        ] {
            let has: bool = self.conn
                .query_row("SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name = ?1", [col], |row| row.get::<_, i32>(0))
                .unwrap_or(0) > 0;
            if !has {
                self.conn.execute(&format!("ALTER TABLE messages ADD COLUMN {} {}", col, decl), [])?;
            }
        }
        if !had_meta {
            // The update trigger used to re-index from_name too; filling in names for every
            // row would re-index (a full scan each) tens of thousands of times.
            self.conn.execute("DROP TRIGGER IF EXISTS messages_au", [])?;
            self.create_messages_au_trigger()?;
        }
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_messages_meta ON messages(meta_ok)", [])?;
        // Attachment lists (JSON of `attachments::Attachment`), fetched when a message is opened.
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS attachment_cache (
                message_id TEXT PRIMARY KEY,
                json TEXT NOT NULL,
                has_real INTEGER NOT NULL DEFAULT 0,
                fetched_at INTEGER NOT NULL
            )", [])?;
        // Categories (Exchange) / user labels (Gmail) per account, with their colours.
        // `pending` = created here, not yet on the provider.
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS category_defs (
                account_id TEXT NOT NULL,
                name TEXT NOT NULL,
                color TEXT NOT NULL,
                provider_color TEXT,
                pending INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (account_id, name)
            )", [])?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS account_flags (
                account_id TEXT NOT NULL,
                key TEXT NOT NULL,
                PRIMARY KEY (account_id, key)
            )", [])?;
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS category_state (
                account_id TEXT PRIMARY KEY,
                master_list INTEGER NOT NULL DEFAULT 1
            )", [])?;

        // action_pending / action_due: archive, delete or move chosen in the UI. The row is hidden
        // from lists at once; the daemon pushes it to the provider once `action_due` (unix secs,
        // end of the undo window) has passed.
        for (col, decl) in [("action_pending", "TEXT"), ("action_due", "INTEGER NOT NULL DEFAULT 0")] {
            let has: bool = self.conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name = ?1",
                    [col],
                    |row| row.get::<_, i32>(0),
                )
                .unwrap_or(0) > 0;
            if !has {
                self.conn.execute(&format!("ALTER TABLE messages ADD COLUMN {} {}", col, decl), [])?;
            }
        }

        // Full bodies are fetched on demand when a message is opened and kept here;
        // `body` stays the short preview the daemon syncs.
        for (col, decl) in [("body_full", "TEXT"), ("body_type", "TEXT")] {
            let has: bool = self.conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name = ?1",
                    [col],
                    |row| row.get::<_, i32>(0),
                )
                .unwrap_or(0) > 0;
            if !has {
                self.conn.execute(&format!("ALTER TABLE messages ADD COLUMN {} {}", col, decl), [])?;
            }
        }

        // Per-sender display preferences (managed in Settings → Senders).
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS sender_prefs (
                email TEXT PRIMARY KEY,
                always_html INTEGER NOT NULL DEFAULT 0,
                always_images INTEGER NOT NULL DEFAULT 0,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;

        // Outgoing mail: queued when the user presses Send, sent by the account's outbox worker
        // once `send_at` (unix ms) passes. state: pending | sending | sent | failed | cancelled.
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS outbox (
                id TEXT PRIMARY KEY,
                account_id TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'pending',
                attempts INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                send_at INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_outbox_due ON outbox(account_id, state, send_at)",
            [],
        )?;

        // Calendar events (separate from mail; created if not present)
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS calendar_events (
                id TEXT PRIMARY KEY,
                subject TEXT NOT NULL,
                body TEXT NOT NULL DEFAULT '',
                start_at TEXT NOT NULL,
                end_at TEXT NOT NULL,
                is_all_day BOOLEAN DEFAULT 0,
                time_zone TEXT,
                cached_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_calendar_events_start ON calendar_events(start_at)",
            [],
        )?;
        for (col, decl) in [("event_type", "TEXT NOT NULL DEFAULT 'singleInstance'"), ("series_master_id", "TEXT")] {
            let has: bool = self.conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('calendar_events') WHERE name = ?1",
                params![col],
                |row| row.get::<_, i32>(0),
            ).unwrap_or(0) > 0;
            if !has {
                self.conn.execute(&format!("ALTER TABLE calendar_events ADD COLUMN {} {}", col, decl), [])?;
            }
        }

        self.init_contacts_schema()?;
        self.init_accounts_schema()?;

        Ok(())
    }
    
    /// Contacts + contact folders. List-valued fields (emails, phones, addresses) are stored
    /// as JSON arrays: they are only ever read back whole, never queried per element.
    fn init_contacts_schema(&self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS invite_info (
                message_id TEXT PRIMARY KEY,
                info TEXT NOT NULL,
                checked_at INTEGER NOT NULL
            )",
            [],
        )?;
        // Replaced by invite_info (the old table held only a meeting type, never refreshed).
        self.conn.execute("DROP TABLE IF EXISTS invite_state", [])?;
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS contact_folders (
                id TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                parent_folder_id TEXT,
                cached_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS contacts (
                id TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                given_name TEXT NOT NULL DEFAULT '',
                surname TEXT NOT NULL DEFAULT '',
                company TEXT NOT NULL DEFAULT '',
                job_title TEXT NOT NULL DEFAULT '',
                emails TEXT NOT NULL DEFAULT '[]',
                phones TEXT NOT NULL DEFAULT '[]',
                addresses TEXT NOT NULL DEFAULT '[]',
                folder_id TEXT,
                created_at TEXT NOT NULL DEFAULT '',
                modified_at TEXT NOT NULL DEFAULT '',
                is_favorite INTEGER NOT NULL DEFAULT 0,
                cached_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;
        self.conn.execute("CREATE INDEX IF NOT EXISTS idx_contacts_folder ON contacts(folder_id)", [])?;
        Ok(())
    }

    /// FTS sync trigger for message updates (unchanged definition; factored out so the
    /// account backfill can suspend it inside its transaction).
    fn create_messages_au_trigger(&self) -> Result<()> {
        self.conn.execute(
            // Only the indexed columns: read flags, folder and account changes must not
            // touch the index (it has no usable key, so each re-index is a full scan).
            "CREATE TRIGGER IF NOT EXISTS messages_au
             AFTER UPDATE OF subject, from_email, body ON messages BEGIN
                DELETE FROM messages_fts WHERE id = old.id;
                INSERT INTO messages_fts(id, subject, from_email, from_name, body)
                VALUES (new.id, new.subject, new.from_email, new.from_name, new.body);
            END",
            [],
        )?;
        Ok(())
    }

    /// Accounts table + `account_id` on every synced table. Rows that predate
    /// multi-account support are assigned to the legacy account.
    fn init_accounts_schema(&self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS accounts (
                id TEXT PRIMARY KEY,
                provider TEXT NOT NULL,
                email TEXT,
                display_name TEXT,
                config TEXT NOT NULL DEFAULT '{}',
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(provider, email)
            )",
            [],
        )?;

        let has_enabled: bool = self.conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('accounts') WHERE name='enabled'",
            [],
            |row| row.get::<_, i32>(0),
        ).unwrap_or(0) > 0;
        if !has_enabled {
            self.conn.execute("ALTER TABLE accounts ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1", [])?;
        }

        let mut backfill_needed = false;
        for table in ["messages", "folders", "calendar_events", "contacts", "contact_folders"] {
            let has_col: bool = self.conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name='account_id'",
                params![table],
                |row| row.get::<_, i32>(0),
            ).unwrap_or(0) > 0;
            if !has_col {
                self.conn.execute(&format!("ALTER TABLE {} ADD COLUMN account_id TEXT", table), [])?;
            }
            self.conn.execute(
                &format!("CREATE INDEX IF NOT EXISTS idx_{}_account ON {}(account_id)", table, table),
                [],
            )?;
            if self.conn.query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM {} WHERE account_id IS NULL)", table),
                [],
                |row| row.get::<_, bool>(0),
            )? {
                backfill_needed = true;
            }
        }

        if backfill_needed {
            // One transaction: either every table is assigned or none is.
            self.conn.execute_batch("BEGIN")?;
            let result = (|| -> Result<()> {
                self.ensure_account_row(DEFAULT_ACCOUNT)?;
                // account_id is not indexed text, so the FTS update trigger would only churn
                // the full-text index for every row; suspend it for this transaction.
                // (DDL is transactional: a rollback restores the trigger.)
                self.conn.execute("DROP TRIGGER IF EXISTS messages_au", [])?;
                for table in ["messages", "folders", "calendar_events", "contacts", "contact_folders"] {
                    let n = self.conn.execute(
                        &format!("UPDATE {} SET account_id = ?1 WHERE account_id IS NULL", table),
                        params![DEFAULT_ACCOUNT],
                    )?;
                    info!("Assigned {} existing {} rows to account {}", n, table, DEFAULT_ACCOUNT);
                }
                self.create_messages_au_trigger()?;
                Ok(())
            })();
            match result {
                Ok(()) => self.conn.execute_batch("COMMIT")?,
                Err(e) => {
                    let _ = self.conn.execute_batch("ROLLBACK");
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// Insert a bare account row if missing (provider derived from the id prefix).
    fn ensure_account_row(&self, account_id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO accounts (id, provider) VALUES (?1, ?2)",
            params![account_id, accounts::provider_of(account_id)],
        )?;
        Ok(())
    }

    /// Register the pre-multi-account account (call only when its token exists).
    pub fn ensure_legacy_account(&self) -> Result<()> {
        self.ensure_account_row(DEFAULT_ACCOUNT)
    }

    /// Insert an account under a caller-chosen id (the id is generated before login
    /// because tokens are stored under it). Fails on a duplicate id or mailbox.
    pub fn insert_account(&self, id: &str, provider: &str, email: Option<&str>, display_name: Option<&str>, config: &str) -> Result<Account> {
        self.conn.execute(
            "INSERT INTO accounts (id, provider, email, display_name, config) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, provider, email, display_name, config],
        )?;
        Ok(Account {
            id: id.to_string(), provider: provider.to_string(),
            email: email.map(|s| s.to_string()), display_name: display_name.map(|s| s.to_string()),
            config: config.to_string(),
            enabled: true,
        })
    }

    /// Create an account with a freshly generated `<provider>-<suffix>` id.
    /// Fails if the same mailbox (provider + email) is already added.
    pub fn create_account(&self, provider: &str, email: Option<&str>, display_name: Option<&str>, config: &str) -> Result<Account> {
        let provider = accounts::provider_slug(provider);
        if let Some(email) = email {
            let dup: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE provider = ?1 AND lower(email) = lower(?2))",
                params![provider, email],
                |row| row.get(0),
            )?;
            if dup {
                return Err(OmarchyError::SettingsError(format!("{} account {} is already added", provider, email)));
            }
        }
        for _ in 0..16 {
            let id = accounts::new_account_id(&provider);
            let inserted = self.conn.execute(
                "INSERT OR IGNORE INTO accounts (id, provider, email, display_name, config) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, provider, email, display_name, config],
            )?;
            if inserted == 1 {
                return Ok(Account {
                    id, provider,
                    email: email.map(|s| s.to_string()),
                    display_name: display_name.map(|s| s.to_string()),
                    config: config.to_string(),
                    enabled: true,
                });
            }
        }
        Err(OmarchyError::SettingsError("could not generate a unique account id".into()))
    }

    pub fn list_accounts(&self) -> Result<Vec<Account>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, provider, email, display_name, config, enabled FROM accounts ORDER BY created_at, id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Account {
                    id: row.get(0)?, provider: row.get(1)?, email: row.get(2)?,
                    display_name: row.get(3)?, config: row.get(4)?, enabled: row.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Log out (false) / log in (true): toggles syncing without touching tokens or data.
    pub fn set_account_enabled(&self, account_id: &str, enabled: bool) -> Result<()> {
        self.conn.execute("UPDATE accounts SET enabled = ?1 WHERE id = ?2", params![enabled, account_id])?;
        Ok(())
    }

    pub fn set_account_email(&self, account_id: &str, email: &str) -> Result<()> {
        self.conn.execute("UPDATE accounts SET email = ?1 WHERE id = ?2", params![email, account_id])?;
        Ok(())
    }

    /// Remove an account and everything synced for it (mail, folders, events).
    /// Tokens live in the keyring and are removed by the token broker.
    pub fn delete_account(&self, account_id: &str) -> Result<()> {
        self.conn.execute_batch("BEGIN")?;
        let result = (|| -> Result<()> {
            for table in ["messages", "folders", "calendar_events", "contacts", "contact_folders", "mail_delta"] {
                self.conn.execute(&format!("DELETE FROM {} WHERE account_id = ?1", table), params![account_id])?;
            }
            self.conn.execute("DELETE FROM accounts WHERE id = ?1", params![account_id])?;
            Ok(())
        })();
        match result {
            Ok(()) => { self.conn.execute_batch("COMMIT")?; Ok(()) }
            Err(e) => { let _ = self.conn.execute_batch("ROLLBACK"); Err(e) }
        }
    }

    /// Cache a message from Graph API
    pub fn cache_message(&self, msg: &Message) -> Result<()> {
        let from_email = msg.from.as_ref()
            .and_then(|r| r.email_address.as_ref())
            .map(|ea| ea.address.clone())
            .unwrap_or_default();
        
        let from_name = msg.from.as_ref()
            .and_then(|r| r.email_address.as_ref())
            .and_then(|ea| ea.name.clone());
        
        let body = msg.body.as_ref()
            .map(|b| b.content.clone())
            .unwrap_or_default();
        
        let received_at = msg.received_date_time.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.to_utc())
            .unwrap_or_else(Utc::now);
        
        // debug!("Caching message: {}", msg.id);
        
        self.conn.execute(
            "INSERT OR REPLACE INTO messages (id, subject, from_email, from_name, body, received_at, is_read, account_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                msg.id,
                msg.subject,
                from_email,
                from_name,
                body,
                received_at,
                msg.is_read.unwrap_or(false),
                self.account_id,
            ],
        )?;
        
        Ok(())
    }
    
    /// Check if email already exists in database (for deduplication)
    pub fn email_exists(&self, id: &str) -> Result<bool> {
        let mut stmt = self.conn.prepare(
            "SELECT 1 FROM messages WHERE id = ?1 LIMIT 1"
        )?;
        
        let exists = stmt.exists(params![id])?;
        
        if exists {
            // debug!("Email {} exists in database", id);
        }
        
        Ok(exists)
    }
    
    /// Get cached message by ID
    pub fn get_message(&self, id: &str) -> Result<Option<CachedMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, subject, from_email, from_name, body, received_at, is_read, cached_at
             FROM messages WHERE id = ?1"
        )?;
        
        let msg = stmt.query_row(params![id], |row| {
            Ok(CachedMessage {
                id: row.get(0)?,
                subject: row.get(1)?,
                from_email: row.get(2)?,
                from_name: row.get(3)?,
                body: row.get(4)?,
                received_at: row.get(5)?,
                is_read: row.get(6)?,
                cached_at: row.get(7)?,
            })
        }).optional()?;
        
        Ok(msg)
    }
    
    /// Get recent cached messages
    pub fn get_recent_messages(&self, limit: usize) -> Result<Vec<CachedMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, subject, from_email, from_name, body, received_at, is_read, cached_at
             FROM messages ORDER BY received_at DESC LIMIT ?1"
        )?;
        
        let messages = stmt.query_map(params![limit as i32], |row| {
            Ok(CachedMessage {
                id: row.get(0)?,
                subject: row.get(1)?,
                from_email: row.get(2)?,
                from_name: row.get(3)?,
                body: row.get(4)?,
                received_at: row.get(5)?,
                is_read: row.get(6)?,
                cached_at: row.get(7)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
        
        Ok(messages)
    }
    
    /// Mark message as read
    pub fn mark_read(&self, id: &str, is_read: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET is_read = ?1 WHERE id = ?2",
            params![is_read, id],
        )?;
        Ok(())
    }
    
    /// Local read/unread change from the UI. Flags the row so the daemon pushes it to the
    /// provider, and keeps the folder's unread count in step until the next folder sync.
    /// Returns whether the state actually changed.
    pub fn set_message_read(&self, id: &str, is_read: bool) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE messages SET is_read = ?1, read_pending = 1 WHERE id = ?2 AND is_read != ?1",
            params![is_read, id],
        )?;
        if changed > 0 {
            let delta: i32 = if is_read { -1 } else { 1 };
            self.conn.execute(
                "UPDATE folders SET unread_item_count = MAX(0, COALESCE(unread_item_count, 0) + ?1)
                 WHERE id = (SELECT folder_id FROM messages WHERE id = ?2)",
                params![delta, id],
            )?;
        }
        Ok(changed > 0)
    }

    /// The cached full body of a message as (content type "html"|"text", content).
    pub fn cached_body(&self, id: &str) -> Result<Option<(String, String)>> {
        let row = self.conn.query_row(
            "SELECT body_type, body_full FROM messages WHERE id = ?1",
            params![id],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
        );
        match row {
            Ok((Some(t), Some(c))) => Ok(Some((t, c))),
            Ok(_) => Ok(None),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn store_body(&self, id: &str, content_type: &str, content: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET body_type = ?1, body_full = ?2 WHERE id = ?3",
            params![content_type, content, id],
        )?;
        Ok(())
    }

    /// Normalise an address for the sender table: trimmed, lower-cased. None if it is not
    /// shaped like `local@domain` (one `@`, no whitespace, both sides non-empty, a dot in
    /// the domain).
    pub fn normalize_sender(email: &str) -> Option<String> {
        let e = email.trim().to_lowercase();
        let (local, domain) = e.split_once('@')?;
        let ok = !local.is_empty()
            && !domain.is_empty()
            && !domain.contains('@')
            && domain.contains('.')
            && !domain.starts_with('.')
            && !domain.ends_with('.')
            && !e.chars().any(|c| c.is_whitespace());
        ok.then_some(e)
    }

    /// Add a sender with both preferences off.
    pub fn add_sender(&self, email: &str) -> Result<SenderAdd> {
        let Some(email) = Self::normalize_sender(email) else { return Ok(SenderAdd::Invalid) };
        let n = self.conn.execute("INSERT OR IGNORE INTO sender_prefs (email) VALUES (?1)", params![email])?;
        Ok(if n > 0 { SenderAdd::Added } else { SenderAdd::AlreadyListed })
    }

    /// All listed senders, alphabetical.
    pub fn list_senders(&self) -> Result<Vec<SenderPref>> {
        let mut stmt = self.conn.prepare(
            "SELECT email, always_html, always_images FROM sender_prefs ORDER BY email ASC",
        )?;
        let rows = stmt
            .query_map([], |r| Ok(SenderPref { email: r.get(0)?, always_html: r.get(1)?, always_images: r.get(2)? }))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Set one preference (`"html"` or `"images"`) for a listed sender. Returns whether a
    /// listed sender was updated (false: unknown address or unknown field).
    pub fn set_sender_pref(&self, email: &str, field: &str, value: bool) -> Result<bool> {
        let column = match field {
            "html" => "always_html",
            "images" => "always_images",
            _ => return Ok(false),
        };
        let Some(email) = Self::normalize_sender(email) else { return Ok(false) };
        let n = self.conn.execute(
            &format!("UPDATE sender_prefs SET {} = ?1 WHERE email = ?2", column),
            params![value, email],
        )?;
        Ok(n > 0)
    }

    pub fn remove_sender(&self, email: &str) -> Result<bool> {
        let Some(email) = Self::normalize_sender(email) else { return Ok(false) };
        Ok(self.conn.execute("DELETE FROM sender_prefs WHERE email = ?1", params![email])? > 0)
    }

    /// The short preview the daemon synced (what the list row is built from).
    pub fn message_preview(&self, id: &str) -> Option<String> {
        self.conn
            .query_row("SELECT body FROM messages WHERE id = ?1", params![id], |r| r.get::<_, String>(0))
            .ok()
    }

    /// Cached invitation lookup of a message: (info JSON, unix seconds it was fetched), if any.
    pub fn invite_info(&self, message_id: &str) -> Result<Option<(String, i64)>> {
        Ok(self.conn.query_row(
            "SELECT info, checked_at FROM invite_info WHERE message_id = :id",
            rusqlite::named_params! { ":id": message_id },
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        ).optional()?)
    }

    /// Store the invitation lookup of a message (upsert, keyed by message id).
    pub fn set_invite_info(&self, message_id: &str, info_json: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO invite_info (message_id, info, checked_at) VALUES (:id, :i, :now)
             ON CONFLICT(message_id) DO UPDATE SET info = excluded.info, checked_at = excluded.checked_at",
            rusqlite::named_params! {
                ":id": message_id,
                ":i": info_json,
                ":now": Utc::now().timestamp(),
            },
        )?;
        Ok(())
    }

    /// The cached attachment list of a message, if it has been fetched.
    pub fn cached_attachments(&self, id: &str) -> Result<Option<Vec<crate::attachments::Attachment>>> {
        match self.conn.query_row("SELECT json FROM attachment_cache WHERE message_id = ?1", params![id], |r| r.get::<_, String>(0)) {
            Ok(j) => Ok(serde_json::from_str(&j).ok()),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Remember a message's attachment list. `has_real` (any non-inline attachment) overrides the
    /// list view's coarse server hint, which counts signature logos; it lives here, not in
    /// `messages`, so a later sync cannot overwrite the correction.
    pub fn store_attachments(&self, id: &str, list: &[crate::attachments::Attachment]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO attachment_cache (message_id, json, has_real, fetched_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(message_id) DO UPDATE SET json = excluded.json, has_real = excluded.has_real, fetched_at = excluded.fetched_at",
            params![id, serde_json::to_string(list).unwrap_or_else(|_| "[]".into()), crate::attachments::any_real(list) as i32, Utc::now().timestamp()],
        )?;
        Ok(())
    }

    /// The account a cached message belongs to.
    pub fn message_account(&self, id: &str) -> Result<Option<String>> {
        match self.conn.query_row(
            "SELECT account_id FROM messages WHERE id = ?1",
            params![id],
            |r| r.get::<_, Option<String>>(0),
        ) {
            Ok(a) => Ok(a),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // ── outbox ───────────────────────────────────────────────────────────────────────────────

    pub fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    /// Queue a message to go out at `send_at_ms`.
    pub fn outbox_enqueue(&self, id: &str, account_id: &str, payload_json: &str, send_at_ms: i64) -> Result<()> {
        let now = Self::now_ms();
        self.conn.execute(
            "INSERT INTO outbox (id, account_id, payload_json, state, attempts, send_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'pending', 0, ?4, ?5, ?5)",
            params![id, account_id, payload_json, send_at_ms, now],
        )?;
        Ok(())
    }

    /// Take every due message of `account_id` (pending, `send_at` passed) and mark it `sending`.
    /// The state change is the claim: a message cancelled in the meantime is not returned, and
    /// two workers can never both get the same one.
    pub fn outbox_claim_due(&self, account_id: &str, now_ms: i64) -> Result<Vec<(String, String, i64)>> {
        let due: Vec<(String, String, i64)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, payload_json, attempts FROM outbox
                 WHERE account_id = ?1 AND state = 'pending' AND send_at <= ?2 ORDER BY send_at, created_at",
            )?;
            let rows = stmt
                .query_map(params![account_id, now_ms], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        let mut claimed = Vec::new();
        for (id, payload, attempts) in due {
            let n = self.conn.execute(
                "UPDATE outbox SET state = 'sending', attempts = attempts + 1, updated_at = ?2
                 WHERE id = ?1 AND state = 'pending'",
                params![id, now_ms],
            )?;
            if n == 1 {
                claimed.push((id, payload, attempts + 1));
            }
        }
        Ok(claimed)
    }

    /// When the earliest still-pending message of the account is due (unix ms).
    pub fn outbox_next_due(&self, account_id: &str) -> Result<Option<i64>> {
        match self.conn.query_row(
            "SELECT MIN(send_at) FROM outbox WHERE account_id = ?1 AND state = 'pending'",
            params![account_id],
            |r| r.get::<_, Option<i64>>(0),
        ) {
            Ok(v) => Ok(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn outbox_mark_sent(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE outbox SET state = 'sent', last_error = NULL, updated_at = ?2 WHERE id = ?1",
            params![id, Self::now_ms()],
        )?;
        Ok(())
    }

    /// A transient failure: back to `pending`, due again at `retry_at_ms`.
    pub fn outbox_mark_retry(&self, id: &str, error: &str, retry_at_ms: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE outbox SET state = 'pending', last_error = ?2, send_at = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, error, retry_at_ms, Self::now_ms()],
        )?;
        Ok(())
    }

    pub fn outbox_mark_failed(&self, id: &str, error: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE outbox SET state = 'failed', last_error = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, error, Self::now_ms()],
        )?;
        Ok(())
    }

    /// Take a message back. Only possible while it is still `pending`; returns whether it was.
    pub fn outbox_cancel(&self, id: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE outbox SET state = 'cancelled', updated_at = ?2 WHERE id = ?1 AND state = 'pending'",
            params![id, Self::now_ms()],
        )?;
        Ok(n == 1)
    }

    /// `(state, last_error, attempts)` of a queued message.
    pub fn outbox_status(&self, id: &str) -> Result<Option<(String, Option<String>, i64)>> {
        match self.conn.query_row(
            "SELECT state, last_error, attempts FROM outbox WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ) {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// After a crash: a message left `sending` may or may not have gone out. It is reported
    /// rather than sent again, because a second copy is worse than asking the user to look.
    pub fn outbox_fail_interrupted(&self, account_id: &str) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE outbox SET state = 'failed', updated_at = ?2,
                 last_error = 'interrupted while sending: check your Sent folder before sending again'
             WHERE account_id = ?1 AND state = 'sending'",
            params![account_id, Self::now_ms()],
        )?)
    }

    /// Housekeeping: forget finished messages older than `max_age_ms`.
    pub fn outbox_prune(&self, max_age_ms: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM outbox WHERE state IN ('sent', 'cancelled') AND updated_at < ?1",
            params![Self::now_ms() - max_age_ms],
        )?)
    }

    /// The signed-in address of an account, for the From line.
    pub fn account_email(&self, account_id: &str) -> Result<Option<String>> {
        match self.conn.query_row(
            "SELECT email FROM accounts WHERE id = ?1",
            params![account_id],
            |r| r.get::<_, Option<String>>(0),
        ) {
            Ok(v) => Ok(v.filter(|e| !e.is_empty())),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Queue `action` for each message in `ids` (those not already queued). The rows disappear
    /// from lists straight away and their folders' unread counts follow; the provider is
    /// told once `due` (unix seconds) has passed. Returns how many messages were queued.
    pub fn queue_message_actions(&self, ids: &[String], action: &crate::models::MessageAction, due: i64) -> Result<usize> {
        let db_action = action.to_db();
        let mut queued = 0;
        for id in ids {
            let changed = self.conn.execute(
                "UPDATE messages SET action_pending = ?1, action_due = ?2 WHERE id = ?3 AND action_pending IS NULL",
                params![db_action, due, id],
            )?;
            if changed > 0 {
                queued += 1;
                self.adjust_unread_for(id, -1)?;
            }
        }
        Ok(queued)
    }

    /// Take queued actions back (undo) for messages the provider has not been told about.
    pub fn cancel_message_actions(&self, ids: &[String]) -> Result<usize> {
        let mut undone = 0;
        for id in ids {
            let changed = self.conn.execute(
                "UPDATE messages SET action_pending = NULL, action_due = 0 WHERE id = ?1 AND action_pending IS NOT NULL",
                params![id],
            )?;
            if changed > 0 {
                undone += 1;
                self.adjust_unread_for(id, 1)?;
            }
        }
        Ok(undone)
    }

    /// Nudge the unread count of a message's folder by `sign` (±1) if the message is unread.
    fn adjust_unread_for(&self, id: &str, sign: i32) -> Result<()> {
        self.conn.execute(
            "UPDATE folders SET unread_item_count = MAX(0, COALESCE(unread_item_count, 0) + ?1)
             WHERE id = (SELECT folder_id FROM messages WHERE id = ?2 AND is_read = 0)",
            params![sign, id],
        )?;
        Ok(())
    }

    /// Queued actions of this account whose undo window is over: (id, folder it is in, action).
    pub fn due_actions(&self, now: i64) -> Result<Vec<(String, String, crate::models::MessageAction)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, COALESCE(folder_id, ''), action_pending FROM messages
             WHERE action_pending IS NOT NULL AND action_due <= ?1 AND account_id = ?2",
        )?;
        let rows = stmt
            .query_map(params![now, self.account_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, folder, a)| crate::models::MessageAction::from_db(&a).map(|act| (id, folder, act)))
            .collect())
    }

    /// The provider carried the action out: the local row is no longer this message's home
    /// (a moved message arrives in its new folder with the next sync).
    pub fn finish_message_action(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM messages WHERE id = ?1 AND action_pending IS NOT NULL", params![id])?;
        Ok(())
    }

    /// The provider refused the action for good: put the message back in its list.
    pub fn revert_message_action(&self, id: &str) -> Result<()> {
        self.cancel_message_actions(&[id.to_string()]).map(|_| ())
    }

    /// Read-state changes made locally that the provider has not heard about yet.
    pub fn pending_reads(&self) -> Result<Vec<(String, bool)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, is_read FROM messages WHERE read_pending = 1 AND account_id = ?1",
        )?;
        let rows = stmt
            .query_map(params![self.account_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The provider accepted `is_read` for this message. Stays pending if the user has
    /// flipped it again in the meantime.
    pub fn clear_read_pending(&self, id: &str, is_read: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET read_pending = 0 WHERE id = ?1 AND is_read = ?2",
            params![id, is_read],
        )?;
        Ok(())
    }

    // ───────────────────────── message details & categories ─────────────────────────

    /// Fill in the detail fields of an already stored message (the one-time repopulate for
    /// providers without a change feed). Never touches a category change that is still
    /// waiting to be pushed.
    pub fn update_message_meta(&self, e: &EmailMessage) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE messages SET from_name = ?1, to_text = ?2, cc_text = ?3, bcc_text = ?4,
                    categories = CASE WHEN cat_pending = 1 THEN categories ELSE ?5 END,
                    categories_synced = ?5, sent_at = ?6, importance = ?7, has_attachments = ?8,
                    conversation_id = ?9, meta_ok = 1
             WHERE id = ?10",
            params![e.from_name, e.to_text, e.cc_text, e.bcc_text, categories_json(&e.categories),
                    e.sent_at, e.importance, e.has_attachments, e.conversation_id, e.id],
        )?;
        Ok(n > 0)
    }

    /// Ids (newest first) of this account's messages still waiting for the detail repopulate.
    pub fn messages_needing_meta(&self, limit: usize) -> Result<Vec<(String, Option<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, folder_id FROM messages WHERE account_id = ?1 AND meta_ok = 0
             ORDER BY received_at DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![self.account_id, limit as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// How many of this account's messages still lack their details.
    pub fn meta_remaining(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE account_id = ?1 AND meta_ok = 0",
            params![self.account_id],
            |r| r.get(0),
        )?)
    }

    /// Forget every folder's change-feed position so the next sync walks each folder again
    /// (the stored links carry the old field list, so only a fresh walk brings the new fields).
    pub fn reset_all_delta_states(&self) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM mail_delta WHERE account_id = ?1", params![self.account_id])?)
    }

    /// One-time markers per account (e.g. "the change feeds were reset for the new fields").
    pub fn flag_is_set(&self, key: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM account_flags WHERE account_id = ?1 AND key = ?2)",
            params![self.account_id, key],
            |r| r.get::<_, bool>(0),
        )?)
    }

    pub fn set_flag(&self, key: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO account_flags (account_id, key) VALUES (?1, ?2)",
            params![self.account_id, key],
        )?;
        Ok(())
    }

    /// Folders whose change-feed walk has not finished (never started, or still paging).
    pub fn delta_walk_incomplete(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM folders f
             LEFT JOIN mail_delta d ON d.account_id = f.account_id AND d.folder_id = f.id
             WHERE f.account_id = ?1 AND (d.folder_id IS NULL OR d.complete = 0)",
            params![self.account_id],
            |r| r.get(0),
        )?)
    }

    /// Add or remove `name` on each message (local edit, pushed to the provider later).
    /// Returns how many messages changed.
    pub fn change_message_categories(&self, ids: &[String], add: bool, name: &str) -> Result<usize> {
        let mut changed = 0;
        for id in ids {
            let cur: Option<Option<String>> = self.conn
                .query_row("SELECT categories FROM messages WHERE id = ?1", params![id], |r| r.get::<_, Option<String>>(0))
                .optional()?;
            let Some(cur) = cur else { continue };
            let mut list = categories_from_json(cur.as_deref());
            let had = list.iter().any(|c| c.eq_ignore_ascii_case(name));
            if add && !had { list.push(name.to_string()); }
            else if !add && had { list.retain(|c| !c.eq_ignore_ascii_case(name)); }
            else { continue }
            self.conn.execute(
                "UPDATE messages SET categories = ?1, cat_pending = 1 WHERE id = ?2",
                params![serde_json::to_string(&list).unwrap_or_else(|_| "[]".into()), id],
            )?;
            changed += 1;
        }
        Ok(changed)
    }

    /// Category changes not yet pushed: (id, wanted names, names the provider last had).
    pub fn pending_category_pushes(&self) -> Result<Vec<(String, Vec<String>, Vec<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, categories, categories_synced FROM messages WHERE cat_pending = 1 AND account_id = ?1",
        )?;
        let rows = stmt
            .query_map(params![self.account_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .map(|(id, want, had)| (id, categories_from_json(want.as_deref()), categories_from_json(had.as_deref())))
            .collect())
    }

    /// The provider took `pushed` for this message. The change stays pending if the user has
    /// edited the categories again in the meantime.
    pub fn finish_category_push(&self, id: &str, pushed: &[String]) -> Result<()> {
        let json = serde_json::to_string(pushed).unwrap_or_else(|_| "[]".into());
        self.conn.execute(
            "UPDATE messages SET categories_synced = ?1,
                    cat_pending = CASE WHEN categories = ?1 THEN 0 ELSE 1 END
             WHERE id = ?2",
            params![json, id],
        )?;
        Ok(())
    }

    // ───────────────────────── category definitions ─────────────────────────

    /// Replace this account's provider-known categories with `defs` (those created here and
    /// not yet on the provider are kept).
    pub fn replace_category_defs(&self, defs: &[crate::models::CategoryDef]) -> Result<()> {
        self.conn.execute_batch("BEGIN")?;
        let r = (|| -> Result<()> {
            self.conn.execute("DELETE FROM category_defs WHERE account_id = ?1 AND pending = 0", params![self.account_id])?;
            for d in defs {
                self.conn.execute(
                    "INSERT INTO category_defs (account_id, name, color, provider_color, pending) VALUES (?1, ?2, ?3, ?4, 0)
                     ON CONFLICT(account_id, name) DO UPDATE SET color = ?3, provider_color = ?4, pending = 0",
                    params![self.account_id, d.name, d.color, d.provider_color],
                )?;
            }
            Ok(())
        })();
        match r {
            Ok(()) => { self.conn.execute_batch("COMMIT")?; Ok(()) }
            Err(e) => { let _ = self.conn.execute_batch("ROLLBACK"); Err(e) }
        }
    }

    /// A category made in the UI: usable at once, created on the provider by the daemon.
    pub fn add_pending_category(&self, name: &str, color_hex: &str) -> Result<bool> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO category_defs (account_id, name, color, provider_color, pending) VALUES (?1, ?2, ?3, NULL, 1)",
            params![self.account_id, name, color_hex],
        )?;
        Ok(n > 0)
    }

    pub fn pending_categories(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare("SELECT name, color FROM category_defs WHERE account_id = ?1 AND pending = 1")?;
        let rows = stmt
            .query_map(params![self.account_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn finish_pending_category(&self, name: &str, provider_color: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE category_defs SET pending = 0, provider_color = ?1 WHERE account_id = ?2 AND name = ?3",
            params![provider_color, self.account_id, name],
        )?;
        Ok(())
    }

    /// The provider refused a category change for good: show what it has again.
    pub fn revert_category_push(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET categories = categories_synced, cat_pending = 0 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// Remember whether this account's category list could be read (Exchange needs the
    /// MailboxSettings permission, which an account only has after signing in again).
    pub fn set_master_list_available(&self, available: bool) -> Result<()> {
        self.conn.execute(
            "INSERT INTO category_state (account_id, master_list) VALUES (?1, ?2)
             ON CONFLICT(account_id) DO UPDATE SET master_list = ?2",
            params![self.account_id, available as i64],
        )?;
        Ok(())
    }

    /// Make the cached read flags of one folder match the provider's list of unread ids.
    /// Rows with an unpushed local change are left alone. Returns rows changed.
    pub fn reconcile_read_state(&self, folder_id: &str, unread_ids: &[String]) -> Result<usize> {
        self.conn.execute_batch("BEGIN")?;
        let result = (|| -> Result<usize> {
            self.conn.execute_batch(
                "CREATE TEMP TABLE IF NOT EXISTS unread_ids (id TEXT PRIMARY KEY); DELETE FROM unread_ids;",
            )?;
            {
                let mut ins = self.conn.prepare("INSERT OR IGNORE INTO unread_ids (id) VALUES (?1)")?;
                for id in unread_ids {
                    ins.execute(params![id])?;
                }
            }
            let n = self.conn.execute(
                "UPDATE messages
                 SET is_read = (id NOT IN (SELECT id FROM unread_ids))
                 WHERE folder_id = ?1 AND read_pending = 0
                   AND is_read != (id NOT IN (SELECT id FROM unread_ids))",
                params![folder_id],
            )?;
            Ok(n)
        })();
        match result {
            Ok(n) => { self.conn.execute_batch("COMMIT")?; Ok(n) }
            Err(e) => { let _ = self.conn.execute_batch("ROLLBACK"); Err(e) }
        }
    }

    /// Delete old messages (retention policy)
    pub fn cleanup_old_messages(&self, days: i32) -> Result<usize> {
        let count = self.conn.execute(
            "DELETE FROM messages WHERE received_at < datetime('now', ?1)",
            params![format!("-{} days", days)],
        )?;
        
        info!("Cleaned up {} old messages", count);
        Ok(count)
    }
    
    /// Get unread count
    pub fn get_unread_count(&self) -> Result<usize> {
        let mut stmt = self.conn.prepare(
            "SELECT COUNT(*) FROM messages WHERE is_read = 0"
        )?;
        
        let count: i32 = stmt.query_row([], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Insert a new email message (used by daemon)
    pub fn insert_email(&self, email: &EmailMessage) -> Result<()> {
        let now = Utc::now();
        // debug!("Inserting email: {}", email.id);

        self.conn.execute(
            "INSERT OR IGNORE INTO messages (id, subject, from_email, body, received_at, is_read, cached_at, folder_id, account_id,
                    from_name, to_text, cc_text, bcc_text, categories, categories_synced, sent_at, importance, has_attachments, conversation_id, meta_ok)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                email.id,
                email.subject,
                email.from,
                email.body,
                email.received,
                email.is_read,
                now,
                email.folder_id,
                self.account_id,
                email.from_name, email.to_text, email.cc_text, email.bcc_text,
                categories_json(&email.categories),
                email.sent_at, email.importance, email.has_attachments, email.conversation_id, email.meta,
            ],
        )?;

        // Backfill folder_id on existing rows that were inserted before the column existed
        if email.folder_id.is_some() {
            self.conn.execute(
                "UPDATE messages SET folder_id = ?1 WHERE id = ?2 AND folder_id IS NULL",
                params![email.folder_id, email.id],
            )?;
        }

        Ok(())
    }

    /// Insert many messages in ONE transaction (one fsync instead of one per row).
    /// Returns how many were new. Rows that already exist are left alone, except that a
    /// missing `folder_id` is backfilled (same rule as `insert_email`).
    pub fn insert_emails_batch(&self, emails: &[EmailMessage]) -> Result<usize> {
        if emails.is_empty() {
            return Ok(0);
        }
        let now = Utc::now();
        self.conn.execute_batch("BEGIN")?;
        let result = (|| -> Result<usize> {
            let mut ins = self.conn.prepare_cached(
                "INSERT OR IGNORE INTO messages (id, subject, from_email, body, received_at, is_read, cached_at, folder_id, account_id,
                        from_name, to_text, cc_text, bcc_text, categories, categories_synced, sent_at, importance, has_attachments, conversation_id, meta_ok)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14, ?15, ?16, ?17, ?18, ?19)")?;
            let mut fix = self.conn.prepare_cached(
                "UPDATE messages SET folder_id = ?1 WHERE id = ?2 AND folder_id IS NULL")?;
            let mut new = 0usize;
            for e in emails {
                let n = ins.execute(params![e.id, e.subject, e.from, e.body, e.received, e.is_read, now, e.folder_id, self.account_id,
                    e.from_name, e.to_text, e.cc_text, e.bcc_text, categories_json(&e.categories),
                    e.sent_at, e.importance, e.has_attachments, e.conversation_id, e.meta])?;
                if n > 0 {
                    new += 1;
                } else if e.folder_id.is_some() {
                    fix.execute(params![e.folder_id, e.id])?;
                }
            }
            Ok(new)
        })();
        match result {
            Ok(n) => { self.conn.execute_batch("COMMIT")?; Ok(n) }
            Err(e) => { let _ = self.conn.execute_batch("ROLLBACK"); Err(e) }
        }
    }

    /// Read flag of one cached message (None if it is not cached).
    pub fn message_is_read(&self, id: &str) -> Result<Option<bool>> {
        Ok(self.conn
            .query_row("SELECT is_read FROM messages WHERE id = ?1", params![id], |r| r.get::<_, bool>(0))
            .optional()?)
    }

    // ───────────────────────────── change feed (Graph delta) ─────────────────────────────

    /// Stored change-feed position of a folder: (link, complete).
    pub fn delta_state(&self, folder_id: &str) -> Result<Option<(String, bool)>> {
        match self.conn.query_row(
            "SELECT link, complete FROM mail_delta WHERE account_id = ?1 AND folder_id = ?2",
            params![self.account_id, folder_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0)),
        ) {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn set_delta_state(&self, folder_id: &str, link: &str, complete: bool) -> Result<()> {
        self.conn.execute(
            "INSERT INTO mail_delta (account_id, folder_id, link, complete, updated_at)
             VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)
             ON CONFLICT(account_id, folder_id) DO UPDATE SET link = ?3, complete = ?4, updated_at = CURRENT_TIMESTAMP",
            params![self.account_id, folder_id, link, complete as i64],
        )?;
        Ok(())
    }

    pub fn clear_delta_state(&self, folder_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM mail_delta WHERE account_id = ?1 AND folder_id = ?2",
            params![self.account_id, folder_id],
        )?;
        Ok(())
    }

    /// Apply one change-feed page in ONE transaction: insert new messages, update the read flag
    /// and folder of existing ones (a message with an unpushed local read change keeps its local
    /// flag), delete removed ones from `folder_id`. `progress` (the page's next link) is stored
    /// in the same transaction so an interrupted first enumeration resumes where it stopped.
    /// Returns (rows inserted or changed, rows deleted).
    pub fn apply_delta_page(
        &self,
        folder_id: &str,
        page: &crate::models::DeltaPage,
        progress: Option<&str>,
    ) -> Result<(usize, usize)> {
        self.conn.execute_batch("BEGIN")?;
        let result = (|| -> Result<(usize, usize)> {
            let now = Utc::now();
            let mut changed = 0usize;
            {
                let mut up = self.conn.prepare_cached(
                    "INSERT INTO messages (id, subject, from_email, body, received_at, is_read, cached_at, folder_id, account_id,
                            from_name, to_text, cc_text, bcc_text, categories, categories_synced, sent_at, importance, has_attachments, conversation_id, meta_ok)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14, ?15, ?16, ?17, ?18, ?19)
                     ON CONFLICT(id) DO UPDATE SET
                        is_read   = CASE WHEN messages.read_pending = 1 THEN messages.is_read ELSE excluded.is_read END,
                        folder_id = excluded.folder_id,
                        -- details: when the feed carries them (meta_ok), take them; a local category change
                        -- that is not pushed yet keeps its own value
                        from_name = CASE WHEN excluded.meta_ok = 1 THEN excluded.from_name ELSE messages.from_name END,
                        to_text   = CASE WHEN excluded.meta_ok = 1 THEN excluded.to_text ELSE messages.to_text END,
                        cc_text   = CASE WHEN excluded.meta_ok = 1 THEN excluded.cc_text ELSE messages.cc_text END,
                        bcc_text  = CASE WHEN excluded.meta_ok = 1 THEN excluded.bcc_text ELSE messages.bcc_text END,
                        sent_at   = CASE WHEN excluded.meta_ok = 1 THEN excluded.sent_at ELSE messages.sent_at END,
                        importance = CASE WHEN excluded.meta_ok = 1 THEN excluded.importance ELSE messages.importance END,
                        has_attachments = CASE WHEN excluded.meta_ok = 1 THEN excluded.has_attachments ELSE messages.has_attachments END,
                        conversation_id = CASE WHEN excluded.meta_ok = 1 THEN excluded.conversation_id ELSE messages.conversation_id END,
                        categories = CASE WHEN excluded.meta_ok = 1 AND messages.cat_pending = 0 THEN excluded.categories ELSE messages.categories END,
                        categories_synced = CASE WHEN excluded.meta_ok = 1 THEN excluded.categories ELSE messages.categories_synced END,
                        meta_ok   = CASE WHEN excluded.meta_ok = 1 THEN 1 ELSE messages.meta_ok END
                     WHERE (messages.read_pending = 0 AND messages.is_read != excluded.is_read)
                        OR messages.folder_id IS NOT excluded.folder_id
                        OR (excluded.meta_ok = 1 AND messages.meta_ok = 0)
                        OR (excluded.meta_ok = 1 AND messages.categories_synced IS NOT excluded.categories)")?;
                for e in &page.upserts {
                    changed += up.execute(params![e.id, e.subject, e.from, e.body, e.received, e.is_read, now, e.folder_id, self.account_id,
                        e.from_name, e.to_text, e.cc_text, e.bcc_text, categories_json(&e.categories),
                        e.sent_at, e.importance, e.has_attachments, e.conversation_id, e.meta])?;
                }
            }
            let mut deleted = 0usize;
            {
                // Only from THIS folder: a message that was moved keeps its row via the other folder's feed.
                let mut del = self.conn.prepare_cached("DELETE FROM messages WHERE id = ?1 AND folder_id = ?2")?;
                for id in &page.removed {
                    deleted += del.execute(params![id, folder_id])?;
                }
            }
            if let Some(link) = progress {
                self.conn.execute(
                    "INSERT INTO mail_delta (account_id, folder_id, link, complete, updated_at)
                     VALUES (?1, ?2, ?3, 0, CURRENT_TIMESTAMP)
                     ON CONFLICT(account_id, folder_id) DO UPDATE SET link = ?3, complete = 0, updated_at = CURRENT_TIMESTAMP",
                    params![self.account_id, folder_id, link],
                )?;
            }
            Ok((changed, deleted))
        })();
        match result {
            Ok(v) => { self.conn.execute_batch("COMMIT")?; Ok(v) }
            Err(e) => { let _ = self.conn.execute_batch("ROLLBACK"); Err(e) }
        }
    }

    /// After a COMPLETE full enumeration of a folder: drop cached rows the provider no longer
    /// has (deleted long ago, before change tracking existed). `seen` = every id the
    /// enumeration returned; `started` = when it began, so rows stored since (e.g. by the
    /// recent-50 fetch) are never touched.
    ///
    /// Safety: the enumeration must be believable. When the provider's own item count for the
    /// folder is known, `seen` has to match it (within 2%, minimum 3: mail arrives and goes
    /// while the walk runs); that is what lets a mostly-emptied folder such as Deleted Items be
    /// cleaned. With no count to compare against, it refuses to delete more than 20% of the
    /// folder. Returns rows deleted.
    pub fn purge_unseen(
        &self,
        folder_id: &str,
        seen: &std::collections::HashSet<String>,
        started: chrono::DateTime<Utc>,
    ) -> Result<usize> {
        if seen.is_empty() {
            return Ok(0);
        }
        let candidates: Vec<String> = {
            let mut st = self.conn.prepare(
                "SELECT id FROM messages WHERE folder_id = ?1 AND account_id = ?2 AND cached_at < ?3")?;
            let rows = st.query_map(params![folder_id, self.account_id, started], |r| r.get::<_, String>(0))?;
            rows.filter_map(|r| r.ok()).filter(|id| !seen.contains(id)).collect()
        };
        let total: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE folder_id = ?1 AND account_id = ?2",
            params![folder_id, self.account_id], |r| r.get(0))?;
        let server_total: Option<i64> = self.conn.query_row(
            "SELECT total_item_count FROM folders WHERE id = ?1 AND account_id = ?2",
            params![folder_id, self.account_id], |r| r.get::<_, Option<i64>>(0),
        ).optional()?.flatten();
        let believable = match server_total {
            Some(t) => {
                let slack = (t / 50).max(3);
                (seen.len() as i64 - t).abs() <= slack
            }
            None => candidates.len() as i64 * 5 <= total,
        };
        if !believable {
            warn!(
                "Not purging {} of {} cached rows in folder {}: the enumeration ({} messages) does not match the provider's count ({:?})",
                candidates.len(), total, folder_id, seen.len(), server_total
            );
            return Ok(0);
        }
        // Chunked: each delete also removes the row from the FTS index, which is not cheap.
        let mut deleted = 0;
        for chunk in candidates.chunks(200) {
            self.conn.execute_batch("BEGIN")?;
            let r = (|| -> Result<usize> {
                let mut n = 0;
                let mut del = self.conn.prepare_cached("DELETE FROM messages WHERE id = ?1")?;
                for id in chunk {
                    n += del.execute(params![id])?;
                }
                Ok(n)
            })();
            match r {
                Ok(n) => { self.conn.execute_batch("COMMIT")?; deleted += n; }
                Err(e) => { let _ = self.conn.execute_batch("ROLLBACK"); return Err(e); }
            }
        }
        Ok(deleted)
    }

    /// Upsert a folder from Graph API
    pub fn upsert_folder(&self, folder: &MailFolder) -> Result<()> {
        // Assign sort_order: well-known folders get fixed low numbers, others 999
        let sort_order: i32 = match folder.well_known_name.as_deref() {
            Some("inbox")        => 1,
            Some("drafts")       => 2,
            Some("sentitems")    => 3,
            Some("deleteditems") => 4,
            Some("junkemail")    => 5,
            Some("archive")      => 6,
            _                    => 999,
        };

        self.conn.execute(
            "INSERT INTO folders (id, display_name, parent_folder_id, unread_item_count, total_item_count, well_known_name, sort_order, cached_at, account_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, CURRENT_TIMESTAMP, ?8)
             ON CONFLICT(id) DO UPDATE SET
               display_name      = excluded.display_name,
               unread_item_count = excluded.unread_item_count,
               total_item_count  = excluded.total_item_count,
               sort_order        = excluded.sort_order,
               cached_at         = CURRENT_TIMESTAMP",
            params![
                folder.id,
                folder.display_name,
                folder.parent_folder_id,
                folder.unread_item_count,
                folder.total_item_count,
                folder.well_known_name,
                sort_order,
                self.account_id,
            ],
        )?;
        Ok(())
    }

    /// This handle's account's folders, sorted: well-known first (by sort_order), then alphabetical.
    /// Scoped to the account: each account's daemon must only ever sync its own folders.
    pub fn get_folders(&self) -> Result<Vec<MailFolder>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, display_name, parent_folder_id, unread_item_count, total_item_count, well_known_name
             FROM folders
             WHERE account_id = ?1
             ORDER BY sort_order ASC, display_name ASC",
        )?;

        let folders = stmt
            .query_map(params![self.account_id], |row| {
                Ok(MailFolder {
                    id:                row.get(0)?,
                    display_name:      row.get(1)?,
                    parent_folder_id:  row.get(2)?,
                    unread_item_count: row.get(3)?,
                    total_item_count:  row.get(4)?,
                    well_known_name:   row.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(folders)
    }

    /// Get cached messages for a specific folder (by folder_id)
    pub fn get_messages_for_folder(&self, folder_id: &str, limit: usize) -> Result<Vec<CachedMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, subject, from_email, from_name, body, received_at, is_read, cached_at
             FROM messages
             WHERE folder_id = ?1
             ORDER BY received_at DESC
             LIMIT ?2",
        )?;

        let messages = stmt
            .query_map(params![folder_id, limit as i32], |row| {
                Ok(CachedMessage {
                    id:          row.get(0)?,
                    subject:     row.get(1)?,
                    from_email:  row.get(2)?,
                    from_name:   row.get(3)?,
                    body:        row.get(4)?,
                    received_at: row.get(5)?,
                    is_read:     row.get(6)?,
                    cached_at:   row.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(messages)
    }


    // ------------------------------------------------------------ contacts

    pub fn upsert_contact_folder(&self, f: &ContactFolder) -> Result<()> {
        self.conn.execute(
            "INSERT INTO contact_folders (id, display_name, parent_folder_id, cached_at, account_id)
             VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, ?4)
             ON CONFLICT(id) DO UPDATE SET
               display_name = excluded.display_name,
               parent_folder_id = excluded.parent_folder_id,
               cached_at = CURRENT_TIMESTAMP",
            params![f.id, f.display_name, f.parent_folder_id, self.account_id],
        )?;
        Ok(())
    }

    pub fn get_contact_folders(&self) -> Result<Vec<ContactFolder>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, display_name, parent_folder_id FROM contact_folders WHERE account_id = ?1 ORDER BY display_name",
        )?;
        let rows = stmt
            .query_map(params![self.account_id], |r| {
                Ok(ContactFolder { id: r.get(0)?, display_name: r.get(1)?, parent_folder_id: r.get(2)? })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Insert or update a contact. Local-only state (is_favorite) is never overwritten by sync.
    pub fn upsert_contact(&self, c: &Contact) -> Result<()> {
        self.conn.execute(
            "INSERT INTO contacts (id, display_name, given_name, surname, company, job_title, emails, phones, addresses,
                                   folder_id, created_at, modified_at, cached_at, account_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, CURRENT_TIMESTAMP, ?13)
             ON CONFLICT(id) DO UPDATE SET
               display_name = excluded.display_name, given_name = excluded.given_name,
               surname = excluded.surname, company = excluded.company, job_title = excluded.job_title,
               emails = excluded.emails, phones = excluded.phones, addresses = excluded.addresses,
               folder_id = excluded.folder_id, created_at = excluded.created_at,
               modified_at = excluded.modified_at, cached_at = CURRENT_TIMESTAMP",
            params![
                c.id, c.display_name, c.given_name, c.surname, c.company, c.job_title,
                serde_json::to_string(&c.emails)?, serde_json::to_string(&c.phones)?,
                serde_json::to_string(&c.addresses)?, c.folder_id, c.created_at, c.modified_at,
                self.account_id,
            ],
        )?;
        Ok(())
    }

    pub fn set_contact_favorite(&self, id: &str, favorite: bool) -> Result<()> {
        self.conn.execute("UPDATE contacts SET is_favorite = ?1 WHERE id = ?2", params![favorite, id])?;
        Ok(())
    }

    /// Contacts for the People view. `view`: all | favorites | lists (grouped by folder,
    /// folders alphabetical). `sort`: first | last | company | recent. Entries missing the
    /// sort key always go last.
    pub fn query_contacts(&self, view: &str, sort: &str) -> Result<Vec<ContactRow>> {
        self.query_contacts_for(view, sort, None)
    }

    /// As `query_contacts`, optionally limited to one account (its id or its email address).
    pub fn query_contacts_for(&self, view: &str, sort: &str, account: Option<&str>) -> Result<Vec<ContactRow>> {
        // `x = ''` sorts empty values after real ones.
        let key = match sort {
            "last"    => "(c.surname = ''), lower(c.surname), lower(c.given_name), lower(c.display_name)",
            "company" => "(c.company = ''), lower(c.company), lower(c.display_name)",
            "recent"  => "c.created_at DESC, lower(c.display_name)",
            _         => "(c.given_name = ''), lower(c.given_name), lower(c.surname), lower(c.display_name)",
        };
        let account_cond = "(?1 IS NULL OR c.account_id = ?1 OR lower(a.email) = lower(?1))";
        let (filter, order) = match view {
            "favorites" => (format!("WHERE c.is_favorite = 1 AND {}", account_cond), key.to_string()),
            "lists"     => (format!("WHERE {}", account_cond), format!("lower(COALESCE(f.display_name, 'Contacts')), {}", key)),
            _           => (format!("WHERE {}", account_cond), key.to_string()),
        };
        let sql = format!(
            "SELECT c.id, c.display_name, c.given_name, c.surname, c.company, c.job_title, c.emails, c.phones,
                    c.addresses, c.folder_id, c.created_at, c.modified_at, COALESCE(f.display_name, 'Contacts'), c.is_favorite,
                    COALESCE(c.account_id, ''), COALESCE(a.email, '')
             FROM contacts c LEFT JOIN contact_folders f ON f.id = c.folder_id
                             LEFT JOIN accounts a ON a.id = c.account_id
             {} ORDER BY {}",
            filter, order
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params![account], |r| {
                let emails: String = r.get(6)?;
                let phones: String = r.get(7)?;
                let addresses: String = r.get(8)?;
                Ok(ContactRow {
                    contact: Contact {
                        id: r.get(0)?, display_name: r.get(1)?, given_name: r.get(2)?, surname: r.get(3)?,
                        company: r.get(4)?, job_title: r.get(5)?,
                        emails: serde_json::from_str(&emails).unwrap_or_default(),
                        phones: serde_json::from_str::<Vec<ContactPhone>>(&phones).unwrap_or_default(),
                        addresses: serde_json::from_str::<Vec<ContactAddress>>(&addresses).unwrap_or_default(),
                        folder_id: r.get(9)?, created_at: r.get(10)?, modified_at: r.get(11)?,
                    },
                    folder_name: r.get(12)?,
                    is_favorite: r.get(13)?,
                    account_id: r.get(14)?,
                    account_email: r.get(15)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ------------------------------------------------------------ calendar

    /// Insert or update a calendar event (updates so reschedules/edits propagate)
    pub fn upsert_event(&self, ev: &CalendarEvent) -> Result<()> {
        self.conn.execute(
            "INSERT INTO calendar_events (id, subject, body, start_at, end_at, is_all_day, time_zone, cached_at, account_id,
                                          event_type, series_master_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, CURRENT_TIMESTAMP, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET
               subject    = excluded.subject,
               body       = excluded.body,
               start_at   = excluded.start_at,
               end_at     = excluded.end_at,
               is_all_day = excluded.is_all_day,
               time_zone  = excluded.time_zone,
               event_type = excluded.event_type,
               series_master_id = excluded.series_master_id,
               cached_at  = CURRENT_TIMESTAMP",
            params![
                ev.id, ev.subject, ev.body, ev.start, ev.end, ev.is_all_day, ev.time_zone, self.account_id,
                if ev.event_type.is_empty() { "singleInstance" } else { ev.event_type.as_str() },
                ev.series_master_id,
            ],
        )?;
        Ok(())
    }

    /// Store the result of expanding a date window ([from, to), "YYYY-MM-DDT00:00:00" bounds).
    /// Recurring occurrences/exceptions previously stored inside the window are replaced, so an
    /// occurrence that was cancelled or moved at the server disappears locally. Single events
    /// returned alongside are upserted as usual. Call only with a COMPLETE fetch of the window.
    pub fn replace_occurrences(&self, from: &str, to: &str, events: &[CalendarEvent]) -> Result<usize> {
        self.conn.execute_batch("BEGIN")?;
        let result = (|| -> Result<usize> {
            self.conn.execute(
                "DELETE FROM calendar_events
                 WHERE account_id = ?1 AND event_type IN ('occurrence', 'exception')
                   AND start_at >= ?2 AND start_at < ?3",
                params![self.account_id, from, to],
            )?;
            for ev in events {
                self.upsert_event(ev)?;
            }
            Ok(events.len())
        })();
        match result {
            Ok(n) => { self.conn.execute_batch("COMMIT")?; Ok(n) }
            Err(e) => { let _ = self.conn.execute_batch("ROLLBACK"); Err(e) }
        }
    }

    /// Drop recurring occurrences outside `[from, to)` (used when the occurrence window is narrowed).
    pub fn purge_occurrences_outside(&self, from: &str, to: &str) -> Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM calendar_events
             WHERE account_id = ?1 AND event_type IN ('occurrence', 'exception')
               AND (start_at < ?2 OR start_at >= ?3)",
            params![self.account_id, from, to],
        )?;
        Ok(n)
    }

    /// Events overlapping the given month ("YYYY-MM"), ordered by start
    pub fn get_events_for_month(&self, month: &str) -> Result<Vec<CalendarEvent>> {
        Ok(self.get_events_for_month_by_account(month, None)?.into_iter().map(|(e, _, _)| e).collect())
    }

    /// Month events of every account (or just `account`: its id or email address), each with
    /// the id and email address of the account it belongs to.
    pub fn get_events_for_month_by_account(&self, month: &str, account: Option<&str>) -> Result<Vec<(CalendarEvent, String, String)>> {
        let (y, m) = month.split_once('-').unwrap_or(("1970", "01"));
        let (y, m): (i32, u32) = (y.parse().unwrap_or(1970), m.parse().unwrap_or(1));
        let (ny, nm) = if m >= 12 { (y + 1, 1) } else { (y, m + 1) };
        let from = format!("{:04}-{:02}-01T00:00:00", y, m);
        let to = format!("{:04}-{:02}-01T00:00:00", ny, nm);

        let mut stmt = self.conn.prepare(
            "SELECT e.id, e.subject, e.body, e.start_at, e.end_at, e.is_all_day, COALESCE(e.time_zone, ''), e.event_type,
                    e.series_master_id, COALESCE(e.account_id, ''), COALESCE(a.email, '')
             FROM calendar_events e LEFT JOIN accounts a ON a.id = e.account_id
             WHERE e.start_at < ?2 AND e.end_at >= ?1 AND e.event_type != 'seriesMaster'
               AND (?3 IS NULL OR e.account_id = ?3 OR lower(a.email) = lower(?3))
             ORDER BY e.start_at ASC",
        )?;
        let events = stmt
            .query_map(params![from, to, account], |row| {
                Ok((CalendarEvent {
                    id:         row.get(0)?,
                    subject:    row.get(1)?,
                    body:       row.get(2)?,
                    start:      row.get(3)?,
                    end:        row.get(4)?,
                    is_all_day: row.get(5)?,
                    time_zone:  row.get(6)?,
                    event_type: row.get(7)?,
                    series_master_id: row.get(8)?,
                }, row.get(9)?, row.get(10)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(events)
    }
}

/// Subset of `ids` already stored in `messages` (read-only side connection, so a provider can
/// skip fetching messages it already has without holding the daemon's `Database` across awaits).
pub fn known_message_ids(path: &std::path::Path, ids: &[String]) -> std::collections::HashSet<String> {
    let mut known = std::collections::HashSet::new();
    let Ok(conn) = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) else { return known };
    for chunk in ids.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        let Ok(mut stmt) = conn.prepare(&format!("SELECT id FROM messages WHERE id IN ({})", marks)) else { continue };
        let found: Vec<String> = match stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |r| r.get::<_, String>(0)) {
            Ok(rows) => rows.flatten().collect(),
            Err(_) => Vec::new(),
        };
        known.extend(found);
    }
    known
}

#[cfg(test)]
mod calendar_tests {
    use super::*;

    fn ev(id: &str, start: &str, end: &str, subject: &str) -> CalendarEvent {
        CalendarEvent {
            id: id.into(), subject: subject.into(), body: "body".into(),
            start: start.into(), end: end.into(), is_all_day: false, time_zone: "America/New_York".into(),
            ..Default::default()
        }
    }

    #[test]
    fn attachment_cache_round_trips_and_tracks_real_attachments() {
        use crate::attachments::Attachment;
        let db = Database::open(":memory:").unwrap();
        assert!(db.cached_attachments("m1").unwrap().is_none());
        let a = |id: &str, inline: bool| Attachment { id: id.into(), name: format!("{}.png", id), size: 1, content_type: "image/png".into(), inline, kind: "file".into() };
        db.store_attachments("m1", &[a("logo", true)]).unwrap();
        assert_eq!(db.cached_attachments("m1").unwrap().unwrap().len(), 1);
        let has_real = |db: &Database| db.conn.query_row("SELECT has_real FROM attachment_cache WHERE message_id='m1'", [], |r| r.get::<_, i32>(0)).unwrap();
        assert_eq!(has_real(&db), 0, "an inline logo is not an attachment");
        db.store_attachments("m1", &[a("logo", true), a("scan", false)]).unwrap();
        assert_eq!(has_real(&db), 1);
        assert_eq!(db.cached_attachments("m1").unwrap().unwrap().len(), 2);
    }

    #[test]
    fn upsert_and_month_query() {
        let db = Database::open(":memory:").unwrap();
        db.upsert_event(&ev("a", "2026-10-03T09:00:00", "2026-10-03T10:00:00", "Standup")).unwrap();
        db.upsert_event(&ev("b", "2026-09-30T23:00:00", "2026-10-01T01:00:00", "Spans in")).unwrap();
        db.upsert_event(&ev("c", "2026-11-01T09:00:00", "2026-11-01T10:00:00", "Next month")).unwrap();
        db.upsert_event(&ev("d", "2026-12-31T20:00:00", "2027-01-01T01:00:00", "Spans out")).unwrap();

        let oct = db.get_events_for_month("2026-10").unwrap();
        assert_eq!(oct.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), vec!["b", "a"]);
        assert_eq!(db.get_events_for_month("2027-01").unwrap().len(), 1);
        assert_eq!(db.get_events_for_month("2026-12").unwrap().len(), 1);

        // upsert updates in place (reschedule)
        db.upsert_event(&ev("a", "2026-10-04T09:00:00", "2026-10-04T10:00:00", "Standup moved")).unwrap();
        let oct = db.get_events_for_month("2026-10").unwrap();
        assert_eq!(oct.len(), 2);
        assert!(oct.iter().any(|e| e.subject == "Standup moved" && e.start.starts_with("2026-10-04")));
    }
}

#[cfg(test)]
mod recurrence_tests {
    use super::*;

    fn occ(id: &str, start: &str, kind: &str) -> CalendarEvent {
        CalendarEvent {
            id: id.into(), subject: "Standup".into(), start: start.into(),
            end: format!("{}:30:00", &start[..start.len() - 6]),
            time_zone: "UTC".into(), event_type: kind.into(), series_master_id: Some("M".into()), ..Default::default()
        }
    }

    #[test]
    fn series_masters_are_stored_but_never_displayed() {
        let db = Database::open(":memory:").unwrap();
        db.upsert_event(&occ("M", "2026-10-01T09:00:00", "seriesMaster")).unwrap();
        db.upsert_event(&occ("o1", "2026-10-02T09:00:00", "occurrence")).unwrap();
        let shown = db.get_events_for_month("2026-10").unwrap();
        assert_eq!(shown.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), vec!["o1"]);
    }

    #[test]
    fn resyncing_a_window_removes_cancelled_occurrences_and_keeps_other_windows() {
        let db = Database::open(":memory:").unwrap();
        let first = vec![occ("o1", "2026-10-05T09:00:00", "occurrence"), occ("o2", "2026-10-12T09:00:00", "occurrence"),
                         occ("o3", "2026-10-19T09:00:00", "occurrence")];
        db.replace_occurrences("2026-10-01T00:00:00", "2026-11-01T00:00:00", &first).unwrap();
        db.upsert_event(&occ("far", "2026-12-07T09:00:00", "occurrence")).unwrap();     // outside the window
        let single = CalendarEvent { id: "s".into(), subject: "One-off".into(), start: "2026-10-06T10:00:00".into(),
            end: "2026-10-06T11:00:00".into(), ..Default::default() };
        db.upsert_event(&single).unwrap();                                              // not an occurrence

        // o2 was cancelled at the server, o3 moved
        let second = vec![occ("o1", "2026-10-05T09:00:00", "occurrence"), occ("o3", "2026-10-20T09:00:00", "exception")];
        db.replace_occurrences("2026-10-01T00:00:00", "2026-11-01T00:00:00", &second).unwrap();

        let oct: Vec<_> = db.get_events_for_month("2026-10").unwrap().into_iter().map(|e| (e.id, e.start[..10].to_string())).collect();
        assert_eq!(oct, vec![("o1".into(), "2026-10-05".into()), ("s".into(), "2026-10-06".into()), ("o3".into(), "2026-10-20".into())]);
        assert_eq!(db.get_events_for_month("2026-12").unwrap().len(), 1, "other windows untouched");
    }

    #[test]
    fn event_type_columns_are_added_to_an_existing_calendar_table() {
        let path = std::env::temp_dir().join(format!("omalook-evtype-{}", std::process::id())).to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch("CREATE TABLE calendar_events (id TEXT PRIMARY KEY, subject TEXT NOT NULL, body TEXT NOT NULL DEFAULT '',
                start_at TEXT NOT NULL, end_at TEXT NOT NULL, is_all_day BOOLEAN DEFAULT 0, time_zone TEXT, cached_at DATETIME DEFAULT CURRENT_TIMESTAMP);
                INSERT INTO calendar_events (id, subject, start_at, end_at) VALUES ('e','old','2026-10-01T09:00:00','2026-10-01T10:00:00');").unwrap();
        }
        let db = Database::open(&path).unwrap();
        let e = &db.get_events_for_month("2026-10").unwrap()[0];
        assert_eq!(e.event_type, "singleInstance");
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod account_tests {
    use super::*;

    #[test]
    fn all_accounts_rows_sum_shared_kinds_only() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE folders (id TEXT PRIMARY KEY, display_name TEXT, unread_item_count INTEGER,
            total_item_count INTEGER, well_known_name TEXT, account_id TEXT);").unwrap();
        let rows = [
            ("a:INBOX", 2, 10, "inbox", "a"),
            ("b:INBOX", 3, 20, "inbox", "b"),
            ("a:SENT", 0, 5, "sentitems", "a"),       // only one account has Sent: no row
            ("a:Work", 1, 1, "", "a"),                // custom folder: never aggregated
            ("b:Work", 1, 1, "", "b"),
        ];
        for (id, u, t, wk, acct) in rows {
            conn.execute("INSERT INTO folders VALUES (?1, ?1, ?2, ?3, ?4, ?5)",
                params![id, u, t, if wk.is_empty() { None } else { Some(wk) }, acct]).unwrap();
        }
        let got = all_accounts_folders(&conn).unwrap();
        assert_eq!(got, vec![AllAccountsFolder { well_known_name: "inbox".into(), accounts: 2,
            unread_item_count: 5, total_item_count: 30 }]);
        assert_eq!(all_accounts_label("inbox"), "Inbox - all accounts");
        assert_eq!(all_accounts_label("sentitems"), "Sent - all accounts");
        assert_eq!(all_accounts_label("deleteditems"), "Deleted - all accounts");
    }

    fn temp_db_path(name: &str) -> String {
        let dir = std::env::temp_dir().join(format!("omalook-test-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_file(&dir);
        dir.to_string_lossy().to_string()
    }

    #[test]
    fn create_account_generates_pattern_ids_even_for_first_account() {
        let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
        let a = db.create_account("Exchange", Some("a@x.com"), None, "{}").unwrap();
        let b = db.create_account("exchange", Some("b@x.com"), None, "{}").unwrap();
        for acc in [&a, &b] {
            assert!(acc.id.starts_with("exchange-") && acc.id != DEFAULT_ACCOUNT, "{}", acc.id);
            assert_eq!(acc.id.len(), "exchange-".len() + 6);
        }
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn duplicate_mailbox_is_rejected_case_insensitively() {
        let db = Database::open(":memory:").unwrap();
        db.create_account("gmail", Some("Me@Gmail.com"), None, "{}").unwrap();
        assert!(db.create_account("gmail", Some("me@gmail.com"), None, "{}").is_err());
        // same address on another provider type is a different account
        assert!(db.create_account("outlook", Some("me@gmail.com"), None, "{}").is_ok());
    }

    #[test]
    fn accounts_start_enabled_and_can_be_toggled() {
        let db = Database::open(":memory:").unwrap();
        let a = db.create_account("exchange", Some("a@x.com"), None, "{}").unwrap();
        assert!(a.enabled);
        db.set_account_enabled(&a.id, false).unwrap();
        assert!(!db.list_accounts().unwrap()[0].enabled);
        db.set_account_enabled(&a.id, true).unwrap();
        assert!(db.list_accounts().unwrap()[0].enabled);
    }

    #[test]
    fn enabled_column_is_added_to_an_existing_accounts_table() {
        let path = temp_db_path("enabled-col");
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch("CREATE TABLE accounts (id TEXT PRIMARY KEY, provider TEXT NOT NULL, email TEXT, display_name TEXT,
                config TEXT NOT NULL DEFAULT '{}', created_at DATETIME DEFAULT CURRENT_TIMESTAMP, UNIQUE(provider, email));
                INSERT INTO accounts (id, provider, email) VALUES ('exchange-aaaaaa','exchange','a@x.com');").unwrap();
        }
        let db = Database::open(&path).unwrap();
        assert!(db.list_accounts().unwrap()[0].enabled, "existing accounts stay signed in");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn open_does_not_invent_accounts() {
        let db = Database::open(":memory:").unwrap();
        assert!(db.list_accounts().unwrap().is_empty(), "fresh installs must not show a phantom account");
        db.ensure_legacy_account().unwrap();
        assert_eq!(db.list_accounts().unwrap()[0].id, "exchange-primary");
    }

    #[test]
    fn read_state_toggle_push_and_reconcile() {
        let db = Database::open_for_account(":memory:", "a1").unwrap();
        let mk = |id: &str, read: bool| EmailMessage {
            id: id.into(), from: "x@y".into(), subject: "s".into(), received: "2026-10-01T00:00:00Z".into(),
            body: "b".into(), folder_id: Some("f".into()), is_read: read, ..Default::default()
        };
        db.insert_email(&mk("m1", false)).unwrap();
        db.insert_email(&mk("m2", true)).unwrap();
        db.insert_email(&mk("m3", false)).unwrap();
        db.conn.execute("INSERT INTO folders (id, display_name, unread_item_count, total_item_count) VALUES ('f','F',2,3)", []).unwrap();
        let unread = |id: &str| -> bool { !db.conn.query_row("SELECT is_read FROM messages WHERE id=?1", [id], |r| r.get::<_, bool>(0)).unwrap() };
        let folder_unread = || -> i32 { db.conn.query_row("SELECT unread_item_count FROM folders WHERE id='f'", [], |r| r.get(0)).unwrap() };

        // insert keeps the provider's flag
        assert!(unread("m1") && !unread("m2"));

        // toggle: changes state, adjusts the folder count, queues a push; repeat is a no-op
        assert!(db.set_message_read("m1", true).unwrap());
        assert!(!db.set_message_read("m1", true).unwrap());
        assert_eq!(folder_unread(), 1);
        assert_eq!(db.pending_reads().unwrap(), vec![("m1".to_string(), true)]);

        // reconcile leaves the pending row alone, fixes the others (m2 is unread upstream, m3 read)
        let n = db.reconcile_read_state("f", &["m1".to_string(), "m2".to_string()]).unwrap();
        assert_eq!(n, 2);
        assert!(!unread("m1") && unread("m2") && !unread("m3"));

        // a flip after the push started keeps the row pending; a matching clear releases it
        db.clear_read_pending("m1", false).unwrap();
        assert_eq!(db.pending_reads().unwrap().len(), 1);
        db.clear_read_pending("m1", true).unwrap();
        assert!(db.pending_reads().unwrap().is_empty());
    }

    #[test]
    fn queued_actions_hide_the_row_undo_restores_and_finish_removes_it() {
        use crate::models::MessageAction;
        let db = Database::open_for_account(":memory:", "a1").unwrap();
        let mk = |id: &str, read: bool| EmailMessage {
            id: id.into(), from: "x@y.z".into(), subject: "s".into(), received: "2026-01-01T00:00:00Z".into(),
            body: "b".into(), folder_id: Some("f".into()), is_read: read, ..Default::default()
        };
        db.insert_email(&mk("m1", false)).unwrap();
        db.insert_email(&mk("m2", true)).unwrap();
        db.conn.execute("INSERT INTO folders (id, display_name, unread_item_count, total_item_count) VALUES ('f','F',1,2)", []).unwrap();
        let unread = || -> i32 { db.conn.query_row("SELECT unread_item_count FROM folders WHERE id='f'", [], |r| r.get(0)).unwrap() };
        let ids = vec!["m1".to_string(), "m2".to_string()];

        // queue: both queued once, unread count drops for the unread one only, repeat is a no-op
        assert_eq!(db.queue_message_actions(&ids, &MessageAction::Archive, 100).unwrap(), 2);
        assert_eq!(db.queue_message_actions(&ids, &MessageAction::Trash, 100).unwrap(), 0);
        assert_eq!(unread(), 0);

        // not due yet / due
        assert!(db.due_actions(99).unwrap().is_empty());
        let due = db.due_actions(100).unwrap();
        assert_eq!(due.len(), 2);
        assert!(due.iter().all(|(_, f, a)| f == "f" && *a == MessageAction::Archive));

        // undo one: it comes back and the count follows
        assert_eq!(db.cancel_message_actions(&["m1".to_string()]).unwrap(), 1);
        assert_eq!(unread(), 1);
        assert_eq!(db.due_actions(100).unwrap().len(), 1);

        // the provider did m2: its row is gone; m1 is untouched
        db.finish_message_action("m2").unwrap();
        let left: i32 = db.conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 1);
        // finish never deletes a message that is not queued
        db.finish_message_action("m1").unwrap();
        let left: i32 = db.conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 1);
    }

    #[test]
    fn details_arrive_with_new_rows_and_a_delta_walk_fills_old_ones() {
        let db = Database::open_for_account(":memory:", "a1").unwrap();
        // an old row (no details), as stored before the fields existed
        db.insert_email(&EmailMessage { id: "old".into(), from: "x@y.z".into(), subject: "s".into(), received: "2026-01-01T00:00:00Z".into(),
            body: "b".into(), folder_id: Some("f".into()), is_read: true, ..Default::default() }).unwrap();
        assert_eq!(db.meta_remaining().unwrap(), 1);
        // a new row with details
        db.insert_email(&EmailMessage { id: "new".into(), from: "a@b.c".into(), subject: "t".into(), received: "2026-02-01T00:00:00Z".into(),
            body: "b".into(), folder_id: Some("f".into()), is_read: false, meta: true, from_name: Some("Ann".into()),
            to_text: Some("Bob <bob@x.y>".into()), cc_text: Some("c@x.y".into()), categories: Some(vec!["Red".into()]),
            importance: Some("high".into()), has_attachments: Some(true), ..Default::default() }).unwrap();
        assert_eq!(db.meta_remaining().unwrap(), 1, "new rows do not need the repopulate");
        let get = |id: &str| -> (Option<String>, Option<String>, Option<String>, i64) {
            db.conn.query_row("SELECT from_name, to_text, categories, meta_ok FROM messages WHERE id=?1", [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap()
        };
        assert_eq!(get("new"), (Some("Ann".into()), Some("Bob <bob@x.y>".into()), Some("[\"Red\"]".into()), 1));

        // a delta walk carrying details fills the old row
        let page = crate::models::DeltaPage {
            upserts: vec![EmailMessage { id: "old".into(), from: "x@y.z".into(), subject: "s".into(), received: "2026-01-01T00:00:00Z".into(),
                body: "b".into(), folder_id: Some("f".into()), is_read: true, meta: true, from_name: Some("Xavier".into()),
                categories: Some(vec![]), ..Default::default() }],
            removed: vec![], next_link: None,
        };
        let (changed, _) = db.apply_delta_page("f", &page, None).unwrap();
        assert_eq!(changed, 1);
        assert_eq!(get("old").0, Some("Xavier".into()));
        assert_eq!(db.meta_remaining().unwrap(), 0);

        // a category edited here and not yet pushed survives a feed page that says otherwise
        assert_eq!(db.change_message_categories(&["old".into()], true, "Blue").unwrap(), 1);
        assert_eq!(db.change_message_categories(&["old".into()], true, "blue").unwrap(), 0, "case-insensitive duplicate");
        let page2 = crate::models::DeltaPage {
            upserts: vec![EmailMessage { id: "old".into(), from: "x@y.z".into(), subject: "s".into(), received: "2026-01-01T00:00:00Z".into(),
                body: "b".into(), folder_id: Some("f".into()), is_read: true, meta: true, categories: Some(vec!["Green".into()]), ..Default::default() }],
            removed: vec![], next_link: None,
        };
        db.apply_delta_page("f", &page2, None).unwrap();
        assert_eq!(get("old").2, Some("[\"Blue\"]".into()));
        let pend = db.pending_category_pushes().unwrap();
        assert_eq!(pend, vec![("old".to_string(), vec!["Blue".to_string()], vec!["Green".to_string()])]);
        // pushing releases it; an edit made meanwhile keeps it pending
        db.finish_category_push("old", &["Blue".to_string()]).unwrap();
        assert!(db.pending_category_pushes().unwrap().is_empty());
        db.change_message_categories(&["old".into()], false, "Blue").unwrap();
        db.finish_category_push("old", &["Other".to_string()]).unwrap();
        assert_eq!(db.pending_category_pushes().unwrap().len(), 1);
        db.revert_category_push("old").unwrap();
        assert_eq!(get("old").2, Some("[\"Other\"]".into()));
    }

    #[test]
    fn category_defs_pending_and_replace() {
        let db = Database::open_for_account(":memory:", "a1").unwrap();
        assert!(db.add_pending_category("Work", "#0078d4").unwrap());
        assert!(!db.add_pending_category("Work", "#e74856").unwrap(), "same name: no second row");
        db.replace_category_defs(&[crate::models::CategoryDef { name: "Home".into(), color: "#10893e".into(), provider_color: Some("preset4".into()) }]).unwrap();
        assert_eq!(db.pending_categories().unwrap(), vec![("Work".to_string(), "#0078d4".to_string())], "pending survives a refresh");
        db.finish_pending_category("Work", Some("preset7")).unwrap();
        assert!(db.pending_categories().unwrap().is_empty());
        let n: i64 = db.conn.query_row("SELECT COUNT(*) FROM category_defs", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
        db.set_master_list_available(false).unwrap();
        assert_eq!(db.conn.query_row("SELECT master_list FROM category_state", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        assert!(!db.flag_is_set("k").unwrap());
        db.set_flag("k").unwrap();
        assert!(db.flag_is_set("k").unwrap());
    }

    #[test]
    fn sender_prefs_add_list_set_remove() {
        let db = Database::open_for_account(":memory:", "a1").unwrap();
        assert_eq!(db.add_sender("  Bob@Example.COM ").unwrap(), SenderAdd::Added);
        assert_eq!(db.add_sender("bob@example.com").unwrap(), SenderAdd::AlreadyListed);
        for bad in ["", "bob", "bob@", "@x.com", "a b@x.com", "a@b@c.com", "a@nodot", "a@.com", "a@x."] {
            assert_eq!(db.add_sender(bad).unwrap(), SenderAdd::Invalid, "{bad}");
        }
        db.add_sender("amy@x.org").unwrap();
        let l = db.list_senders().unwrap();
        assert_eq!(l.iter().map(|s| s.email.as_str()).collect::<Vec<_>>(), vec!["amy@x.org", "bob@example.com"]);
        assert!(l.iter().all(|s| !s.always_html && !s.always_images));

        assert!(db.set_sender_pref("BOB@example.com", "html", true).unwrap());
        assert!(db.set_sender_pref("bob@example.com", "images", true).unwrap());
        assert!(db.set_sender_pref("bob@example.com", "html", false).unwrap());
        assert!(!db.set_sender_pref("bob@example.com", "bogus", true).unwrap());
        assert!(!db.set_sender_pref("nobody@x.com", "html", true).unwrap());
        let bob = db.list_senders().unwrap().into_iter().find(|s| s.email == "bob@example.com").unwrap();
        assert_eq!((bob.always_html, bob.always_images), (false, true));

        assert!(db.remove_sender("Amy@x.org").unwrap());
        assert!(!db.remove_sender("amy@x.org").unwrap());
        assert_eq!(db.list_senders().unwrap().len(), 1);
    }

    #[test]
    fn insert_account_uses_given_id_and_rejects_duplicates() {
        let db = Database::open(":memory:").unwrap();
        db.insert_account("exchange-abc123", "exchange", Some("a@x.com"), None, "{}").unwrap();
        assert!(db.insert_account("exchange-abc123", "exchange", Some("b@x.com"), None, "{}").is_err());
        assert!(db.insert_account("exchange-def456", "exchange", Some("a@x.com"), None, "{}").is_err());
    }

    #[test]
    fn writes_are_stamped_with_the_handle_account() {
        let db = Database::open_for_account(":memory:", "gmail-123abc").unwrap();
        db.insert_email(&EmailMessage {
            id: "m1".into(), from: "x@y".into(), subject: "s".into(), received: "2026-10-01T00:00:00Z".into(),
            body: "b".into(), folder_id: Some("f".into()), is_read: false, ..Default::default()
        }).unwrap();
        db.upsert_event(&CalendarEvent {
            id: "e1".into(), subject: "s".into(), body: "".into(), start: "2026-10-01T09:00:00".into(),
            end: "2026-10-01T10:00:00".into(), is_all_day: false, time_zone: "UTC".into(), ..Default::default()
        }).unwrap();
        let m: String = db.conn.query_row("SELECT account_id FROM messages WHERE id='m1'", [], |r| r.get(0)).unwrap();
        let e: String = db.conn.query_row("SELECT account_id FROM calendar_events WHERE id='e1'", [], |r| r.get(0)).unwrap();
        assert_eq!((m.as_str(), e.as_str()), ("gmail-123abc", "gmail-123abc"));
    }

    #[test]
    fn existing_rows_are_backfilled_to_the_legacy_account() {
        // Build a pre-multi-account database (no accounts table, no account_id).
        let path = temp_db_path("backfill");
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE messages (id TEXT PRIMARY KEY, subject TEXT NOT NULL, from_email TEXT NOT NULL, from_name TEXT,
                    body TEXT NOT NULL, received_at DATETIME NOT NULL, is_read BOOLEAN DEFAULT 0,
                    cached_at DATETIME DEFAULT CURRENT_TIMESTAMP, folder_id TEXT);
                 CREATE TABLE folders (id TEXT PRIMARY KEY, display_name TEXT NOT NULL, parent_folder_id TEXT,
                    unread_item_count INTEGER DEFAULT 0, total_item_count INTEGER DEFAULT 0, well_known_name TEXT,
                    sort_order INTEGER DEFAULT 999, cached_at DATETIME DEFAULT CURRENT_TIMESTAMP);
                 CREATE TABLE calendar_events (id TEXT PRIMARY KEY, subject TEXT NOT NULL, body TEXT NOT NULL DEFAULT '',
                    start_at TEXT NOT NULL, end_at TEXT NOT NULL, is_all_day BOOLEAN DEFAULT 0, time_zone TEXT,
                    cached_at DATETIME DEFAULT CURRENT_TIMESTAMP);
                 INSERT INTO messages (id, subject, from_email, body, received_at) VALUES ('m1','s','a@b','x','2026-01-01'),('m2','s','a@b','x','2026-01-02');
                 INSERT INTO folders (id, display_name) VALUES ('f1','Inbox');
                 INSERT INTO calendar_events (id, subject, start_at, end_at) VALUES ('e1','s','2026-01-01T09:00:00','2026-01-01T10:00:00');",
            ).unwrap();
        }
        let db = Database::open(&path).unwrap();
        for (table, n) in [("messages", 2), ("folders", 1), ("calendar_events", 1)] {
            let c: i32 = db.conn.query_row(&format!("SELECT COUNT(*) FROM {} WHERE account_id = 'exchange-primary'", table), [], |r| r.get(0)).unwrap();
            let nulls: i32 = db.conn.query_row(&format!("SELECT COUNT(*) FROM {} WHERE account_id IS NULL", table), [], |r| r.get(0)).unwrap();
            assert_eq!((c, nulls), (n, 0), "{}", table);
        }
        let trig: i32 = db.conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name='messages_au'", [], |r| r.get(0)).unwrap();
        assert_eq!(trig, 1, "FTS update trigger must be restored after backfill");
        let accts = db.list_accounts().unwrap();
        assert_eq!(accts.len(), 1);
        assert_eq!((accts[0].id.as_str(), accts[0].provider.as_str()), ("exchange-primary", "exchange"));

        // Re-opening is idempotent.
        drop(db);
        let db = Database::open(&path).unwrap();
        assert_eq!(db.list_accounts().unwrap().len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn delete_account_removes_only_its_data() {
        let a = Database::open_for_account(":memory:", "gmail-aaaaaa").unwrap();
        let ev = |id: &str| CalendarEvent { id: id.into(), subject: "s".into(), body: "".into(), start: "2026-10-01T09:00:00".into(),
            end: "2026-10-01T10:00:00".into(), is_all_day: false, time_zone: "UTC".into(), ..Default::default() };
        a.upsert_event(&ev("e1")).unwrap();
        a.conn.execute("INSERT INTO calendar_events (id, subject, start_at, end_at, account_id) VALUES ('e2','s','2026-10-01T09:00:00','2026-10-01T10:00:00','gmail-bbbbbb')", []).unwrap();
        a.delete_account("gmail-aaaaaa").unwrap();
        let left: Vec<String> = a.conn.prepare("SELECT id FROM calendar_events").unwrap()
            .query_map([], |r| r.get(0)).unwrap().filter_map(|r| r.ok()).collect();
        assert_eq!(left, vec!["e2"]);
        assert!(a.list_accounts().unwrap().is_empty());
    }
}

#[cfg(test)]
mod real_db_copy {
    use super::*;

    /// Opens a COPY of a real database (path in OMARCHY_DB_COPY) and checks the account backfill.
    /// Run with: OMARCHY_DB_COPY=/path/copy.db cargo test real_db_copy -- --ignored --nocapture
    #[test]
    #[ignore]
    fn backfill_on_copy_of_real_db() {
        let path = std::env::var("OMARCHY_DB_COPY").expect("set OMARCHY_DB_COPY");
        let before = Connection::open(&path).unwrap();
        let counts = |c: &Connection| -> Vec<i64> {
            ["messages", "folders", "calendar_events"].iter()
                .map(|t| c.query_row(&format!("SELECT COUNT(*) FROM {}", t), [], |r| r.get(0)).unwrap()).collect()
        };
        let n_before = counts(&before);
        drop(before);

        let t = std::time::Instant::now();
        let db = Database::open(&path).unwrap();
        println!("open+backfill took {:?}", t.elapsed());

        assert_eq!(counts(&db.conn), n_before, "row counts must not change");
        for table in ["messages", "folders", "calendar_events"] {
            let nulls: i64 = db.conn.query_row(&format!("SELECT COUNT(*) FROM {} WHERE account_id IS NULL", table), [], |r| r.get(0)).unwrap();
            assert_eq!(nulls, 0, "{}", table);
        }
        println!("rows before/after: {:?}", n_before);
        println!("accounts: {:?}", db.list_accounts().unwrap());
        let fts: i64 = db.conn.query_row("SELECT COUNT(*) FROM messages_fts", [], |r| r.get(0)).unwrap();
        println!("messages_fts rows: {}", fts);
    }
}

#[cfg(test)]
mod contact_tests {
    use super::*;

    fn c(id: &str, given: &str, sur: &str, company: &str, created: &str) -> Contact {
        Contact {
            id: id.into(), display_name: format!("{} {}", given, sur).trim().to_string(),
            given_name: given.into(), surname: sur.into(), company: company.into(),
            created_at: created.into(), ..Default::default()
        }
    }
    fn ids(rows: &[ContactRow]) -> Vec<&str> { rows.iter().map(|r| r.contact.id.as_str()).collect() }

    fn seeded() -> Database {
        let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
        db.upsert_contact_folder(&ContactFolder { id: "f1".into(), display_name: "Zeta list".into(), parent_folder_id: None }).unwrap();
        db.upsert_contact_folder(&ContactFolder { id: "f2".into(), display_name: "Alpha list".into(), parent_folder_id: None }).unwrap();
        let mut a = c("a", "Bob", "Zimmer", "", "2026-01-01T00:00:00Z"); a.folder_id = Some("f1".into());
        let mut b = c("b", "alice", "Young", "Acme", "2026-03-01T00:00:00Z"); b.folder_id = Some("f2".into());
        let d = c("d", "", "", "Zorg", "2026-02-01T00:00:00Z");           // no first/last name
        db.upsert_contact(&a).unwrap(); db.upsert_contact(&b).unwrap(); db.upsert_contact(&d).unwrap();
        db
    }

    #[test]
    fn sorts_by_each_key_with_missing_values_last() {
        let db = seeded();
        assert_eq!(ids(&db.query_contacts("all", "first").unwrap()), vec!["b", "a", "d"], "case-insensitive, empty last");
        assert_eq!(ids(&db.query_contacts("all", "last").unwrap()), vec!["b", "a", "d"]);
        assert_eq!(ids(&db.query_contacts("all", "company").unwrap()), vec!["b", "d", "a"]);
        assert_eq!(ids(&db.query_contacts("all", "recent").unwrap()), vec!["b", "d", "a"]);
    }

    #[test]
    fn favorites_view_and_sync_never_clears_a_favorite() {
        let db = seeded();
        db.set_contact_favorite("a", true).unwrap();
        assert_eq!(ids(&db.query_contacts("favorites", "first").unwrap()), vec!["a"]);
        // a sync re-upserts the same contact with a changed name: favorite must survive
        let mut a = c("a", "Robert", "Zimmer", "", "2026-01-01T00:00:00Z"); a.folder_id = Some("f1".into());
        db.upsert_contact(&a).unwrap();
        let fav = db.query_contacts("favorites", "first").unwrap();
        assert_eq!(fav.len(), 1);
        assert_eq!(fav[0].contact.given_name, "Robert");
        assert!(fav[0].is_favorite);
    }

    #[test]
    fn lists_view_groups_by_folder_alphabetically() {
        let db = seeded();
        let rows = db.query_contacts("lists", "first").unwrap();
        // Alpha list (b), Contacts (d: no folder), Zeta list (a)
        assert_eq!(ids(&rows), vec!["b", "d", "a"]);
        assert_eq!(rows[0].folder_name, "Alpha list");
        assert_eq!(rows[1].folder_name, "Contacts");
    }

    #[test]
    fn list_fields_round_trip() {
        let db = Database::open(":memory:").unwrap();
        let mut x = c("x", "A", "B", "Co", "2026-01-01T00:00:00Z");
        x.emails = vec!["a@x.com".into(), "b@x.com".into()];
        x.phones = vec![ContactPhone { kind: "mobile".into(), number: "555-1".into() }, ContactPhone { kind: "home".into(), number: "555-2".into() }];
        x.addresses = vec![ContactAddress { kind: "home".into(), text: "1 Main St, Town, ST 12345, US".into() }];
        db.upsert_contact(&x).unwrap();
        let got = &db.query_contacts("all", "first").unwrap()[0].contact;
        assert_eq!(got, &x);
    }

    #[test]
    fn contacts_are_stamped_with_account_and_deleted_with_it() {
        let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
        db.upsert_contact(&c("x", "A", "B", "", "2026-01-01T00:00:00Z")).unwrap();
        let acct: String = db.conn.query_row("SELECT account_id FROM contacts WHERE id='x'", [], |r| r.get(0)).unwrap();
        assert_eq!(acct, "exchange-aaaaaa");
        db.delete_account("exchange-aaaaaa").unwrap();
        assert!(db.query_contacts("all", "first").unwrap().is_empty());
    }
}

#[cfg(test)]
mod multi_account_data_tests {
    use super::*;
    use crate::models::{CalendarEvent, Contact, ContactFolder, EmailMessage, MailFolder};

    /// One shared database file, two accounts (Exchange + Gmail), each with its own handle.
    fn two_accounts(name: &str) -> (String, Database, Database) {
        let path = std::env::temp_dir().join(format!("omalook-multi-{}-{}.db", name, std::process::id())).to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);
        let ex = Database::open_for_account(&path, "exchange-aaaaaa").unwrap();
        let gm = Database::open_for_account(&path, "gmail-bbbbbb").unwrap();
        ex.insert_account("exchange-aaaaaa", "exchange", Some("adam@work.com"), None, "{}").unwrap();
        gm.insert_account("gmail-bbbbbb", "gmail", Some("Adam@Gmail.com"), None, "{}").unwrap();
        (path, ex, gm)
    }

    fn folder(id: &str) -> MailFolder {
        MailFolder { id: id.into(), display_name: "Inbox".into(), parent_folder_id: None, unread_item_count: Some(1), total_item_count: Some(2), well_known_name: Some("inbox".into()) }
    }

    #[test]
    fn each_account_only_sees_and_syncs_its_own_folders() {
        let (path, ex, gm) = two_accounts("folders");
        ex.upsert_folder(&folder("AAMk-inbox")).unwrap();
        gm.upsert_folder(&folder("gmail-bbbbbb:INBOX")).unwrap();
        assert_eq!(ex.get_folders().unwrap().iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["AAMk-inbox"]);
        assert_eq!(gm.get_folders().unwrap().iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["gmail-bbbbbb:INBOX"]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn contacts_and_events_carry_and_filter_by_account_email() {
        let (path, ex, gm) = two_accounts("rows");
        let contact = |id: &str, name: &str| Contact { id: id.into(), display_name: name.into(), ..Default::default() };
        ex.upsert_contact_folder(&ContactFolder { id: "contacts".into(), display_name: "Contacts".into(), parent_folder_id: None }).unwrap();
        ex.upsert_contact(&contact("c-ex", "Exchange Eve")).unwrap();
        gm.upsert_contact(&contact("gmail-bbbbbb:people/c1", "Gmail Gus")).unwrap();
        let ev = |id: &str| CalendarEvent { id: id.into(), subject: "s".into(), start: "2026-10-05T09:00:00".into(), end: "2026-10-05T10:00:00".into(), ..Default::default() };
        ex.upsert_event(&ev("e-ex")).unwrap();
        gm.upsert_event(&ev("gmail-bbbbbb:e1")).unwrap();

        let all = ex.query_contacts_for("all", "first", None).unwrap();
        assert_eq!(all.len(), 2);
        let by_email = |email: &str| ex.query_contacts_for("all", "first", Some(email)).unwrap();
        let g = by_email("adam@gmail.com"); // case-insensitive address match
        assert_eq!((g.len(), g[0].contact.display_name.as_str(), g[0].account_email.as_str(), g[0].account_id.as_str()), (1, "Gmail Gus", "Adam@Gmail.com", "gmail-bbbbbb"));
        assert_eq!(by_email("adam@work.com")[0].contact.display_name, "Exchange Eve");
        assert_eq!(ex.query_contacts_for("all", "first", Some("gmail-bbbbbb")).unwrap().len(), 1, "id works as filter too");
        assert!(by_email("nobody@x.com").is_empty());
        assert_eq!(ex.query_contacts_for("lists", "first", Some("adam@gmail.com")).unwrap().len(), 1);

        let events = ex.get_events_for_month_by_account("2026-10", None).unwrap();
        assert_eq!(events.len(), 2);
        let g: Vec<_> = events.iter().filter(|(_, id, _)| id == "gmail-bbbbbb").collect();
        assert_eq!((g.len(), g[0].0.id.as_str(), g[0].2.as_str()), (1, "gmail-bbbbbb:e1", "Adam@Gmail.com"));
        assert_eq!(ex.get_events_for_month_by_account("2026-10", Some("adam@work.com")).unwrap().len(), 1);
        assert_eq!(ex.get_events_for_month("2026-10").unwrap().len(), 2, "plain month query still spans accounts");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn known_message_ids_reports_only_stored_ones() {
        let (path, _ex, gm) = two_accounts("known");
        let msg = |id: &str| EmailMessage { id: id.into(), from: "a@b.c".into(), subject: "s".into(), received: "2026-10-01T00:00:00Z".into(), body: "".into(), folder_id: None, is_read: true, ..Default::default() };
        gm.insert_email(&msg("gmail-bbbbbb:1")).unwrap();
        gm.insert_email(&msg("gmail-bbbbbb:2")).unwrap();
        let ids: Vec<String> = ["gmail-bbbbbb:1", "gmail-bbbbbb:3", "gmail-bbbbbb:2"].iter().map(|s| s.to_string()).collect();
        let known = known_message_ids(std::path::Path::new(&path), &ids);
        assert_eq!(known.len(), 2);
        assert!(known.contains("gmail-bbbbbb:1") && !known.contains("gmail-bbbbbb:3"));
        assert!(known_message_ids(std::path::Path::new("/nonexistent/x.db"), &ids).is_empty());
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod batch_insert_tests {
    use super::*;
    use crate::models::EmailMessage;

    fn msg(id: &str, folder: Option<&str>) -> EmailMessage {
        EmailMessage { id: id.into(), from: "a@b.c".into(), subject: format!("s {id}"), received: "2026-10-06T12:00:00Z".into(),
                       body: "p".into(), folder_id: folder.map(String::from), is_read: false, ..Default::default() }
    }

    #[test]
    fn batch_counts_only_new_rows_and_is_idempotent() {
        let db = Database::open(":memory:").unwrap();
        let first = vec![msg("1", Some("f")), msg("2", Some("f")), msg("3", Some("f"))];
        assert_eq!(db.insert_emails_batch(&first).unwrap(), 3);
        let again = vec![msg("2", Some("f")), msg("3", Some("f")), msg("4", Some("f"))];
        assert_eq!(db.insert_emails_batch(&again).unwrap(), 1, "only id 4 is new");
        assert_eq!(db.insert_emails_batch(&[]).unwrap(), 0);
        assert!(db.email_exists("4").unwrap());
    }

    #[test]
    fn batch_backfills_missing_folder_id_on_existing_rows() {
        let db = Database::open(":memory:").unwrap();
        db.insert_emails_batch(&[msg("1", None)]).unwrap();
        db.insert_emails_batch(&[msg("1", Some("inbox-id"))]).unwrap();
        let n: i64 = db.conn.query_row("SELECT COUNT(*) FROM messages WHERE id='1' AND folder_id='inbox-id'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }
}

#[cfg(test)]
mod outbox_tests {
    use super::*;

    #[test]
    fn queue_claim_cancel_and_retry() {
        let db = Database::open_for_account(":memory:", "a1").unwrap();
        db.outbox_enqueue("o1", "a1", "{}", 1_000).unwrap();
        db.outbox_enqueue("o2", "a1", "{}", 5_000).unwrap();
        db.outbox_enqueue("o3", "other", "{}", 0).unwrap();
        assert_eq!(db.outbox_next_due("a1").unwrap(), Some(1_000));

        // nothing is due before its time; another account's rows are never touched
        assert!(db.outbox_claim_due("a1", 999).unwrap().is_empty());
        let got = db.outbox_claim_due("a1", 1_000).unwrap();
        assert_eq!(got.iter().map(|g| g.0.as_str()).collect::<Vec<_>>(), vec!["o1"]);
        assert_eq!(got[0].2, 1, "attempt number");
        assert_eq!(db.outbox_status("o1").unwrap().unwrap().0, "sending");
        assert!(db.outbox_claim_due("a1", 1_000).unwrap().is_empty(), "a claimed message is not claimed twice");
        assert_eq!(db.outbox_next_due("a1").unwrap(), Some(5_000));

        // undo works while pending, not once sending
        assert!(db.outbox_cancel("o2").unwrap());
        assert!(!db.outbox_cancel("o2").unwrap());
        assert!(!db.outbox_cancel("o1").unwrap(), "too late: already sending");
        assert!(db.outbox_claim_due("a1", 99_999).unwrap().is_empty(), "cancelled never goes out");

        // transient failure: pending again, later, with the error kept; attempts keep counting
        db.outbox_mark_retry("o1", "offline", 7_000).unwrap();
        assert_eq!(db.outbox_next_due("a1").unwrap(), Some(7_000));
        let again = db.outbox_claim_due("a1", 7_000).unwrap();
        assert_eq!(again[0].2, 2);
        db.outbox_mark_sent("o1").unwrap();
        let st = db.outbox_status("o1").unwrap().unwrap();
        assert_eq!((st.0.as_str(), st.1), ("sent", None));
        assert!(db.outbox_status("nope").unwrap().is_none());
    }

    #[test]
    fn a_crash_mid_send_is_reported_not_resent() {
        let db = Database::open_for_account(":memory:", "a1").unwrap();
        db.outbox_enqueue("o1", "a1", "{}", 0).unwrap();
        db.outbox_claim_due("a1", 1).unwrap();
        assert_eq!(db.outbox_fail_interrupted("a1").unwrap(), 1);
        let st = db.outbox_status("o1").unwrap().unwrap();
        assert_eq!(st.0, "failed");
        assert!(st.1.unwrap().contains("Sent folder"));
        assert!(db.outbox_claim_due("a1", 99).unwrap().is_empty());
    }
}

