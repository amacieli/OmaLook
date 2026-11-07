//! Local rule engine (docs/PLAN-tags-and-rules.md, OL-040 phase 4): pure evaluation, no I/O.
//!
//! The model is exactly what a provider can store, nothing more (decision, user, 2026-11-07): a Graph
//! `messageRule` is a flat list of predicates, so a rule here is
//!   - `conditions`: ALL of these predicates must match (each predicate kind at most once);
//!   - inside one predicate: ANY of its values matches;
//!   - `exceptions`: the rule is skipped if ANY of these predicates matches (same shape);
//!   - `actions`: tags to add, and `stop` (stop processing more rules).
//! No groups, no parentheses, no NOT, no OR across different predicates. A rule is therefore always
//! writable to Exchange unchanged (`to_graph`) and any Exchange rule made of supported predicates can be read
//! back (`from_graph`); a rule using anything else is refused, never approximated, so a local result can
//! never differ from what the server would do. (The AND/OR semantics above are documented for Exchange
//! mail-flow rules; for inbox rules they match Outlook's behaviour but are unverified by a live test.)

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// One predicate. Text predicates match when ANY value matches; matching is case-insensitive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Pred {
    /// `fromAddresses`: the sender address equals one of these.
    FromAddresses(Vec<String>),
    /// `senderContains`: the From (address or display name) contains one of these.
    SenderContains(Vec<String>),
    /// `recipientContains`: To or Cc contains one of these.
    RecipientContains(Vec<String>),
    SubjectContains(Vec<String>),
    BodyContains(Vec<String>),
    BodyOrSubjectContains(Vec<String>),
    /// `categories`: the message already carries one of these tags.
    Categories(Vec<String>),
    HasAttachments(bool),
    /// `low`, `normal` or `high`.
    Importance(String),
}

impl Pred {
    /// Graph property name; also the "at most once per list" key.
    pub fn key(&self) -> &'static str {
        match self {
            Pred::FromAddresses(_) => "fromAddresses",
            Pred::SenderContains(_) => "senderContains",
            Pred::RecipientContains(_) => "recipientContains",
            Pred::SubjectContains(_) => "subjectContains",
            Pred::BodyContains(_) => "bodyContains",
            Pred::BodyOrSubjectContains(_) => "bodyOrSubjectContains",
            Pred::Categories(_) => "categories",
            Pred::HasAttachments(_) => "hasAttachments",
            Pred::Importance(_) => "importance",
        }
    }

    fn values(&self) -> Option<&Vec<String>> {
        match self {
            Pred::FromAddresses(v) | Pred::SenderContains(v) | Pred::RecipientContains(v) | Pred::SubjectContains(v)
            | Pred::BodyContains(v) | Pred::BodyOrSubjectContains(v) | Pred::Categories(v) => Some(v),
            _ => None,
        }
    }

    pub fn eval(&self, m: &Msg) -> bool {
        let any = |v: &Vec<String>, f: &dyn Fn(&str) -> bool| v.iter().any(|x| !x.trim().is_empty() && f(x.trim()));
        match self {
            Pred::FromAddresses(v) => any(v, &|x| m.from_email.trim().eq_ignore_ascii_case(x)),
            Pred::SenderContains(v) => any(v, &|x| has(&m.from_email, x) || has(&m.from_name, x)),
            Pred::RecipientContains(v) => any(v, &|x| has(&m.to_text, x) || has(&m.cc_text, x)),
            Pred::SubjectContains(v) => any(v, &|x| has(&m.subject, x)),
            Pred::BodyContains(v) => any(v, &|x| has(&m.body, x)),
            Pred::BodyOrSubjectContains(v) => any(v, &|x| has(&m.body, x) || has(&m.subject, x)),
            Pred::Categories(v) => any(v, &|x| m.categories.iter().any(|c| c.trim().eq_ignore_ascii_case(x))),
            Pred::HasAttachments(b) => m.has_attachments == *b,
            Pred::Importance(i) => m.importance.eq_ignore_ascii_case(i.trim()),
        }
    }
}

fn has(hay: &str, needle: &str) -> bool {
    hay.to_lowercase().contains(&needle.to_lowercase())
}

fn yes() -> bool { true }

/// Whether mail of this provider is tagged by the local engine as it arrives. Exchange has a server rule
/// engine that already runs on delivery, so it is left to the server; anything else (Gmail) is ours.
pub fn has_local_engine(provider: &str) -> bool {
    !provider.eq_ignore_ascii_case("exchange")
}

