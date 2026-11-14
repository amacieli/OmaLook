//! Mail outbox: queued sends, claim/retry/undo, and pruning.

use super::*;

impl Database {

    // ── outbox ───────────────────────────────────────────────────────────────────────────────

    /// Run an `ALTER TABLE ... ADD COLUMN`. Every sync thread opens the database at launch, and the
    /// first launch after an upgrade has several of them migrating at once: each checks that the
    /// column is missing, then one wins and the rest hit "duplicate column name". That is success
    /// (the column is there), not an error: treating it as one made `Database::open_for_account` fail
    /// and the account's mail / calendar / outbox workers quit without a trace.
    pub(crate) fn add_column(&self, sql: &str) -> Result<()> {
        match self.conn.execute(sql, []) {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(_, Some(msg))) if msg.contains("duplicate column name") => Ok(()),
            Err(e) => Err(e.into()),
        }
    }


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


    /// Ids of every message (any account) that may still need its attachment spool: queued or
    /// being sent. Everything else in the spool directory is an orphan.
    pub fn outbox_live_ids(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT id FROM outbox WHERE state IN ('pending', 'sending')")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
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
}
