//! Queueing Gmail filter changes (OL-040 phase 5b). Filters are cached in `mail_rules` as the provider's raw
//! JSON plus `omalookTags`, and changes ride the same `rule_ops` queue as Exchange rule changes, with kinds
//! `gmail_add`, `gmail_edit` and `gmail_delete` (the Exchange pusher skips those). Gmail has no edit call, so an
//! edit is "create the new filter, check it, then delete the old one": a failure in between never leaves the
//! mailbox without the rule.

use super::Database;
use crate::gmail_filters as gf;
use crate::local_rules::LocalRule;
use serde_json::{json, Value};

/// A filter change as asked for in the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum FilterChange {
    Add(LocalRule),
    /// Replace filter `id` with `rule`.
    Edit { id: String, rule: LocalRule },
    Delete { id: String },
}

impl Database {
    /// Check a change against the cached filters and queue it for the daemon, which repeats the checks against
    /// the server's own list before writing anything.
    pub fn queue_filter_change(&self, change: &FilterChange) -> Result<i64, String> {
        let is_gmail = self.accounts_basic().map_err(|e| e.to_string())?.iter()
            .any(|(id, p, _)| id == self.account_id() && p.eq_ignore_ascii_case("gmail"));
        if !is_gmail { return Err("Gmail filters exist only on Gmail accounts".into()) }
        let cached = self.cached_rules_for_write().map_err(|m| if m.contains("sign in") {
            "sign in to this Gmail account again to let OmaLook read and change its filters".to_string() } else { m })?;
        let pending = self.pending_rule_ops().map_err(|e| e.to_string())?;
        let touching = |id: &str| pending.iter().any(|o| o.kind.starts_with("gmail_") && o.payload["id"].as_str() == Some(id));
        let find = |id: &str| cached.iter().find(|f| f["id"].as_str() == Some(id)).ok_or_else(|| "no such filter (the list may have changed; it has been refreshed)".to_string());
        let check_rule = |r: &LocalRule| -> Result<(), String> {
            gf::writable(r)?;
            if !r.applies(self.account_id()) { return Err("the rule does not apply to this account".into()) }
            Ok(())
        };
        let (kind, payload) = match change {
            FilterChange::Add(r) => {
                check_rule(r)?;
                let dup = pending.iter().filter(|o| o.kind == "gmail_add" || o.kind == "gmail_edit")
                    .any(|o| o.payload["rule"]["conditions"] == json!(r.conditions) && o.payload["rule"]["exceptions"] == json!(r.exceptions) && o.payload["rule"]["actions"] == json!(r.actions));
                if dup { return Err("that filter is already waiting to be created".into()) }
                ("gmail_add", json!({ "rule": r }))
            }
            FilterChange::Edit { id, rule } => {
                check_rule(rule)?;
                let f = find(id)?;
                if let Err(why) = gf::from_gmail(f, &tag_map(f), self.account_id()) { return Err(format!("this filter cannot be edited here: {why}")) }
                if touching(id) { return Err("a change to this filter is already waiting".into()) }
                ("gmail_edit", json!({ "id": id, "rule": rule, "expect": gf::fingerprint(f) }))
            }
            FilterChange::Delete { id } => {
                let f = find(id)?;
                if let Err(why) = gf::from_gmail(f, &tag_map(f), self.account_id()) { return Err(format!("this filter cannot be deleted here: {why}")) }
                if touching(id) { return Err("a change to this filter is already waiting".into()) }
                ("gmail_delete", json!({ "id": id, "expect": gf::fingerprint(f) }))
            }
        };
        self.enqueue_rule_op(kind, &payload).map_err(|e| e.to_string())
    }

    /// Hex colour OmaLook keeps for `tag` on this account (its own list first, else the name-based fallback).
    pub fn tag_color_hex(&self, tag: &str) -> String {
        self.conn.query_row(
            "SELECT color FROM category_defs WHERE account_id = ?1 AND lower(trim(name)) = ?2 AND delete_pending = 0 LIMIT 1",
            rusqlite::params![self.account_id(), tag.trim().to_lowercase()], |r| r.get::<_, String>(0))
            .unwrap_or_else(|_| crate::models::fallback_category_color(tag).to_string())
    }

    /// Cached Gmail filters of this account as page summaries.
    pub fn filter_summaries(&self, account_id: &str) -> Vec<gf::FilterSummary> {
        self.rules_raw(account_id).unwrap_or_default().iter()
            .map(|f| gf::summarize(f, &tag_map(f), account_id)).collect()
    }
}

