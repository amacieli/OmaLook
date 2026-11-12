//! Contacts beyond the plain cache (docs/PLAN-contacts.md): full-fidelity rows, delta links, the
//! contact write queue, and the optimistic local state of a queued write.
//!
//! A contact row keeps the provider's whole JSON in `data`, the version it is (`change_key`, the
//! `@odata.etag` Graph answers `If-Match` with), and a `sync_state`:
//!
//! * `ok`       the row is what the server last said;
//! * `pending`  a write is queued or running (the row already shows the user's edit);
//! * `failed`   the write was refused or gave up (the row still shows the edit, the queue says why);
//! * `conflict` the server's copy changed under the edit (nothing was written);
//! * `deleted`  a delete is queued (hidden from every list).
//!
//! A pull (delta or full walk) never overwrites a row that is not `ok`: the user's edit wins until
//! the queue has finished with it.

use super::Database;
use crate::errors::Result;
use crate::models::{Contact, ContactDeltaPage};
use rusqlite::params;
use serde_json::Value;

/// What the writer needs to know about one cached contact.
#[derive(Debug, Clone)]
pub struct ContactRecord {
    pub id: String,
    pub raw: Value,
    /// `@odata.etag` (or the changeKey) of the cached copy.
    pub etag: String,
    pub folder_id: Option<String>,
    pub account_id: String,
    pub sync_state: String,
    pub is_favorite: bool,
}

/// One queued contact write, as the worker and the People view see it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ContactOp {
    pub id: String,
    pub op: String,
    pub contact_id: String,
    pub state: String,
    pub attempts: i64,
    pub error: Option<String>,
    pub payload: String,
    pub base_etag: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub result_id: Option<String>,
}