/// Same tag for the engine's purposes: letters and digits only, case-insensitive, so `Saint Lucia` and
/// `SaintLucia` count as present for each other. The user keeps near-duplicate spellings (plan Q3) and a
/// retroactive run must not stack a second spelling on a message that already has the first.
pub fn tag_eq(a: &str, b: &str) -> bool {
    let k = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect::<String>();
    let (a, b) = (k(a), k(b));
    !a.is_empty() && a == b
}

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
    /// ALL must match.
    pub conditions: Vec<Pred>,
    /// ANY match skips the rule.
    #[serde(default)]
    pub exceptions: Vec<Pred>,
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

fn check_list(what: &str, preds: &[Pred]) -> Result<(), String> {
    let mut seen: Vec<&str> = Vec::new();
    for p in preds {
        if seen.contains(&p.key()) { return Err(format!("{what}: {} appears twice; put all its values in one entry", p.key())); }
        seen.push(p.key());
        if let Some(v) = p.values() {
            if v.iter().all(|x| x.trim().is_empty()) { return Err(format!("{what}: {} needs at least one value", p.key())); }
        }
        if let Pred::Importance(i) = p {
            if !["low", "normal", "high"].contains(&i.trim().to_lowercase().as_str()) { return Err(format!("{what}: importance must be low, normal or high")); }
        }
    }
    Ok(())
}

impl LocalRule {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() { return Err("a rule needs a name".into()); }
        if self.conditions.is_empty() { return Err("a rule needs at least one condition (an empty rule would match every message)".into()); }
        check_list("conditions", &self.conditions)?;
        check_list("exceptions", &self.exceptions)?;
        if self.actions.tags.iter().all(|t| t.trim().is_empty()) { return Err("a rule needs at least one tag".into()); }
        Ok(())
    }

    pub fn applies(&self, account_id: &str) -> bool {
        self.applies_to.is_empty() || self.applies_to.iter().any(|a| a == account_id)
    }

    /// ALL conditions match and NO exception does. An invalid (empty) rule never matches.
    pub fn matches(&self, m: &Msg) -> bool {
        !self.conditions.is_empty() && self.conditions.iter().all(|p| p.eval(m)) && !self.exceptions.iter().any(|p| p.eval(m))
    }
}

/// (rule id, tag) pairs the enabled rules would add to `m`, in rule order, each tag once. Honours `stop` and
/// tags the message already has. `rules` need not be sorted.
pub fn additions(rules: &[LocalRule], m: &Msg) -> Vec<(String, String)> {
    let mut sorted: Vec<&LocalRule> = rules.iter().filter(|r| r.enabled && r.applies(&m.account_id)).collect();
    sorted.sort_by_key(|r| r.sequence);
    let mut out: Vec<(String, String)> = Vec::new();
    for r in sorted {
        if !r.matches(m) { continue; }
        for t in &r.actions.tags {
            let t = t.trim();
            let same = |c: &str| tag_eq(c, t);
            if !t.is_empty() && !m.categories.iter().any(|c| same(c)) && !out.iter().any(|(_, c)| same(c)) {
                out.push((r.id.clone(), t.to_string()));
            }
        }
        if r.actions.stop { break; }
    }
    out
}

pub fn tags_for(rules: &[LocalRule], m: &Msg) -> Vec<String> {
    additions(rules, m).into_iter().map(|(_, t)| t).collect()
}

fn pred_to_json(p: &Pred, out: &mut Map<String, Value>) {
    let addr = |v: &Vec<String>| json!(v.iter().map(|a| json!({ "emailAddress": { "address": a } })).collect::<Vec<_>>());
    let val = match p {
        Pred::FromAddresses(v) => addr(v),
        Pred::SenderContains(v) | Pred::RecipientContains(v) | Pred::SubjectContains(v) | Pred::BodyContains(v)
        | Pred::BodyOrSubjectContains(v) | Pred::Categories(v) => json!(v),
        Pred::HasAttachments(b) => json!(b),
        Pred::Importance(i) => json!(i.trim().to_lowercase()),
    };
    out.insert(p.key().to_string(), val);
}

/// The Graph `messageRule` body for this rule (`displayName`, `sequence`, `isEnabled`, `conditions`,
/// `exceptions`, `actions`). Always possible: the model is the Graph shape.
pub fn to_graph(rule: &LocalRule) -> Value {
    let (mut c, mut e) = (Map::new(), Map::new());
    rule.conditions.iter().for_each(|p| pred_to_json(p, &mut c));
    rule.exceptions.iter().for_each(|p| pred_to_json(p, &mut e));
    json!({ "displayName": rule.name, "sequence": rule.sequence, "isEnabled": rule.enabled,
            "conditions": c, "exceptions": e,
            "actions": { "assignCategories": rule.actions.tags, "stopProcessingRules": rule.actions.stop } })
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| match x {
        Value::String(s) => Some(s.trim().to_string()),
        Value::Object(_) => x["emailAddress"]["address"].as_str().or_else(|| x["address"].as_str()).map(|s| s.trim().to_string()),
        _ => None,
    }).filter(|s| !s.is_empty()).collect()).unwrap_or_default()
}

