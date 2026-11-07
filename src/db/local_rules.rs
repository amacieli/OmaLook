//! Storage and dry-run for the local rule engine (OL-040 phase 4). Rules are kept as JSON next to a few
//! columns for ordering. Nothing here changes a message: `dry_run` only reads. Applying (phase 4b) goes
//! through `change_message_categories`, which marks `cat_pending` so the normal push sends it.

use super::{categories_from_json, Database};
use crate::errors::{OmarchyError, Result};
use crate::local_rules::{additions, LocalRule, Msg};
use rusqlite::params;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct DryRun {
    /// Inbox messages looked at.
    pub scanned: usize,
    /// Messages that would gain at least one tag.
    pub messages: usize,
    /// Tag additions in total (a message gaining two tags counts twice).
    pub additions: usize,
    /// Per rule id: messages it would tag.
    pub by_rule: BTreeMap<String, usize>,
    /// Per tag: messages that would gain it.
    pub by_tag: BTreeMap<String, usize>,
    /// Up to `SAMPLE` message ids with what they would gain, for the preview list.
    pub sample: Vec<(String, Vec<String>)>,
}

const SAMPLE: usize = 20;

impl Database {
    pub(crate) fn init_local_rules_schema(&self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS local_rules (
                id TEXT PRIMARY KEY,
                sequence INTEGER NOT NULL DEFAULT 0,
                json TEXT NOT NULL
            )", [])?;
        Ok(())
    }

    pub fn save_local_rule(&self, rule: &LocalRule) -> Result<()> {
        rule.validate().map_err(OmarchyError::SettingsError)?;
        self.conn.execute(
            "INSERT INTO local_rules (id, sequence, json) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET sequence = excluded.sequence, json = excluded.json",
            params![rule.id, rule.sequence, serde_json::to_string(rule)?])?;
        Ok(())
    }

    pub fn delete_local_rule(&self, id: &str) -> Result<bool> {
        Ok(self.conn.execute("DELETE FROM local_rules WHERE id = ?1", params![id])? > 0)
    }

    /// Rules in running order. A row that no longer parses is skipped, never fatal.
    pub fn local_rules(&self) -> Result<Vec<LocalRule>> {
        let mut st = self.conn.prepare("SELECT json FROM local_rules ORDER BY sequence, id")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for j in rows {
            if let Ok(r) = serde_json::from_str::<LocalRule>(&j?) { out.push(r); }
        }
        Ok(out)
    }

