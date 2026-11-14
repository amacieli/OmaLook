//! Calendar outbox: queued event writes (same claim / undo discipline as the mail outbox).

use super::*;

/// `(id, op, payload, transaction_id, attempt)`.
pub type CalOutboxDue = (String, String, String, String, i64);
/// `(state, last_error, result_id)`.
pub type CalOutboxStatus = (String, Option<String>, Option<String>);

impl Database {

    // ── calendar outbox (same claim / undo discipline as the mail outbox) ───────────────────────

    #[allow(clippy::too_many_arguments)] // flat column list of one outbox row; callers pass each field by name
    pub fn cal_outbox_enqueue(&self, id: &str, account_id: &str, op: &str, payload_json: &str, attendees: usize, tx: &str, due_ms: i64) -> Result<()> {
        let now = Self::now_ms();
        self.conn.execute(
            "INSERT INTO calendar_outbox (id, account_id, op, payload_json, state, attendees_count, attempts, due_at, transaction_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5, 0, ?6, ?7, ?8, ?8)",
            params![id, account_id, op, payload_json, attendees as i64, due_ms, tx, now],
        )?;
        Ok(())
    }


    /// Claim every due operation of the account: `(id, op, payload, transaction_id, attempt)`.
    pub fn cal_outbox_claim_due(&self, account_id: &str, now_ms: i64) -> Result<Vec<CalOutboxDue>> {
        let due: Vec<(String, String, String, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, op, payload_json, transaction_id FROM calendar_outbox
                 WHERE account_id = ?1 AND state = 'pending' AND due_at <= ?2 ORDER BY due_at, created_at",
            )?;
            let rows = stmt
                .query_map(params![account_id, now_ms], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        let mut claimed = Vec::new();
        for (id, op, payload, tx) in due {
            let n = self.conn.execute(
                "UPDATE calendar_outbox SET state = 'sending', attempts = attempts + 1, updated_at = ?2 WHERE id = ?1 AND state = 'pending'",
                params![id, now_ms],
            )?;
            if n == 1 {
                let attempts: i64 = self.conn.query_row("SELECT attempts FROM calendar_outbox WHERE id = ?1", params![id], |r| r.get(0))?;
                claimed.push((id, op, payload, tx, attempts));
            }
        }
        Ok(claimed)
    }


    pub fn cal_outbox_next_due(&self, account_id: &str) -> Result<Option<i64>> {
        match self.conn.query_row(
            "SELECT MIN(due_at) FROM calendar_outbox WHERE account_id = ?1 AND state = 'pending'",
            params![account_id],
            |r| r.get::<_, Option<i64>>(0),
        ) {
            Ok(v) => Ok(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }


    pub fn cal_outbox_mark_sent(&self, id: &str, result_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE calendar_outbox SET state = 'sent', last_error = NULL, result_id = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, result_id, Self::now_ms()],
        )?;
        Ok(())
    }


    pub fn cal_outbox_mark_retry(&self, id: &str, error: &str, retry_at_ms: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE calendar_outbox SET state = 'pending', last_error = ?2, due_at = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, error, retry_at_ms, Self::now_ms()],
        )?;
        Ok(())
    }


    pub fn cal_outbox_mark_failed(&self, id: &str, error: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE calendar_outbox SET state = 'failed', last_error = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, error, Self::now_ms()],
        )?;
        Ok(())
    }


    /// Undo: only possible while the operation is still `pending` (not yet claimed by the worker).
    /// A `failed` one can also be taken back (it never reached the server), which dismisses its mark.
    pub fn cal_outbox_cancel(&self, id: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE calendar_outbox SET state = 'cancelled', updated_at = ?2 WHERE id = ?1 AND state IN ('pending', 'failed')",
            params![id, Self::now_ms()],
        )?;
        Ok(n == 1)
    }


    /// Operations that are queued, running or failed (the editor shows them as pending marks):
    /// `(id, account_id, op, state, attendees_count, last_error, due_at, payload_json, result_id)`.
    #[allow(clippy::type_complexity)]
    pub fn cal_outbox_list(&self) -> Result<Vec<(String, String, String, String, i64, Option<String>, i64, String, Option<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, account_id, op, state, attendees_count, last_error, due_at, payload_json, result_id FROM calendar_outbox
             WHERE state IN ('pending', 'sending', 'failed') OR (state = 'sent' AND updated_at > ?1) ORDER BY created_at",
        )?;
        let rows = stmt
            .query_map(params![Self::now_ms() - 60_000], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }


    /// Queue "set the tags of this event" (OL-040 phase 6). A recurring event is tagged as a whole (decision Q8):
    /// an occurrence resolves to its series master. Returns (the id that will be written, whether it is a series).
    pub fn cal_queue_event_tags(&self, outbox_id: &str, event_id: &str, tags: &[String], due_ms: i64) -> std::result::Result<(String, bool), String> {
        use rusqlite::OptionalExtension;
        let row: Option<(String, Option<String>, String)> = self.conn
            .query_row("SELECT id, series_master_id, event_type FROM calendar_events WHERE id = ?1 AND account_id = ?2",
                       params![event_id, self.account_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional().map_err(|e| e.to_string())?;
        let Some((id, master, kind)) = row else { return Err("no such event in this account".into()) };
        let target = master.filter(|m| !m.is_empty()).unwrap_or(id);
        let series = kind == "seriesMaster" || kind == "occurrence" || kind == "exception";
        let pending: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM calendar_outbox WHERE account_id = ?1 AND op = 'set_tags' AND state IN ('pending','sending') AND payload_json LIKE ?2 ESCAPE '\\'",
            params![self.account_id, format!("%\"event_id\":{}%", crate::db::like_escape(&serde_json::to_string(&target).unwrap_or_default()))], |r| r.get(0)).map_err(|e| e.to_string())?;
        if pending > 0 { return Err("a tag change for this event is already waiting".into()) }
        let payload = serde_json::json!({ "event_id": target, "tags": tags, "series": series }).to_string();
        self.cal_outbox_enqueue(outbox_id, &self.account_id, "set_tags", &payload, 0, outbox_id, due_ms).map_err(|e| e.to_string())?;
        Ok((target, series))
    }


    /// After the provider confirmed new tags: the event and, for a series master, its cached occurrences.
    pub fn cal_set_event_tags_local(&self, event_id: &str, tags: &[String]) -> Result<usize> {
        let json = if tags.is_empty() { String::new() } else { serde_json::to_string(tags).unwrap_or_default() };
        Ok(self.conn.execute(
            "UPDATE calendar_events SET tags_json = ?3 WHERE account_id = ?1 AND (id = ?2 OR series_master_id = ?2)",
            params![self.account_id, event_id, json])?)
    }


    /// The cached tags of one event (series masters included, which the month query does not return).
    pub fn cal_event_tags(&self, event_id: &str) -> Vec<String> {
        self.conn.query_row("SELECT tags_json FROM calendar_events WHERE id = ?1 AND account_id = ?2", params![event_id, self.account_id], |r| r.get::<_, String>(0))
            .ok().and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default()
    }


    /// What an edit, cancel, decline or delete acts on (plan phase 3). `series` asks for the whole series: a
    /// member resolves to its master id. Without it the id is the event itself (one occurrence alone).
    /// A series master always changes as a whole, since it has no occurrence id of its own.
    pub fn cal_event_ref(&self, event_id: &str, series: bool) -> std::result::Result<Option<CalEventRef>, String> {
        let row = self.conn
            .query_row("SELECT id, series_master_id, event_type, subject, start_at, end_at, is_all_day, location, attendees_json
                        FROM calendar_events WHERE id = ?1 AND account_id = ?2",
                       params![event_id, self.account_id],
                       |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?,
                                r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?,
                                r.get::<_, bool>(6)?, r.get::<_, String>(7)?, r.get::<_, String>(8)?)))
            .optional().map_err(|e| e.to_string())?;
        let Some((id, master, kind, subject, start, end, all_day, location, attendees)) = row else { return Ok(None) };
        let member = kind == "seriesMaster" || kind == "occurrence" || kind == "exception";
        let whole = (series || kind == "seriesMaster") && member;
        let target = if whole { master.filter(|m| !m.is_empty()).unwrap_or(id) } else { id };
        let invitees = crate::calendar_write::invitee_count_json(&attendees);
        Ok(Some(CalEventRef { target, series: whole, subject, start, end, all_day, location, invitees }))
    }


    /// Queue one edit / cancel / decline / delete of an event (plan phase 3). The payload gets the resolved
    /// `target` and `series`, and at most one such change may wait per target (a second edit would race the first).
    pub fn cal_queue_event_change(&self, outbox_id: &str, op: &str, target: &str, payload: &serde_json::Value, attendees: usize, due_ms: i64) -> std::result::Result<(), String> {
        let pending: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM calendar_outbox WHERE account_id = ?1 AND op IN ('update','cancel','decline','delete') AND state IN ('pending','sending') AND payload_json LIKE ?2 ESCAPE '\\'",
            params![self.account_id, format!("%\"target\":{}%", crate::db::like_escape(&serde_json::to_string(target).unwrap_or_default()))], |r| r.get(0)).map_err(|e| e.to_string())?;
        if pending > 0 { return Err("a change to this event is already waiting".into()) }
        let mut p = payload.clone();
        p["target"] = serde_json::json!(target);
        // The version the user's change is based on (R3.6): the provider refuses the write (412) if the
        // event changed since. Empty (never synced with versions) = unconditional, as before.
        p["etag"] = serde_json::json!(self.event_etag(target));
        self.cal_outbox_enqueue(outbox_id, &self.account_id, op, &p.to_string(), attendees, outbox_id, due_ms).map_err(|e| e.to_string())
    }


    pub fn cal_outbox_status(&self, id: &str) -> Result<Option<CalOutboxStatus>> {
        match self.conn.query_row(
            "SELECT state, last_error, result_id FROM calendar_outbox WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ) {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }


    /// After a crash an operation left `sending` is reported ("check your calendar"), never re-sent blindly.
    pub fn cal_outbox_fail_interrupted(&self, account_id: &str) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE calendar_outbox SET state = 'failed', updated_at = ?2,
                 last_error = 'interrupted while saving: check your calendar before trying again'
             WHERE account_id = ?1 AND state = 'sending'",
            params![account_id, Self::now_ms()],
        )?)
    }


    pub fn cal_outbox_prune(&self, max_age_ms: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM calendar_outbox WHERE state IN ('sent', 'cancelled') AND updated_at < ?1",
            params![Self::now_ms() - max_age_ms],
        )?)
    }
}
