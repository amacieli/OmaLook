//! Cache of each account's mailbox rules (Graph `messageRules`), kept as the provider's raw JSON so the
//! summary wording can improve without a re-fetch. Read-only in phase 2 (OL-040): nothing here is pushed.

use super::Database;
use crate::errors::Result;
use rusqlite::params;
use serde_json::Value;

/// A rule change the user asked for, waiting for (or finished by) the mail daemon.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RuleOp {
    pub id: i64,
    pub account_id: String,
    /// `add`, `enable`, `delete` or `move`.
    pub kind: String,
    pub payload: Value,
    /// `pending`, `done`, `failed` (the server refused or did something unexpected), `conflict` (the rule was
    /// changed elsewhere since the user looked), `exists` (an identical rule is already there).
    pub state: String,
    pub message: String,
    pub attempts: i64,
    pub created_at: i64,
}

/// A rule change as asked for in the UI (before it becomes a queued [`RuleOp`]).
#[derive(Debug, Clone, PartialEq)]
pub enum RuleChange {
    Add(crate::rules::NewRule),
    Enable { id: String, on: bool },
    Delete { id: String },
    /// One place up or down in the whole list.
    Move { id: String, up: bool },
}

/// What a new rule would do, worked out from the cache: shown before the user confirms.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RulePreview {
    /// Cached messages from that sender / domain.
    pub matches: i64,
    /// Of those, how many already carry the tag.
    pub already_tagged: i64,
    /// Earlier enabled rules that already catch this sender and stop (the new rule, last in the list, would never run).
    pub shadowed_by: Vec<String>,
    /// An identical rule is already on the server.
    pub exists: bool,
}

/// What the last fetch for an account said.
#[derive(Debug, Clone, PartialEq)]
pub struct RulesState {
    /// `ok`, `needs_signin` (permission missing), `unsupported` (provider has no server rules here), `error`.
    pub state: String,
    pub error: String,
    pub fetched_at: i64,
}

impl Database {
    pub(crate) fn init_rules_schema(&self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS mail_rules (
                account_id TEXT NOT NULL,
                rule_id TEXT NOT NULL,
                sequence INTEGER NOT NULL DEFAULT 0,
                raw TEXT NOT NULL,
                PRIMARY KEY (account_id, rule_id)
            )", [])?;
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS rules_state (
                account_id TEXT PRIMARY KEY,
                state TEXT NOT NULL,
                error TEXT NOT NULL DEFAULT '',
                fetched_at INTEGER NOT NULL DEFAULT 0
            )", [])?;
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS rule_ops (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                account_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                payload TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'pending',
                message TEXT NOT NULL DEFAULT '',
                attempts INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL
            )", [])?;
        Ok(())
    }

