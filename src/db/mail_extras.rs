//! Small per-message / per-account caches for the compose extras (OL-051 identities, OL-053 list headers).

use super::Database;
use crate::errors::Result;
use crate::identities::Identity;
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
        // Send-as identities per account (OL-051), replaced as a whole on every refresh; the user's own default per account.
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS identities (
                account_id TEXT NOT NULL,
                email TEXT NOT NULL,
                name TEXT NOT NULL DEFAULT '',
                is_primary INTEGER NOT NULL DEFAULT 0,
                is_default INTEGER NOT NULL DEFAULT 0,
                fetched_at INTEGER NOT NULL,
                PRIMARY KEY (account_id, email)
            )",
            [],
        )?;
        self.conn.execute("CREATE TABLE IF NOT EXISTS identity_pref (account_id TEXT PRIMARY KEY, email TEXT NOT NULL)", [])?;
        Ok(())
    }

    /// The cached identities of an account and when (unix seconds) they were read; None when never read.
    pub fn identities_get(&self, account_id: &str) -> Result<Option<(Vec<Identity>, i64)>> {
        let mut st = self.conn.prepare("SELECT email, name, is_primary, is_default, fetched_at FROM identities WHERE account_id = ?1 ORDER BY is_primary DESC, email")?;
        let rows = st.query_map(params![account_id], |r| {
            Ok((Identity { email: r.get(0)?, name: r.get(1)?, primary: r.get::<_, i64>(2)? != 0, is_default: r.get::<_, i64>(3)? != 0 }, r.get::<_, i64>(4)?))
        })?.collect::<std::result::Result<Vec<_>, _>>()?;
        let at = rows.iter().map(|(_, t)| *t).min();
        Ok(at.map(|t| (rows.into_iter().map(|(i, _)| i).collect(), t)))
    }

    pub fn identities_replace(&self, account_id: &str, list: &[Identity]) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        self.conn.execute("DELETE FROM identities WHERE account_id = ?1", params![account_id])?;
        for i in list {
            self.conn.execute(
                "INSERT OR REPLACE INTO identities (account_id, email, name, is_primary, is_default, fetched_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![account_id, i.email, i.name, i.primary as i64, i.is_default as i64, now],
            )?;
        }
        Ok(())
    }

    pub fn identity_pref_get(&self, account_id: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT email FROM identity_pref WHERE account_id = ?1", params![account_id], |r| r.get::<_, String>(0)).optional()?)
    }

    /// Remember the user's default From address for an account (empty clears it).
    pub fn identity_pref_set(&self, account_id: &str, email: &str) -> Result<()> {
        if email.trim().is_empty() {
            self.conn.execute("DELETE FROM identity_pref WHERE account_id = ?1", params![account_id])?;
        } else {
            self.conn.execute("INSERT INTO identity_pref (account_id, email) VALUES (?1, ?2) ON CONFLICT(account_id) DO UPDATE SET email = excluded.email", params![account_id, email.trim()])?;
        }
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

    #[test]
    fn identities_are_replaced_per_account_and_the_default_is_remembered() {
        let db = Database::open(":memory:").unwrap();
        assert_eq!(db.identities_get("a").unwrap(), None);
        let l = vec![
            Identity { email: "me@x.org".into(), name: "Me".into(), primary: true, is_default: false },
            Identity { email: "al@x.org".into(), name: "".into(), primary: false, is_default: true },
        ];
        db.identities_replace("a", &l).unwrap();
        db.identities_replace("b", &l[..1]).unwrap();
        let (got, at) = db.identities_get("a").unwrap().unwrap();
        assert_eq!(got, l);
        assert!(at > 0);
        db.identities_replace("a", &l[..1]).unwrap();
        assert_eq!(db.identities_get("a").unwrap().unwrap().0.len(), 1);
        assert_eq!(db.identities_get("b").unwrap().unwrap().0.len(), 1);
        assert_eq!(db.identity_pref_get("a").unwrap(), None);
        db.identity_pref_set("a", "al@x.org").unwrap();
        db.identity_pref_set("a", "me@x.org").unwrap();
        assert_eq!(db.identity_pref_get("a").unwrap().as_deref(), Some("me@x.org"));
        db.identity_pref_set("a", "").unwrap();
        assert_eq!(db.identity_pref_get("a").unwrap(), None);
    }
}
