//! Schema: open, migrations (`init_schema` and friends), and the shared connection setup.

use super::*;
use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::sync::{LazyLock, Mutex};

/// Identity of a database file for the once-per-process schema check (R3.3): canonical path plus
/// device and inode. A file deleted and recreated at the same path, or moved aside by db_health,
/// is a different file, so its DDL runs again.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct FileKey {
    path: std::path::PathBuf,
    dev: u64,
    ino: u64,
}

impl FileKey {
    /// `None` for `:memory:`, empty paths and `file:` URIs (no stable file identity).
    /// Call only after `Connection::open` has created the file.
    fn of(path: &str) -> Option<Self> {
        if path.is_empty() || path == ":memory:" || path.starts_with("file:") {
            return None;
        }
        let path = std::fs::canonicalize(path).ok()?;
        let meta = std::fs::metadata(&path).ok()?;
        Some(Self { path, dev: meta.dev(), ino: meta.ino() })
    }
}

/// Files whose schema this process has already created (see `Database::ensure_schema`).
static SCHEMA_READY: LazyLock<Mutex<HashSet<FileKey>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

#[cfg(test)]
thread_local! {
    /// Test-only: DDL runs on the thread that opens the database, so a thread-local counter is exact
    /// even while other tests run in parallel.
    static SCHEMA_RUNS_HERE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn schema_runs_on_this_thread() -> usize {
    SCHEMA_RUNS_HERE.with(|c| c.get())
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
        db.ensure_schema(path)?;
        info!("Database initialized");
        
        Ok(db)
    }

    /// Run the DDL for `path` unless this process already did it for the same file (R3.3).
    /// The check and the DDL run under one global lock, so concurrent first opens serialise and
    /// the rest skip the DDL. `:memory:` and `file:` URIs always run it (each is a new database).
    fn ensure_schema(&self, path: &str) -> Result<()> {
        let Some(key) = FileKey::of(path) else { return self.run_schema() };
        let mut ready = SCHEMA_READY.lock().unwrap_or_else(|e| e.into_inner());
        // The inode alone is not proof: a deleted file's inode can be reused by the next file at
        // the same path. A cheap look for the `messages` table catches that case.
        if ready.contains(&key) && self.schema_present() {
            return Ok(());
        }
        self.run_schema()?;
        ready.insert(key);
        Ok(())
    }

    fn schema_present(&self) -> bool {
        self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'messages')",
            [],
            |row| row.get::<_, bool>(0),
        ).unwrap_or(false)
    }

    /// Every DDL statement a database needs. Called through `ensure_schema` only.
    fn run_schema(&self) -> Result<()> {
        #[cfg(test)]
        SCHEMA_RUNS_HERE.with(|c| c.set(c.get() + 1));
        self.init_schema()?;
        // Serves "newest N in folder" without scanning and sorting the whole table.
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_messages_folder_received ON messages(folder_id, received_at DESC)", [])?;
        // Change-feed position per account + folder (Graph delta). `complete` = 0 while the
        // first full enumeration is still running (the link then resumes it next launch).
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS mail_delta (
                account_id TEXT NOT NULL,
                folder_id  TEXT NOT NULL,
                link       TEXT NOT NULL,
                complete   INTEGER NOT NULL DEFAULT 0,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (account_id, folder_id)
            )", [])?;
        Ok(())
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
            self.add_column("ALTER TABLE messages ADD COLUMN folder_id TEXT")?;
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
            self.add_column("ALTER TABLE messages ADD COLUMN read_pending INTEGER NOT NULL DEFAULT 0")?;
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
                self.add_column(&format!("ALTER TABLE messages ADD COLUMN {} {}", col, decl))?;
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
                self.add_column(&format!("ALTER TABLE messages ADD COLUMN {} {}", col, decl))?;
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
                self.add_column(&format!("ALTER TABLE messages ADD COLUMN {} {}", col, decl))?;
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

        self.init_lists_schema()?;
        self.init_mail_extras_schema()?;
        self.init_tags_schema()?;
        self.init_rules_schema()?;
        self.init_local_rules_schema()?;
        self.init_tasks_schema()?;

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

        // Drafts (docs/PLAN-compose.md Phase B2): the local copy of every compose draft; the provider
        // copy is `remote_id`. See db/drafts.rs for the rules. dirty = not yet confirmed by the provider.
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS drafts (
                local_id TEXT PRIMARY KEY,
                account_id TEXT NOT NULL,
                remote_id TEXT,
                remote_version TEXT,
                kind TEXT NOT NULL DEFAULT 'new',
                in_reply_to TEXT NOT NULL DEFAULT '',
                payload_json TEXT NOT NULL,
                mode TEXT NOT NULL DEFAULT 'system',
                attachments_sig TEXT NOT NULL DEFAULT '',
                dirty INTEGER NOT NULL DEFAULT 1,
                state TEXT NOT NULL DEFAULT 'editing',
                attempts INTEGER NOT NULL DEFAULT 0,
                next_attempt_at INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                notice TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_drafts_account ON drafts(account_id, state, dirty)",
            [],
        )?;

        // Calendar writes (docs/PLAN-calendar-events.md section 6): queued by the editor, sent by the
        // account's calendar outbox worker once `due_at` (unix ms) passes, which is the undo window.
        // op: create (update / delete / cancel come with phase 3). state: pending | sending | sent |
        // failed | cancelled. `result_id` is the provider's id of the created event.
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS calendar_outbox (
                id TEXT PRIMARY KEY,
                account_id TEXT NOT NULL,
                op TEXT NOT NULL DEFAULT 'create',
                payload_json TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'pending',
                attendees_count INTEGER NOT NULL DEFAULT 0,
                attempts INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                due_at INTEGER NOT NULL,
                transaction_id TEXT NOT NULL,
                result_id TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_calendar_outbox_due ON calendar_outbox(account_id, state, due_at)",
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
        for (col, decl) in [
            ("event_type", "TEXT NOT NULL DEFAULT 'singleInstance'"), ("series_master_id", "TEXT"),
            ("location", "TEXT NOT NULL DEFAULT ''"), ("organizer_name", "TEXT NOT NULL DEFAULT ''"),
            ("organizer_email", "TEXT NOT NULL DEFAULT ''"), ("is_organizer", "BOOLEAN NOT NULL DEFAULT 0"),
            ("attendees_json", "TEXT NOT NULL DEFAULT ''"), ("my_response", "TEXT NOT NULL DEFAULT ''"),
            ("show_as", "TEXT NOT NULL DEFAULT ''"), ("online_provider", "TEXT NOT NULL DEFAULT ''"),
            ("online_url", "TEXT NOT NULL DEFAULT ''"), ("reminder_minutes", "INTEGER"),
            ("web_link", "TEXT NOT NULL DEFAULT ''"), ("sensitivity", "TEXT NOT NULL DEFAULT ''"),
            ("recurrence_json", "TEXT NOT NULL DEFAULT ''"), ("calendar_id", "TEXT NOT NULL DEFAULT ''"),
            ("tags_json", "TEXT NOT NULL DEFAULT ''"), ("etag", "TEXT NOT NULL DEFAULT ''"),
        ] {
            let has: bool = self.conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('calendar_events') WHERE name = ?1",
                params![col],
                |row| row.get::<_, i32>(0),
            ).unwrap_or(0) > 0;
            if !has {
                self.add_column(&format!("ALTER TABLE calendar_events ADD COLUMN {} {}", col, decl))?;
            }
        }

        self.init_contacts_schema()?;
        self.init_sms_schema()?;
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
        self.init_contacts_ext_schema()?;
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
            self.add_column("ALTER TABLE accounts ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1")?;
        }

        let mut backfill_needed = false;
        for table in ["messages", "folders", "calendar_events", "contacts", "contact_folders"] {
            let has_col: bool = self.conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name='account_id'",
                params![table],
                |row| row.get::<_, i32>(0),
            ).unwrap_or(0) > 0;
            if !has_col {
                self.add_column(&format!("ALTER TABLE {} ADD COLUMN account_id TEXT", table))?;
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


    /// Run `f` inside one BEGIN/COMMIT: either every write it makes lands or none does (an `Err`
    /// rolls back). Not reentrant: `f` must not call another transaction helper, and no caller may
    /// hold a transaction open (SQLite has no nested BEGIN). Keep `f` free of awaits.
    pub fn in_transaction<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.conn.execute_batch("BEGIN")?;
        match f() {
            Ok(v) => match self.conn.execute_batch("COMMIT") {
                Ok(()) => Ok(v),
                Err(e) => {
                    let _ = self.conn.execute_batch("ROLLBACK");
                    Err(e.into())
                }
            },
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod schema_once_tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("omalook-schema-once-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn remove_db(path: &std::path::Path) {
        for suffix in ["", "-wal", "-shm"] {
            let mut p = path.as_os_str().to_owned();
            p.push(suffix);
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn second_open_of_the_same_file_runs_no_ddl() {
        let dir = scratch("second-open");
        let path = dir.join("m.db");
        let p = path.to_str().unwrap();
        let before = schema_runs_on_this_thread();
        let first = Database::open(p).unwrap();
        assert_eq!(schema_runs_on_this_thread(), before + 1, "first open runs the DDL once");
        drop(first);
        let _second = Database::open(p).unwrap();
        assert_eq!(schema_runs_on_this_thread(), before + 1, "second open of the same file skips the DDL");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn memory_databases_always_run_the_ddl() {
        let before = schema_runs_on_this_thread();
        let _a = Database::open(":memory:").unwrap();
        let _b = Database::open(":memory:").unwrap();
        assert_eq!(schema_runs_on_this_thread(), before + 2);
    }

    #[test]
    fn file_deleted_and_recreated_at_the_same_path_gets_its_schema_again() {
        let dir = scratch("recreate");
        let path = dir.join("m.db");
        let p = path.to_str().unwrap();
        drop(Database::open(p).unwrap());
        let before = schema_runs_on_this_thread();
        remove_db(&path);
        // Same path, new (empty) file: the inode may be reused, so the table check must catch it.
        let db = Database::open(p).unwrap();
        assert_eq!(schema_runs_on_this_thread(), before + 1, "recreated file runs the DDL again");
        assert!(db.schema_present());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_replaced_by_a_new_inode_gets_its_schema_again() {
        // db_health moves the damaged file aside and builds a new one at the same path; a rename
        // over the path gives a different inode while the old file still exists.
        let dir = scratch("replaced");
        let path = dir.join("m.db");
        let p = path.to_str().unwrap();
        drop(Database::open(p).unwrap());
        let before = schema_runs_on_this_thread();
        let fresh = dir.join("fresh.db");
        drop(rusqlite::Connection::open(&fresh).unwrap());
        std::fs::rename(&fresh, &path).unwrap();
        let db = Database::open(p).unwrap();
        assert_eq!(schema_runs_on_this_thread(), before + 1, "a different inode runs the DDL again");
        assert!(db.schema_present());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
