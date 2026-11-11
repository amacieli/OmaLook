//! Gmail server filters (docs/PLAN-tags-and-rules.md, OL-040 phase 5a): pure mapping, no I/O.
//!
//! A `LocalRule` is written to Gmail as a `users.settings.filters` object, and a filter OmaLook can fully
//! understand is read back as a `LocalRule`. Same law as the Exchange side (decision 2026-11-07): the
//! provider must hold exactly what the rule says, so anything Gmail would match differently is REFUSED, not
//! approximated:
//!   - Gmail matches whole words / tokens, not arbitrary substrings. So `senderContains` is accepted only when
//!     each value is an address (`a@b.com`), an `@domain.tld` or a bare `domain.tld`; `subjectContains`,
//!     `bodyContains`, `bodyOrSubjectContains` (word-vs-substring) are refused.
//!   - Filters have no "stop processing": every filter that matches applies. A rule with `stop` is refused.
//!   - `addLabelIds` takes label ids, and one label per filter here (one tag per rule).
//!   - Not expressible: importance, categories (tags already on the message), `hasAttachments=false`.
//! Edit is impossible on Gmail (PUT and PATCH both 404, phase 0), so an edit is delete + create; the caller
//! creates first, then deletes the old one, so a failure never leaves the mailbox without the rule.
//! UNVERIFIED LIVE: that a delivered message actually gets the label, and how Gmail matches `@domain.tld`
//! inside `from` (phase 0 only proved filter create/get/delete and the label round trip).

use crate::local_rules::{Actions, LocalRule, Pred};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

/// `a@b.co`: one `@`, text on both sides, a dot in the domain, no spaces.
fn is_address(s: &str) -> bool {
    let s = s.trim();
    match s.split_once('@') {
        Some((l, d)) => !l.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.')
            && !s.chars().any(|c| c.is_whitespace() || "()\"{}:".contains(c)) && !d.contains('@'),
        None => false,
    }
}

