//! Tags page (docs/PLAN-tags-and-rules.md, OL-040 phase 1): one view of every account's categories /
//! labels, with message counts, recolour, delete and "adopt on another account".
//!
//! A *tag* is OmaLook's word for an Exchange category or a Gmail label. Tags of different accounts are
//! the same tag when their names match ignoring case and surrounding space (no merge or rename:
//! decision Q3, the user cleans near-duplicates by hand). Everything here is a local edit that the mail
//! daemon pushes to the provider afterwards (`push_pending_categories`), the same shape as `t`.
//!
//! Local intent flags on `category_defs`:
//! - `color_push = 1`   the colour was changed here and the provider has not taken it yet;
//! - `delete_pending=1` the tag is being deleted: hidden from the pickers, stripped from the account's
//!   messages (through the normal `cat_pending` push), and removed on the provider once no message of the
//!   account still carries it.

use super::{categories_from_json, Database};
use crate::errors::Result;
use rusqlite::params;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TagAccount {
    pub account_id: String,
    pub email: String,
    pub provider: String,
    /// The account has this tag in its category list (Exchange master list / Gmail label).
    pub defined: bool,
    /// Created here, not on the provider yet.
    pub pending: bool,
    /// Colour changed here, not on the provider yet.
    pub color_pending: bool,
    pub color: Option<String>,
    pub messages: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TagRow {
    pub name: String,
    pub color: String,
    pub accounts: Vec<TagAccount>,
    pub total: i64,
}

/// Names are the same tag when they match ignoring case and surrounding space.
pub fn tag_key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Letters and digits only, lower case, "saint" written "st": `Saint Lucia`, `SaintLucia` and `St Lucia`
/// all give `stlucia`, `Civil Air Patrol` and `CivilAirPatrol` give `civilairpatrol`.
pub(crate) fn squash(name: &str) -> String {
    let lower = name.to_lowercase().replace("saint", "st");
    lower.chars().filter(|c| c.is_alphanumeric()).collect()
}

/// True when `a` and `b` look like one tag written two ways (not when they are identical ignoring
/// case: that is the same tag). The tag picker warns about these before it makes a second one.
pub fn near_match(a: &str, b: &str) -> bool {
    if tag_key(a) == tag_key(b) { return false; }
    let (x, y) = (squash(a), squash(b));
    if x.is_empty() || y.is_empty() { return false; }
    if x == y { return true; }
    let (short, long) = if x.len() <= y.len() { (&x, &y) } else { (&y, &x) };
    short.len() >= 4 && long.contains(short.as_str())
}

impl Database {
    pub(crate) fn init_tags_schema(&self) -> Result<()> {
        for (col, decl) in [("color_push", "INTEGER NOT NULL DEFAULT 0"), ("delete_pending", "INTEGER NOT NULL DEFAULT 0")] {
            let has: bool = self.conn
                .query_row("SELECT COUNT(*) FROM pragma_table_info('category_defs') WHERE name = ?1", [col], |r| r.get::<_, i32>(0))
                .unwrap_or(0) > 0;
            if !has {
                // Several account daemons open the database at once: a racing thread may add it first.
                self.add_column(&format!("ALTER TABLE category_defs ADD COLUMN {col} {decl}"))?;
            }
        }
        Ok(())
    }

    /// Every tag known to any account (defined in a category list, or carried by a cached message),
    /// merged by name, with per-account presence and message counts. Newest spelling wins the display
    /// name: the one in a category list, else the most used on messages.
    pub fn tags_overview(&self) -> Result<Vec<TagRow>> {
        // accounts that can have tags
        let mut accounts: Vec<(String, String, String)> = Vec::new();
        {
            let mut st = self.conn.prepare("SELECT id, COALESCE(email,''), provider FROM accounts ORDER BY created_at, id")?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            for r in rows { accounts.push(r?); }
        }

        // key -> (display name, per-account entries)
        struct Acc { defined: bool, pending: bool, color_pending: bool, color: Option<String>, messages: i64 }
        let mut tags: BTreeMap<String, (String, i64, BTreeMap<String, Acc>)> = BTreeMap::new();

        {
            let mut st = self.conn.prepare(
                "SELECT account_id, name, color, pending, color_push FROM category_defs WHERE delete_pending = 0")?;
            let rows = st.query_map([], |r| Ok((
                r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?)))?;
            for r in rows {
                let (acct, name, color, pending, cpush) = r?;
                let e = tags.entry(tag_key(&name)).or_insert_with(|| (name.clone(), i64::MAX, BTreeMap::new()));
                e.0 = name; e.1 = i64::MAX; // a list spelling beats a message spelling
                e.2.insert(acct, Acc { defined: true, pending: pending != 0, color_pending: cpush != 0, color: Some(color), messages: 0 });
            }
        }

        // counts from messages: (account, spelling) -> n
        let mut spellings: BTreeMap<(String, String), BTreeMap<String, i64>> = BTreeMap::new();
        {
            let mut st = self.conn.prepare(
                "SELECT account_id, categories FROM messages
                 WHERE categories IS NOT NULL AND categories != '[]' AND action_pending IS NULL")?;
            let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for r in rows {
                let (acct, cats) = r?;
                let mut seen: Vec<String> = Vec::new();
                for c in categories_from_json(Some(&cats)) {
                    let key = tag_key(&c);
                    if key.is_empty() || seen.contains(&key) { continue }
                    seen.push(key.clone());
                    *spellings.entry((acct.clone(), key)).or_default().entry(c).or_default() += 1;
                }
            }
        }
        for ((acct, key), by_spelling) in spellings {
            let n: i64 = by_spelling.values().sum();
            let best = by_spelling.iter().max_by_key(|(_, v)| **v).map(|(k, _)| k.clone()).unwrap_or_else(|| key.clone());
            // a tag that is being deleted is not shown even though messages still carry it for a moment
            let deleting: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM category_defs WHERE account_id = ?1 AND delete_pending = 1 AND lower(trim(name)) = ?2",
                params![acct, key], |r| r.get(0)).unwrap_or(0);
            if deleting > 0 { continue }
            let e = tags.entry(key).or_insert_with(|| (best.clone(), 0, BTreeMap::new()));
            if e.1 != i64::MAX && n > e.1 { e.0 = best; e.1 = n; }
            e.2.entry(acct).or_insert(Acc { defined: false, pending: false, color_pending: false, color: None, messages: 0 }).messages = n;
        }

        let mut out: Vec<TagRow> = tags.into_iter().map(|(key, (name, _, per))| {
            let color = per.values().find_map(|a| a.color.clone()).unwrap_or_else(|| crate::models::fallback_category_color(&key).to_string());
            let accs: Vec<TagAccount> = accounts.iter().map(|(id, email, provider)| {
                match per.get(id) {
                    Some(a) => TagAccount { account_id: id.clone(), email: email.clone(), provider: provider.clone(), defined: a.defined,
                        pending: a.pending, color_pending: a.color_pending, color: a.color.clone(), messages: a.messages },
                    None => TagAccount { account_id: id.clone(), email: email.clone(), provider: provider.clone(), defined: false,
                        pending: false, color_pending: false, color: None, messages: 0 },
                }
            }).collect();
            let total = accs.iter().map(|a| a.messages).sum();
            TagRow { name, color, accounts: accs, total }
        }).collect();
        out.sort_by(|a, b| b.total.cmp(&a.total).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        Ok(out)
    }

    /// Tags (across all accounts) whose names look like `name` but are not the same tag.
    pub fn tags_similar(&self, name: &str) -> Result<Vec<String>> {
        let mut out: Vec<String> = Vec::new();
        for t in self.tags_overview()? {
            if near_match(name, &t.name) && !out.iter().any(|o| tag_key(o) == tag_key(&t.name)) { out.push(t.name); }
        }
        Ok(out)
    }

    /// Recolour this account's tag. The new colour shows at once; the daemon pushes it.
    /// Returns false when the account has no such tag.
    pub fn tag_set_color(&self, name: &str, hex: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE category_defs SET color = ?1, color_push = 1
             WHERE account_id = ?2 AND lower(trim(name)) = ?3 AND delete_pending = 0",
            params![hex, self.account_id, tag_key(name)])?;
        Ok(n > 0)
    }

    /// Colour changes not yet on the provider: (name, hex).
    pub fn pending_color_pushes(&self) -> Result<Vec<(String, String)>> {
        let mut st = self.conn.prepare(
            "SELECT name, color FROM category_defs WHERE account_id = ?1 AND color_push = 1 AND pending = 0 AND delete_pending = 0")?;
        let rows = st.query_map(params![self.account_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The provider took `hex` for `name`. A colour chosen again meanwhile stays pending.
    pub fn finish_color_push(&self, name: &str, hex: &str, provider_color: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE category_defs SET provider_color = ?1, color_push = CASE WHEN color = ?2 THEN 0 ELSE 1 END
             WHERE account_id = ?3 AND name = ?4",
            params![provider_color, hex, self.account_id, name])?;
        Ok(())
    }

    /// The provider refused the colour: drop the local intent (the next category read shows its colour).
    pub fn drop_color_push(&self, name: &str) -> Result<()> {
        self.conn.execute("UPDATE category_defs SET color_push = 0 WHERE account_id = ?1 AND name = ?2", params![self.account_id, name])?;
        Ok(())
    }

    /// How many of this account's messages carry `name`.
    pub fn tag_message_count(&self, name: &str) -> Result<i64> {
        let mut n = 0;
        let mut st = self.conn.prepare("SELECT categories FROM messages WHERE account_id = ?1 AND categories LIKE '%' || ?2 || '%' COLLATE NOCASE")?;
        let key = tag_key(name);
        let rows = st.query_map(params![self.account_id, name.trim()], |r| r.get::<_, Option<String>>(0))?;
        for r in rows {
            if categories_from_json(r?.as_deref()).iter().any(|c| tag_key(c) == key) { n += 1; }
        }
        Ok(n)
    }

    /// Start deleting this account's tag: hide it, and strip it from the account's messages through the
    /// normal pending push. Returns (messages changed, the definition existed).
    pub fn tag_delete(&self, name: &str) -> Result<(usize, bool)> {
        let key = tag_key(name);
        let existed = self.conn.execute(
            "UPDATE category_defs SET delete_pending = 1 WHERE account_id = ?1 AND lower(trim(name)) = ?2",
            params![self.account_id, key])? > 0;
        let ids = self.strip_tag_pending(name)?;
        Ok((ids, existed))
    }

    /// Remove `name` from every message of this account (local edit, `cat_pending`). Returns messages changed.
    pub fn strip_tag_pending(&self, name: &str) -> Result<usize> {
        let key = tag_key(name);
        let mut ids: Vec<String> = Vec::new();
        {
            let mut st = self.conn.prepare("SELECT id, categories FROM messages WHERE account_id = ?1 AND categories LIKE '%' || ?2 || '%' COLLATE NOCASE")?;
            let rows = st.query_map(params![self.account_id, name.trim()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?;
            for r in rows {
                let (id, cats) = r?;
                if categories_from_json(cats.as_deref()).iter().any(|c| tag_key(c) == key) { ids.push(id); }
            }
        }
        if ids.is_empty() { return Ok(0) }
        self.conn.execute_batch("BEGIN")?;
        let r = (|| -> Result<usize> {
            let mut n = 0;
            for id in &ids {
                let cur: Option<String> = self.conn.query_row("SELECT categories FROM messages WHERE id = ?1", params![id], |r| r.get(0))?;
                let mut list = categories_from_json(cur.as_deref());
                let before = list.len();
                list.retain(|c| tag_key(c) != key);
                if list.len() != before {
                    self.conn.execute("UPDATE messages SET categories = ?1, cat_pending = 1 WHERE id = ?2",
                        params![serde_json::to_string(&list).unwrap_or_else(|_| "[]".into()), id])?;
                    n += 1;
                }
            }
            Ok(n)
        })();
        match r {
            Ok(n) => { self.conn.execute_batch("COMMIT")?; Ok(n) }
            Err(e) => { let _ = self.conn.execute_batch("ROLLBACK"); Err(e) }
        }
    }

    /// Tags of this account being deleted that no message carries any more (neither wanted nor as the
    /// provider last saw it) once every pending category push of the account is done: safe to delete
    /// on the provider.
    pub fn deletable_tags(&self) -> Result<Vec<String>> {
        let mut names: Vec<String> = Vec::new();
        {
            let mut st = self.conn.prepare("SELECT name FROM category_defs WHERE account_id = ?1 AND delete_pending = 1")?;
            let rows = st.query_map(params![self.account_id], |r| r.get::<_, String>(0))?;
            for r in rows { names.push(r?); }
        }
        if names.is_empty() { return Ok(Vec::new()) }
        // Any unpushed category change on the account may be one of this tag's removals (rows from
        // before `categories_synced` existed have none to compare with), so wait for all of them.
        let waiting: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE account_id = ?1 AND cat_pending = 1", params![self.account_id], |r| r.get(0))?;
        if waiting > 0 { return Ok(Vec::new()) }
        let mut out = Vec::new();
        for name in names {
            let key = tag_key(&name);
            let mut st = self.conn.prepare(
                "SELECT categories, categories_synced FROM messages WHERE account_id = ?1 AND
                 (categories LIKE '%' || ?2 || '%' COLLATE NOCASE OR categories_synced LIKE '%' || ?2 || '%' COLLATE NOCASE)")?;
            let rows = st.query_map(params![self.account_id, name.trim()], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)))?;
            let mut still = false;
            for r in rows {
                let (c, s) = r?;
                if categories_from_json(c.as_deref()).iter().chain(categories_from_json(s.as_deref()).iter()).any(|x| tag_key(x) == key) { still = true; break }
            }
            if !still { out.push(name); }
        }
        Ok(out)
    }

    /// The provider no longer has `name`: forget the definition.
    pub fn finish_tag_delete(&self, name: &str) -> Result<()> {
        self.conn.execute("DELETE FROM category_defs WHERE account_id = ?1 AND lower(trim(name)) = ?2 AND delete_pending = 1",
            params![self.account_id, tag_key(name)])?;
        Ok(())
    }

    /// Delete was refused for good: show the tag again.
    pub fn cancel_tag_delete(&self, name: &str) -> Result<()> {
        self.conn.execute("UPDATE category_defs SET delete_pending = 0 WHERE account_id = ?1 AND lower(trim(name)) = ?2",
            params![self.account_id, tag_key(name)])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CategoryDef, EmailMessage};

    fn account(db: &Database, id: &str, provider: &str, email: &str) {
        db.conn.execute("INSERT INTO accounts (id, provider, email) VALUES (?1, ?2, ?3)", params![id, provider, email]).unwrap();
    }

    fn put(db: &Database, id: &str, cats: &[&str]) {
        db.insert_email(&EmailMessage { id: id.into(), from: "a@b.c".into(), subject: "s".into(), received: "2026-10-06T12:00:00Z".into(),
            body: "p".into(), folder_id: Some("f".into()), is_read: true, meta: true,
            categories: Some(cats.iter().map(|s| s.to_string()).collect()), ..Default::default() }).unwrap();
    }

    fn def(name: &str, color: &str) -> CategoryDef { CategoryDef { name: name.into(), color: color.into(), provider_color: Some("preset0".into()) } }

    #[test]
    fn near_matches_find_the_real_duplicates_and_not_the_same_tag() {
        assert!(near_match("Saint Lucia", "SaintLucia"));
        assert!(near_match("St Lucia", "Saint Lucia"));
        assert!(near_match("CivilAirPatrol", "Civil Air Patrol"));
        assert!(near_match("Panama", "Panama 2026"));
        assert!(!near_match("Family", "family"), "same tag ignoring case is not a near match");
        assert!(!near_match("Adam Job", "Job hunting"));
        assert!(!near_match("BBQ", "BBQ2022"), "short names are not substring-matched");
        assert!(!near_match("", "x"));
    }

    #[test]
    fn overview_merges_accounts_counts_messages_and_flags_missing_definitions() {
        let a = Database::open_for_account(":memory:", "ex").unwrap();
        account(&a, "ex", "exchange", "me@work.org");
        account(&a, "gm", "gmail", "me@gmail.com");
        a.replace_category_defs(&[def("Family", "#e74856"), def("Panama", "#10893e")]).unwrap();
        put(&a, "1", &["Family"]); put(&a, "2", &["Family", "Panama"]); put(&a, "3", &["family"]); put(&a, "4", &[]);
        // a message tag with no definition (Exchange accepts unlisted names)
        put(&a, "5", &["Ghost"]);

        let rows = a.tags_overview().unwrap();
        let fam = rows.iter().find(|r| r.name == "Family").unwrap();
        assert_eq!(fam.total, 3, "case variants count together");
        let ex = fam.accounts.iter().find(|x| x.account_id == "ex").unwrap();
        assert!(ex.defined && ex.messages == 3);
        let gm = fam.accounts.iter().find(|x| x.account_id == "gm").unwrap();
        assert!(!gm.defined && gm.messages == 0, "gmail does not have it yet: adoptable");
        let ghost = rows.iter().find(|r| r.name == "Ghost").unwrap();
        assert!(!ghost.accounts[0].defined && ghost.accounts[0].messages == 1, "used on a message, not in the list");
        assert_eq!(rows[0].name, "Family", "most used first");
    }

    #[test]
    fn recolour_is_local_first_and_survives_a_category_refresh() {
        let db = Database::open_for_account(":memory:", "ex").unwrap();
        db.replace_category_defs(&[def("Family", "#e74856")]).unwrap();
        assert!(db.tag_set_color(" family ", "#0078d4").unwrap());
        assert!(!db.tag_set_color("Nope", "#0078d4").unwrap());
        assert_eq!(db.pending_color_pushes().unwrap(), vec![("Family".to_string(), "#0078d4".to_string())]);
        // the provider still says red: the local choice stays until pushed
        db.replace_category_defs(&[def("Family", "#e74856")]).unwrap();
        assert_eq!(db.pending_color_pushes().unwrap().len(), 1);
        db.finish_color_push("Family", "#0078d4", Some("preset7")).unwrap();
        assert!(db.pending_color_pushes().unwrap().is_empty());
        // chosen again while a push was in flight: stays pending
        db.tag_set_color("Family", "#10893e").unwrap();
        db.finish_color_push("Family", "#0078d4", Some("preset7")).unwrap();
        assert_eq!(db.pending_color_pushes().unwrap().len(), 1);
        db.drop_color_push("Family").unwrap();
        assert!(db.pending_color_pushes().unwrap().is_empty());
    }

    #[test]
    fn delete_strips_messages_then_waits_for_the_pushes_before_the_provider_delete() {
        let db = Database::open_for_account(":memory:", "ex").unwrap();
        account(&db, "ex", "exchange", "me@work.org");
        db.replace_category_defs(&[def("Old", "#e74856"), def("Keep", "#10893e")]).unwrap();
        put(&db, "1", &["Old", "Keep"]); put(&db, "2", &["old"]); put(&db, "3", &["Keep"]);
        assert_eq!(db.tag_message_count("Old").unwrap(), 2);
        let (changed, existed) = db.tag_delete("Old").unwrap();
        assert_eq!((changed, existed), (2, true));
        assert!(db.tags_overview().unwrap().iter().all(|t| t.name != "Old"), "hidden at once");
        assert!(db.tags_overview().unwrap().iter().any(|t| t.name == "Keep" && t.total == 2), "other tags untouched");
        // messages not pushed yet: the provider still has Old on them, so no provider delete yet
        assert!(db.deletable_tags().unwrap().is_empty());
        let pend = db.pending_category_pushes().unwrap();
        assert_eq!(pend.len(), 2);
        for (id, wanted, _had) in pend { db.finish_category_push(&id, &wanted).unwrap(); }
        assert_eq!(db.deletable_tags().unwrap(), vec!["Old".to_string()]);
        // a category refresh that still lists it must not bring it back
        db.replace_category_defs(&[def("Old", "#e74856"), def("Keep", "#10893e")]).unwrap();
        assert!(db.tags_overview().unwrap().iter().all(|t| t.name != "Old"));
        db.finish_tag_delete("Old").unwrap();
        let n: i64 = db.conn.query_row("SELECT COUNT(*) FROM category_defs WHERE name='Old'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
        // rows with no provider copy recorded (older cache rows) must not let the delete jump the pushes
        db.replace_category_defs(&[def("Keep", "#10893e"), def("Gone", "#0078d4")]).unwrap();
        put(&db, "9", &["Gone"]);
        db.conn.execute("UPDATE messages SET categories_synced = NULL WHERE id = '9'", []).unwrap();
        db.tag_delete("Gone").unwrap();
        assert!(db.deletable_tags().unwrap().is_empty(), "message 9 still has its removal to push");
        let pend = db.pending_category_pushes().unwrap();
        for (id, wanted, _) in pend { db.finish_category_push(&id, &wanted).unwrap(); }
        assert_eq!(db.deletable_tags().unwrap(), vec!["Gone".to_string()]);
        db.cancel_tag_delete("Gone").unwrap();
        // a refused delete shows the tag again
        db.replace_category_defs(&[def("Keep", "#10893e"), def("Pinned", "#0078d4")]).unwrap();
        db.tag_delete("Pinned").unwrap();
        db.cancel_tag_delete("Pinned").unwrap();
        assert!(db.tags_overview().unwrap().iter().any(|t| t.name == "Pinned"));
    }

    #[test]
    fn similar_tags_are_reported_for_a_new_name() {
        let db = Database::open_for_account(":memory:", "ex").unwrap();
        account(&db, "ex", "exchange", "me@work.org");
        db.replace_category_defs(&[def("Saint Lucia", "#e74856"), def("Family", "#10893e")]).unwrap();
        assert_eq!(db.tags_similar("SaintLucia").unwrap(), vec!["Saint Lucia".to_string()]);
        assert!(db.tags_similar("saint lucia").unwrap().is_empty(), "the very same tag");
        assert!(db.tags_similar("Receipts").unwrap().is_empty());
    }
}
