//! Local drafts (docs/PLAN-compose.md Phase B2): the compose pane's text and attachments kept in
//! SQLite on every autosave, and pushed to the provider's Drafts folder by the draft worker.
//!
//! The provider is the source of truth once a draft has a `remote_id`; this row is the copy that
//! survives a crash or an offline stretch. Rules the callers rely on:
//! * `draft_save` is the only writer of content. It sets `dirty = 1`, so the push worker sees it.
//! * `draft_mark_pushed` clears `dirty` only when nothing was saved while the push was in flight
//!   (it compares `updated_at` with the value read before the push).
//! * `draft_delete` / `draft_mark_sent` are called only after the provider confirmed the delete
//!   or send. The local copy is never removed before that.
//! * `state`: `editing` (autosaved while the compose pane was open; a leftover one with
//!   `dirty = 1` after a crash is offered for recovery), `saved` (the user saved, closed or
//!   quit), `sent` (pruned by `draft_prune_sent`).

use super::Database;
use crate::errors::Result;
use rusqlite::{params, OptionalExtension};

/// `next_attempt_at` of a draft that failed for a reason retrying will not fix (4xx). It waits
/// for the next user save, which resets it.
pub const PARKED: i64 = i64::MAX;

#[derive(Debug, Clone, PartialEq)]
pub struct DraftRow {
    pub local_id: String,
    pub account_id: String,
    pub remote_id: Option<String>,
    /// Graph `changeKey` or Gmail message id as last seen after our own push.
    pub remote_version: Option<String>,
    pub kind: String,
    pub in_reply_to: String,
    /// `compose::OutgoingMessage` as JSON (recipients, subject, body, mode, attachments...).
    pub payload_json: String,
    pub mode: String,
    /// Signature of the attachment set last pushed to the provider (see `attachments_sig`).
    pub attachments_sig: String,
    pub dirty: bool,
    pub state: String,
    pub attempts: i64,
    pub next_attempt_at: i64,
    pub last_error: Option<String>,
    /// One line for the user the next time they look at the draft (e.g. a conflict was split off).
    pub notice: Option<String>,
    pub updated_at: i64,
}

const COLS: &str = "local_id, account_id, remote_id, remote_version, kind, in_reply_to, payload_json, mode, \
    attachments_sig, dirty, state, attempts, next_attempt_at, last_error, notice, updated_at";

fn row_of(r: &rusqlite::Row) -> rusqlite::Result<DraftRow> {
    Ok(DraftRow {
        local_id: r.get(0)?,
        account_id: r.get(1)?,
        remote_id: r.get(2)?,
        remote_version: r.get(3)?,
        kind: r.get(4)?,
        in_reply_to: r.get(5)?,
        payload_json: r.get(6)?,
        mode: r.get(7)?,
        attachments_sig: r.get(8)?,
        dirty: r.get::<_, i64>(9)? != 0,
        state: r.get(10)?,
        attempts: r.get(11)?,
        next_attempt_at: r.get(12)?,
        last_error: r.get(13)?,
        notice: r.get(14)?,
        updated_at: r.get(15)?,
    })
}

/// Signature of an attachment set: what the provider's copy must match. Built from the spool
/// path, size and original name of each file, in order.
pub fn attachments_sig(files: &[crate::compose::OutgoingAttachment]) -> String {
    files
        .iter()
        .map(|a| format!("{}|{}|{}", a.path, a.size, a.display_name()))
        .collect::<Vec<_>>()
        .join("\n")
}