/// `@b.co` or `b.co`.
fn is_domain(s: &str) -> bool {
    let d = s.trim().trim_start_matches('@');
    !d.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.')
        && d.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

fn sender_ok(v: &str) -> bool { is_address(v) || is_domain(v) }

/// `a OR b` (no parentheses: the criteria field takes search syntax directly); one value stays bare.
fn any_of(vals: &[String]) -> String {
    vals.iter().map(|v| v.trim()).collect::<Vec<_>>().join(" OR ")
}

fn check_values(kind: &str, vals: &[String], ok: &dyn Fn(&str) -> bool, why: &str) -> Result<(), String> {
    if vals.iter().all(|v| v.trim().is_empty()) { return Err(format!("{kind} has no value")); }
    match vals.iter().find(|v| !ok(v)) {
        Some(bad) => Err(format!("Gmail cannot match {kind} \"{}\" the way OmaLook does: {why}", bad.trim())),
        None => Ok(()),
    }
}

const WORDS: &str = "Gmail filters match whole words, not parts of words";

/// Whether `rule` can live on Gmail exactly as written; the reason when it cannot.
pub fn writable(rule: &LocalRule) -> Result<(), String> {
    to_gmail(rule, "x").map(|_| ())
}

/// The Gmail filter for `rule`; `label_id` is the (already existing) label of its single tag.
pub fn to_gmail(rule: &LocalRule, label_id: &str) -> Result<Value, String> {
    rule.validate()?;
    if rule.actions.stop { return Err("Gmail filters cannot stop other filters, so a \"stop processing\" rule would behave differently there".into()); }
    if rule.actions.tags.len() != 1 { return Err("a Gmail filter here carries exactly one tag".into()); }
    if rule.conditions.is_empty() { return Err("a filter with no condition would tag every message".into()); }

    let mut crit = Map::new();
    let mut extra: Vec<String> = Vec::new(); // more search terms, ANDed with the criteria fields
    for p in &rule.conditions {
        match p {
            Pred::FromAddresses(v) => {
                check_values("fromAddresses", v, &is_address, "it needs full addresses")?;
                put_from(&mut crit, &mut extra, any_of(v));
            }
            Pred::SenderContains(v) => {
                check_values("senderContains", v, &sender_ok, "only an address or a domain such as @example.com works")?;
                put_from(&mut crit, &mut extra, any_of(v));
            }
            Pred::RecipientContains(v) => {
                check_values("recipientContains", v, &sender_ok, "only an address or a domain works")?;
                crit.insert("to".into(), json!(any_of(v)));
            }
            Pred::HasAttachments(true) => { crit.insert("hasAttachment".into(), json!(true)); }
            Pred::HasAttachments(false) => return Err("Gmail cannot match \"has no attachment\"".into()),
            Pred::SubjectContains(_) | Pred::BodyContains(_) | Pred::BodyOrSubjectContains(_) => {
                return Err(format!("Gmail cannot match {} the way OmaLook does: {WORDS}", p.key()));
            }
            Pred::Categories(_) | Pred::Importance(_) => {
                return Err(format!("Gmail filters have no {} condition", p.key()));
            }
        }
    }
    if !extra.is_empty() { crit.insert("query".into(), json!(extra.join(" "))); }

    let mut negs: Vec<String> = Vec::new();
    for p in &rule.exceptions {
        match p {
            Pred::FromAddresses(v) => { check_values("exception fromAddresses", v, &is_address, "it needs full addresses")?; negs.push(format!("from:({})", any_of(v))); }
            Pred::SenderContains(v) => { check_values("exception senderContains", v, &sender_ok, "only an address or a domain works")?; negs.push(format!("from:({})", any_of(v))); }
            Pred::RecipientContains(v) => { check_values("exception recipientContains", v, &sender_ok, "only an address or a domain works")?; negs.push(format!("to:({})", any_of(v))); }
            other => return Err(format!("Gmail cannot hold an exception on {}", other.key())),
        }
    }
    if !negs.is_empty() { crit.insert("negatedQuery".into(), json!(negs.join(" OR "))); }

    Ok(json!({ "criteria": Value::Object(crit), "action": { "addLabelIds": [label_id] } }))
}

fn put_from(crit: &mut Map<String, Value>, extra: &mut Vec<String>, terms: String) {
    if crit.contains_key("from") { extra.push(format!("from:({terms})")); } else { crit.insert("from".into(), json!(terms)); }
}

fn split_any(s: &str) -> Vec<String> {
    s.split(" OR ").map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
}

/// `from:(a OR b)` / `to:(...)` -> (key, values).
fn parse_term(t: &str) -> Option<(&'static str, Vec<String>)> {
    let t = t.trim();
    for (pre, key) in [("from:(", "from"), ("to:(", "to")] {
        if let Some(rest) = t.strip_prefix(pre) {
            return rest.strip_suffix(')').map(|inner| (key, split_any(inner)));
        }
    }
    None
}

fn sender_pred(vals: Vec<String>) -> Pred {
    if vals.iter().all(|v| is_address(v)) { Pred::FromAddresses(vals) } else { Pred::SenderContains(vals) }
}

fn recipient_pred(vals: Vec<String>) -> Pred { Pred::RecipientContains(vals) }

/// Read a Gmail filter back as a rule, but only when it has exactly the shape `to_gmail` writes. `labels`
/// maps label id -> name. Err(reason) for anything else (the caller shows that filter read-only).
pub fn from_gmail(filter: &Value, labels: &HashMap<String, String>, account_id: &str) -> Result<LocalRule, String> {
    let id = filter["id"].as_str().ok_or("filter has no id")?.to_string();
    let crit = filter["criteria"].as_object().ok_or("filter has no criteria")?;
    let act = filter["action"].as_object().ok_or("filter has no action")?;
    const KNOWN: [&str; 5] = ["from", "to", "query", "negatedQuery", "hasAttachment"];
    if let Some(k) = crit.keys().find(|k| !KNOWN.contains(&k.as_str())) { return Err(format!("uses \"{k}\", which OmaLook does not manage")); }
    if act.keys().any(|k| k != "addLabelIds") { return Err("does more than add a label (archive, forward, delete, ...)".into()); }
    let ids = act.get("addLabelIds").and_then(|a| a.as_array()).ok_or("does not add a label")?;
    if ids.len() != 1 { return Err("adds more than one label".into()); }
    let lid = ids[0].as_str().ok_or("bad label id")?;
    let tag = labels.get(lid).ok_or("adds a system label or one OmaLook has not seen")?.clone();

    let mut conditions: Vec<Pred> = Vec::new();
    if let Some(f) = crit.get("from").and_then(|v| v.as_str()) { conditions.push(sender_pred(split_any(f))); }
    if let Some(t) = crit.get("to").and_then(|v| v.as_str()) { conditions.push(recipient_pred(split_any(t))); }
    if let Some(q) = crit.get("query").and_then(|v| v.as_str()) {
        for part in q.split(") ").map(|p| if p.ends_with(')') { p.to_string() } else { format!("{p})") }) {
            match parse_term(&part) {
                Some(("from", v)) => conditions.push(sender_pred(v)),
                _ => return Err(format!("search text \"{q}\" is not something OmaLook wrote")),
            }
        }
    }
    match crit.get("hasAttachment") {
        Some(Value::Bool(true)) => conditions.push(Pred::HasAttachments(true)),
        Some(Value::Bool(false)) | None => {}
        _ => return Err("odd hasAttachment".into()),
    }
    if conditions.is_empty() { return Err("has no condition OmaLook understands".into()); }

    let mut exceptions: Vec<Pred> = Vec::new();
    if let Some(n) = crit.get("negatedQuery").and_then(|v| v.as_str()) {
        for part in n.split(") OR ").map(|p| if p.ends_with(')') { p.to_string() } else { format!("{p})") }) {
            match parse_term(&part) {
                Some(("from", v)) => exceptions.push(sender_pred(v)),
                Some(("to", v)) => exceptions.push(recipient_pred(v)),
                _ => return Err(format!("\"except\" text \"{n}\" is not something OmaLook wrote")),
            }
        }
    }
    let rule = LocalRule {
        id, name: format!("Gmail filter: {tag}"), enabled: true, sequence: 0,
        applies_to: vec![account_id.to_string()], conditions, exceptions,
        actions: Actions { tags: vec![tag], stop: false },
    };
    // Prove it: what we read must be exactly writable back to the same filter.
    match to_gmail(&rule, lid) {
        Ok(back) if same_effect(&back, filter) => Ok(rule),
        Ok(_) => Err("is close to a rule OmaLook writes but not identical".into()),
        Err(e) => Err(e),
    }
}

/// Names of the user labels a filter adds, from the label map (id -> name); ids not in the map are skipped.
pub fn tags_of(filter: &Value, labels: &HashMap<String, String>) -> Vec<String> {
    filter["action"]["addLabelIds"].as_array().into_iter().flatten()
        .filter_map(|i| i.as_str()).filter_map(|i| labels.get(i).cloned()).collect()
}

/// The cached copy of a filter: the provider's JSON plus `omalookTags` (names), so the page never needs the
/// label list. Ignored by `same_effect` and `fingerprint`.
pub fn with_tags(mut filters: Vec<Value>, labels: &HashMap<String, String>) -> Vec<Value> {
    for f in &mut filters {
        let t = tags_of(f, labels);
        f["omalookTags"] = json!(t);
    }
    filters
}

/// One filter as the Settings page shows it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct FilterSummary {
    pub id: String,
    pub tags: Vec<String>,
    /// Plain-language condition text, always filled (from the raw criteria when the filter is not readable).
    pub text: String,
    /// OmaLook can edit and delete it (it has exactly the shape OmaLook writes). Others are shown read-only.
    pub editable: bool,
    /// Why it is read-only; empty when editable.
    pub reason: String,
    /// The same filter as an editable rule when `editable`.
    pub rule: Option<LocalRule>,
}