    pub(crate) fn now_secs() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
    }

    /// Queue a rule change for this account's daemon.
    pub fn enqueue_rule_op(&self, kind: &str, payload: &Value) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO rule_ops (account_id, kind, payload, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![self.account_id, kind, payload.to_string(), Self::now_secs()])?;
        Ok(self.conn.last_insert_rowid())
    }

    fn row_to_op(r: &rusqlite::Row) -> rusqlite::Result<RuleOp> {
        let payload: String = r.get(3)?;
        Ok(RuleOp { id: r.get(0)?, account_id: r.get(1)?, kind: r.get(2)?, payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
            state: r.get(4)?, message: r.get(5)?, attempts: r.get(6)?, created_at: r.get(7)? })
    }

    /// This account's waiting changes, oldest first.
    pub fn pending_rule_ops(&self) -> Result<Vec<RuleOp>> {
        let mut st = self.conn.prepare(
            "SELECT id, account_id, kind, payload, state, message, attempts, created_at FROM rule_ops
             WHERE account_id = ?1 AND state = 'pending' ORDER BY id")?;
        let rows = st.query_map(params![self.account_id], Self::row_to_op)?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Count one more try (a transient failure before anything was written). Returns the new count.
    pub fn bump_rule_op_attempts(&self, id: i64) -> Result<i64> {
        self.conn.execute("UPDATE rule_ops SET attempts = attempts + 1 WHERE id = ?1", params![id])?;
        Ok(self.conn.query_row("SELECT attempts FROM rule_ops WHERE id = ?1", params![id], |r| r.get(0))?)
    }

    pub fn finish_rule_op(&self, id: i64, state: &str, message: &str) -> Result<()> {
        self.conn.execute("UPDATE rule_ops SET state = ?1, message = ?2 WHERE id = ?3 AND state = 'pending'", params![state, message, id])?;
        Ok(())
    }

    /// Changes that are still waiting, plus finished ones from the last `since_secs` (newest first).
    pub fn recent_rule_ops(&self, account_id: &str, since_secs: i64) -> Result<Vec<RuleOp>> {
        let cutoff = Self::now_secs() - since_secs;
        let mut st = self.conn.prepare(
            "SELECT id, account_id, kind, payload, state, message, attempts, created_at FROM rule_ops
             WHERE account_id = ?1 AND (state = 'pending' OR created_at >= ?2) ORDER BY id DESC LIMIT 30")?;
        let rows = st.query_map(params![account_id, cutoff], Self::row_to_op)?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn cached_rules_for_write(&self) -> std::result::Result<Vec<Value>, String> {
        match self.rules_state(self.account_id()).map_err(|e| e.to_string())? {
            Some(s) if s.state == "ok" => {}
            Some(s) if s.state == "unsupported" => return Err("this account's rules cannot be changed from OmaLook".into()),
            Some(s) if s.state == "needs_signin" => return Err("sign in again to let OmaLook read and change this account's rules".into()),
            _ => return Err("this account's rules have not been read yet; wait for the next sync".into()),
        }
        self.rules_raw(self.account_id()).map_err(|e| e.to_string())
    }

    /// Cached counts and warnings for a rule that does not exist yet.
    pub fn rule_preview(&self, rule: &crate::rules::NewRule) -> std::result::Result<RulePreview, String> {
        let rules = self.cached_rules_for_write()?;
        let like = match &rule.matcher {
            crate::rules::Match::Address(a) => a.trim().to_lowercase(),
            crate::rules::Match::Domain(d) => format!("%@{}", d.trim().trim_start_matches('@').to_lowercase().replace('%', "").replace('_', "\\_")),
        };
        let esc = if matches!(rule.matcher, crate::rules::Match::Domain(_)) { " ESCAPE '\\'" } else { "" };
        let sql = format!("SELECT COUNT(*), COALESCE(SUM(CASE WHEN EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid(categories) THEN categories ELSE '[]' END) j WHERE lower(trim(j.value)) = ?3) THEN 1 ELSE 0 END), 0) \
             FROM messages WHERE account_id = ?1 AND lower(from_email) LIKE ?2{}", esc);
        let (matches, already): (i64, i64) = self.conn
            .query_row(&sql, params![self.account_id(), like, rule.tag.trim().to_lowercase()], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?;
        let body = crate::rules::new_rule_body(rule);
        let exists = rules.iter().any(|a| crate::rules::same_effect(a, &body));
        Ok(RulePreview { matches, already_tagged: already, shadowed_by: crate::rules::shadowed_by(&rule.matcher, &rules), exists })
    }

    /// Check a change against the cached rules and queue it for the daemon. The checks here are for a quick,
    /// clear answer; the daemon repeats them against the server's own copy before writing anything.
    pub fn queue_rule_change(&self, change: &RuleChange) -> std::result::Result<i64, String> {
        use crate::rules as r;
        let rules = self.cached_rules_for_write()?;
        let pending = self.pending_rule_ops().map_err(|e| e.to_string())?;
        let touching = |id: &str| pending.iter().any(|o| o.payload["id"].as_str() == Some(id) || o.payload["neighbour_id"].as_str() == Some(id));
        let find = |id: &str| rules.iter().find(|x| x["id"].as_str() == Some(id)).ok_or_else(|| "no such rule (the list may have changed; it has been refreshed)".to_string());
        let (kind, payload) = match change {
            RuleChange::Add(n) => {
                if !r::valid_tag_name(&n.tag) { return Err("not a usable tag name".into()) }
                match &n.matcher {
                    r::Match::Address(a) if !r::valid_address(a) => return Err("that is not an email address".into()),
                    r::Match::Domain(d) if !r::valid_domain(d) => return Err("that is not a domain (like example.org)".into()),
                    _ => {}
                }
                let body = r::new_rule_body(n);
                if pending.iter().any(|o| o.kind == "add" && crate::rules::same_effect(&o.payload["body"], &body)) {
                    return Err("that rule is already waiting to be created".into())
                }
                ("add", serde_json::json!({ "body": body }))
            }
            RuleChange::Enable { id, on } => {
                let rule = find(id)?;
                r::writable(rule)?;
                if touching(id) { return Err("a change to this rule is already waiting".into()) }
                ("enable", serde_json::json!({ "id": id, "on": on, "expect": r::fingerprint(rule) }))
            }
            RuleChange::Delete { id } => {
                let rule = find(id)?;
                r::writable(rule)?;
                if touching(id) { return Err("a change to this rule is already waiting".into()) }
                ("delete", serde_json::json!({ "id": id, "expect": r::fingerprint(rule) }))
            }
            RuleChange::Move { id, up } => {
                let rule = find(id)?;
                r::writable(rule)?;
                if touching(id) { return Err("a change to this rule is already waiting".into()) }
                let mut order: Vec<&Value> = rules.iter().collect();
                order.sort_by_key(|x| x["sequence"].as_i64().unwrap_or(i64::MAX));
                let at = order.iter().position(|x| x["id"].as_str() == Some(id.as_str())).unwrap_or(0);
                let n = if *up { at.checked_sub(1) } else { Some(at + 1) }.and_then(|i| order.get(i)).ok_or_else(|| if *up { "already first" } else { "already last" }.to_string())?;
                if n["isReadOnly"].as_bool() == Some(true) || r::summarize(n).kind == "system" { return Err("a read-only rule is in the way".into()) }
                let nid = n["id"].as_str().unwrap_or_default();
                if touching(nid) { return Err("a change to the neighbouring rule is already waiting".into()) }
                ("move", serde_json::json!({ "id": id, "expect": r::fingerprint(rule), "from": rule["sequence"], "to": n["sequence"], "neighbour_id": nid }))
            }
        };
        self.enqueue_rule_op(kind, &payload).map_err(|e| e.to_string())
    }

    /// Replace this account's cached rules with what the provider just returned.
    pub fn replace_rules(&self, rules: &[Value]) -> Result<()> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        self.conn.execute_batch("BEGIN")?;
        let r = (|| -> Result<()> {
            self.conn.execute("DELETE FROM mail_rules WHERE account_id = ?1", params![self.account_id])?;
            for rule in rules {
                let Some(id) = rule["id"].as_str() else { continue };
                self.conn.execute(
                    "INSERT OR REPLACE INTO mail_rules (account_id, rule_id, sequence, raw) VALUES (?1, ?2, ?3, ?4)",
                    params![self.account_id, id, rule["sequence"].as_i64().unwrap_or(0), rule.to_string()])?;
            }
            self.conn.execute(
                "INSERT INTO rules_state (account_id, state, error, fetched_at) VALUES (?1, 'ok', '', ?2)
                 ON CONFLICT(account_id) DO UPDATE SET state = 'ok', error = '', fetched_at = ?2",
                params![self.account_id, now])?;
            Ok(())
        })();
        match r {
            Ok(()) => { self.conn.execute_batch("COMMIT")?; Ok(()) }
            Err(e) => { let _ = self.conn.execute_batch("ROLLBACK"); Err(e) }
        }
    }

    /// The fetch failed or is not possible: keep the cached rules, record why.
    pub fn set_rules_state(&self, state: &str, error: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO rules_state (account_id, state, error, fetched_at) VALUES (?1, ?2, ?3, 0)
             ON CONFLICT(account_id) DO UPDATE SET state = ?2, error = ?3",
            params![self.account_id, state, error])?;
        Ok(())
    }

    /// Cached raw rules of one account, in rule order.
    pub fn rules_raw(&self, account_id: &str) -> Result<Vec<Value>> {
        let mut st = self.conn.prepare("SELECT raw FROM mail_rules WHERE account_id = ?1 ORDER BY sequence, rule_id")?;
        let rows = st.query_map(params![account_id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().filter_map(|s| serde_json::from_str(&s).ok()).collect())
    }

    /// (id, provider, email) of every account, oldest first.
    pub fn accounts_basic(&self) -> Result<Vec<(String, String, String)>> {
        let mut st = self.conn.prepare("SELECT id, provider, COALESCE(email,'') FROM accounts ORDER BY created_at, id")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn rules_state(&self, account_id: &str) -> Result<Option<RulesState>> {
        use rusqlite::OptionalExtension;
        Ok(self.conn.query_row(
            "SELECT state, error, fetched_at FROM rules_state WHERE account_id = ?1", params![account_id],
            |r| Ok(RulesState { state: r.get(0)?, error: r.get(1)?, fetched_at: r.get(2)? })).optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rule_ops_queue_finish_once_and_list_recent() {
        let a = Database::open_for_account(":memory:", "ex").unwrap();
        let id = a.enqueue_rule_op("add", &json!({ "tag": "T" })).unwrap();
        let id2 = a.enqueue_rule_op("delete", &json!({ "id": "r1" })).unwrap();
        assert_eq!(a.pending_rule_ops().unwrap().iter().map(|o| o.id).collect::<Vec<_>>(), vec![id, id2], "oldest first");
        assert_eq!(a.pending_rule_ops().unwrap()[0].payload["tag"], "T");
        assert_eq!(a.bump_rule_op_attempts(id).unwrap(), 1);
        a.finish_rule_op(id, "done", "").unwrap();
        a.finish_rule_op(id, "failed", "second answer is ignored").unwrap();
        let recent = a.recent_rule_ops("ex", 60).unwrap();
        assert_eq!(recent.iter().map(|o| (o.id, o.state.as_str())).collect::<Vec<_>>(), vec![(id2, "pending"), (id, "done")], "newest first");
        assert_eq!(a.pending_rule_ops().unwrap().len(), 1);
        assert!(a.recent_rule_ops("other", 60).unwrap().is_empty());
    }

    #[test]
    fn replace_keeps_one_accounts_rules_apart_from_another_and_records_state() {
        let a = Database::open_for_account(":memory:", "ex").unwrap();
        a.replace_rules(&[json!({ "id": "r2", "sequence": 2, "displayName": "two" }), json!({ "id": "r1", "sequence": 1 }), json!({ "noid": true })]).unwrap();
        let got = a.rules_raw("ex").unwrap();
        assert_eq!(got.iter().map(|r| r["id"].as_str().unwrap()).collect::<Vec<_>>(), vec!["r1", "r2"], "rule order, rows without an id skipped");
        assert!(a.rules_raw("other").unwrap().is_empty());
        assert_eq!(a.rules_state("ex").unwrap().unwrap().state, "ok");
        // a failed fetch keeps what we had and says why
        a.set_rules_state("needs_signin", "403").unwrap();
        assert_eq!(a.rules_raw("ex").unwrap().len(), 2);
        let st = a.rules_state("ex").unwrap().unwrap();
        assert_eq!((st.state.as_str(), st.error.as_str()), ("needs_signin", "403"));
        assert!(st.fetched_at > 0, "the time of the last good fetch is kept");
        // the next good fetch replaces the set (a rule deleted in Outlook disappears)
        a.replace_rules(&[json!({ "id": "r1", "sequence": 1 })]).unwrap();
        assert_eq!(a.rules_raw("ex").unwrap().len(), 1);
        assert_eq!(a.rules_state("ex").unwrap().unwrap().state, "ok");
        assert!(a.rules_state("nobody").unwrap().is_none());
    }

    fn ok_account(a: &Database, rules: &[Value]) {
        a.replace_rules(rules).unwrap();
        a.set_rules_state("ok", "").unwrap();
    }

    fn tag_rule(id: &str, seq: i64, sender: &str, tag: &str) -> Value {
        json!({ "id": id, "displayName": id, "sequence": seq, "isEnabled": true, "isReadOnly": false,
                "conditions": { "senderContains": [sender] }, "actions": { "assignCategories": [tag], "stopProcessingRules": true } })
    }

    #[test]
    fn queueing_checks_the_cache_and_builds_guarded_ops() {
        use crate::rules::{Match, NewRule};
        let a = Database::open_for_account(":memory:", "ex").unwrap();
        let add = |m: Match| RuleChange::Add(NewRule { tag: "Family".into(), matcher: m, enabled: true, stop: true });
        assert!(a.queue_rule_change(&add(Match::Address("a@b.co".into()))).unwrap_err().contains("not been read"), "no rules read yet");
        let cp = json!({ "id": "cp", "displayName": "CP1", "sequence": 1, "isEnabled": true, "isReadOnly": true, "conditions": {}, "actions": {} });
        let fwd = json!({ "id": "fwd", "displayName": "f", "sequence": 4, "isEnabled": true, "isReadOnly": false,
                          "conditions": { "senderContains": ["Q"] }, "actions": { "forwardTo": [{ "emailAddress": { "address": "e@x.net" } }] } });
        ok_account(&a, &[cp, tag_rule("t1", 2, "ONE.ORG", "One"), tag_rule("t2", 3, "TWO.ORG", "Two"), fwd, tag_rule("t3", 5, "THREE.ORG", "Three")]);

        // add: validated, queued once
        assert!(a.queue_rule_change(&add(Match::Address("nope".into()))).unwrap_err().contains("email address"));
        assert!(a.queue_rule_change(&add(Match::Domain("nope".into()))).unwrap_err().contains("domain"));
        assert!(a.queue_rule_change(&RuleChange::Add(NewRule { tag: "a,b".into(), matcher: Match::Address("a@b.co".into()), enabled: true, stop: true })).unwrap_err().contains("tag name"));
        assert!(a.queue_rule_change(&add(Match::Address("a@b.co".into()))).is_ok());
        assert!(a.queue_rule_change(&add(Match::Address("A@B.co".into()))).unwrap_err().contains("already waiting"));

        // enable / delete: only plain tag rules; the op carries a fingerprint of what the user saw
        assert!(a.queue_rule_change(&RuleChange::Enable { id: "cp".into(), on: false }).unwrap_err().contains("read-only"));
        assert!(a.queue_rule_change(&RuleChange::Delete { id: "fwd".into() }).unwrap_err().contains("only plain tag rules"));
        assert!(a.queue_rule_change(&RuleChange::Delete { id: "ghost".into() }).unwrap_err().contains("no such rule"));
        a.queue_rule_change(&RuleChange::Enable { id: "t1".into(), on: false }).unwrap();
        let op = a.pending_rule_ops().unwrap().into_iter().find(|o| o.kind == "enable").unwrap();
        assert_eq!(op.payload["expect"], crate::rules::fingerprint(&a.rules_raw("ex").unwrap().into_iter().find(|x| x["id"] == "t1").unwrap()));
        assert!(a.queue_rule_change(&RuleChange::Delete { id: "t1".into() }).unwrap_err().contains("already waiting"));

        // move: neighbour by order, never past a read-only rule, never past the ends
        assert!(a.queue_rule_change(&RuleChange::Move { id: "t2".into(), up: true }).unwrap_err().contains("already waiting"), "t1 is its neighbour and has a change waiting");
        assert!(a.queue_rule_change(&RuleChange::Move { id: "t3".into(), up: false }).unwrap_err().contains("already last"));
        a.queue_rule_change(&RuleChange::Move { id: "t3".into(), up: true }).expect("moving past a forward rule is fine; only read-only ones block");
        let mv = a.pending_rule_ops().unwrap().into_iter().find(|o| o.kind == "move").unwrap();
        assert_eq!((mv.payload["from"].as_i64(), mv.payload["to"].as_i64(), mv.payload["neighbour_id"].as_str()), (Some(5), Some(4), Some("fwd")));
        // the first rule after the read-only one cannot go up
        let b = Database::open_for_account(":memory:", "ex2").unwrap();
        ok_account(&b, &[json!({ "id": "cp", "displayName": "CP1", "sequence": 1, "isEnabled": true, "isReadOnly": true, "conditions": {}, "actions": {} }), tag_rule("t1", 2, "ONE.ORG", "One")]);
        assert!(b.queue_rule_change(&RuleChange::Move { id: "t1".into(), up: true }).unwrap_err().contains("read-only"));
    }

    #[test]
    fn preview_counts_cached_mail_from_the_sender_or_domain_and_warns_about_shadowing() {
        use crate::models::EmailMessage;
        use crate::rules::{Match, NewRule};
        let a = Database::open_for_account(":memory:", "ex").unwrap();
        a.conn.execute("INSERT INTO accounts (id, provider, email) VALUES ('ex', 'exchange', 'me@work.org')", []).unwrap();
        let put = |id: &str, from: &str, cats: &[&str]| a.insert_email(&EmailMessage { id: id.into(), from: from.into(), subject: "s".into(),
            received: "2026-10-06T12:00:00Z".into(), body: "p".into(), folder_id: Some("f".into()), is_read: true, meta: true,
            categories: Some(cats.iter().map(|s| s.to_string()).collect()), ..Default::default() }).unwrap();
        put("1", "Pastor@Carmel.org", &["Church"]); put("2", "office@carmel.org", &[]); put("3", "x@mail.carmel.org", &[]); put("4", "z@notcarmel.org", &[]);
        put("5", "a_b@carmel.org", &["church"]);
        ok_account(&a, &[tag_rule("t1", 1, "OFFICE@CARMEL.ORG", "Other")]);
        let new = |m| NewRule { tag: "Church".into(), matcher: m, enabled: true, stop: true };
        let d = a.rule_preview(&new(Match::Domain("carmel.org".into()))).unwrap();
        assert_eq!((d.matches, d.already_tagged), (3, 2), "pastor@, office@ and a_b@ are at carmel.org; mail.carmel.org and notcarmel.org are not");
        let one = a.rule_preview(&new(Match::Address("office@carmel.org".into()))).unwrap();
        assert_eq!((one.matches, one.already_tagged, one.shadowed_by.len()), (1, 0, 1), "the existing rule for that address comes first and stops");
        assert!(!one.exists);
        let none = a.rule_preview(&new(Match::Address("nobody@else.org".into()))).unwrap();
        assert_eq!((none.matches, none.shadowed_by.len()), (0, 0));
        let pct = a.rule_preview(&new(Match::Domain("car%el.org".into()))).unwrap();
        assert_eq!(pct.matches, 0, "a percent sign in the text is not a wildcard");
    }
}