/// Label id -> name for the labels a cached filter itself names (its `omalookTags` are in the same order as the
/// ids that resolved). Enough to read the filter back without asking Gmail again.
fn tag_map(f: &Value) -> std::collections::HashMap<String, String> {
    let ids: Vec<&str> = f["action"]["addLabelIds"].as_array().into_iter().flatten().filter_map(|i| i.as_str()).collect();
    let names: Vec<&str> = f["omalookTags"].as_array().into_iter().flatten().filter_map(|i| i.as_str()).collect();
    // `with_tags` drops ids it could not name, so only a one-to-one pairing is trusted
    if ids.len() == names.len() { ids.into_iter().zip(names).map(|(i, n)| (i.to_string(), n.to_string())).collect() } else { Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_rules::{Actions, Pred};

    fn rule(from: &str, tag: &str) -> LocalRule {
        LocalRule { id: "r".into(), name: "n".into(), enabled: true, sequence: 1, applies_to: vec![],
            conditions: vec![Pred::FromAddresses(vec![from.into()])], exceptions: vec![], actions: Actions { tags: vec![tag.into()], stop: false } }
    }

    fn gmail_db() -> Database {
        let d = Database::open_for_account(":memory:", "g1").unwrap();
        d.conn.execute("INSERT INTO accounts (id, provider, email, created_at) VALUES ('g1', 'gmail', 'me@gmail.com', 1)", []).unwrap();
        d
    }

    fn cache(d: &Database) -> Value {
        let mut f = gf::to_gmail(&rule("a@x.com", "Family"), "Label_7").unwrap();
        f["id"] = json!("F1");
        f["omalookTags"] = json!(["Family"]);
        d.replace_rules(&[f.clone()]).unwrap();
        f
    }

    #[test]
    fn nothing_is_queued_before_filters_were_read_or_for_other_providers() {
        let d = gmail_db();
        assert!(d.queue_filter_change(&FilterChange::Add(rule("a@x.com", "Family"))).unwrap_err().contains("not been read"));
        d.set_rules_state("needs_signin", "").unwrap();
        assert!(d.queue_filter_change(&FilterChange::Add(rule("a@x.com", "Family"))).unwrap_err().contains("sign in"));
        let ex = Database::open_for_account(":memory:", "ex").unwrap();
        ex.conn.execute("INSERT INTO accounts (id, provider, email, created_at) VALUES ('ex', 'exchange', 'a@b.c', 1)", []).unwrap();
        assert!(ex.queue_filter_change(&FilterChange::Add(rule("a@x.com", "Family"))).unwrap_err().contains("only on Gmail"));
    }

    #[test]
    fn add_edit_delete_queue_with_checks_and_no_double_booking() {
        let d = gmail_db();
        let f = cache(&d);
        // add: validated as Gmail-writable
        let mut bad = rule("a@x.com", "Family"); bad.actions.stop = true;
        assert!(d.queue_filter_change(&FilterChange::Add(bad)).unwrap_err().contains("stop"));
        let op = d.queue_filter_change(&FilterChange::Add(rule("b@y.org", "Family"))).unwrap();
        assert!(d.queue_filter_change(&FilterChange::Add(rule("b@y.org", "Family"))).unwrap_err().contains("already waiting"));
        // edit and delete of the cached filter, once each
        let e = d.queue_filter_change(&FilterChange::Edit { id: "F1".into(), rule: rule("c@z.io", "Family") }).unwrap();
        assert!(d.queue_filter_change(&FilterChange::Delete { id: "F1".into() }).unwrap_err().contains("already waiting"));
        let ops = d.pending_rule_ops().unwrap();
        assert_eq!(ops.iter().map(|o| (o.id, o.kind.as_str())).collect::<Vec<_>>(), vec![(op, "gmail_add"), (e, "gmail_edit")]);
        assert_eq!(ops[1].payload["expect"], json!(gf::fingerprint(&f)));
        assert!(d.queue_filter_change(&FilterChange::Delete { id: "nope".into() }).unwrap_err().contains("no such filter"));
    }

    #[test]
    fn graph_rule_changes_are_refused_for_a_gmail_account_even_with_filters_cached() {
        use crate::db::rules::RuleChange;
        use crate::rules::{Match, NewRule};
        let d = gmail_db();
        cache(&d);
        let new = NewRule { tag: "Family".into(), matcher: Match::Address("a@x.com".into()), enabled: true, stop: true };
        assert!(d.queue_rule_change(&RuleChange::Add(new.clone())).unwrap_err().contains("not Exchange rules"));
        assert!(d.queue_rule_change(&RuleChange::Delete { id: "F1".into() }).unwrap_err().contains("not Exchange rules"));
        assert!(d.rule_preview(&new).unwrap_err().contains("not Exchange rules"));
        assert!(d.pending_rule_ops().unwrap().is_empty(), "nothing was queued");
    }

    #[test]
    fn a_filter_omalook_did_not_write_cannot_be_edited_or_deleted() {
        let d = gmail_db();
        let other = json!({ "id": "F9", "criteria": { "from": "a@x.com" }, "action": { "addLabelIds": ["Label_7"], "removeLabelIds": ["INBOX"] }, "omalookTags": ["Family"] });
        d.replace_rules(&[other]).unwrap();
        assert!(d.queue_filter_change(&FilterChange::Delete { id: "F9".into() }).unwrap_err().contains("cannot be deleted"));
        assert!(d.queue_filter_change(&FilterChange::Edit { id: "F9".into(), rule: rule("a@x.com", "Family") }).unwrap_err().contains("cannot be edited"));
        let s = d.filter_summaries("g1");
        assert_eq!((s.len(), s[0].editable), (1, false));
    }
}

#[cfg(test)]
impl Database {
    /// Raw connection for tests in other modules that need to seed rows.
    pub(crate) fn conn_for_tests(&self) -> &rusqlite::Connection { &self.conn }
}