fn plain(crit: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    let get = |k: &str| crit[k].as_str().map(|s| s.to_string());
    if let Some(v) = get("from") { parts.push(format!("from {}", v.replace(" OR ", " or "))); }
    if let Some(v) = get("to") { parts.push(format!("to {}", v.replace(" OR ", " or "))); }
    if let Some(v) = get("subject") { parts.push(format!("subject has {v}")); }
    if let Some(v) = get("query") { parts.push(format!("matching {v}")); }
    if crit["hasAttachment"].as_bool() == Some(true) { parts.push("with an attachment".into()); }
    if let Some(v) = get("negatedQuery") { parts.push(format!("unless {}", v.replace(" OR ", " or "))); }
    if parts.is_empty() { "everything".into() } else { parts.join(", ") }
}

/// Summary for the page. `labels` maps label id -> name.
pub fn summarize(filter: &Value, labels: &HashMap<String, String>, account_id: &str) -> FilterSummary {
    let id = filter["id"].as_str().unwrap_or("").to_string();
    let mut tags = tags_of(filter, labels);
    if tags.is_empty() {
        // a cached copy carries the names it was read with
        tags = filter["omalookTags"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(String::from)).collect();
    }
    let text = plain(&filter["criteria"]);
    match from_gmail(filter, labels, account_id) {
        Ok(rule) => FilterSummary { id, tags, text, editable: true, reason: String::new(), rule: Some(rule) },
        Err(reason) => FilterSummary { id, tags, text, editable: false, reason, rule: None },
    }
}

