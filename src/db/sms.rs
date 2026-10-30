//! SMS cache (KDE Connect). Device-scoped, not account-scoped: one phone's messages
//! are mirrored here by `sms_daemon` and read by the `/sms/*` routes.
//!
//! * `sms_messages` — every message the phone has reported. Key = (device, mms, uid): the
//!   phone keeps SMS and MMS in separate tables whose ids can collide.
//! * `sms_threads` — per-thread bookkeeping for the full-history pull (`history_complete`).
//! * `sms_devices` — one row per phone: name and when its full pull finished.
//!
//! Read/unread: `is_read` is what omalook shows; `phone_read` is the phone's own flag.
//! KDE Connect has no "mark read" call, so omalook's state is local and wins: a sync
//! may turn a message read (the phone read it) but never turns a locally read one unread.

use super::Database;
use crate::errors::Result;
use rusqlite::params;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq)]
pub struct SmsAttachment {
    pub part_id: i64,
    pub mime: String,
    pub unique_id: String,
    /// Base64 PNG thumbnail the phone sends along (shown while the full picture is fetched).
    pub preview: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SmsMessage {
    pub device_id: String,
    pub uid: i64,
    pub mms: bool,
    pub thread_id: i64,
    pub body: String,
    pub addresses: Vec<String>,
    pub date_ms: i64,
    /// Android telephony type: 1 received, 2 sent, 3 draft, 4 outbox, 5 failed, 6 queued.
    pub direction: i32,
    pub event: i32,
    pub sub_id: i64,
    pub attachments: Vec<SmsAttachment>,
    pub phone_read: bool,
}

#[derive(Debug, Serialize)]
pub struct SmsMessageRow {
    pub device_id: String,
    pub uid: i64,
    pub mms: bool,
    pub thread_id: i64,
    pub body: String,
    pub addresses: Vec<String>,
    pub date_ms: i64,
    pub direction: i32,
    pub is_read: bool,
    pub attachments: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct SmsThreadRow {
    pub device_id: String,
    pub thread_id: i64,
    pub addresses: Vec<String>,
    pub last_date_ms: i64,
    pub last_body: String,
    pub last_direction: i32,
    pub message_count: i64,
    pub unread_count: i64,
    pub history_complete: bool,
}

fn json_list(s: &str) -> Vec<String> {
    serde_json::from_str(s).unwrap_or_default()
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// A comparable form of a recipient list: last ten digits of each number, sorted, de-duplicated.
pub fn address_keys(addresses: &[String]) -> Vec<String> {
    let mut v: Vec<String> = addresses.iter().map(|a| {
        let d: String = a.chars().filter(|c| c.is_ascii_digit()).collect();
        if d.len() >= 7 { d[d.len().saturating_sub(10)..].to_string() } else { a.trim().to_lowercase() }
    }).collect();
    v.sort();
    v.dedup();
    v
}

/// Stand-in thread id for a conversation the phone has not reported yet: negative, stable per recipient set.
pub fn pending_thread_id(addresses: &[String]) -> i64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for k in address_keys(addresses) { for b in k.bytes().chain([b'|']) { h = (h ^ b as u64).wrapping_mul(0x100000001b3); } }
    -(((h >> 24) & 0xff_ffff_ffff) as i64 + 1)
}

/// FTS5 MATCH expression for free text: each word must appear (as a word prefix). None for no words.
pub fn fts_query(q: &str) -> Option<String> {
    let words: Vec<String> = q.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty())
        .map(|w| format!("\"{}\"*", w)).collect();
    if words.is_empty() { None } else { Some(words.join(" ")) }
}

impl Database {
    pub(super) fn init_sms_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sms_messages (
                device_id   TEXT NOT NULL,
                mms         INTEGER NOT NULL DEFAULT 0,
                uid         INTEGER NOT NULL,
                thread_id   INTEGER NOT NULL,
                body        TEXT NOT NULL DEFAULT '',
                addresses   TEXT NOT NULL DEFAULT '[]',
                date_ms     INTEGER NOT NULL,
                direction   INTEGER NOT NULL DEFAULT 1,
                event       INTEGER NOT NULL DEFAULT 0,
                sub_id      INTEGER NOT NULL DEFAULT 0,
                attachments TEXT NOT NULL DEFAULT '[]',
                is_read     INTEGER NOT NULL DEFAULT 0,
                phone_read  INTEGER NOT NULL DEFAULT 0,
                cached_at   DATETIME DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (device_id, mms, uid)
            );
            CREATE INDEX IF NOT EXISTS idx_sms_thread ON sms_messages(device_id, thread_id, date_ms DESC);
            CREATE INDEX IF NOT EXISTS idx_sms_unread ON sms_messages(device_id, thread_id) WHERE is_read = 0;
            CREATE TABLE IF NOT EXISTS sms_threads (
                device_id        TEXT NOT NULL,
                thread_id        INTEGER NOT NULL,
                history_complete INTEGER NOT NULL DEFAULT 0,
                phone_count      INTEGER NOT NULL DEFAULT 0,
                updated_at       DATETIME DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (device_id, thread_id)
            );
            CREATE TABLE IF NOT EXISTS sms_devices (
                device_id      TEXT PRIMARY KEY,
                name           TEXT NOT NULL DEFAULT '',
                full_synced_at DATETIME,
                last_seen_at   DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            -- Full-text index over message bodies (external content: no second copy of the text).
            CREATE VIRTUAL TABLE IF NOT EXISTS sms_fts USING fts5(body, content='sms_messages', content_rowid='rowid');
            CREATE TRIGGER IF NOT EXISTS sms_fts_ai AFTER INSERT ON sms_messages BEGIN
                INSERT INTO sms_fts(rowid, body) VALUES (new.rowid, new.body);
            END;
            CREATE TRIGGER IF NOT EXISTS sms_fts_ad AFTER DELETE ON sms_messages BEGIN
                INSERT INTO sms_fts(sms_fts, rowid, body) VALUES ('delete', old.rowid, old.body);
            END;
            CREATE TRIGGER IF NOT EXISTS sms_fts_au AFTER UPDATE OF body ON sms_messages BEGIN
                INSERT INTO sms_fts(sms_fts, rowid, body) VALUES ('delete', old.rowid, old.body);
                INSERT INTO sms_fts(rowid, body) VALUES (new.rowid, new.body);
            END;",
        )?;
        // First run on an existing cache: index what is already there.
        let indexed: i64 = self.conn.query_row("SELECT COUNT(*) FROM sms_fts_docsize", [], |r| r.get(0)).unwrap_or(0);
        if indexed == 0 {
            let rows: i64 = self.conn.query_row("SELECT COUNT(*) FROM sms_messages", [], |r| r.get(0)).unwrap_or(0);
            if rows > 0 { self.conn.execute("INSERT INTO sms_fts(sms_fts) VALUES ('rebuild')", [])?; }
        }
        Ok(())
    }