impl Database {
    /// Create or replace a draft's content. Every call marks it dirty and makes it due at once.
    /// `state` is `editing` for autosaves and `saved` for explicit saves / close / quit.
    pub fn draft_save(&self, local_id: &str, kind: &str, in_reply_to: &str, payload_json: &str, mode: &str, state: &str) -> Result<()> {
        let now = Self::now_ms();
        self.conn.execute(
            "INSERT INTO drafts (local_id, account_id, kind, in_reply_to, payload_json, mode, dirty, state, next_attempt_at, updated_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, 0, ?8, ?8)
             ON CONFLICT(local_id) DO UPDATE SET
                kind = excluded.kind, in_reply_to = excluded.in_reply_to, payload_json = excluded.payload_json,
                mode = excluded.mode, dirty = 1, state = excluded.state, next_attempt_at = 0,
                attempts = 0, updated_at = excluded.updated_at",
            params![local_id, self.account_id, kind, in_reply_to, payload_json, mode, state, now],
        )?;
        Ok(())
    }

    pub fn draft_get(&self, local_id: &str) -> Result<Option<DraftRow>> {
        Ok(self.conn.query_row(&format!("SELECT {} FROM drafts WHERE local_id = ?1", COLS), params![local_id], row_of).optional()?)
    }

    /// Drafts of an account that are not sent, newest first.
    pub fn draft_list(&self, account_id: &str) -> Result<Vec<DraftRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {} FROM drafts WHERE account_id = ?1 AND state != 'sent' ORDER BY updated_at DESC",
            COLS
        ))?;
        let rows = st.query_map(params![account_id], row_of)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Dirty drafts of an account that are due now, oldest first. An autosaved draft (`editing`)
    /// waits until `debounce_ms` has passed since its last change; a saved one goes at once.
    pub fn draft_due(&self, account_id: &str, now_ms: i64, debounce_ms: i64) -> Result<Vec<DraftRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {} FROM drafts WHERE account_id = ?1 AND dirty = 1 AND state != 'sent' AND next_attempt_at <= ?2
               AND (state = 'saved' OR updated_at <= ?3)
             ORDER BY updated_at ASC",
            COLS
        ))?;
        let rows = st
            .query_map(params![account_id, now_ms, now_ms - debounce_ms], row_of)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// When the next dirty draft of an account becomes due (None: nothing waits). Parked drafts
    /// do not count.
    pub fn draft_next_due(&self, account_id: &str, debounce_ms: i64) -> Result<Option<i64>> {
        let mut st = self.conn.prepare(
            "SELECT next_attempt_at, updated_at, state FROM drafts WHERE account_id = ?1 AND dirty = 1 AND state != 'sent' AND next_attempt_at < ?2",
        )?;
        let rows = st.query_map(params![account_id, PARKED], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))?;
        let mut best: Option<i64> = None;
        for row in rows {
            let (at, updated, state) = row?;
            let due = if state == "saved" { at } else { at.max(updated + debounce_ms) };
            best = Some(best.map_or(due, |b| b.min(due)));
        }
        Ok(best)
    }

    /// Drafts left in `editing` with unsaved changes: what a crash left behind.
    pub fn draft_recovery_candidates(&self, account_id: &str) -> Result<Vec<DraftRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {} FROM drafts WHERE account_id = ?1 AND state = 'editing' AND dirty = 1 ORDER BY updated_at DESC",
            COLS
        ))?;
        let rows = st.query_map(params![account_id], row_of)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The local draft that already stands for this provider draft, if any (Drafts-folder dedupe).
    pub fn draft_local_for_remote(&self, account_id: &str, remote_id: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row(
            "SELECT local_id FROM drafts WHERE account_id = ?1 AND remote_id = ?2 AND state != 'sent'",
            params![account_id, remote_id],
            |r| r.get(0),
        ).optional()?)
    }

    /// Provider ids of this account's live local drafts (hidden from the synced Drafts list).
    pub fn draft_remote_ids(&self, account_id: &str) -> Result<Vec<String>> {
        let mut st = self.conn.prepare("SELECT remote_id FROM drafts WHERE account_id = ?1 AND remote_id IS NOT NULL AND state != 'sent'")?;
        let rows = st.query_map(params![account_id], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The provider accepted the draft. `read_at` is the `updated_at` the push started from:
    /// if the user saved again meanwhile the row stays dirty and is pushed once more.
    pub fn draft_mark_pushed(&self, local_id: &str, read_at: i64, remote_id: &str, remote_version: Option<&str>, sig: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE drafts SET remote_id = ?2, remote_version = ?3, attachments_sig = ?4, last_error = NULL,
                attempts = 0, next_attempt_at = 0, dirty = CASE WHEN updated_at = ?5 THEN 0 ELSE dirty END
             WHERE local_id = ?1",
            params![local_id, remote_id, remote_version, sig, read_at],
        )?;
        Ok(n == 1)
    }

    /// Record a failed push. `retry_at` = `PARKED` for a failure that retrying will not fix; the
    /// draft then stays dirty (and local) until the user saves it again.
    pub fn draft_mark_failed(&self, local_id: &str, error: &str, retry_at: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE drafts SET attempts = attempts + 1, last_error = ?2, next_attempt_at = ?3 WHERE local_id = ?1",
            params![local_id, error, retry_at],
        )?;
        Ok(())
    }

    pub fn draft_set_notice(&self, local_id: &str, notice: Option<&str>) -> Result<()> {
        self.conn.execute("UPDATE drafts SET notice = ?2 WHERE local_id = ?1", params![local_id, notice])?;
        Ok(())
    }

    /// Re-point a draft at a new provider copy (conflict path: ours became a new draft).
    pub fn draft_set_remote(&self, local_id: &str, remote_id: &str, remote_version: Option<&str>, sig: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE drafts SET remote_id = ?2, remote_version = ?3, attachments_sig = ?4 WHERE local_id = ?1",
            params![local_id, remote_id, remote_version, sig],
        )?;
        Ok(())
    }

    /// The draft was sent: state `sent`, clean. `draft_prune_sent` then removes the row.
    pub fn draft_mark_sent(&self, local_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE drafts SET state = 'sent', dirty = 0, last_error = NULL, updated_at = ?2 WHERE local_id = ?1",
            params![local_id, Self::now_ms()],
        )?;
        Ok(())
    }

    /// Remove a draft's row. Callers call this only after the provider's copy is gone (or was never made).
    pub fn draft_delete(&self, local_id: &str) -> Result<bool> {
        Ok(self.conn.execute("DELETE FROM drafts WHERE local_id = ?1", params![local_id])? == 1)
    }

    /// Ids of every live draft of every account. The spool root is shared by accounts, so the
    /// sweep must keep them all.
    pub fn draft_all_live_ids(&self) -> Result<Vec<String>> {
        let mut st = self.conn.prepare("SELECT local_id FROM drafts WHERE state != 'sent'")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn draft_prune_sent(&self) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM drafts WHERE state = 'sent'", [])?)
    }
}