fn lc(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(s.trim().to_lowercase()),
        Value::Array(a) => Value::Array(a.iter().map(lc).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), lc(v))).collect()),
        o => o.clone(),
    }
}

/// Criteria + action only (no id), lower-cased: two filters with the same effect compare equal.
pub fn same_effect(a: &Value, b: &Value) -> bool {
    // `action` ids are opaque and case-sensitive; compare them as they are, criteria case-insensitively.
    lc(&a["criteria"]) == lc(&b["criteria"]) && a["action"] == b["action"]
}

/// A stable text for change detection (Gmail filters have no etag): id plus the effect.
pub fn fingerprint(filter: &Value) -> String {
    format!("{}|{}|{}", filter["id"].as_str().unwrap_or(""), lc(&filter["criteria"]), filter["action"])
}

/// After a create: the one new filter the server holds, found by effect, is the one we wrote.
pub fn verify_create(before: &[Value], after: &[Value], body: &Value) -> Result<String, String> {
    let known: Vec<&str> = before.iter().filter_map(|f| f["id"].as_str()).collect();
    let fresh: Vec<&Value> = after.iter().filter(|f| f["id"].as_str().map_or(false, |i| !known.contains(&i))).collect();
    match fresh.as_slice() {
        [one] if same_effect(one, body) => Ok(one["id"].as_str().unwrap_or("").to_string()),
        [one] => Err(format!("the server stored a different filter than asked: {}", one["criteria"])),
        [] => Err("the server did not keep the filter".into()),
        _ => Err("more than one new filter appeared; leaving them alone".into()),
    }
}

