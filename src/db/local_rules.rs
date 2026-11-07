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
    /// Messages the enabled rules match at all, tagged already or not.
    pub matched: usize,
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
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS local_rule_stats (
                rule_id TEXT PRIMARY KEY,
                applied INTEGER NOT NULL DEFAULT 0,
                last_applied_at INTEGER NOT NULL DEFAULT 0
            )", [])?;
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS local_rules_state (
                account_id TEXT PRIMARY KEY,
                since TEXT NOT NULL
            )", [])?;
        self.conn.execute("CREATE TABLE IF NOT EXISTS local_rule_seen (message_id TEXT PRIMARY KEY)", [])?;
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
        self.conn.execute("DELETE FROM local_rule_stats WHERE rule_id = ?1", params![id])?;
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

    /// Evaluate `rules` over cached Inbox messages (`extra_where` narrows further, with `args` bound to its
    /// `?` marks). Returns the ids looked at and, for those that would gain tags, `(id, [(rule id, tag)])`.
    /// Read-only. Messages with a pending move or delete are skipped, as are rows whose details were never
    /// fetched (`meta_ok = 0`): matching on missing recipients or categories would be guessing. Rules are for
    /// incoming mail, so only the Inbox is considered.
    fn scan_inbox(&self, rules: &[LocalRule], extra_where: &str, args: &[&dyn rusqlite::ToSql])
        -> Result<(Vec<String>, Vec<(String, Vec<(String, String)>)>, usize)> {
        let (mut looked, mut plan, mut matched) = (Vec::new(), Vec::new(), 0usize);
        let sql = format!(
            "SELECT m.id, m.account_id, m.from_email, COALESCE(m.from_name,''), COALESCE(m.to_text,''),
                    COALESCE(m.cc_text,''), m.subject, m.body, COALESCE(m.importance,''),
                    COALESCE(m.has_attachments,0), m.categories
             FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE lower(COALESCE(f.well_known_name,'')) = 'inbox'
               AND m.action_pending IS NULL AND m.meta_ok = 1 {extra_where}");
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map(args, |r| {
            Ok((r.get::<_, String>(0)?, Msg {
                account_id: r.get(1)?, from_email: r.get(2)?, from_name: r.get(3)?, to_text: r.get(4)?,
                cc_text: r.get(5)?, subject: r.get(6)?, body: r.get(7)?, importance: r.get(8)?,
                has_attachments: r.get::<_, i64>(9)? != 0,
                categories: categories_from_json(r.get::<_, Option<String>>(10)?.as_deref()),
            }))
        })?;
        for row in rows {
            let (id, m) = row?;
            let add = additions(rules, &m);
            if rules.iter().any(|r| r.enabled && r.applies(&m.account_id) && r.matches(&m)) { matched += 1; }
            looked.push(id.clone());
            if !add.is_empty() { plan.push((id, add)); }
        }
        Ok((looked, plan, matched))
    }

    /// Every cached Inbox message (all accounts) `rules` would add tags to, plus how many were looked at.
    pub fn plan_local_rules(&self, rules: &[LocalRule]) -> Result<(usize, Vec<(String, Vec<(String, String)>)>)> {
        let (looked, plan, _) = self.scan_inbox(rules, "", &[])?;
        Ok((looked.len(), plan))
    }

    /// Counts for the preview: what `plan_local_rules` would do. Read-only.
    pub fn dry_run(&self, rules: &[LocalRule]) -> Result<DryRun> {
        let (looked, plan, matched) = self.scan_inbox(rules, "", &[])?;
        let mut out = DryRun { scanned: looked.len(), matched, messages: plan.len(), ..Default::default() };
        for (id, add) in &plan {
            out.additions += add.len();
            let mut seen_rules: Vec<&str> = Vec::new();
            for (rid, tag) in add {
                *out.by_tag.entry(tag.clone()).or_default() += 1;
                if !seen_rules.contains(&rid.as_str()) {
                    seen_rules.push(rid);
                    *out.by_rule.entry(rid.clone()).or_default() += 1;
                }
            }
            if out.sample.len() < SAMPLE { out.sample.push((id.clone(), add.iter().map(|(_, t)| t.clone()).collect())); }
        }
        Ok(out)
    }

    /// Write a plan: add each tag through `change_message_categories` (local edit, `cat_pending` = 1, pushed to
    /// the provider by the normal category push) and count it per rule in `local_rule_stats`.
    fn apply_plan(&self, plan: &[(String, Vec<(String, String)>)]) -> Result<(usize, BTreeMap<String, usize>)> {
        let now = Self::now_secs();
        let mut messages = 0;
        let mut per_rule: BTreeMap<String, usize> = BTreeMap::new();
        for (id, add) in plan {
            let mut touched = false;
            let mut rules_seen: Vec<&str> = Vec::new();
            for (rid, tag) in add {
                if self.change_message_categories(std::slice::from_ref(id), true, tag)? > 0 {
                    touched = true;
                    if !rules_seen.contains(&rid.as_str()) {
                        rules_seen.push(rid);
                        *per_rule.entry(rid.clone()).or_default() += 1;
                    }
                }
            }
            if touched { messages += 1; }
        }
        for (rid, n) in &per_rule {
            self.conn.execute(
                "INSERT INTO local_rule_stats (rule_id, applied, last_applied_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(rule_id) DO UPDATE SET applied = applied + ?2, last_applied_at = ?3",
                params![rid, *n as i64, now])?;
        }
        Ok((messages, per_rule))
    }

    /// Retroactive apply. The plan is recomputed here, not taken from an earlier preview, so it reflects the
    /// cache now. If more than `max_messages` would change and `confirmed` is false nothing is written and
    /// `Refused` is returned with the count (plan section 5: show the count, refuse over the threshold
    /// without an explicit confirm).
    pub fn apply_local_rules(&self, rules: &[LocalRule], max_messages: usize, confirmed: bool) -> Result<ApplyOutcome> {
        let (_, plan) = self.plan_local_rules(rules)?;
        if plan.len() > max_messages && !confirmed {
            return Ok(ApplyOutcome::Refused { would_change: plan.len(), limit: max_messages });
        }
        let (messages, per_rule) = self.apply_plan(&plan)?;
        Ok(ApplyOutcome::Applied { messages, per_rule })
    }

    /// Swap this rule's place with its neighbour (`up` = runs earlier). False when it cannot move.
    pub fn move_local_rule(&self, id: &str, up: bool) -> Result<bool> {
        let rules = self.local_rules()?;
        let Some(i) = rules.iter().position(|r| r.id == id) else { return Ok(false) };
        let j = if up { i.checked_sub(1) } else { Some(i + 1).filter(|j| *j < rules.len()) };
        let Some(j) = j else { return Ok(false) };
        // renumber the whole list 1..n so equal or gappy sequences cannot make the swap a no-op
        let mut order: Vec<LocalRule> = rules;
        order.swap(i, j);
        for (n, mut r) in order.into_iter().enumerate() {
            r.sequence = n as i64 + 1;
            self.save_local_rule(&r)?;
        }
        Ok(true)
    }

    /// Messages each rule has tagged so far (retroactive runs and arrivals), by rule id.
    pub fn local_rule_stats(&self) -> Result<BTreeMap<String, i64>> {
        let mut st = self.conn.prepare("SELECT rule_id, applied FROM local_rule_stats")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Run the local rules on mail that ARRIVED since this account's rules were first evaluated (called by the
    /// mail daemon after each sync). Returns how many messages gained a tag.
    ///
    /// - Only accounts whose provider has no server rule engine: Exchange rules already run on delivery, so
    ///   evaluating them again here would double up (plan section 2, engine per account).
    /// - New mail only: the first call records "now" and later calls look at Inbox messages received since
    ///   a day before that mark (clock skew) that were not evaluated before. With no enabled rule for the
    ///   account the mark simply moves to now, so a rule written later never reaches back (retroactive apply
    ///   is the explicit, confirmed way to do that).
    /// - Each message is evaluated once (`local_rule_seen`), after its details are in (`meta_ok`), so a later
    ///   edit of a rule does not re-tag old mail behind the user's back.
    /// - It only adds tags; it never removes or changes one a person set.
    pub fn apply_local_rules_to_new(&self) -> Result<usize> {
        let acct = self.account_id().to_string();
        if !crate::local_rules::has_local_engine(crate::accounts::provider_of(&acct)) { return Ok(0); }
        let now = chrono::Utc::now();
        let iso = |t: chrono::DateTime<chrono::Utc>| t.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let since: Option<String> = self.conn.query_row(
            "SELECT since FROM local_rules_state WHERE account_id = ?1", params![acct], |r| r.get(0)).ok();
        let rules: Vec<LocalRule> = self.local_rules()?.into_iter().filter(|r| r.enabled && r.applies(&acct)).collect();
        let set_mark = |t: String| -> Result<()> {
            self.conn.execute(
                "INSERT INTO local_rules_state (account_id, since) VALUES (?1, ?2)
                 ON CONFLICT(account_id) DO UPDATE SET since = ?2", params![acct, t])?;
            Ok(())
        };
        let Some(since) = since else { set_mark(iso(now))?; return Ok(0) };
        if rules.is_empty() { set_mark(iso(now))?; return Ok(0); }
        let floor = chrono::DateTime::parse_from_rfc3339(&since).map(|t| iso(t.with_timezone(&chrono::Utc) - chrono::Duration::days(1)))
            .unwrap_or(since);
        let (looked, plan, _) = self.scan_inbox(&rules,
            "AND m.account_id = ?1 AND m.received_at >= ?2 AND m.id NOT IN (SELECT message_id FROM local_rule_seen)",
            &[&acct, &floor])?;
        let (messages, _) = self.apply_plan(&plan)?;
        for id in &looked {
            self.conn.execute("INSERT OR IGNORE INTO local_rule_seen (message_id) VALUES (?1)", params![id])?;
        }
        Ok(messages)
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ApplyOutcome {
    /// Nothing written: the run would change more messages than the limit and was not confirmed.
    Refused { would_change: usize, limit: usize },
    Applied { messages: usize, per_rule: BTreeMap<String, usize> },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_rules::{Actions, Pred};
    use crate::models::EmailMessage;

    fn rule(id: &str, seq: i64, from: &str, tag: &str, stop: bool) -> LocalRule {
        LocalRule { id: id.into(), name: id.into(), enabled: true, sequence: seq, applies_to: vec![],
            conditions: vec![Pred::SenderContains(vec![from.into()])], exceptions: vec![],
            actions: Actions { tags: vec![tag.into()], stop } }
    }

    fn db() -> Database { db_for("ex") }

    fn db_for(acct: &str) -> Database {
        let d = Database::open_for_account(":memory:", acct).unwrap();
        d.conn.execute("INSERT INTO accounts (id, provider, email) VALUES (?1,'x','me@x.org')", params![acct]).unwrap();
        d.conn.execute("INSERT INTO folders (id, display_name, well_known_name, account_id) VALUES ('inb','Inbox','inbox',?1)", params![acct]).ok();
        d.conn.execute("INSERT INTO folders (id, display_name, well_known_name, account_id) VALUES ('oth','Other',NULL,?1)", params![acct]).ok();
        d
    }

    fn put_at(d: &Database, id: &str, from: &str, received: &str, meta: bool) {
        d.insert_email(&EmailMessage { id: id.into(), from: from.into(), subject: "s".into(), received: received.into(),
            body: "b".into(), folder_id: Some("inb".into()), is_read: true, meta,
            categories: Some(vec![]), ..Default::default() }).unwrap();
    }

    fn cats(d: &Database, id: &str) -> Vec<String> {
        categories_from_json(d.conn.query_row("SELECT categories FROM messages WHERE id = ?1", params![id], |r| r.get::<_, Option<String>>(0)).unwrap().as_deref())
    }

    fn iso_ago(days: i64) -> String { (chrono::Utc::now() - chrono::Duration::days(days)).format("%Y-%m-%dT%H:%M:%SZ").to_string() }

    #[test]
    fn arrivals_are_tagged_once_only_after_the_mark_and_only_when_details_are_in() {
        let d = db_for("gmail-t");
        d.save_local_rule(&rule("r1", 1, "bob@", "Work", false)).unwrap();
        put_at(&d, "old", "bob@x.com", &iso_ago(10), true);
        assert_eq!(d.apply_local_rules_to_new().unwrap(), 0, "first call only sets the mark");
        assert!(cats(&d, "old").is_empty());
        put_at(&d, "old2", "bob@x.com", &iso_ago(10), true);   // crawled later but received long ago
        put_at(&d, "new", "bob@x.com", &iso_ago(0), true);
        put_at(&d, "nometa", "bob@x.com", &iso_ago(0), false);
        put_at(&d, "other", "amy@x.com", &iso_ago(0), true);
        assert_eq!(d.apply_local_rules_to_new().unwrap(), 1);
        assert_eq!(cats(&d, "new"), vec!["Work"]);
        assert!(cats(&d, "old").is_empty() && cats(&d, "old2").is_empty() && cats(&d, "other").is_empty() && cats(&d, "nometa").is_empty());
        assert_eq!(d.apply_local_rules_to_new().unwrap(), 0, "evaluated messages are not looked at again");
        d.conn.execute("UPDATE messages SET meta_ok = 1 WHERE id = 'nometa'", []).unwrap();
        assert_eq!(d.apply_local_rules_to_new().unwrap(), 1, "tagged once its details arrive");
        assert_eq!(cats(&d, "nometa"), vec!["Work"]);
        assert_eq!(d.local_rule_stats().unwrap().get("r1"), Some(&2));
        // a rule edited later does not re-tag what was already evaluated
        d.save_local_rule(&rule("r2", 2, "amy@", "Friends", false)).unwrap();
        assert_eq!(d.apply_local_rules_to_new().unwrap(), 0);
        assert!(cats(&d, "other").is_empty());
    }

    #[test]
    fn a_rule_written_later_never_reaches_back_and_exchange_is_left_to_the_server() {
        let d = db_for("gmail-t");
        assert_eq!(d.apply_local_rules_to_new().unwrap(), 0, "mark set, no rules yet");
        put_at(&d, "m0", "bob@x.com", &iso_ago(3), true);   // received before the rule exists, outside the skew window
        assert_eq!(d.apply_local_rules_to_new().unwrap(), 0, "no rules: the mark moves to now");
        d.save_local_rule(&rule("r1", 1, "bob@", "Work", false)).unwrap();
        d.apply_local_rules_to_new().unwrap();
        assert!(cats(&d, "m0").is_empty(), "a rule written later does not reach back");

        let ex = db_for("exchange-primary");
        ex.save_local_rule(&rule("r1", 1, "bob@", "Work", false)).unwrap();
        assert_eq!(ex.apply_local_rules_to_new().unwrap(), 0);
        put_at(&ex, "e1", "bob@x.com", &iso_ago(0), true);
        assert_eq!(ex.apply_local_rules_to_new().unwrap(), 0);
        assert!(cats(&ex, "e1").is_empty(), "Exchange arrivals are tagged by the server rules, not here");
        let marks: i64 = ex.conn.query_row("SELECT COUNT(*) FROM local_rules_state", [], |r| r.get(0)).unwrap();
        assert_eq!(marks, 0);
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

    fn pending(d: &Database) -> i64 { d.conn.query_row("SELECT COUNT(*) FROM messages WHERE cat_pending = 1", [], |r| r.get(0)).unwrap() }

    #[test]
    fn apply_refuses_over_the_limit_then_tags_once_confirmed_and_counts() {
        let d = db();
        for i in 0..5 { put(&d, &format!("b{i}"), "bob@x.com", "inb", &[], true); }
        put(&d, "t", "bob@x.com", "inb", &["Work"], true);
        put(&d, "o", "bob@x.com", "oth", &[], true);
        let rules = vec![rule("r1", 1, "bob@", "Work", true)];
        assert_eq!(d.apply_local_rules(&rules, 4, false).unwrap(), ApplyOutcome::Refused { would_change: 5, limit: 4 });
        assert_eq!(pending(&d), 0, "refusal writes nothing");
        let ApplyOutcome::Applied { messages, per_rule } = d.apply_local_rules(&rules, 4, true).unwrap() else { panic!("applied") };
        assert_eq!((messages, per_rule.get("r1")), (5, Some(&5)));
        assert_eq!(pending(&d), 5, "queued for the provider push; the tagged and non-Inbox rows untouched");
        assert_eq!(d.local_rule_stats().unwrap().get("r1"), Some(&5));
        // second run: nothing left to do, nothing double counted
        let ApplyOutcome::Applied { messages, .. } = d.apply_local_rules(&rules, 4, false).unwrap() else { panic!("applied") };
        assert_eq!(messages, 0);
        assert_eq!(d.local_rule_stats().unwrap().get("r1"), Some(&5));
        assert!(d.delete_local_rule("r1").is_ok());
        assert!(d.local_rule_stats().unwrap().is_empty());
    }

    #[test]
    fn move_swaps_neighbours_even_when_sequences_collide_and_stops_at_the_ends() {
        let d = db();
        for id in ["a", "b", "c"] { d.save_local_rule(&rule(id, 5, &format!("{id}@"), "T", false)).unwrap(); } // equal sequences
        let ids = |d: &Database| d.local_rules().unwrap().iter().map(|r| r.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&d), vec!["a", "b", "c"]);
        assert!(d.move_local_rule("c", true).unwrap());
        assert_eq!(ids(&d), vec!["a", "c", "b"]);
        assert!(!d.move_local_rule("a", true).unwrap(), "already first");
        assert!(!d.move_local_rule("b", false).unwrap(), "already last");
        assert!(!d.move_local_rule("zz", true).unwrap());
    }

    #[test]
    fn dry_run_reports_matches_separately_from_additions() {
        let d = db();
        put(&d, "1", "bob@x.com", "inb", &["Work"], true);   // matches, already tagged
        put(&d, "2", "bob@x.com", "inb", &[], true);          // matches, would gain
        let r = d.dry_run(&[rule("r1", 1, "bob@", "Work", true)]).unwrap();
        assert_eq!((r.matched, r.messages), (2, 1));
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

#[cfg(test)]
mod real_cache {
    //! Manual check against a COPY of the real cache: `OMALOOK_DRYRUN_DB=/path/copy.db cargo test --lib
    //! real_cache -- --ignored --nocapture`. Read-only; prints counts, no addresses.
    use super::*;
    use crate::local_rules::{from_graph, Msg};

    #[test]
    #[ignore]
    fn saint_lucia_rules_against_the_cache() {
        let path = std::env::var("OMALOOK_DRYRUN_DB").expect("set OMALOOK_DRYRUN_DB to a copy of messages.db");
        let tag_key = std::env::var("OMALOOK_DRYRUN_TAG").unwrap_or_else(|_| "saintlucia".into());
        let d = Database::open_for_account(&path, "exchange-primary").unwrap();
        let squash = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
        let mut rules = Vec::new();
        let mut skipped = Vec::new();
        for raw in d.rules_raw("exchange-primary").unwrap() {
            let tags: Vec<String> = raw["actions"]["assignCategories"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            if !tags.iter().any(|t| squash(t) == tag_key) { continue; }
            match from_graph(&raw) { Ok(r) => rules.push(r), Err(k) => skipped.push((raw["displayName"].as_str().unwrap_or("?").to_string(), k)) }
        }
        println!("rules for tag '{tag_key}': {} usable ({} enabled), {} unsupported {:?}", rules.len(), rules.iter().filter(|r| r.enabled).count(), skipped.len(), skipped);
        for r in &rules { println!("  seq {:>3} enabled={} stop={} conds={:?} tags={:?}", r.sequence, r.enabled, r.actions.stop, r.conditions.iter().map(|p| p.key()).collect::<Vec<_>>(), r.actions.tags); }

        // Evaluate each rule alone (matches), over every cached Exchange message, by folder kind.
        let mut st = d.conn.prepare(
            "SELECT m.account_id, m.from_email, COALESCE(m.from_name,''), COALESCE(m.to_text,''), COALESCE(m.cc_text,''), m.subject, m.body,
                    COALESCE(m.importance,''), COALESCE(m.has_attachments,0), m.categories, COALESCE(lower(f.well_known_name),'(custom)'), m.meta_ok, m.action_pending
             FROM messages m LEFT JOIN folders f ON f.id = m.folder_id WHERE m.account_id = 'exchange-primary'").unwrap();
        let rows = st.query_map([], |r| Ok((Msg { account_id: r.get(0)?, from_email: r.get(1)?, from_name: r.get(2)?, to_text: r.get(3)?, cc_text: r.get(4)?,
            subject: r.get(5)?, body: r.get(6)?, importance: r.get(7)?, has_attachments: r.get::<_, i64>(8)? != 0,
            categories: categories_from_json(r.get::<_, Option<String>>(9)?.as_deref()) },
            r.get::<_, String>(10)?, r.get::<_, i64>(11)?, r.get::<_, Option<String>>(12)?))).unwrap();
        let (mut scanned, mut matched, mut tagged, mut gain) = (0, 0, 0, 0);
        let mut by_folder: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        let (mut inbox_gain, mut no_meta_match) = (0, 0);
        for row in rows {
            let (m, folder, meta, pending) = row.unwrap();
            scanned += 1;
            let hit = rules.iter().filter(|r| r.enabled).any(|r| r.matches(&m));
            if !hit { continue; }
            matched += 1;
            let already = m.categories.iter().any(|c| squash(c) == tag_key);
            if already { tagged += 1; } else { gain += 1; if folder == "inbox" && pending.is_none() && meta == 1 { inbox_gain += 1; } }
            if meta != 1 { no_meta_match += 1; }
            let e = by_folder.entry(folder).or_default(); e.0 += 1; if !already { e.1 += 1; }
        }
        println!("scanned {scanned}; rules match {matched}: already tagged {tagged}, would gain {gain}; of the gain {inbox_gain} are applicable Inbox rows; {no_meta_match} matches have meta_ok=0");
        println!("by folder (matches, would gain): {by_folder:?}");
        let dr = d.dry_run(&rules).unwrap();
        println!("dry_run (Inbox, details fetched, rules in order with stop): scanned {} messages {} additions {} by_rule {:?}", dr.scanned, dr.messages, dr.additions, dr.by_rule);
    }
}