#[cfg(test)]
mod draft_tests {
    use super::*;

    fn db() -> Database {
        Database::open_for_account(":memory:", "a1").unwrap()
    }

    #[test]
    fn save_is_dirty_and_due_then_pushed_clears_dirty() {
        let db = db();
        db.draft_save("d1", "new", "", "{\"subject\":\"x\"}", "system", "editing").unwrap();
        let row = db.draft_get("d1").unwrap().unwrap();
        assert!(row.dirty);
        assert_eq!(row.state, "editing");
        assert_eq!(row.remote_id, None);
        assert_eq!(db.draft_due("a1", i64::MAX - 1, 0).unwrap().len(), 1);
        db.draft_mark_pushed("d1", row.updated_at, "AAMk-1", Some("ck1"), "sig").unwrap();
        let row = db.draft_get("d1").unwrap().unwrap();
        assert!(!row.dirty, "nothing changed during the push: clean");
        assert_eq!(row.remote_id.as_deref(), Some("AAMk-1"));
        assert_eq!(row.remote_version.as_deref(), Some("ck1"));
        assert!(db.draft_due("a1", i64::MAX - 1, 0).unwrap().is_empty());
    }

    #[test]
    fn edit_during_push_keeps_dirty() {
        let db = db();
        db.draft_save("d1", "new", "", "{}", "system", "editing").unwrap();
        let read_at = db.draft_get("d1").unwrap().unwrap().updated_at;
        // The user types again while the request is in flight: updated_at moves on.
        std::thread::sleep(std::time::Duration::from_millis(3));
        db.draft_save("d1", "new", "", "{\"more\":1}", "system", "editing").unwrap();
        db.draft_mark_pushed("d1", read_at, "AAMk-1", None, "sig").unwrap();
        assert!(db.draft_get("d1").unwrap().unwrap().dirty, "the newer text still has to go up");
    }