/// After a delete: the filter is gone and nothing else changed.
pub fn verify_delete(before: &[Value], after: &[Value], id: &str) -> Result<(), String> {
    if after.iter().any(|f| f["id"].as_str() == Some(id)) { return Err("the filter is still there".into()); }
    let want: Vec<String> = before.iter().filter(|f| f["id"].as_str() != Some(id)).map(fingerprint).collect();
    let got: Vec<String> = after.iter().map(fingerprint).collect();
    if want == got { Ok(()) } else { Err("other filters changed during the delete".into()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(conds: Vec<Pred>, excs: Vec<Pred>, tag: &str, stop: bool) -> LocalRule {
        LocalRule { id: "r1".into(), name: "n".into(), enabled: true, sequence: 1, applies_to: vec!["g1".into()],
            conditions: conds, exceptions: excs, actions: Actions { tags: vec![tag.into()], stop } }
    }
    fn labels() -> HashMap<String, String> {
        [("Label_7".to_string(), "Family".to_string())].into_iter().collect()
    }
    fn s(v: &[&str]) -> Vec<String> { v.iter().map(|x| x.to_string()).collect() }

    #[test]
    fn address_and_domain_shapes() {
        assert!(is_address("a@b.com") && !is_address("a@b") && !is_address("@b.com") && !is_address("a b@c.com"));
        assert!(is_domain("@b.com") && is_domain("b.com") && !is_domain("b") && !is_domain("lucia"));
    }

    #[test]
    fn writes_senders_as_from_with_or_between_values() {
        let r = rule(vec![Pred::FromAddresses(s(&["a@x.com", "b@y.org"]))], vec![], "Family", false);
        let f = to_gmail(&r, "Label_7").unwrap();
        assert_eq!(f["criteria"], json!({ "from": "a@x.com OR b@y.org" }));
        assert_eq!(f["action"], json!({ "addLabelIds": ["Label_7"] }));
    }

    #[test]
    fn two_sender_predicates_use_the_query_for_the_second() {
        let r = rule(vec![Pred::FromAddresses(s(&["a@x.com"])), Pred::SenderContains(s(&["@y.org"]))], vec![], "Family", false);
        let f = to_gmail(&r, "Label_7").unwrap();
        assert_eq!(f["criteria"], json!({ "from": "a@x.com", "query": "from:(@y.org)" }));
    }

    #[test]
    fn recipient_attachment_and_exceptions() {
        let r = rule(vec![Pred::SenderContains(s(&["@x.com"])), Pred::RecipientContains(s(&["me@z.com"])), Pred::HasAttachments(true)],
            vec![Pred::FromAddresses(s(&["boss@x.com"])), Pred::RecipientContains(s(&["@spam.io"]))], "Family", false);
        let f = to_gmail(&r, "Label_7").unwrap();
        assert_eq!(f["criteria"], json!({ "from": "@x.com", "to": "me@z.com", "hasAttachment": true,
            "negatedQuery": "from:(boss@x.com) OR to:(@spam.io)" }));
    }

    #[test]
    fn refuses_what_gmail_would_match_differently() {
        let t = |c: Vec<Pred>, e: Vec<Pred>, stop: bool| to_gmail(&rule(c, e, "Family", stop), "L").unwrap_err();
        assert!(t(vec![Pred::SenderContains(s(&["lucia"]))], vec![], false).contains("domain"));
        assert!(t(vec![Pred::SubjectContains(s(&["sale"]))], vec![], false).contains("whole words"));
        assert!(t(vec![Pred::BodyContains(s(&["sale"]))], vec![], false).contains("whole words"));
        assert!(t(vec![Pred::FromAddresses(s(&["a@x.com"]))], vec![], true).contains("stop"));
        assert!(t(vec![Pred::HasAttachments(false)], vec![], false).contains("no attachment"));
        assert!(t(vec![Pred::Importance("high".into())], vec![], false).contains("importance"));
        assert!(t(vec![Pred::FromAddresses(s(&["a@x.com"]))], vec![Pred::SubjectContains(s(&["x"]))], false).contains("exception"));
        assert!(t(vec![Pred::FromAddresses(s(&["not-an-address"]))], vec![], false).contains("full addresses"));
        let mut two = rule(vec![Pred::FromAddresses(s(&["a@x.com"]))], vec![], "Family", false);
        two.actions.tags.push("Other".into());
        assert!(to_gmail(&two, "L").unwrap_err().contains("exactly one tag"));
    }

    #[test]
    fn round_trip_through_gmail_shape() {
        let r = rule(vec![Pred::SenderContains(s(&["@x.com", "@y.org"])), Pred::RecipientContains(s(&["me@z.com"])), Pred::HasAttachments(true)],
            vec![Pred::FromAddresses(s(&["boss@x.com"]))], "Family", false);
        let mut f = to_gmail(&r, "Label_7").unwrap();
        f["id"] = json!("ANe1B");
        let back = from_gmail(&f, &labels(), "g1").unwrap();
        assert_eq!(back.conditions, r.conditions);
        assert_eq!(back.exceptions, r.exceptions);
        assert_eq!(back.actions.tags, vec!["Family".to_string()]);
        // two sender predicates survive through the query form
        let r2 = rule(vec![Pred::FromAddresses(s(&["a@x.com"])), Pred::SenderContains(s(&["@y.org"]))], vec![], "Family", false);
        let mut f2 = to_gmail(&r2, "Label_7").unwrap();
        f2["id"] = json!("X");
        assert_eq!(from_gmail(&f2, &labels(), "g1").unwrap().conditions, r2.conditions);
    }

    #[test]
    fn filters_it_did_not_write_stay_unreadable_rather_than_misread() {
        let id = |mut f: Value| { f["id"] = json!("F"); f };
        let ok = |f: Value| from_gmail(&id(f), &labels(), "g1");
        assert!(ok(json!({ "criteria": { "from": "a@x.com", "size": 5 }, "action": { "addLabelIds": ["Label_7"] } })).is_err());
        assert!(ok(json!({ "criteria": { "from": "a@x.com" }, "action": { "addLabelIds": ["Label_7"], "removeLabelIds": ["INBOX"] } })).is_err());
        assert!(ok(json!({ "criteria": { "from": "a@x.com" }, "action": { "addLabelIds": ["Label_7"], "forward": "x@y.com" } })).is_err());
        assert!(ok(json!({ "criteria": { "from": "a@x.com" }, "action": { "addLabelIds": ["STARRED"] } })).is_err());
        assert!(ok(json!({ "criteria": { "query": "list:foo" }, "action": { "addLabelIds": ["Label_7"] } })).is_err());
        assert!(ok(json!({ "criteria": { "subject": "sale" }, "action": { "addLabelIds": ["Label_7"] } })).is_err());
        assert!(ok(json!({ "criteria": { "from": "lucia" }, "action": { "addLabelIds": ["Label_7"] } })).is_err());
        assert!(ok(json!({ "criteria": { "from": "a@x.com" }, "action": { "addLabelIds": ["Label_7"] } })).is_ok());
    }

    #[test]
    fn summaries_say_what_a_filter_does_and_why_some_are_read_only() {
        let r = rule(vec![Pred::FromAddresses(s(&["a@x.com", "b@y.org"]))], vec![Pred::SenderContains(s(&["@z.io"]))], "Family", false);
        let mut f = to_gmail(&r, "Label_7").unwrap();
        f["id"] = json!("F1");
        let sm = summarize(&f, &labels(), "g1");
        assert!(sm.editable && sm.rule.is_some());
        assert_eq!(sm.tags, vec!["Family".to_string()]);
        assert_eq!(sm.text, "from a@x.com or b@y.org, unless from:(@z.io)");
        let other = json!({ "id": "F2", "criteria": { "from": "a@x.com", "subject": "sale" }, "action": { "addLabelIds": ["Label_7"], "removeLabelIds": ["INBOX"] } });
        let so = summarize(&other, &labels(), "g1");
        assert!(!so.editable && so.reason.contains("subject") && so.text == "from a@x.com, subject has sale");
        // a cached copy keeps its tag names even when the label map is empty
        let cached = with_tags(vec![f.clone()], &labels());
        assert_eq!(summarize(&cached[0], &HashMap::new(), "g1").tags, vec!["Family".to_string()]);
        assert!(same_effect(&cached[0], &f) && fingerprint(&cached[0]) == fingerprint(&f));
    }

    #[test]
    fn create_and_delete_verification() {
        let body = json!({ "criteria": { "from": "A@x.com" }, "action": { "addLabelIds": ["Label_7"] } });
        let old = json!({ "id": "1", "criteria": { "from": "z@z.com" }, "action": { "addLabelIds": ["Label_7"] } });
        let mut stored = body.clone(); stored["id"] = json!("2"); stored["criteria"]["from"] = json!("a@x.com");
        assert_eq!(verify_create(&[old.clone()], &[old.clone(), stored.clone()], &body).unwrap(), "2");
        assert!(verify_create(&[old.clone()], &[old.clone()], &body).is_err());
        let mut other = stored.clone(); other["criteria"]["from"] = json!("q@q.com");
        assert!(verify_create(&[old.clone()], &[old.clone(), other], &body).is_err());
        assert!(verify_delete(&[old.clone(), stored.clone()], &[old.clone()], "2").is_ok());
        assert!(verify_delete(&[old.clone(), stored.clone()], &[old.clone(), stored.clone()], "2").is_err());
        assert!(verify_delete(&[old.clone(), stored.clone()], &[], "2").is_err());
    }
}
