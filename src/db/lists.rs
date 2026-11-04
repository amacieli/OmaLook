//! Distribution lists: named groups of addresses kept by OmaLook itself (docs/PLAN-distribution-lists.md).
//!
//! A list is not tied to an account: any From account can send to it. Members are stored as
//! addresses (with an optional display name), not as contact ids, so a list works for people
//! who are in no address book. Lists are expanded into addresses when a message is queued
//! (`compose::expand_lists`), never at the moment the chip is picked.

use super::Database;
use crate::errors::Result;
use rusqlite::params;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DlMember {
    pub email: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DlList {
    pub id: i64,
    pub name: String,
    pub members: Vec<DlMember>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ListOp {
    Done,
    /// The name (or the member) is already there.
    Exists,
    /// Empty name, or not an email address.
    Invalid,
    /// No such list.
    Missing,
}

/// Names are unique ignoring case and surrounding space.
fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

impl Database {
    pub(crate) fn init_lists_schema(&self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS dl_lists (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                name_key TEXT NOT NULL UNIQUE,
                description TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS dl_members (
                list_id INTEGER NOT NULL REFERENCES dl_lists(id) ON DELETE CASCADE,
                member_key TEXT NOT NULL,
                address TEXT NOT NULL,
                display_name TEXT NOT NULL DEFAULT '',
                added_at INTEGER NOT NULL,
                PRIMARY KEY (list_id, member_key)
            )",
            [],
        )?;
        Ok(())
    }

    fn touch_list(&self, id: i64) -> Result<()> {
        self.conn.execute("UPDATE dl_lists SET updated_at = ?1 WHERE id = ?2", params![Self::now_ms(), id])?;
        Ok(())
    }

    /// Every list with its members, lists by name, members by name then address.
    pub fn lists_all(&self) -> Result<Vec<DlList>> {
        let mut lists: Vec<DlList> = {
            let mut st = self.conn.prepare("SELECT id, name FROM dl_lists ORDER BY name_key")?;
            let rows = st.query_map([], |r| Ok(DlList { id: r.get(0)?, name: r.get(1)?, members: vec![] }))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        let mut st = self.conn.prepare(
            "SELECT list_id, address, display_name FROM dl_members ORDER BY lower(CASE WHEN display_name = '' THEN address ELSE display_name END), member_key",
        )?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, DlMember { email: r.get(1)?, name: r.get(2)? })))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (id, m) in rows {
            if let Some(l) = lists.iter_mut().find(|l| l.id == id) { l.members.push(m); }
        }
        Ok(lists)
    }

    pub fn list_members(&self, id: i64) -> Result<Option<Vec<DlMember>>> {
        Ok(self.lists_all()?.into_iter().find(|l| l.id == id).map(|l| l.members))
    }

    /// Create a list; returns its id unless the name is empty or taken.
    pub fn list_create(&self, name: &str) -> Result<(ListOp, Option<i64>)> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 { return Ok((ListOp::Invalid, None)); }
        let now = Self::now_ms();
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO dl_lists (name, name_key, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params![name, name_key(name), now],
        )?;
        if n == 0 { return Ok((ListOp::Exists, None)); }
        Ok((ListOp::Done, Some(self.conn.last_insert_rowid())))
    }

    pub fn list_rename(&self, id: i64, name: &str) -> Result<ListOp> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 { return Ok(ListOp::Invalid); }
        let taken: bool = self.conn.query_row(
            "SELECT COUNT(*) FROM dl_lists WHERE name_key = ?1 AND id <> ?2", params![name_key(name), id], |r| r.get::<_, i64>(0),
        )? > 0;
        if taken { return Ok(ListOp::Exists); }
        let n = self.conn.execute(
            "UPDATE dl_lists SET name = ?1, name_key = ?2, updated_at = ?3 WHERE id = ?4",
            params![name.trim(), name_key(name), Self::now_ms(), id],
        )?;
        Ok(if n > 0 { ListOp::Done } else { ListOp::Missing })
    }

    pub fn list_delete(&self, id: i64) -> Result<bool> {
        self.conn.execute("DELETE FROM dl_members WHERE list_id = ?1", params![id])?;
        Ok(self.conn.execute("DELETE FROM dl_lists WHERE id = ?1", params![id])? > 0)
    }

    /// Add an address to a list. A name already on the member is kept unless `name` is given.
    pub fn list_member_add(&self, id: i64, email: &str, name: &str) -> Result<ListOp> {
        let Some(email) = Self::normalize_sender(email) else { return Ok(ListOp::Invalid) };
        let exists: bool = self.conn.query_row("SELECT COUNT(*) FROM dl_lists WHERE id = ?1", params![id], |r| r.get::<_, i64>(0))? > 0;
        if !exists { return Ok(ListOp::Missing); }
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO dl_members (list_id, member_key, address, display_name, added_at) VALUES (?1, ?2, ?2, ?3, ?4)",
            params![id, email, name.trim(), Self::now_ms()],
        )?;
        if n == 0 {
            if !name.trim().is_empty() {
                self.conn.execute("UPDATE dl_members SET display_name = ?1 WHERE list_id = ?2 AND member_key = ?3", params![name.trim(), id, email])?;
            }
            return Ok(ListOp::Exists);
        }
        self.touch_list(id)?;
        Ok(ListOp::Done)
    }

    pub fn list_member_remove(&self, id: i64, email: &str) -> Result<bool> {
        let n = self.conn.execute(
            "DELETE FROM dl_members WHERE list_id = ?1 AND member_key = ?2", params![id, email.trim().to_lowercase()],
        )?;
        if n > 0 { self.touch_list(id)?; }
        Ok(n > 0)
    }

    /// Everything address suggestions are built from, newest message first.
    pub fn harvest_addresses(&self, max_messages: usize) -> Result<Vec<crate::addresses::Seen>> {
        let own: std::collections::HashSet<String> = {
            let mut st = self.conn.prepare("SELECT lower(email) FROM accounts WHERE email IS NOT NULL AND email <> ''")?;
            let rows = st.query_map([], |r| r.get::<_, String>(0))?.collect::<std::result::Result<_, _>>()?;
            rows
        };
        let mut st = self.conn.prepare(
            "SELECT from_email, COALESCE(from_name, ''), COALESCE(to_text, ''), COALESCE(cc_text, ''), COALESCE(bcc_text, '')
             FROM messages ORDER BY received_at DESC LIMIT ?1",
        )?;
        let rows = st.query_map(params![max_messages as i64], |r| Ok(crate::addresses::MessageAddrs {
            from_email: r.get(0)?, from_name: r.get(1)?, to: r.get(2)?, cc: r.get(3)?, bcc: r.get(4)?,
        }))?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(crate::addresses::harvest(rows, &own))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Database { Database::open(":memory:").unwrap() }

    #[test]
    fn create_rename_delete_and_unique_names() {
        let d = db();
        let (op, id) = d.list_create("  Cadets ").unwrap();
        assert_eq!(op, ListOp::Done);
        let id = id.unwrap();
        assert_eq!(d.list_create("cadets").unwrap().0, ListOp::Exists);
        assert_eq!(d.list_create("   ").unwrap().0, ListOp::Invalid);
        let (_, id2) = d.list_create("Leadership").unwrap();
        assert_eq!(d.list_rename(id2.unwrap(), "CADETS").unwrap(), ListOp::Exists);
        assert_eq!(d.list_rename(id, "Cadets").unwrap(), ListOp::Done, "renaming to its own name is allowed");
        assert_eq!(d.list_rename(id2.unwrap(), "Staff").unwrap(), ListOp::Done);
        assert_eq!(d.lists_all().unwrap().iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), vec!["Cadets", "Staff"]);
        assert!(d.list_delete(id).unwrap());
        assert!(!d.list_delete(id).unwrap());
    }

    #[test]
    fn members_add_dedupe_case_remove_and_cascade() {
        let d = db();
        let id = d.list_create("A").unwrap().1.unwrap();
        assert_eq!(d.list_member_add(id, "Bob@X.org", "Bob").unwrap(), ListOp::Done);
        assert_eq!(d.list_member_add(id, "bob@x.org", "").unwrap(), ListOp::Exists);
        assert_eq!(d.list_member_add(id, "not an address", "").unwrap(), ListOp::Invalid);
        assert_eq!(d.list_member_add(999, "a@b.co", "").unwrap(), ListOp::Missing);
        let l = d.lists_all().unwrap();
        assert_eq!(l[0].members, vec![DlMember { email: "bob@x.org".into(), name: "Bob".into() }]);
        assert!(d.list_member_remove(id, "BOB@x.org").unwrap());
        assert!(!d.list_member_remove(id, "bob@x.org").unwrap());
        d.list_member_add(id, "c@d.co", "").unwrap();
        d.list_delete(id).unwrap();
        assert!(d.lists_all().unwrap().is_empty());
        let n: i64 = d.conn.query_row("SELECT COUNT(*) FROM dl_members", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }
}