    #[test]
    fn failure_backoff_and_parking() {
        let db = db();
        db.draft_save("d1", "new", "", "{}", "system", "saved").unwrap();
        db.draft_mark_failed("d1", "offline", 5_000).unwrap();
        assert!(db.draft_due("a1", 4_999, 0).unwrap().is_empty());
        assert_eq!(db.draft_next_due("a1", 0).unwrap(), Some(5_000));
        assert_eq!(db.draft_due("a1", 5_000, 0).unwrap().len(), 1);
        db.draft_mark_failed("d1", "400 bad", PARKED).unwrap();
        assert_eq!(db.draft_next_due("a1", 0).unwrap(), None, "parked drafts do not keep the worker awake");
        assert!(db.draft_get("d1").unwrap().unwrap().dirty, "a 4xx keeps the local copy dirty");
        // A new save un-parks it.
        db.draft_save("d1", "new", "", "{}", "system", "saved").unwrap();
        assert_eq!(db.draft_due("a1", 0, 0).unwrap().len(), 1);
    }

    #[test]
    fn crash_recovery_lists_editing_dirty_only() {
        let db = db();
        db.draft_save("typing", "new", "", "{}", "system", "editing").unwrap();
        db.draft_save("closed", "new", "", "{}", "system", "saved").unwrap();
        let rec = db.draft_recovery_candidates("a1").unwrap();
        assert_eq!(rec.iter().map(|r| r.local_id.as_str()).collect::<Vec<_>>(), vec!["typing"]);
        let r = db.draft_get("closed").unwrap().unwrap();
        db.draft_mark_pushed("closed", r.updated_at, "R1", None, "").unwrap();
        db.draft_save("typing", "new", "", "{}", "system", "saved").unwrap();
        assert!(db.draft_recovery_candidates("a1").unwrap().is_empty());
    }

    #[test]
    fn remote_dedupe_and_sent_lifecycle() {
        let db = db();
        db.draft_save("d1", "new", "", "{}", "system", "saved").unwrap();
        let r = db.draft_get("d1").unwrap().unwrap();
        db.draft_mark_pushed("d1", r.updated_at, "R1", None, "").unwrap();
        assert_eq!(db.draft_local_for_remote("a1", "R1").unwrap().as_deref(), Some("d1"));
        assert_eq!(db.draft_remote_ids("a1").unwrap(), vec!["R1".to_string()]);
        db.draft_mark_sent("d1").unwrap();
        assert!(db.draft_list("a1").unwrap().is_empty(), "sent drafts are not listed");
        assert_eq!(db.draft_local_for_remote("a1", "R1").unwrap(), None);
        assert_eq!(db.draft_prune_sent().unwrap(), 1);
        assert!(db.draft_get("d1").unwrap().is_none());
    }

    #[test]
    fn drafts_are_per_account() {
        let db = db();
        db.draft_save("d1", "new", "", "{}", "system", "saved").unwrap();
        assert!(db.draft_list("other").unwrap().is_empty());
        assert_eq!(db.draft_list("a1").unwrap().len(), 1);
        assert!(db.draft_delete("d1").unwrap());
        assert!(!db.draft_delete("d1").unwrap());
    }
}
