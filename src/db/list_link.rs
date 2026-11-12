//! Storage for the two-way list link (docs/PLAN-contacts.md section 7): what was in sync last time
//! (the base of the three-way merge), a short log, and the contact queries the merge needs.

use super::Database;
use crate::errors::Result;
use rusqlite::params;
use serde_json::Value;
use std::collections::BTreeSet;

/// A contact as the list link needs to see it.
#[derive(Debug, Clone)]
pub struct LinkContact {
    pub id: String,
    pub name: String,
    pub emails: Vec<String>,
    pub categories: Vec<String>,
    pub state: String,
}

impl LinkContact {
    /// The address a contact stands for in a list: its first one, lower case.
    pub fn address(&self) -> Option<String> {
        self.emails.first().map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty())
    }
    pub fn has_category(&self, cat: &str) -> bool {
        self.categories.iter().any(|c| c.trim().eq_ignore_ascii_case(cat))
    }
}

impl Database {
    pub(crate) fn init_list_link_schema(&self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS dl_sync_base (
                list_id INTEGER NOT NULL REFERENCES dl_lists(id) ON DELETE CASCADE,
                account_id TEXT NOT NULL,
                category TEXT NOT NULL,
                members TEXT NOT NULL,
                synced_at INTEGER NOT NULL,
                PRIMARY KEY (list_id, account_id)
            )",
            [],
        )?;
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS dl_sync_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                at INTEGER NOT NULL,
                account_id TEXT NOT NULL,
                list_name TEXT NOT NULL,
                text TEXT NOT NULL
            )",
            [],
        )?;
        self.conn.execute("CREATE TABLE IF NOT EXISTS dl_sync_ignored (category TEXT PRIMARY KEY)", [])?;
        Ok(())
    }

    /// Category names (lower case) that must not be adopted as lists: those of deleted lists and the
    /// previous names of renamed ones.
    pub fn list_link_not_adoptable(&self) -> Result<BTreeSet<String>> {
        let mut out = BTreeSet::new();
        let mut st = self.conn.prepare("SELECT category FROM dl_sync_ignored")?;
        for r in st.query_map([], |r| r.get::<_, String>(0))? { out.insert(r?.to_lowercase()); }
        let mut st = self.conn.prepare("SELECT category FROM dl_sync_base")?;
        for r in st.query_map([], |r| r.get::<_, String>(0))? { out.insert(r?.to_lowercase()); }
        Ok(out)
    }

    pub fn list_link_ignore(&self, category: &str, on: bool) -> Result<()> {
        if on {
            self.conn.execute("INSERT OR IGNORE INTO dl_sync_ignored (category) VALUES (?1)", params![category.trim().to_lowercase()])?;
        } else {
            self.conn.execute("DELETE FROM dl_sync_ignored WHERE category = ?1", params![category.trim().to_lowercase()])?;
        }
        Ok(())
    }

    /// `(category name last used, members at the last sync)`, or None before the first sync.
    pub fn list_link_base(&self, list_id: i64, account_id: &str) -> Result<Option<(String, BTreeSet<String>)>> {
        match self.conn.query_row(
            "SELECT category, members FROM dl_sync_base WHERE list_id = ?1 AND account_id = ?2",
            params![list_id, account_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        ) {
            Ok((cat, m)) => Ok(Some((cat, serde_json::from_str::<Vec<String>>(&m).unwrap_or_default().into_iter().collect()))),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn set_list_link_base(&self, list_id: i64, account_id: &str, category: &str, members: &BTreeSet<String>) -> Result<()> {
        let json = serde_json::to_string(&members.iter().collect::<Vec<_>>()).unwrap_or_else(|_| "[]".into());
        self.conn.execute(
            "INSERT INTO dl_sync_base (list_id, account_id, category, members, synced_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(list_id, account_id) DO UPDATE SET category = excluded.category, members = excluded.members, synced_at = excluded.synced_at",
            params![list_id, account_id, category, json, Self::now_ms()],
        )?;
        Ok(())
    }

    pub fn clear_list_link_base(&self, list_id: i64, account_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM dl_sync_base WHERE list_id = ?1 AND account_id = ?2", params![list_id, account_id])?;
        Ok(())
    }

    pub fn list_link_log_add(&self, account_id: &str, list_name: &str, text: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO dl_sync_log (at, account_id, list_name, text) VALUES (?1, ?2, ?3, ?4)",
            params![Self::now_ms(), account_id, list_name, text],
        )?;
        self.conn.execute("DELETE FROM dl_sync_log WHERE id <= (SELECT MAX(id) FROM dl_sync_log) - 300", [])?;
        Ok(())
    }

    pub fn list_link_log(&self, limit: usize) -> Result<Vec<Value>> {
        let mut st = self.conn.prepare("SELECT at, account_id, list_name, text FROM dl_sync_log ORDER BY id DESC LIMIT ?1")?;
        let rows = st
            .query_map(params![limit as i64], |r| {
                Ok(serde_json::json!({ "at": r.get::<_, i64>(0)?, "account_id": r.get::<_, String>(1)?, "list": r.get::<_, String>(2)?, "text": r.get::<_, String>(3)? }))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn link_contacts(&self, where_sql: &str, arg: &str) -> Result<Vec<LinkContact>> {
        let sql = format!(
            "SELECT id, display_name, emails, categories, sync_state FROM contacts WHERE account_id = ?1 AND sync_state <> 'deleted' AND {where_sql}"
        );
        let mut st = self.conn.prepare(&sql)?;
        let rows = st
            .query_map(params![self.account_id, arg], |r| {
                Ok(LinkContact {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    emails: serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or_default(),
                    categories: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default(),
                    state: r.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The account's contacts that carry the category (any case).
    pub fn contacts_with_category(&self, category: &str) -> Result<Vec<LinkContact>> {
        let like = format!("%{}%", category.to_lowercase().replace('%', "").replace('_', ""));
        Ok(self.link_contacts("lower(categories) LIKE ?2", &like)?.into_iter().filter(|c| c.has_category(category)).collect())
    }

    /// The account's contacts whose first address is this one.
    pub fn contacts_by_first_address(&self, address: &str) -> Result<Vec<LinkContact>> {
        let a = address.trim().to_lowercase();
        let like = format!("%{}%", a.replace('%', "").replace('_', ""));
        Ok(self.link_contacts("lower(emails) LIKE ?2", &like)?.into_iter().filter(|c| c.address().as_deref() == Some(a.as_str())).collect())
    }

    /// Category names starting with a prefix that appear on the account's contacts.
    pub fn categories_with_prefix(&self, prefix: &str) -> Result<BTreeSet<String>> {
        let like = format!("%{}%", prefix.to_lowercase());
        let mut out = BTreeSet::new();
        for c in self.link_contacts("lower(categories) LIKE ?2", &like)? {
            for cat in c.categories {
                if cat.to_lowercase().starts_with(&prefix.to_lowercase()) { out.insert(cat.trim().to_string()); }
            }
        }
        Ok(out)
    }

    /// Contact writes of this account that are not finished or need attention.
    pub fn contact_writes_open(&self) -> Result<usize> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM contact_outbox WHERE account_id = ?1 AND state IN ('pending','sending','failed','conflict')",
            params![self.account_id],
            |r| r.get::<_, i64>(0),
        )? as usize)
    }
}