    /// Insert or update one message. Returns true when the stored row changed (new message,
    /// edited body, or read state moved) so the caller only bumps the UI serial for real changes.
    pub fn sms_upsert(&self, m: &SmsMessage) -> Result<bool> {
        let atts = serde_json::Value::Array(
            m.attachments.iter().map(|a| serde_json::json!({
                "part_id": a.part_id, "mime": a.mime, "unique_id": a.unique_id, "preview": a.preview
            })).collect(),
        ).to_string();
        let addrs = serde_json::to_string(&m.addresses).unwrap_or_else(|_| "[]".into());
        let changed = self.conn.execute(
            "INSERT INTO sms_messages
               (device_id, mms, uid, thread_id, body, addresses, date_ms, direction, event, sub_id,
                attachments, is_read, phone_read)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?12)
             ON CONFLICT(device_id, mms, uid) DO UPDATE SET
                thread_id = excluded.thread_id, body = excluded.body, addresses = excluded.addresses,
                date_ms = excluded.date_ms, direction = excluded.direction, event = excluded.event,
                sub_id = excluded.sub_id, attachments = excluded.attachments,
                phone_read = excluded.phone_read,
                is_read = CASE WHEN phone_read IS NOT excluded.phone_read
                               THEN MAX(is_read, excluded.phone_read) ELSE is_read END
             WHERE thread_id IS NOT excluded.thread_id OR body IS NOT excluded.body
                OR date_ms IS NOT excluded.date_ms OR direction IS NOT excluded.direction
                OR attachments IS NOT excluded.attachments OR phone_read IS NOT excluded.phone_read",
            params![m.device_id, m.mms, m.uid, m.thread_id, m.body, addrs, m.date_ms, m.direction,
                    m.event, m.sub_id, atts, m.phone_read],
        )?;
        Ok(changed > 0)
    }

    /// Mark messages read/unread locally: the whole thread, or just one message (`uid`).
    /// Returns how many rows changed.
    pub fn sms_set_read(&self, device_id: Option<&str>, thread_id: i64, uid: Option<i64>, read: bool) -> Result<usize> {
        let n = self.conn.execute(
            "UPDATE sms_messages SET is_read = ?1
             WHERE thread_id = ?2 AND (?3 IS NULL OR device_id = ?3) AND (?4 IS NULL OR uid = ?4) AND is_read != ?1",
            params![read, thread_id, device_id, uid],
        )?;
        Ok(n)
    }

    pub fn sms_unread_totals(&self) -> Result<(i64, i64)> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*), COUNT(DISTINCT device_id || ':' || thread_id) FROM sms_messages
             WHERE is_read = 0 AND direction = 1",
            [], |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }

    /// Thread list, newest first. Unread counts only received messages.
    pub fn sms_threads(&self, device_id: Option<&str>, limit: i64) -> Result<Vec<SmsThreadRow>> {
        let mut st = self.conn.prepare(
            "SELECT m.device_id, m.thread_id, m.addresses, m.date_ms, m.body, m.direction,
                    (SELECT COUNT(*) FROM sms_messages x WHERE x.device_id = m.device_id AND x.thread_id = m.thread_id),
                    (SELECT COUNT(*) FROM sms_messages x WHERE x.device_id = m.device_id AND x.thread_id = m.thread_id
                        AND x.is_read = 0 AND x.direction = 1),
                    COALESCE((SELECT history_complete FROM sms_threads t
                              WHERE t.device_id = m.device_id AND t.thread_id = m.thread_id), 0)
             FROM sms_messages m
             WHERE (?1 IS NULL OR m.device_id = ?1)
               AND m.date_ms = (SELECT MAX(date_ms) FROM sms_messages y
                                WHERE y.device_id = m.device_id AND y.thread_id = m.thread_id)
             GROUP BY m.device_id, m.thread_id
             ORDER BY m.date_ms DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![device_id, limit], |r| {
            Ok(SmsThreadRow {
                device_id: r.get(0)?, thread_id: r.get(1)?, addresses: json_list(&r.get::<_, String>(2)?),
                last_date_ms: r.get(3)?, last_body: r.get(4)?, last_direction: r.get(5)?,
                message_count: r.get(6)?, unread_count: r.get(7)?, history_complete: r.get::<_, i64>(8)? != 0,
            })
        })?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Messages of one thread, newest first; `before_ms` pages backwards.
    pub fn sms_messages(&self, device_id: Option<&str>, thread_id: i64, before_ms: Option<i64>, limit: i64) -> Result<Vec<SmsMessageRow>> {
        let mut st = self.conn.prepare(
            "SELECT device_id, uid, mms, thread_id, body, addresses, date_ms, direction, is_read, attachments
             FROM sms_messages
             WHERE thread_id = ?1 AND (?2 IS NULL OR device_id = ?2) AND (?3 IS NULL OR date_ms < ?3)
             ORDER BY date_ms DESC, uid DESC LIMIT ?4",
        )?;
        let rows = st.query_map(params![thread_id, device_id, before_ms, limit], |r| {
            Ok(SmsMessageRow {
                device_id: r.get(0)?, uid: r.get(1)?, mms: r.get::<_, i64>(2)? != 0, thread_id: r.get(3)?,
                body: r.get(4)?, addresses: json_list(&r.get::<_, String>(5)?), date_ms: r.get(6)?,
                direction: r.get(7)?, is_read: r.get::<_, i64>(8)? != 0,
                attachments: serde_json::from_str(&r.get::<_, String>(9)?).unwrap_or(serde_json::json!([])),
            })
        })?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Mark a thread unread the way mail does: only its newest received message turns unread
    /// (one blue bar, a badge of 1), not the whole history. Returns rows changed.
    pub fn sms_mark_thread_unread(&self, device_id: Option<&str>, thread_id: i64) -> Result<usize> {
        let n = self.conn.execute(
            "UPDATE sms_messages SET is_read = 0
             WHERE rowid = (SELECT rowid FROM sms_messages
                            WHERE thread_id = ?1 AND (?2 IS NULL OR device_id = ?2) AND direction = 1
                            ORDER BY date_ms DESC, uid DESC LIMIT 1)
               AND is_read = 1",
            params![thread_id, device_id],
        )?;
        Ok(n)
    }

    /// Newest message per thread whose body matches `q` (every word, word-prefix): the hit's
    /// (device, thread, body). Newest threads first.
    pub fn sms_search_bodies(&self, device_id: Option<&str>, q: &str, limit: i64) -> Result<Vec<(String, i64, String)>> {
        let Some(fts) = fts_query(q) else { return Ok(vec![]) };
        let mut st = self.conn.prepare(
            "SELECT m.device_id, m.thread_id, m.body FROM sms_fts f JOIN sms_messages m ON m.rowid = f.rowid
             WHERE sms_fts MATCH ?1 AND (?2 IS NULL OR m.device_id = ?2) AND m.uid > 0
             ORDER BY m.date_ms DESC LIMIT ?3")?;
        let rows = st.query_map(params![fts, device_id, limit], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut seen = std::collections::HashSet::new();
        Ok(rows.into_iter().filter(|(d, t, _)| seen.insert((d.clone(), *t))).collect())
    }

    // ── Outgoing texts ────────────────────────────────────────────────────────
    // KDE Connect only accepts a send and (10–60 s later) echoes the phone's stored copy as a
    // normal sent message. Until then the text lives here as a *pending* row: negative uid,
    // direction 6 (queued), or 5 (failed). The echo replaces it (`sms_reconcile_pending`).

    /// Store a just-sent text as pending. `thread_id` is the real thread, or a stand-in for a
    /// brand-new conversation (`pending_thread_id`). Returns the pending row's uid.
    pub fn sms_add_pending(&self, device_id: &str, thread_id: i64, addresses: &[String], body: &str, failed: bool) -> Result<i64> {
        use std::sync::atomic::{AtomicI64, Ordering};
        static SEQ: AtomicI64 = AtomicI64::new(0);
        let now = now_ms();
        let uid = -(now * 1000 + SEQ.fetch_add(1, Ordering::SeqCst) % 1000);
        let addrs = serde_json::to_string(addresses).unwrap_or_else(|_| "[]".into());
        self.conn.execute(
            "INSERT INTO sms_messages (device_id, mms, uid, thread_id, body, addresses, date_ms, direction, is_read, phone_read)
             VALUES (?1, 0, ?2, ?3, ?4, ?5, ?6, ?7, 1, 1)",
            params![device_id, uid, thread_id, body, addrs, now, if failed { 5 } else { 6 }])?;
        Ok(uid)
    }

    /// A sent text arrived from the phone: drop the pending/failed row it answers.
    /// Matches on body, a time window, and the same thread (or, for a stand-in thread, the same
    /// recipients). Returns how many pending rows went away.
    pub fn sms_reconcile_pending(&self, m: &SmsMessage) -> Result<usize> {
        if m.direction != 2 || m.uid <= 0 { return Ok(0); }
        let mut st = self.conn.prepare(
            "SELECT uid, thread_id, addresses FROM sms_messages
             WHERE device_id = ?1 AND uid < 0 AND body = ?2 AND date_ms BETWEEN ?3 AND ?4 AND direction IN (5, 6)
             ORDER BY date_ms LIMIT 20")?;
        let cands = st.query_map(params![m.device_id, m.body, m.date_ms - 5 * 60_000, m.date_ms + 60_000],
                                 |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let want = address_keys(&m.addresses);
        for (uid, thread, addrs) in cands {
            let same = thread == m.thread_id || (thread < 0 && address_keys(&json_list(&addrs)) == want);
            if same {
                self.conn.execute("DELETE FROM sms_messages WHERE device_id = ?1 AND uid = ?2", params![m.device_id, uid])?;
                return Ok(1);   // one echo answers one send
            }
        }
        Ok(0)
    }

    /// Queued texts older than `timeout_ms` that never echoed back become failed. Returns rows changed.
    pub fn sms_expire_pending(&self, timeout_ms: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE sms_messages SET direction = 5 WHERE uid < 0 AND direction = 6 AND date_ms < ?1",
            params![now_ms() - timeout_ms])?)
    }

    /// A failed text's data, for a retry: (thread_id, addresses, body).
    pub fn sms_failed_row(&self, device_id: &str, uid: i64) -> Result<Option<(i64, Vec<String>, String)>> {
        let mut st = self.conn.prepare(
            "SELECT thread_id, addresses, body FROM sms_messages WHERE device_id = ?1 AND uid = ?2 AND uid < 0 AND direction = 5")?;
        let mut rows = st.query_map(params![device_id, uid], |r| Ok((r.get::<_, i64>(0)?, json_list(&r.get::<_, String>(1)?), r.get::<_, String>(2)?)))?;
        Ok(rows.next().transpose()?)
    }

    /// Uids of the failed pending texts in a thread, oldest first.
    pub fn sms_failed_in_thread(&self, device_id: &str, thread_id: i64) -> Result<Vec<i64>> {
        let mut st = self.conn.prepare(
            "SELECT uid FROM sms_messages WHERE device_id = ?1 AND thread_id = ?2 AND uid < 0 AND direction = 5 ORDER BY date_ms")?;
        let v = st.query_map(params![device_id, thread_id], |r| r.get(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(v)
    }

    pub fn sms_set_pending_state(&self, device_id: &str, uid: i64, failed: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE sms_messages SET direction = ?3, date_ms = CASE WHEN ?3 = 6 THEN ?4 ELSE date_ms END
             WHERE device_id = ?1 AND uid = ?2 AND uid < 0",
            params![device_id, uid, if failed { 5 } else { 6 }, now_ms()])?;
        Ok(())
    }

    pub fn sms_remove_pending(&self, device_id: &str, uid: i64) -> Result<()> {
        self.conn.execute("DELETE FROM sms_messages WHERE device_id = ?1 AND uid = ?2 AND uid < 0", params![device_id, uid])?;
        Ok(())
    }

    /// The real thread whose recipients are exactly `addresses` (compared on last-10-digit keys), if cached.
    pub fn sms_thread_for_addresses(&self, device_id: &str, addresses: &[String]) -> Result<Option<i64>> {
        let want = address_keys(addresses);
        let mut st = self.conn.prepare(
            "SELECT thread_id, addresses FROM sms_messages WHERE device_id = ?1 AND thread_id > 0
             GROUP BY thread_id ORDER BY MAX(date_ms) DESC")?;
        let rows = st.query_map(params![device_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().find(|(_, a)| address_keys(&json_list(a)) == want).map(|(t, _)| t))
    }

    /// The phone most recently seen with SMS (for a new conversation when none is named).
    pub fn sms_default_device(&self) -> Result<Option<String>> {
        let mut st = self.conn.prepare("SELECT device_id FROM sms_devices ORDER BY last_seen_at DESC LIMIT 1")?;
        let mut rows = st.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.next().transpose()?)
    }

    pub fn sms_message_count(&self, device_id: &str) -> Result<i64> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM sms_messages WHERE device_id = ?1", params![device_id], |r| r.get(0))?)
    }

    pub fn sms_thread_message_count(&self, device_id: &str, thread_id: i64) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM sms_messages WHERE device_id = ?1 AND thread_id = ?2",
            params![device_id, thread_id], |r| r.get(0))?)
    }

    pub fn sms_delete_thread(&self, device_id: &str, thread_id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM sms_messages WHERE device_id = ?1 AND thread_id = ?2", params![device_id, thread_id])?;
        self.conn.execute("DELETE FROM sms_threads WHERE device_id = ?1 AND thread_id = ?2", params![device_id, thread_id])?;
        Ok(())
    }

    /// Record that the phone finished sending a thread's history (`phone_count` messages).
    pub fn sms_thread_loaded(&self, device_id: &str, thread_id: i64, phone_count: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sms_threads (device_id, thread_id, history_complete, phone_count) VALUES (?1,?2,1,?3)
             ON CONFLICT(device_id, thread_id) DO UPDATE SET history_complete = 1, phone_count = ?3, updated_at = CURRENT_TIMESTAMP",
            params![device_id, thread_id, phone_count])?;
        Ok(())
    }

    pub fn sms_thread_is_complete(&self, device_id: &str, thread_id: i64) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT COALESCE((SELECT history_complete FROM sms_threads WHERE device_id = ?1 AND thread_id = ?2), 0)",
            params![device_id, thread_id], |r| r.get::<_, i64>(0))? != 0)
    }

    /// Every thread id cached for the device (cached latest-message rows count as known threads).
    pub fn sms_known_threads(&self, device_id: &str) -> Result<Vec<i64>> {
        let mut st = self.conn.prepare("SELECT DISTINCT thread_id FROM sms_messages WHERE device_id = ?1 AND thread_id > 0")?;
        let v = st.query_map(params![device_id], |r| r.get(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(v)
    }

    pub fn sms_touch_device(&self, device_id: &str, name: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sms_devices (device_id, name) VALUES (?1,?2)
             ON CONFLICT(device_id) DO UPDATE SET name = ?2, last_seen_at = CURRENT_TIMESTAMP",
            params![device_id, name])?;
        Ok(())
    }

    pub fn sms_mark_full_sync(&self, device_id: &str) -> Result<()> {
        self.conn.execute("UPDATE sms_devices SET full_synced_at = CURRENT_TIMESTAMP WHERE device_id = ?1", params![device_id])?;
        Ok(())
    }

    pub fn sms_full_synced(&self, device_id: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM sms_devices WHERE device_id = ?1 AND full_synced_at IS NOT NULL",
            params![device_id], |r| r.get::<_, i64>(0))? > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(uid: i64, thread: i64, dir: i32, read: bool, body: &str) -> SmsMessage {
        SmsMessage {
            device_id: "dev".into(), uid, mms: false, thread_id: thread, body: body.into(),
            addresses: vec!["+15550001".into()], date_ms: 1000 + uid, direction: dir, event: 1, sub_id: 1,
            attachments: vec![], phone_read: read,
        }
    }

    #[test]
    fn upsert_is_idempotent_and_reports_changes() {
        let db = Database::open(":memory:").unwrap();
        let m = msg(1, 10, 1, false, "hi");
        assert!(db.sms_upsert(&m).unwrap());
        assert!(!db.sms_upsert(&m).unwrap(), "identical re-send must not count as a change");
        assert_eq!(db.sms_message_count("dev").unwrap(), 1);
        assert!(db.sms_upsert(&msg(1, 10, 1, false, "edited")).unwrap());
    }

    #[test]
    fn sms_and_mms_with_same_uid_coexist() {
        let db = Database::open(":memory:").unwrap();
        let mut a = msg(5, 1, 1, false, "sms"); let mut b = msg(5, 1, 1, false, "mms");
        a.mms = false; b.mms = true;
        db.sms_upsert(&a).unwrap(); db.sms_upsert(&b).unwrap();
        assert_eq!(db.sms_message_count("dev").unwrap(), 2);
    }

    #[test]
    fn local_read_survives_a_stale_unread_from_the_phone() {
        let db = Database::open(":memory:").unwrap();
        db.sms_upsert(&msg(1, 10, 1, false, "hi")).unwrap();
        assert_eq!(db.sms_unread_totals().unwrap(), (1, 1));
        assert_eq!(db.sms_set_read(None, 10, None, true).unwrap(), 1);
        assert_eq!(db.sms_unread_totals().unwrap(), (0, 0));
        // phone re-reports the message still unread: local state wins
        assert!(!db.sms_upsert(&msg(1, 10, 1, false, "hi")).unwrap());
        assert_eq!(db.sms_unread_totals().unwrap(), (0, 0));
    }

    #[test]
    fn phone_reading_a_message_marks_it_read_here() {
        let db = Database::open(":memory:").unwrap();
        db.sms_upsert(&msg(1, 10, 1, false, "hi")).unwrap();
        assert!(db.sms_upsert(&msg(1, 10, 1, true, "hi")).unwrap());
        assert_eq!(db.sms_unread_totals().unwrap(), (0, 0));
    }

    #[test]
    fn thread_summary_uses_newest_message_and_counts_unread_received_only() {
        let db = Database::open(":memory:").unwrap();
        db.sms_upsert(&msg(1, 10, 1, false, "old in")).unwrap();
        db.sms_upsert(&msg(2, 10, 2, true, "newest out")).unwrap();
        db.sms_upsert(&msg(3, 11, 1, true, "other")).unwrap();
        let t = db.sms_threads(None, 50).unwrap();
        assert_eq!(t.len(), 2);
        let t10 = t.iter().find(|t| t.thread_id == 10).unwrap();
        assert_eq!((t10.last_body.as_str(), t10.message_count, t10.unread_count), ("newest out", 2, 1));
        let m = db.sms_messages(None, 10, None, 10).unwrap();
        assert_eq!(m.iter().map(|m| m.uid).collect::<Vec<_>>(), vec![2, 1]);
    }

    #[test]
    fn history_bookkeeping_and_delete() {
        let db = Database::open(":memory:").unwrap();
        db.sms_upsert(&msg(1, 10, 1, true, "x")).unwrap();
        assert!(!db.sms_thread_is_complete("dev", 10).unwrap());
        db.sms_thread_loaded("dev", 10, 1).unwrap();
        assert!(db.sms_thread_is_complete("dev", 10).unwrap());
        db.sms_delete_thread("dev", 10).unwrap();
        assert_eq!(db.sms_message_count("dev").unwrap(), 0);
        assert!(!db.sms_thread_is_complete("dev", 10).unwrap());
    }

    #[test]
    fn local_unread_survives_a_refresh_from_the_phone() {
        let db = Database::open(":memory:").unwrap();
        db.sms_upsert(&msg(1, 10, 1, true, "hi")).unwrap();
        assert_eq!(db.sms_mark_thread_unread(Some("dev"), 10).unwrap(), 1);
        // the phone re-reports the same (read) message: still unread here
        assert!(!db.sms_upsert(&msg(1, 10, 1, true, "hi")).unwrap());
        assert_eq!(db.sms_threads(None, 10).unwrap()[0].unread_count, 1);
        // but the phone going unread -> read later still wins
        db.sms_upsert(&msg(2, 10, 1, false, "yo")).unwrap();
        db.sms_upsert(&msg(2, 10, 1, true, "yo")).unwrap();
        let t = &db.sms_threads(None, 10).unwrap()[0];
        assert_eq!(t.unread_count, 1);  // uid 1 stays unread, uid 2 read by the phone
    }

    #[test]
    fn mark_unread_touches_only_the_newest_received_message() {
        let db = Database::open(":memory:").unwrap();
        for u in 1..=3 { db.sms_upsert(&msg(u, 10, 1, true, "x")).unwrap(); }
        db.sms_upsert(&msg(4, 10, 2, true, "mine")).unwrap();
        db.sms_mark_thread_unread(None, 10).unwrap();
        assert_eq!(db.sms_threads(None, 10).unwrap()[0].unread_count, 1);
        assert_eq!(db.sms_set_read(None, 10, None, true).unwrap(), 1);
    }

    #[test]
    fn full_text_search_finds_words_and_prefixes_and_follows_edits() {
        let db = Database::open(":memory:").unwrap();
        db.sms_upsert(&msg(1, 10, 1, false, "Dinner at the Melting Pot on Friday")).unwrap();
        db.sms_upsert(&msg(2, 11, 1, false, "see you friday")).unwrap();
        db.sms_upsert(&msg(3, 10, 1, false, "melting again?")).unwrap();
        let hits = db.sms_search_bodies(None, "melt", 50).unwrap();
        assert_eq!(hits.len(), 1);                      // one hit per thread, newest message
        assert_eq!(hits[0].2, "melting again?");
        assert_eq!(db.sms_search_bodies(None, "friday dinner", 50).unwrap().len(), 1);
        assert!(db.sms_search_bodies(None, "   ", 50).unwrap().is_empty());
        assert!(db.sms_search_bodies(None, "\"; DROP", 50).unwrap().is_empty());
        db.sms_upsert(&msg(2, 11, 1, false, "see you saturday")).unwrap();   // edited body
        assert!(db.sms_search_bodies(None, "friday", 50).unwrap().iter().all(|h| h.1 != 11));
        assert_eq!(db.sms_search_bodies(None, "saturday", 50).unwrap().len(), 1);
    }

    #[test]
    fn a_pending_text_is_replaced_by_the_phones_echo() {
        let db = Database::open(":memory:").unwrap();
        let uid = db.sms_add_pending("dev", 10, &["+155****0001".into()], "hello", false).unwrap();
        assert!(uid < 0);
        assert_eq!(db.sms_threads(None, 10).unwrap()[0].last_direction, 6);
        let mut echo = msg(50, 10, 2, true, "hello");
        echo.date_ms = now_ms() + 2000;
        assert_eq!(db.sms_reconcile_pending(&echo).unwrap(), 1);
        db.sms_upsert(&echo).unwrap();
        assert_eq!(db.sms_thread_message_count("dev", 10).unwrap(), 1);
        // a different body does not match
        db.sms_add_pending("dev", 10, &["+155****0001".into()], "other", false).unwrap();
        assert_eq!(db.sms_reconcile_pending(&echo).unwrap(), 0);
    }

    #[test]
    fn a_new_conversation_stand_in_thread_resolves_by_recipients() {
        let db = Database::open(":memory:").unwrap();
        let addrs = vec!["917-412-9408".to_string()];
        let tid = pending_thread_id(&addrs);
        assert!(tid < 0);
        assert_eq!(tid, pending_thread_id(&["(917) 412 9408".to_string()]));
        db.sms_add_pending("dev", tid, &addrs, "hi there", false).unwrap();
        assert!(db.sms_known_threads("dev").unwrap().is_empty());   // never asked of the phone
        let mut echo = msg(70, 269, 2, true, "hi there");
        echo.addresses = vec!["9174129408".into()];
        echo.date_ms = now_ms() + 3000;
        assert_eq!(db.sms_reconcile_pending(&echo).unwrap(), 1);
    }

    #[test]
    fn unanswered_sends_fail_after_the_timeout_and_can_be_retried() {
        let db = Database::open(":memory:").unwrap();
        let uid = db.sms_add_pending("dev", 10, &["+155****0001".into()], "late", false).unwrap();
        assert_eq!(db.sms_expire_pending(60_000).unwrap(), 0);
        assert_eq!(db.sms_expire_pending(-1).unwrap(), 1);
        assert_eq!(db.sms_threads(None, 10).unwrap()[0].last_direction, 5);
        assert_eq!(db.sms_failed_in_thread("dev", 10).unwrap(), vec![uid]);
        let (t, a, b) = db.sms_failed_row("dev", uid).unwrap().unwrap();
        assert_eq!((t, a.len(), b.as_str()), (10, 1, "late"));
        // a late echo still heals a failed row
        let mut echo = msg(80, 10, 2, true, "late");
        echo.date_ms = now_ms() + 1000;
        assert_eq!(db.sms_reconcile_pending(&echo).unwrap(), 1);
    }

    #[test]
    fn finds_an_existing_thread_by_recipients() {
        let db = Database::open(":memory:").unwrap();
        let mut m = msg(1, 269, 1, true, "x");
        m.addresses = vec!["9174129408".into()];
        db.sms_upsert(&m).unwrap();
        assert_eq!(db.sms_thread_for_addresses("dev", &["+1 917-412-9408".into()]).unwrap(), Some(269));
        assert_eq!(db.sms_thread_for_addresses("dev", &["5550001111".into()]).unwrap(), None);
    }
}
