//! Accounts: account rows, cache keys, delete, and the all-accounts folder helpers.

use super::*;


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

impl Database {

    /// Insert a bare account row if missing (provider derived from the id prefix).
    pub(super) fn ensure_account_row(&self, account_id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO accounts (id, provider) VALUES (?1, ?2)",
            params![account_id, crate::accounts::provider_of(account_id)],
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
        let provider = crate::accounts::provider_slug(provider);
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
            let id = crate::accounts::new_account_id(&provider);
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


    /// Ids that name per-account files in the cache directories; read BEFORE `delete_account`.
    pub fn account_cache_keys(&self, account_id: &str) -> Result<AccountCacheKeys> {
        let col = |sql: &str| -> Result<Vec<String>> {
            let mut st = self.conn.prepare(sql)?;
            let v = st.query_map(params![account_id], |r| r.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(v)
        };
        Ok(AccountCacheKeys {
            message_ids: col("SELECT id FROM messages WHERE account_id = ?1")?,
            draft_ids: col("SELECT local_id FROM drafts WHERE account_id = ?1")?,
            outbox_ids: col("SELECT id FROM outbox WHERE account_id = ?1")?,
            contact_ids: col("SELECT id FROM contacts WHERE account_id = ?1")?,
        })
    }


    /// Remove an account and everything stored for it: every table in `ACCOUNT_TABLES` and
    /// `MESSAGE_KEYED_TABLES`, the FTS rows, and the account row, in ONE transaction.
    /// `secure_delete` is set on this connection first so freed pages are zeroed (it is not
    /// global: it costs a write per freed page, so it is only on for this delete).
    /// Tokens live in the keyring and are removed by the token broker.
    pub fn delete_account(&self, account_id: &str) -> Result<()> {
        self.conn.pragma_update(None, "secure_delete", "ON")?;
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<()> {
            // Message-keyed tables first, while the messages still exist to name them.
            for (table, col) in MESSAGE_KEYED_TABLES {
                self.conn.execute(
                    &format!("DELETE FROM {table} WHERE {col} IN (SELECT id FROM messages WHERE account_id = ?1)"),
                    params![account_id])?;
            }
            // messages_fts.id is UNINDEXED, so the per-row messages_ad trigger would scan the whole
            // index once per message. Clear the index in one pass and swap the trigger out meanwhile
            // (DDL is transactional: a rollback restores it).
            self.conn.execute("DELETE FROM messages_fts WHERE id IN (SELECT id FROM messages WHERE account_id = ?1)", params![account_id])?;
            self.conn.execute("DROP TRIGGER IF EXISTS messages_ad", [])?;
            for table in ACCOUNT_TABLES {
                self.conn.execute(&format!("DELETE FROM {table} WHERE account_id = ?1"), params![account_id])?;
            }
            self.conn.execute(
                "CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
                    DELETE FROM messages_fts WHERE id = old.id;
                END", [])?;
            self.conn.execute("DELETE FROM accounts WHERE id = ?1", params![account_id])?;
            Ok(())
        })();
        let out = match result {
            Ok(()) => self.conn.execute_batch("COMMIT").map_err(Into::into),
            Err(e) => { let _ = self.conn.execute_batch("ROLLBACK"); Err(e) }
        };
        let _ = self.conn.pragma_update(None, "secure_delete", "OFF");
        out
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
}

impl Database {
    /// `(id, email)` of every account; email is empty when unset. MCP scoping reads this (R3.5).
    pub fn account_ids_emails(&self) -> rusqlite::Result<Vec<(String, String)>> {
        let mut st = self.conn.prepare("SELECT id, COALESCE(email, '') FROM accounts")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.flatten().collect())
    }
}