impl Database {
    pub(crate) fn init_contacts_ext_schema(&self) -> Result<()> {
        for sql in [
            "ALTER TABLE contacts ADD COLUMN data TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE contacts ADD COLUMN change_key TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE contacts ADD COLUMN categories TEXT NOT NULL DEFAULT '[]'",
            "ALTER TABLE contacts ADD COLUMN sync_state TEXT NOT NULL DEFAULT 'ok'",
            "ALTER TABLE contacts ADD COLUMN search_text TEXT NOT NULL DEFAULT ''",
        ] {
            self.add_column(sql)?;
        }
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS contact_sync (
                account_id TEXT NOT NULL,
                folder_id TEXT NOT NULL,
                delta_link TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (account_id, folder_id)
            )",
            [],
        )?;
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS contact_outbox (
                id TEXT PRIMARY KEY,
                account_id TEXT NOT NULL,
                op TEXT NOT NULL,
                contact_id TEXT NOT NULL DEFAULT '',
                payload_json TEXT NOT NULL,
                base_etag TEXT NOT NULL DEFAULT '',
                state TEXT NOT NULL DEFAULT 'pending',
                attempts INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                due_at INTEGER NOT NULL,
                result_id TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_contact_outbox_due ON contact_outbox(account_id, state, due_at)",
            [],
        )?;
        self.init_list_link_schema()?;
        // The old pseudo id of the default folder collided between accounts; the real id replaces it
        // (the next folder sync stores it, and the next delta walk re-reads the contacts).
        self.conn.execute("DELETE FROM contact_folders WHERE id = 'contacts'", [])?;
        Ok(())
    }

    // ---- delta links -----------------------------------------------------------------------

    pub fn contact_delta_link(&self, folder_id: &str) -> Result<Option<String>> {
        match self.conn.query_row(
            "SELECT delta_link FROM contact_sync WHERE account_id = ?1 AND folder_id = ?2",
            params![self.account_id, folder_id],
            |r| r.get::<_, String>(0),
        ) {
            Ok(l) => Ok(Some(l)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn set_contact_delta_link(&self, folder_id: &str, link: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO contact_sync (account_id, folder_id, delta_link, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(account_id, folder_id) DO UPDATE SET delta_link = excluded.delta_link, updated_at = excluded.updated_at",
            params![self.account_id, folder_id, link, Self::now_ms()],
        )?;
        Ok(())
    }

    pub fn clear_contact_delta_link(&self, folder_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM contact_sync WHERE account_id = ?1 AND folder_id = ?2", params![self.account_id, folder_id])?;
        Ok(())
    }

    pub fn clear_contact_delta_links(&self) -> Result<()> {
        self.conn.execute("DELETE FROM contact_sync WHERE account_id = ?1", params![self.account_id])?;
        Ok(())
    }

    // ---- rows ------------------------------------------------------------------------------

    /// One page of a delta read: store the contacts, drop the removed ones. Rows that are not `ok`
    /// (a write of ours is in flight) are left alone either way. Returns (stored, removed).
    pub fn apply_contact_delta_page(&self, page: &ContactDeltaPage) -> Result<(usize, usize)> {
        let mut stored = 0;
        for c in &page.upserts {
            self.upsert_contact(c)?;
            stored += 1;
        }
        let mut removed = 0;
        for id in &page.removed {
            removed += self.conn.execute(
                "DELETE FROM contacts WHERE id = ?1 AND account_id = ?2 AND sync_state = 'ok'",
                params![id, self.account_id],
            )?;
        }
        Ok((stored, removed))
    }

    pub fn get_contact_record(&self, id: &str) -> Result<Option<ContactRecord>> {
        let r = self.conn.query_row(
            "SELECT data, change_key, folder_id, COALESCE(account_id, ''), sync_state, is_favorite FROM contacts WHERE id = ?1",
            params![id],
            |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, bool>(5)?))
            },
        );
        match r {
            Ok((data, etag, folder_id, account_id, sync_state, is_favorite)) => Ok(Some(ContactRecord {
                id: id.to_string(),
                raw: serde_json::from_str(&data).unwrap_or(Value::Null),
                etag,
                folder_id,
                account_id,
                sync_state,
                is_favorite,
            })),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Write a contact row from the provider's JSON in a given state, whatever state it was in (the
    /// optimistic edit, and the server's answer after a write). Keeps the favourite flag.
    pub fn put_contact_json(&self, raw: &Value, folder_id: Option<&str>, state: &str) -> Result<Contact> {
        let mut c = crate::providers::contacts::parse_contact(raw);
        if let Some(f) = folder_id {
            c.folder_id = Some(f.to_string());
        }
        self.write_contact_row(&c, state, false)?;
        Ok(c)
    }

    pub fn set_contact_state(&self, id: &str, state: &str) -> Result<()> {
        self.conn.execute("UPDATE contacts SET sync_state = ?2 WHERE id = ?1", params![id, state])?;
        Ok(())
    }

    pub fn delete_contact_row(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM contacts WHERE id = ?1 AND account_id = ?2", params![id, self.account_id])?;
        Ok(())
    }

    /// Give a favourite flag to another row (a created contact replaces its local placeholder).
    pub fn move_contact_favorite(&self, from: &str, to: &str) -> Result<()> {
        let fav: bool = self.conn.query_row("SELECT is_favorite FROM contacts WHERE id = ?1", params![from], |r| r.get(0)).unwrap_or(false);
        if fav {
            self.conn.execute("UPDATE contacts SET is_favorite = 1 WHERE id = ?1", params![to])?;
        }
        Ok(())
    }

    /// The one write path of a contact row. `only_if_ok`: a pull must not touch rows with a write in flight.
    pub(crate) fn write_contact_row(&self, c: &Contact, state: &str, only_if_ok: bool) -> Result<usize> {
        let search = if c.raw.is_empty() {
            let mut parts = vec![c.display_name.clone(), c.company.clone(), c.job_title.clone()];
            parts.extend(c.emails.iter().cloned());
            parts.extend(c.phones.iter().map(|p| p.number.clone()));
            parts.extend(c.addresses.iter().map(|a| a.text.clone()));
            parts.extend(c.categories.iter().cloned());
            parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n").to_lowercase()
        } else {
            crate::providers::contacts::search_text(&serde_json::from_str(&c.raw).unwrap_or(Value::Null))
        };
        let sql = format!(
            "INSERT INTO contacts (id, display_name, given_name, surname, company, job_title, emails, phones, addresses,
                                   folder_id, created_at, modified_at, cached_at, account_id,
                                   data, change_key, categories, sync_state, search_text)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, CURRENT_TIMESTAMP, ?13, ?14, ?15, ?16, ?17, ?18)
             ON CONFLICT(id) DO UPDATE SET
               display_name = excluded.display_name, given_name = excluded.given_name,
               surname = excluded.surname, company = excluded.company, job_title = excluded.job_title,
               emails = excluded.emails, phones = excluded.phones, addresses = excluded.addresses,
               folder_id = excluded.folder_id, created_at = excluded.created_at,
               modified_at = excluded.modified_at, cached_at = CURRENT_TIMESTAMP,
               data = excluded.data, change_key = excluded.change_key, categories = excluded.categories,
               sync_state = excluded.sync_state, search_text = excluded.search_text
             {}",
            if only_if_ok { "WHERE contacts.sync_state = 'ok'" } else { "" }
        );
        let n = self.conn.execute(
            &sql,
            params![
                c.id, c.display_name, c.given_name, c.surname, c.company, c.job_title,
                serde_json::to_string(&c.emails)?, serde_json::to_string(&c.phones)?,
                serde_json::to_string(&c.addresses)?, c.folder_id, c.created_at, c.modified_at,
                self.account_id, c.raw, etag_of(c), serde_json::to_string(&c.categories)?, state, search,
            ],
        )?;
        Ok(n)
    }

    // ---- the write queue -------------------------------------------------------------------

    pub fn contact_outbox_enqueue(&self, id: &str, op: &str, contact_id: &str, payload_json: &str, base_etag: &str, due_ms: i64) -> Result<()> {
        let now = Self::now_ms();
        self.conn.execute(
            "INSERT INTO contact_outbox (id, account_id, op, contact_id, payload_json, base_etag, state, due_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?8, ?8)",
            params![id, self.account_id, op, contact_id, payload_json, base_etag, due_ms, now],
        )?;
        Ok(())
    }

    /// Claim every due operation of the account in the order it was queued (a second edit of one
    /// contact must not run before the first): `(id, op, contact_id, payload, base_etag, attempt)`.
    pub fn contact_outbox_claim_due(&self, now_ms: i64) -> Result<Vec<(String, String, String, String, String, i64)>> {
        let due: Vec<(String, String, String, String, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, op, contact_id, payload_json, base_etag FROM contact_outbox
                 WHERE account_id = ?1 AND state = 'pending' AND due_at <= ?2 ORDER BY created_at, rowid",
            )?;
            let rows = stmt
                .query_map(params![self.account_id, now_ms], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        let mut claimed = Vec::new();
        for (id, op, cid, payload, etag) in due {
            let n = self.conn.execute(
                "UPDATE contact_outbox SET state = 'sending', attempts = attempts + 1, updated_at = ?2 WHERE id = ?1 AND state = 'pending'",
                params![id, now_ms],
            )?;
            if n == 1 {
                let attempts: i64 = self.conn.query_row("SELECT attempts FROM contact_outbox WHERE id = ?1", params![id], |r| r.get(0))?;
                claimed.push((id, op, cid, payload, etag, attempts));
            }
        }
        Ok(claimed)
    }

    pub fn contact_outbox_next_due(&self) -> Result<Option<i64>> {
        match self.conn.query_row(
            "SELECT MIN(due_at) FROM contact_outbox WHERE account_id = ?1 AND state = 'pending'",
            params![self.account_id],
            |r| r.get::<_, Option<i64>>(0),
        ) {
            Ok(v) => Ok(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn contact_outbox_mark(&self, id: &str, state: &str, error: Option<&str>, result_id: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE contact_outbox SET state = ?2, last_error = ?3, result_id = COALESCE(?4, result_id), updated_at = ?5 WHERE id = ?1",
            params![id, state, error, result_id, Self::now_ms()],
        )?;
        Ok(())
    }

    pub fn contact_outbox_mark_retry(&self, id: &str, error: &str, retry_at_ms: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE contact_outbox SET state = 'pending', last_error = ?2, due_at = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, error, retry_at_ms, Self::now_ms()],
        )?;
        Ok(())
    }

    /// After a crash an operation left `sending` is reported, never sent again blindly.
    pub fn contact_outbox_fail_interrupted(&self) -> Result<Vec<(String, String)>> {
        let rows: Vec<(String, String)> = {
            let mut stmt = self.conn.prepare("SELECT id, contact_id FROM contact_outbox WHERE account_id = ?1 AND state = 'sending'")?;
            let v = stmt.query_map(params![self.account_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<Vec<_>, _>>()?;
            v
        };
        for (id, cid) in &rows {
            self.contact_outbox_mark(id, "failed", Some("the app stopped while this was being sent; check the contact on the server before trying again"), None)?;
            if !cid.is_empty() {
                self.set_contact_state(cid, "failed")?;
            }
        }
        Ok(rows)
    }

    pub fn contact_outbox_prune(&self, max_age_ms: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM contact_outbox WHERE state IN ('sent','cancelled','undone') AND updated_at < ?1",
            params![Self::now_ms() - max_age_ms],
        )?)
    }

    pub fn contact_outbox_get(&self, id: &str) -> Result<Option<ContactOp>> {
        let r = self.conn.query_row(
            "SELECT id, op, contact_id, state, attempts, last_error, payload_json, base_etag, created_at, updated_at, result_id FROM contact_outbox WHERE id = ?1",
            params![id],
            map_op,
        );
        match r {
            Ok(o) => Ok(Some(o)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Everything not yet done, oldest first, plus what finished in the last minute (for the status bar).
    pub fn contact_outbox_list(&self) -> Result<Vec<ContactOp>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, op, contact_id, state, attempts, last_error, payload_json, base_etag, created_at, updated_at, result_id FROM contact_outbox
             WHERE account_id = ?1 AND (state IN ('pending','sending','failed','conflict') OR (state = 'sent' AND updated_at > ?2))
             ORDER BY created_at, rowid",
        )?;
        let rows = stmt.query_map(params![self.account_id, Self::now_ms() - 60_000], map_op)?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Operations of one contact that are still waiting or running.
    pub fn contact_outbox_open_for(&self, contact_id: &str) -> Result<usize> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM contact_outbox WHERE account_id = ?1 AND contact_id = ?2 AND state IN ('pending','sending')",
            params![self.account_id, contact_id],
            |r| r.get::<_, i64>(0),
        )? as usize)
    }

    /// Give up a failed or conflicting write: the row goes back to `ok` so the next pull may replace
    /// it with the server's copy. A created contact that never reached the server is removed.
    pub fn contact_outbox_discard(&self, id: &str) -> Result<Option<ContactOp>> {
        let op = self.contact_outbox_get(id)?;
        if let Some(o) = &op {
            if o.state == "failed" || o.state == "conflict" {
                self.contact_outbox_mark(id, "cancelled", None, None)?;
                if !o.contact_id.is_empty() && self.contact_outbox_open_for(&o.contact_id)? == 0 {
                    if o.op == "create" {
                        self.delete_contact_row(&o.contact_id)?;
                    } else {
                        self.set_contact_state(&o.contact_id, "ok")?;
                    }
                }
            }
        }
        Ok(op)
    }
}