fn preds_from(obj: &Value, unsupported: &mut Vec<String>) -> Vec<Pred> {
    let mut out = Vec::new();
    let Some(o) = obj.as_object() else { return out };
    for (k, v) in o {
        let set = match v { Value::Null | Value::Bool(false) => false, Value::Array(a) => !a.is_empty(), Value::String(s) => !s.is_empty(), _ => true };
        if !set { continue; }
        match k.as_str() {
            "fromAddresses" => out.push(Pred::FromAddresses(strings(v))),
            "senderContains" => out.push(Pred::SenderContains(strings(v))),
            "recipientContains" => out.push(Pred::RecipientContains(strings(v))),
            "subjectContains" => out.push(Pred::SubjectContains(strings(v))),
            "bodyContains" => out.push(Pred::BodyContains(strings(v))),
            "bodyOrSubjectContains" => out.push(Pred::BodyOrSubjectContains(strings(v))),
            "categories" => out.push(Pred::Categories(strings(v))),
            "hasAttachments" => out.push(Pred::HasAttachments(v.as_bool().unwrap_or(true))),
            "importance" => out.push(Pred::Importance(v.as_str().unwrap_or("").to_string())),
            other => unsupported.push(other.to_string()),
        }
    }
    out
}

/// Read a cached Graph rule. `Err(keys)` lists the condition / exception / action keys this engine cannot
/// reproduce exactly (forward, move, delete, headerContains...): such a rule is not evaluated locally.
/// Only tag rules (`assignCategories`, optionally `stopProcessingRules`) are accepted.
pub fn from_graph(raw: &Value) -> Result<LocalRule, Vec<String>> {
    let mut bad = Vec::new();
    let conditions = preds_from(&raw["conditions"], &mut bad);
    let exceptions = preds_from(&raw["exceptions"], &mut bad);
    let mut actions = Actions::default();
    if let Some(a) = raw["actions"].as_object() {
        for (k, v) in a {
            match k.as_str() {
                "assignCategories" => actions.tags = strings(v),
                "stopProcessingRules" => actions.stop = v.as_bool().unwrap_or(false),
                _ => { let set = match v { Value::Null | Value::Bool(false) => false, Value::Array(x) => !x.is_empty(), _ => true }; if set { bad.push(format!("action:{k}")); } }
            }
        }
    }
    if !bad.is_empty() { return Err(bad); }
    Ok(LocalRule {
        id: raw["id"].as_str().unwrap_or("").to_string(),
        name: raw["displayName"].as_str().unwrap_or("").to_string(),
        enabled: raw["isEnabled"].as_bool().unwrap_or(true),
        sequence: raw["sequence"].as_i64().unwrap_or(0),
        applies_to: vec![],
        conditions, exceptions, actions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> { v.iter().map(|x| x.to_string()).collect() }
    fn rule(id: &str, seq: i64, conditions: Vec<Pred>, tags: &[&str], stop: bool) -> LocalRule {
        LocalRule { id: id.into(), name: id.into(), enabled: true, sequence: seq, applies_to: vec![],
            conditions, exceptions: vec![], actions: Actions { tags: s(tags), stop } }
    }
    fn msg() -> Msg {
        Msg { account_id: "a1".into(), from_email: "Bob@Example.com".into(), from_name: "Bob B".into(),
              to_text: "me@x.net".into(), subject: "Invoice 42".into(), body: "please pay".into(),
              importance: "high".into(), ..Default::default() }
    }

    #[test]
    fn values_in_one_predicate_are_or_and_case_insensitive() {
        assert!(Pred::SubjectContains(s(&["nope", "INVOICE"])).eval(&msg()));
        assert!(!Pred::SubjectContains(s(&["nope", "zzz"])).eval(&msg()));
        assert!(!Pred::SubjectContains(s(&[" "])).eval(&msg()), "blank value never matches everything");
    }

    #[test]
    fn conditions_are_and_exceptions_are_or_and_both_are_flat() {
        let m = msg();
        let mut r = rule("r", 1, vec![Pred::SenderContains(s(&["bob@"])), Pred::SubjectContains(s(&["invoice"]))], &["T"], false);
        assert!(r.matches(&m));
        r.conditions.push(Pred::BodyContains(s(&["absent"])));
        assert!(!r.matches(&m), "one failing condition fails the rule");
        r.conditions.pop();
        r.exceptions = vec![Pred::BodyContains(s(&["nothing"])), Pred::HasAttachments(true)];
        assert!(r.matches(&m), "no exception matches");
        r.exceptions.push(Pred::SubjectContains(s(&["42"])));
        assert!(!r.matches(&m), "any one exception skips the rule");
        assert!(!rule("e", 1, vec![], &["T"], false).matches(&m), "no conditions never matches");
    }

    #[test]
    fn from_addresses_is_exact_and_sender_contains_covers_domain_and_name() {
        let m = msg();
        assert!(Pred::FromAddresses(s(&["bob@example.com"])).eval(&m));
        assert!(!Pred::FromAddresses(s(&["bob@example"])).eval(&m));
        assert!(Pred::SenderContains(s(&["@example.com"])).eval(&m), "a domain rule is senderContains");
        assert!(Pred::SenderContains(s(&["bob b"])).eval(&m));
    }

    #[test]
    fn tags_follow_sequence_stop_and_existing_tags() {
        let r1 = rule("r1", 1, vec![Pred::SenderContains(s(&["bob@"]))], &["Work"], true);
        let r2 = rule("r2", 2, vec![Pred::SubjectContains(s(&["invoice"]))], &["Money"], false);
        let mut m = msg();
        assert_eq!(tags_for(&[r2.clone(), r1.clone()], &m), vec!["Work"], "r1 first, it stops");
        let r1n = rule("r1", 1, vec![Pred::SenderContains(s(&["bob@"]))], &["Work"], false);
        assert_eq!(tags_for(&[r2.clone(), r1n.clone()], &m), vec!["Work", "Money"]);
        m.categories = vec!["work".into()];
        assert_eq!(tags_for(&[r1n.clone(), r2.clone()], &m), vec!["Money"]);
        let spaced = rule("sp", 1, vec![Pred::SenderContains(s(&["bob@"]))], &["SaintLucia"], false);
        m.categories = vec!["Saint Lucia".into()];
        assert!(tags_for(&[spaced], &m).is_empty(), "a near-duplicate spelling counts as already tagged");
    }

    #[test]
    fn disabled_and_other_account_rules_are_skipped() {
        let mut r = rule("r", 1, vec![Pred::SenderContains(s(&["bob@"]))], &["Work"], false);
        r.enabled = false;
        assert!(tags_for(&[r.clone()], &msg()).is_empty());
        r.enabled = true; r.applies_to = vec!["other".into()];
        assert!(tags_for(&[r.clone()], &msg()).is_empty());
        r.applies_to = vec!["a1".into()];
        assert_eq!(tags_for(&[r], &msg()), vec!["Work"]);
    }

    #[test]
    fn validate_refuses_what_graph_cannot_hold() {
        let ok = vec![Pred::SenderContains(s(&["a"]))];
        assert!(rule("x", 0, ok.clone(), &["T"], false).validate().is_ok());
        assert!(rule("x", 0, vec![], &["T"], false).validate().is_err());
        assert!(rule("x", 0, vec![Pred::SenderContains(s(&[" "]))], &["T"], false).validate().is_err());
        assert!(rule("x", 0, ok.clone(), &[], false).validate().is_err());
        let twice = vec![Pred::SubjectContains(s(&["a"])), Pred::SubjectContains(s(&["b"]))];
        assert!(rule("x", 0, twice, &["T"], false).validate().is_err(), "AND within one field is not storable");
        assert!(rule("x", 0, vec![Pred::Importance("urgent".into())], &["T"], false).validate().is_err());
    }

    #[test]
    fn graph_round_trip_and_unsupported_rules_are_refused() {
        let mut r = rule("g", 4, vec![Pred::FromAddresses(s(&["a@b.c"])), Pred::SubjectContains(s(&["x", "y"])), Pred::HasAttachments(true)], &["T"], true);
        r.exceptions = vec![Pred::Importance("low".into())];
        let mut g = to_graph(&r);
        g["id"] = json!("g");
        assert_eq!(g["conditions"]["fromAddresses"][0]["emailAddress"]["address"], json!("a@b.c"));
        assert_eq!(from_graph(&g).unwrap().conditions.len(), 3);
        let back = from_graph(&g).unwrap();
        assert_eq!(back.exceptions, r.exceptions);
        assert_eq!(back.actions, r.actions);

        let fwd = json!({"id":"f","conditions":{"senderContains":["a"]},"actions":{"forwardTo":[{"emailAddress":{"address":"x@y.z"}}]}});
        assert_eq!(from_graph(&fwd).unwrap_err(), vec!["action:forwardTo"]);
        let hdr = json!({"id":"h","conditions":{"headerContains":["a"]},"actions":{"assignCategories":["T"]}});
        assert_eq!(from_graph(&hdr).unwrap_err(), vec!["headerContains"]);
    }
}
