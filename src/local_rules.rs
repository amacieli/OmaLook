//! Local rule engine (docs/PLAN-tags-and-rules.md, OL-040 phase 4): pure evaluation, no I/O.
//!
//! Provider rules (Graph `messageRule`) are flat: ALL of the condition predicates, each predicate matching
//! ANY of its values, unless ANY exception predicate matches. The local engine is a superset: a rule's `when`
//! is a tree of `all` / `any` / `not` over predicates, so parentheses and OR across different fields work
//! here, for every account. `to_graph()` says whether a rule also fits the flat provider shape, which is
//! what decides the engine badge (server or local) in the UI.
//!
//! Matching is case-insensitive "contains" like Outlook's; `from` / `to` match addresses, `domain` the part
//! after the @. Empty value lists never match (a predicate with no values is rejected by `validate`).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    /// Sender address.
    From,
    /// Sender display name.
    FromName,
    /// To or Cc addresses.
    Recipient,
    Subject,
    Body,
    BodyOrSubject,
    /// Sender domain (exact, or a subdomain of it).
    Domain,
    /// Category already on the message (exact, case-insensitive).
    Category,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Flag {
    HasAttachments,
    HighImportance,
    LowImportance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Expr {
    /// Text predicate: matches when ANY of `values` matches the field.
    Text { field: Field, values: Vec<String> },
    Flag { flag: Flag, #[serde(default = "yes")] is: bool },
    All { of: Vec<Expr> },
    Any { of: Vec<Expr> },
    Not { of: Box<Expr> },
}

fn yes() -> bool { true }

/// Actions the local engine may take. Tags only in phase 4 (plan §5: nothing destructive without the
/// phase-7 confirm gate); `stop` ends rule processing for the message like Outlook's flag.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Actions {
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub stop: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalRule {
    pub id: String,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Lower runs first.
    #[serde(default)]
    pub sequence: i64,
    /// Account ids; empty = every account.
    #[serde(default)]
    pub applies_to: Vec<String>,
    pub when: Expr,
    #[serde(default)]
    pub actions: Actions,
}

/// What the engine looks at.
#[derive(Debug, Clone, Default)]
pub struct Msg {
    pub account_id: String,
    pub from_email: String,
    pub from_name: String,
    pub to_text: String,
    pub cc_text: String,
    pub subject: String,
    pub body: String,
    pub importance: String,
    pub has_attachments: bool,
    pub categories: Vec<String>,
}

fn has(hay: &str, needle: &str) -> bool {
    !needle.trim().is_empty() && hay.to_lowercase().contains(&needle.trim().to_lowercase())
}

fn domain_matches(addr: &str, dom: &str) -> bool {
    let d = dom.trim().trim_start_matches('@').to_lowercase();
    if d.is_empty() { return false; }
    let Some((_, host)) = addr.trim().to_lowercase().rsplit_once('@').map(|(a, b)| (a.to_string(), b.to_string())) else { return false };
    host == d || host.ends_with(&format!(".{d}"))
}

impl Expr {
    pub fn eval(&self, m: &Msg) -> bool {
        match self {
            Expr::Text { field, values } => values.iter().any(|v| match field {
                Field::From => has(&m.from_email, v),
                Field::FromName => has(&m.from_name, v),
                Field::Recipient => has(&m.to_text, v) || has(&m.cc_text, v),
                Field::Subject => has(&m.subject, v),
                Field::Body => has(&m.body, v),
                Field::BodyOrSubject => has(&m.body, v) || has(&m.subject, v),
                Field::Domain => domain_matches(&m.from_email, v),
                Field::Category => m.categories.iter().any(|c| c.trim().eq_ignore_ascii_case(v.trim())),
            }),
            Expr::Flag { flag, is } => {
                let actual = match flag {
                    Flag::HasAttachments => m.has_attachments,
                    Flag::HighImportance => m.importance.eq_ignore_ascii_case("high"),
                    Flag::LowImportance => m.importance.eq_ignore_ascii_case("low"),
                };
                actual == *is
            }
            // An empty `all` is rejected by validate(); evaluating one as "no match" keeps a broken rule inert.
            Expr::All { of } => !of.is_empty() && of.iter().all(|e| e.eval(m)),
            Expr::Any { of } => of.iter().any(|e| e.eval(m)),
            Expr::Not { of } => !of.eval(m),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Expr::Text { values, .. } => {
                if values.iter().all(|v| v.trim().is_empty()) { Err("a text condition needs at least one value".into()) } else { Ok(()) }
            }
            Expr::Flag { .. } => Ok(()),
            Expr::All { of } | Expr::Any { of } => {
                if of.is_empty() { return Err("an empty group matches nothing".into()); }
                of.iter().try_for_each(|e| e.validate())
            }
            Expr::Not { of } => of.validate(),
        }
    }

    /// Nesting depth, to cap what the editor lets through.
    pub fn depth(&self) -> usize {
        match self {
            Expr::All { of } | Expr::Any { of } => 1 + of.iter().map(|e| e.depth()).max().unwrap_or(0),
            Expr::Not { of } => 1 + of.depth(),
            _ => 0,
        }
    }
}

impl LocalRule {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() { return Err("a rule needs a name".into()); }
        self.when.validate()?;
        if self.when.depth() > 4 { return Err("conditions nest too deeply (max 4)".into()); }
        if self.actions.tags.iter().all(|t| t.trim().is_empty()) { return Err("a rule needs at least one tag".into()); }
        Ok(())
    }

    pub fn applies(&self, account_id: &str) -> bool {
        self.applies_to.is_empty() || self.applies_to.iter().any(|a| a == account_id)
    }
}

/// (rule id, tag) pairs the enabled rules would add to `m`, in rule order, each tag once. Honours `stop` and
/// tags the message already has (never "adds" a tag that is present). `rules` need not be sorted.
pub fn additions(rules: &[LocalRule], m: &Msg) -> Vec<(String, String)> {
    let mut sorted: Vec<&LocalRule> = rules.iter().filter(|r| r.enabled && r.applies(&m.account_id)).collect();
    sorted.sort_by_key(|r| r.sequence);
    let mut out: Vec<(String, String)> = Vec::new();
    for r in sorted {
        if !r.when.eval(m) { continue; }
        for t in &r.actions.tags {
            let t = t.trim();
            let same = |c: &str| c.trim().eq_ignore_ascii_case(t);
            if !t.is_empty() && !m.categories.iter().any(|c| same(c)) && !out.iter().any(|(_, c)| same(c)) {
                out.push((r.id.clone(), t.to_string()));
            }
        }
        if r.actions.stop { break; }
    }
    out
}

/// Just the tags of `additions`.
pub fn tags_for(rules: &[LocalRule], m: &Msg) -> Vec<String> {
    additions(rules, m).into_iter().map(|(_, t)| t).collect()
}

/// The same rule as Graph `conditions` / `exceptions` objects, when it fits the flat provider shape:
/// `all` of text/flag predicates (a lone predicate counts as an `all` of one), optionally with one top-level
/// `not(any(...))` as the exceptions. Anything else (nested OR across fields, `not` inside) returns None, which
/// means local engine only. Domain and from_name have no exact Graph form and are not exported.
pub fn to_graph(rule: &LocalRule) -> Option<Value> {
    let parts: Vec<&Expr> = match &rule.when {
        Expr::All { of } => of.iter().collect(),
        other => vec![other],
    };
    let mut cond = serde_json::Map::new();
    let mut exc = serde_json::Map::new();
    for p in parts {
        match p {
            Expr::Not { of } => {
                let inner: Vec<&Expr> = match &**of { Expr::Any { of } => of.iter().collect(), o => vec![o] };
                for e in inner { put(&mut exc, e)?; }
            }
            e => put(&mut cond, e)?,
        }
    }
    Some(json!({ "conditions": cond, "exceptions": exc,
                 "actions": { "assignCategories": rule.actions.tags, "stopProcessingRules": rule.actions.stop } }))
}

fn put(m: &mut serde_json::Map<String, Value>, e: &Expr) -> Option<()> {
    let (key, val) = match e {
        Expr::Text { field, values } => {
            let k = match field {
                Field::From => "senderContains",
                Field::Recipient => "recipientContains",
                Field::Subject => "subjectContains",
                Field::Body => "bodyContains",
                Field::BodyOrSubject => "bodyOrSubjectContains",
                Field::Category => "categories",
                Field::Domain | Field::FromName => return None,
            };
            (k, json!(values))
        }
        Expr::Flag { flag: Flag::HasAttachments, is: true } => ("hasAttachments", json!(true)),
        Expr::Flag { flag: Flag::HighImportance, is: true } => ("importance", json!("high")),
        Expr::Flag { flag: Flag::LowImportance, is: true } => ("importance", json!("low")),
        _ => return None,
    };
    // Two predicates of the same kind would have to be ANDed; a flat Graph object cannot hold both.
    if m.contains_key(key) { return None; }
    m.insert(key.to_string(), val);
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(f: Field, v: &[&str]) -> Expr { Expr::Text { field: f, values: v.iter().map(|s| s.to_string()).collect() } }
    fn rule(id: &str, seq: i64, when: Expr, tags: &[&str], stop: bool) -> LocalRule {
        LocalRule { id: id.into(), name: id.into(), enabled: true, sequence: seq, applies_to: vec![], when,
            actions: Actions { tags: tags.iter().map(|s| s.to_string()).collect(), stop } }
    }
    fn msg() -> Msg {
        Msg { account_id: "a1".into(), from_email: "Bob@Example.com".into(), from_name: "Bob B".into(),
              to_text: "me@x.net".into(), subject: "Invoice 42".into(), body: "please pay".into(),
              importance: "high".into(), ..Default::default() }
    }

    #[test]
    fn values_in_one_predicate_are_or_and_case_insensitive() {
        assert!(text(Field::Subject, &["nope", "INVOICE"]).eval(&msg()));
        assert!(!text(Field::Subject, &["nope", "zzz"]).eval(&msg()));
    }

    #[test]
    fn all_is_and_any_is_or_not_negates_and_empty_is_inert() {
        let m = msg();
        let a = text(Field::From, &["bob@"]);
        let b = text(Field::Subject, &["nothing"]);
        assert!(!Expr::All { of: vec![a.clone(), b.clone()] }.eval(&m));
        assert!(Expr::Any { of: vec![a.clone(), b.clone()] }.eval(&m));
        assert!(Expr::Not { of: Box::new(b) }.eval(&m));
        assert!(!Expr::All { of: vec![] }.eval(&m));
        assert!(!text(Field::Subject, &[""]).eval(&m), "blank value never matches everything");
    }

    #[test]
    fn parentheses_work_locally() {
        // (from bob OR from alice) AND (subject invoice OR subject receipt) AND NOT body "spam"
        let e = Expr::All { of: vec![
            Expr::Any { of: vec![text(Field::From, &["bob@"]), text(Field::From, &["alice@"])] },
            Expr::Any { of: vec![text(Field::Subject, &["invoice"]), text(Field::Subject, &["receipt"])] },
            Expr::Not { of: Box::new(text(Field::Body, &["spam"])) },
        ]};
        assert!(e.eval(&msg()));
        let mut spam = msg(); spam.body = "total SPAM".into();
        assert!(!e.eval(&spam));
    }

    #[test]
    fn domain_matches_exact_and_subdomain_but_not_lookalikes() {
        let mut m = msg();
        assert!(text(Field::Domain, &["example.com"]).eval(&m));
        assert!(text(Field::Domain, &["@example.com"]).eval(&m));
        m.from_email = "x@mail.example.com".into();
        assert!(text(Field::Domain, &["example.com"]).eval(&m));
        m.from_email = "x@notexample.com".into();
        assert!(!text(Field::Domain, &["example.com"]).eval(&m));
    }

    #[test]
    fn tags_follow_sequence_stop_and_existing_tags() {
        let r1 = rule("r1", 1, text(Field::From, &["bob@"]), &["Work"], true);
        let r2 = rule("r2", 2, text(Field::Subject, &["invoice"]), &["Money"], false);
        let mut m = msg();
        assert_eq!(tags_for(&[r2.clone(), r1.clone()], &m), vec!["Work"], "r1 first, it stops");
        let r1n = rule("r1", 1, text(Field::From, &["bob@"]), &["Work"], false);
        assert_eq!(tags_for(&[r2.clone(), r1n.clone()], &m), vec!["Work", "Money"]);
        m.categories = vec!["work".into()];
        assert_eq!(tags_for(&[r1n, r2], &m), vec!["Money"], "already tagged: not added again");
    }

    #[test]
    fn disabled_and_other_account_rules_are_skipped() {
        let mut r = rule("r", 1, text(Field::From, &["bob@"]), &["Work"], false);
        r.enabled = false;
        assert!(tags_for(&[r.clone()], &msg()).is_empty());
        r.enabled = true; r.applies_to = vec!["other".into()];
        assert!(tags_for(&[r.clone()], &msg()).is_empty());
        r.applies_to = vec!["a1".into()];
        assert_eq!(tags_for(&[r], &msg()), vec!["Work"]);
    }

    #[test]
    fn validate_rejects_empty_and_deep_rules() {
        assert!(rule("x", 0, Expr::All { of: vec![] }, &["T"], false).validate().is_err());
        assert!(rule("x", 0, text(Field::From, &[" "]), &["T"], false).validate().is_err());
        assert!(rule("x", 0, text(Field::From, &["a"]), &[], false).validate().is_err());
        let mut e = text(Field::From, &["a"]);
        for _ in 0..5 { e = Expr::Not { of: Box::new(e) }; }
        assert!(rule("x", 0, e, &["T"], false).validate().is_err());
        assert!(rule("x", 0, text(Field::From, &["a"]), &["T"], false).validate().is_ok());
    }

    #[test]
    fn graph_export_only_for_flat_shapes() {
        let flat = rule("f", 0, Expr::All { of: vec![
            text(Field::From, &["a@", "b@"]), Expr::Flag { flag: Flag::HasAttachments, is: true },
            Expr::Not { of: Box::new(Expr::Any { of: vec![text(Field::Subject, &["spam"]), text(Field::Body, &["x"])] }) },
        ]}, &["T"], true);
        let g = to_graph(&flat).expect("flat");
        assert_eq!(g["conditions"]["senderContains"], json!(["a@", "b@"]));
        assert_eq!(g["conditions"]["hasAttachments"], json!(true));
        assert_eq!(g["exceptions"]["subjectContains"], json!(["spam"]));
        assert_eq!(g["actions"]["stopProcessingRules"], json!(true));

        let or_across = rule("o", 0, Expr::Any { of: vec![text(Field::From, &["a"]), text(Field::Subject, &["b"])] }, &["T"], false);
        assert!(to_graph(&or_across).is_none(), "OR across fields has no flat form");
        let dup = rule("d", 0, Expr::All { of: vec![text(Field::Subject, &["a"]), text(Field::Subject, &["b"])] }, &["T"], false);
        assert!(to_graph(&dup).is_none(), "two subject predicates ANDed cannot be one Graph object");
        assert!(to_graph(&rule("m", 0, text(Field::Domain, &["x.com"]), &["T"], false)).is_none());
        assert!(to_graph(&rule("s", 0, text(Field::From, &["a"]), &["T"], false)).is_some(), "lone predicate");
    }

    #[test]
    fn json_round_trip() {
        let r = rule("j", 3, Expr::All { of: vec![text(Field::From, &["a"]), Expr::Not { of: Box::new(Expr::Flag { flag: Flag::HasAttachments, is: true }) }] }, &["T"], true);
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<LocalRule>(&s).unwrap(), r);
    }
}