impl Database {
    /// This account's own (top level) contact folder: where a new contact goes unless told otherwise.
    pub fn default_contact_folder(&self) -> Result<Option<String>> {
        match self.conn.query_row(
            "SELECT id FROM contact_folders WHERE account_id = ?1 AND parent_folder_id IS NULL ORDER BY (display_name = 'Contacts') DESC, display_name LIMIT 1",
            params![self.account_id],
            |r| r.get::<_, String>(0),
        ) {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// The create of a contact that has not been claimed by the worker yet: `(operation id, payload)`.
    pub fn contact_outbox_pending_create(&self, contact_id: &str) -> Result<Option<(String, String)>> {
        match self.conn.query_row(
            "SELECT id, payload_json FROM contact_outbox WHERE account_id = ?1 AND contact_id = ?2 AND op = 'create' AND state = 'pending'",
            params![self.account_id, contact_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        ) {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn contact_outbox_set_payload(&self, id: &str, payload: &str) -> Result<()> {
        self.conn.execute("UPDATE contact_outbox SET payload_json = ?2, updated_at = ?3 WHERE id = ?1 AND state = 'pending'", params![id, payload, Self::now_ms()])?;
        Ok(())
    }

    /// After a write went through, later waiting writes of the same contact guard against the new version.
    pub fn contact_outbox_rebase(&self, contact_id: &str, etag: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE contact_outbox SET base_etag = ?3 WHERE account_id = ?1 AND contact_id = ?2 AND state = 'pending' AND op = 'update'",
            params![self.account_id, contact_id, etag],
        )?;
        Ok(())
    }

    /// Later writes aimed at a placeholder id follow the contact to its real id.
    pub fn contact_outbox_retarget(&self, from: &str, to: &str) -> Result<()> {
        self.conn.execute("UPDATE contact_outbox SET contact_id = ?3 WHERE account_id = ?1 AND contact_id = ?2", params![self.account_id, from, to])?;
        Ok(())
    }

    /// Put a failed write back in the queue (retry).
    pub fn contact_outbox_retry(&self, id: &str) -> Result<Option<ContactOp>> {
        let op = self.contact_outbox_get(id)?;
        if let Some(o) = &op {
            if o.state == "failed" {
                self.contact_outbox_mark_retry(id, "", Self::now_ms())?;
                if !o.contact_id.is_empty() { self.set_contact_state(&o.contact_id, if o.op == "delete" { "deleted" } else { "pending" })?; }
            }
        }
        Ok(op)
    }

    /// The newest write of the account that Ctrl-Z may take back (not itself undone, recent enough).
    pub fn contact_outbox_newest_undoable(&self, since_ms: i64) -> Result<Option<ContactOp>> {
        match self.conn.query_row(
            "SELECT id, op, contact_id, state, attempts, last_error, payload_json, base_etag, created_at, updated_at, result_id FROM contact_outbox
             WHERE account_id = ?1 AND state IN ('pending','sent','failed','conflict') AND created_at >= ?2
               AND payload_json NOT LIKE '%\"undo_of\"%' ORDER BY created_at DESC, rowid DESC LIMIT 1",
            params![self.account_id, since_ms],
            map_op,
        ) {
            Ok(o) => Ok(Some(o)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// For each account: when its newest write that Ctrl-Z may take back was queued: `(created_at, account_id)`.
    pub fn contact_outbox_newest_per_account(&self) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT MAX(created_at), account_id FROM contact_outbox
             WHERE state IN ('pending','sent','failed','conflict') AND created_at >= ?1 AND payload_json NOT LIKE '%\"undo_of\"%' GROUP BY account_id",
        )?;
        let rows = stmt.query_map(params![Self::now_ms() - 15 * 60_000], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Every contact folder (optionally one account's) with its account and how many contacts it holds.
    pub fn list_contact_folders_ext(&self, account: Option<&str>) -> Result<Vec<Value>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.id, f.display_name, f.parent_folder_id, COALESCE(f.account_id, ''), COALESCE(a.email, ''),
                    (SELECT COUNT(*) FROM contacts c WHERE c.folder_id = f.id AND c.sync_state <> 'deleted')
             FROM contact_folders f LEFT JOIN accounts a ON a.id = f.account_id
             WHERE (?1 IS NULL OR f.account_id = ?1 OR lower(a.email) = lower(?1))
             ORDER BY lower(COALESCE(a.email, '')), (f.parent_folder_id IS NOT NULL), lower(f.display_name)",
        )?;
        let rows = stmt
            .query_map(params![account], |r| {
                Ok(serde_json::json!({
                    "id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?, "parent_id": r.get::<_, Option<String>>(2)?,
                    "account_id": r.get::<_, String>(3)?, "account_email": r.get::<_, String>(4)?, "count": r.get::<_, i64>(5)?,
                }))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The newest failed or conflicting write of a contact (what the detail pane says is wrong).
    pub fn contact_last_error(&self, contact_id: &str) -> Result<Option<(String, String, String)>> {
        match self.conn.query_row(
            "SELECT id, state, COALESCE(last_error, '') FROM contact_outbox
             WHERE contact_id = ?1 AND state IN ('failed', 'conflict') ORDER BY created_at DESC, rowid DESC LIMIT 1",
            params![contact_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
        ) {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// (account email, folder name) of a cached contact, for display.
    pub fn contact_context(&self, id: &str) -> Result<(String, String)> {
        match self.conn.query_row(
            "SELECT COALESCE(a.email, ''), COALESCE(f.display_name, 'Contacts')
             FROM contacts c LEFT JOIN accounts a ON a.id = c.account_id LEFT JOIN contact_folders f ON f.id = c.folder_id WHERE c.id = ?1",
            params![id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        ) {
            Ok(v) => Ok(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok((String::new(), String::new())),
            Err(e) => Err(e.into()),
        }
    }
}

fn map_op(r: &rusqlite::Row<'_>) -> rusqlite::Result<ContactOp> {
    Ok(ContactOp {
        id: r.get(0)?,
        op: r.get(1)?,
        contact_id: r.get(2)?,
        state: r.get(3)?,
        attempts: r.get(4)?,
        error: r.get(5)?,
        payload: r.get(6)?,
        base_etag: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
        result_id: r.get(10)?,
    })
}

/// The version string stored with a row: Graph's `@odata.etag` when the JSON has it, else the changeKey.
fn etag_of(c: &Contact) -> String {
    if c.raw.is_empty() {
        return c.change_key.clone();
    }
    serde_json::from_str::<Value>(&c.raw)
        .ok()
        .and_then(|v| v["@odata.etag"].as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| c.change_key.clone())
}