    /// What `rules` would add to the cached Inbox messages. Read-only. Messages with a pending move or
    /// delete are skipped, as are rows whose details were never fetched (`meta_ok = 0`): matching on
    /// missing recipients or categories would be guessing.
    pub fn dry_run(&self, rules: &[LocalRule]) -> Result<DryRun> {
        let mut out = DryRun::default();
        let mut st = self.conn.prepare(
            "SELECT m.id, m.account_id, m.from_email, COALESCE(m.from_name,''), COALESCE(m.to_text,''),
                    COALESCE(m.cc_text,''), m.subject, m.body, COALESCE(m.importance,''),
                    COALESCE(m.has_attachments,0), m.categories
             FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE lower(COALESCE(f.well_known_name,'')) = 'inbox'
               AND m.action_pending IS NULL AND m.meta_ok = 1")?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, Msg {
                account_id: r.get(1)?, from_email: r.get(2)?, from_name: r.get(3)?, to_text: r.get(4)?,
                cc_text: r.get(5)?, subject: r.get(6)?, body: r.get(7)?, importance: r.get(8)?,
                has_attachments: r.get::<_, i64>(9)? != 0,
                categories: categories_from_json(r.get::<_, Option<String>>(10)?.as_deref()),
            }))
        })?;
        for row in rows {
            let (id, m) = row?;
            out.scanned += 1;
            let add = additions(rules, &m);
            if add.is_empty() { continue; }
            out.messages += 1;
            out.additions += add.len();
            let mut seen_rules: Vec<&str> = Vec::new();
            for (rid, tag) in &add {
                *out.by_tag.entry(tag.clone()).or_default() += 1;
                if !seen_rules.contains(&rid.as_str()) {
                    seen_rules.push(rid);
                    *out.by_rule.entry(rid.clone()).or_default() += 1;
                }
            }
            if out.sample.len() < SAMPLE { out.sample.push((id, add.into_iter().map(|(_, t)| t).collect())); }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_rules::{Actions, Expr, Field};
    use crate::models::EmailMessage;

    fn rule(id: &str, seq: i64, from: &str, tag: &str, stop: bool) -> LocalRule {
        LocalRule { id: id.into(), name: id.into(), enabled: true, sequence: seq, applies_to: vec![],
            when: Expr::Text { field: Field::From, values: vec![from.into()] },
            actions: Actions { tags: vec![tag.into()], stop } }
    }

    fn db() -> Database {
        let d = Database::open_for_account(":memory:", "ex").unwrap();
        d.conn.execute("INSERT INTO accounts (id, provider, email) VALUES ('ex','exchange','me@x.org')", []).unwrap();
        d.conn.execute("INSERT INTO folders (id, display_name, well_known_name, account_id) VALUES ('inb','Inbox','inbox','ex')", []).ok();
        d.conn.execute("INSERT INTO folders (id, display_name, well_known_name, account_id) VALUES ('oth','Other',NULL,'ex')", []).ok();
        d
    }

    fn put(d: &Database, id: &str, from: &str, folder: &str, cats: &[&str], meta: bool) {
        d.insert_email(&EmailMessage { id: id.into(), from: from.into(), subject: "s".into(), received: "2026-11-01T12:00:00Z".into(),
            body: "b".into(), folder_id: Some(folder.into()), is_read: true, meta,
            categories: Some(cats.iter().map(|s| s.to_string()).collect()), ..Default::default() }).unwrap();
    }

    #[test]
    fn rules_round_trip_in_order_and_invalid_ones_are_refused() {
        let d = db();
        d.save_local_rule(&rule("b", 2, "b@", "B", false)).unwrap();
        d.save_local_rule(&rule("a", 1, "a@", "A", false)).unwrap();
        assert_eq!(d.local_rules().unwrap().iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
        assert!(d.save_local_rule(&rule("c", 3, " ", "C", false)).is_err());
        assert!(d.delete_local_rule("a").unwrap());
        assert!(!d.delete_local_rule("a").unwrap());
        assert_eq!(d.local_rules().unwrap().len(), 1);
    }

    #[test]
    fn dry_run_counts_inbox_only_skips_tagged_unfetched_and_changes_nothing() {
        let d = db();
        put(&d, "1", "bob@x.com", "inb", &[], true);
        put(&d, "2", "bob@x.com", "inb", &["Work"], true);   // already tagged
        put(&d, "3", "bob@x.com", "oth", &[], true);          // not in the Inbox
        put(&d, "4", "bob@x.com", "inb", &[], false);         // details never fetched
        put(&d, "5", "amy@x.com", "inb", &[], true);
        let rules = vec![rule("r1", 1, "bob@", "Work", true), rule("r2", 2, "amy@", "Friends", false)];
        let before: i64 = d.conn.query_row("SELECT COUNT(*) FROM messages WHERE cat_pending = 1", [], |r| r.get(0)).unwrap();
        let r = d.dry_run(&rules).unwrap();
        assert_eq!(r.scanned, 3, "1, 2, 5");
        assert_eq!(r.messages, 2);
        assert_eq!(r.additions, 2);
        assert_eq!(r.by_rule.get("r1"), Some(&1));
        assert_eq!(r.by_tag.get("Friends"), Some(&1));
        assert_eq!(r.sample.len(), 2);
        let after: i64 = d.conn.query_row("SELECT COUNT(*) FROM messages WHERE cat_pending = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(before, after, "dry run must not mark anything pending");
    }
}
