//! Small per-message / per-account caches for the compose extras (OL-051 identities, OL-053 list headers).

use super::Database;
use crate::errors::Result;
use rusqlite::{params, OptionalExtension};

impl Database {
    pub(crate) fn init_mail_extras_schema(&self) -> Result<()> {
        // One row per message the user asked about; the answer never changes, so it is kept for good.
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS list_info (
                message_id TEXT PRIMARY KEY,
                info TEXT NOT NULL,
                fetched_at INTEGER NOT NULL
            )",
            [],
        )?;
        Ok(())
    }

    /// Cached list lookup of a message (JSON of `ListInfo`).
    pub fn list_info_get(&self, message_id: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT info FROM list_info WHERE message_id = ?1", params![message_id], |r| r.get::<_, String>(0)).optional()?)
    }

    pub fn list_info_set(&self, message_id: &str, info_json: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO list_info (message_id, info, fetched_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(message_id) DO UPDATE SET info = excluded.info, fetched_at = excluded.fetched_at",
            params![message_id, info_json, chrono::Utc::now().timestamp()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_info_is_cached_per_message() {
        let db = Database::open(":memory:").unwrap();
        assert_eq!(db.list_info_get("m1").unwrap(), None);
        db.list_info_set("m1", "{\"is_list\":true}").unwrap();
        db.list_info_set("m1", "{\"is_list\":false}").unwrap();
        assert_eq!(db.list_info_get("m1").unwrap().as_deref(), Some("{\"is_list\":false}"));
        assert_eq!(db.list_info_get("m2").unwrap(), None);
    }
}
