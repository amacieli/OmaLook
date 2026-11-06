//! Mailbox rules as the user sees them (docs/PLAN-tags-and-rules.md, OL-040 phase 2, read-only).
//!
//! The raw rule JSON of a provider (Graph `messageRule`) is cached as received; this module turns it into
//! plain-language lines and groups it for the Settings -> Rules page: tag rules grouped by tag (the real
//! account has one rule per sender or domain, so ten rules feed one tag), everything else (forward, delete,
//! importance, move...) listed on its own, and read-only placeholders apart. Nothing here writes to a
//! provider. Anything it does not understand is listed under `unknown` instead of being dropped, so the
//! page can never show a rule as harmless when it is not.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RuleSummary {
    pub id: String,
    pub name: String,
    pub sequence: i64,
    pub enabled: bool,
    pub read_only: bool,
    pub has_error: bool,
    /// Tags the rule assigns.
    pub tags: Vec<String>,
    pub conditions: Vec<String>,
    pub exceptions: Vec<String>,
    /// Actions other than assigning tags and stopping.
    pub actions: Vec<String>,
    /// "Stop processing more rules" is set: the first matching rule wins.
    pub stops: bool,
    /// `tag` (assigns tags, nothing else but stop), `other`, or `system` (read-only or empty placeholder).
    pub kind: &'static str,
    /// Condition / action keys this code has no wording for.
    pub unknown: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TagGroup {
    pub tag: String,
    pub rules: Vec<RuleSummary>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Organised {
    pub groups: Vec<TagGroup>,
    pub other: Vec<RuleSummary>,
    pub system: Vec<RuleSummary>,
}

/// Non-empty values of a JSON field: arrays of strings or recipient objects, flattened to text.
fn texts(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.iter().filter_map(|x| match x {
            Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
            Value::Object(_) => x["emailAddress"]["address"].as_str().map(|s| s.to_string())
                .or_else(|| x["address"].as_str().map(|s| s.to_string())),
            _ => None,
        }).collect(),
        Value::String(s) if !s.trim().is_empty() => vec![s.trim().to_string()],
        _ => Vec::new(),
    }
}

fn is_set(v: &Value) -> bool {
    match v {
        Value::Null | Value::Bool(false) => false,
        Value::Array(a) => !a.is_empty(),
        Value::String(s) => !s.is_empty(),
        Value::Object(o) => !o.is_empty(),
        _ => true,
    }
}

fn quoted_or(items: &[String]) -> String {
    items.iter().map(|s| format!("\u{201c}{}\u{201d}", s)).collect::<Vec<_>>().join(" or ")
}

/// Graph predicate (condition or exception) object -> lines; unrecognised keys go to `unknown`.
fn predicate_lines(p: &Value, unknown: &mut Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    let Some(obj) = p.as_object() else { return out };
    for (k, v) in obj {
        if !is_set(v) { continue }
        let list = texts(v);
        let line = match k.as_str() {
            "fromAddresses" => format!("from {}", list.join(" or ")),
            "sentToAddresses" => format!("sent to {}", list.join(" or ")),
            "senderContains" => format!("sender contains {}", quoted_or(&list)),
            "subjectContains" => format!("subject contains {}", quoted_or(&list)),
            "bodyContains" => format!("body contains {}", quoted_or(&list)),
            "bodyOrSubjectContains" => format!("subject or body contains {}", quoted_or(&list)),
            "headerContains" => format!("header contains {}", quoted_or(&list)),
            "recipientContains" => format!("recipient contains {}", quoted_or(&list)),
            "categories" => format!("has tag {}", quoted_or(&list)),
            "sentToMe" => "sent to me".into(),
            "sentOnlyToMe" => "sent only to me".into(),
            "sentCcMe" => "I am on Cc".into(),
            "sentToOrCcMe" => "sent or Cc to me".into(),
            "notSentToMe" => "not sent to me".into(),
            "hasAttachments" => "has an attachment".into(),
            "importance" => format!("importance is {}", v.as_str().unwrap_or("?")),
            "sensitivity" => format!("sensitivity is {}", v.as_str().unwrap_or("?")),
            "isAutomaticReply" => "is an automatic reply".into(),
            "isAutomaticForward" => "is an automatic forward".into(),
            "isMeetingRequest" => "is a meeting request".into(),
            "isMeetingResponse" => "is a meeting response".into(),
            "isNonDeliveryReport" => "is a delivery report".into(),
            "isPermissionControlled" => "is permission controlled".into(),
            "isReadReceipt" => "is a read receipt".into(),
            "isSigned" => "is signed".into(),
            "isVoicemail" => "is voicemail".into(),
            "messageActionFlag" => format!("flagged {}", v.as_str().unwrap_or("?")),
            "withinSizeRange" => "within a size range".into(),
            _ => { unknown.push(k.clone()); continue }
        };
        out.push(line);
    }
    out
}

/// One Graph `messageRule` -> a summary.
pub fn summarize(rule: &Value) -> RuleSummary {
    let mut unknown: Vec<String> = Vec::new();
    let conditions = predicate_lines(&rule["conditions"], &mut unknown);
    let exceptions = predicate_lines(&rule["exceptions"], &mut unknown);

    let mut tags: Vec<String> = Vec::new();
    let mut actions: Vec<String> = Vec::new();
    let mut stops = false;
    if let Some(obj) = rule["actions"].as_object() {
        for (k, v) in obj {
            if !is_set(v) { continue }
            match k.as_str() {
                "assignCategories" => tags = texts(v),
                "stopProcessingRules" => stops = v.as_bool().unwrap_or(true),
                "markAsRead" => actions.push("mark as read".into()),
                "markImportance" => actions.push(format!("set importance to {}", v.as_str().unwrap_or("?"))),
                "delete" => actions.push("move to Deleted Items".into()),
                "permanentDelete" => actions.push("DELETE permanently".into()),
                "moveToFolder" => actions.push("move to a folder".into()),
                "copyToFolder" => actions.push("copy to a folder".into()),
                "forwardTo" => actions.push(format!("forward to {}", texts(v).join(", "))),
                "forwardAsAttachmentTo" => actions.push(format!("forward as attachment to {}", texts(v).join(", "))),
                "redirectTo" => actions.push(format!("redirect to {}", texts(v).join(", "))),
                _ => unknown.push(k.clone()),
            }
        }
    }

    let read_only = rule["isReadOnly"].as_bool().unwrap_or(false);
    let empty = conditions.is_empty() && exceptions.is_empty() && tags.is_empty() && actions.is_empty() && !stops && unknown.is_empty();
    let kind = if read_only || empty { "system" }
        else if !tags.is_empty() && actions.is_empty() && unknown.is_empty() { "tag" }
        else { "other" };

    RuleSummary {
        id: rule["id"].as_str().unwrap_or_default().to_string(),
        name: rule["displayName"].as_str().unwrap_or_default().to_string(),
        sequence: rule["sequence"].as_i64().unwrap_or(0),
        enabled: rule["isEnabled"].as_bool().unwrap_or(true),
        read_only,
        has_error: rule["hasError"].as_bool().unwrap_or(false),
        tags, conditions, exceptions, actions, stops, kind, unknown,
    }
}

/// Group summaries for display: tag rules by tag (a rule with two tags appears under each), in rule
/// order within a group and most rules first across groups; other rules and system rules in rule order.
pub fn organise(mut rules: Vec<RuleSummary>) -> Organised {
    rules.sort_by_key(|r| r.sequence);
    let mut groups: Vec<TagGroup> = Vec::new();
    let mut other = Vec::new();
    let mut system = Vec::new();
    for r in rules {
        match r.kind {
            "tag" => {
                for t in r.tags.clone() {
                    match groups.iter_mut().find(|g| g.tag.eq_ignore_ascii_case(&t)) {
                        Some(g) => g.rules.push(r.clone()),
                        None => groups.push(TagGroup { tag: t, rules: vec![r.clone()] }),
                    }
                }
            }
            "system" => system.push(r),
            _ => other.push(r),
        }
    }
    groups.sort_by(|a, b| b.rules.len().cmp(&a.rules.len()).then_with(|| a.tag.to_lowercase().cmp(&b.tag.to_lowercase())));
    Organised { groups, other, system }
}

// ───────────────────────────────────────────────────────────── writing (OL-040 phase 3)
//
// Everything below is pure: it builds request bodies and judges what the server did. The network part
// lives in the mail daemon (`push_rule_ops`): fetch, check nothing changed behind our back, write, fetch
// again, `verify_*`, store what the server really has. Graph rules have no etag, so "nothing changed" is a
// fingerprint of the rule as the user saw it. Only plain tag rules may be written (forward / delete /
// importance / read-only rules stay as they are until phase 7's confirm gate).

use serde_json::json;

/// What a new rule matches.
#[derive(Debug, Clone, PartialEq)]
pub enum Match {
    /// One exact sender address (`fromAddresses`).
    Address(String),
    /// Every sender at a domain (`senderContains "@domain"`; the leading @ keeps `x.org` from matching `box.org`).
    Domain(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewRule {
    pub tag: String,
    pub matcher: Match,
    pub enabled: bool,
    pub stop: bool,
}

/// An address OmaLook is willing to put in a rule (one `@`, no spaces, a dot in the domain).
pub fn valid_address(a: &str) -> bool {
    let a = a.trim();
    let mut parts = a.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else { return false };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
        && !a.chars().any(|c| c.is_whitespace() || c.is_control() || c == ',' || c == ';' || c == '"' || c == '<' || c == '>')
}

pub fn valid_domain(d: &str) -> bool {
    let d = d.trim().trim_start_matches('@');
    d.contains('.') && !d.starts_with('.') && !d.ends_with('.')
        && d.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

pub fn valid_tag_name(t: &str) -> bool {
    let t = t.trim();
    !t.is_empty() && t.chars().count() <= 64 && !t.chars().any(|c| c.is_control() || c == ';' || c == ',')
}

/// Request body for `POST .../messageRules`. `sequence` is required by Graph; a big number is appended at the end.
pub fn new_rule_body(r: &NewRule) -> Value {
    let (conditions, what) = match &r.matcher {
        Match::Address(a) => (json!({ "fromAddresses": [{ "emailAddress": { "address": a.trim().to_lowercase() } }] }), a.trim().to_lowercase()),
        Match::Domain(d) => {
            let d = d.trim().trim_start_matches('@').to_lowercase();
            (json!({ "senderContains": [format!("@{}", d)] }), format!("@{}", d))
        }
    };
    json!({
        "displayName": format!("omalook: {} <- {}", r.tag.trim(), what),
        "sequence": 9999,
        "isEnabled": r.enabled,
        "conditions": conditions,
        "actions": { "assignCategories": [r.tag.trim()], "stopProcessingRules": r.stop },
    })
}

/// The rule as the user saw it, without its position: what we compare to detect an edit made elsewhere.
pub fn fingerprint(rule: &Value) -> String {
    let mut v = rule.clone();
    if let Some(o) = v.as_object_mut() { o.remove("sequence"); }
    v.to_string()
}

/// Only plain tag rules (assign tags, maybe stop) may be changed from here.
pub fn writable(rule: &Value) -> Result<(), String> {
    let s = summarize(rule);
    match s.kind {
        "tag" => Ok(()),
        "system" => Err("that is a read-only rule made by Outlook".into()),
        _ => Err("only plain tag rules can be changed here; edit forward, delete and other rules in Outlook".into()),
    }
}

/// A rule part in a form that two copies of the same rule agree on: strings lower-cased (Graph upper-cases
/// condition text), empty and null parts dropped, `false` flags treated as absent, and an address reduced to
/// the address (Graph adds a `name` to every `emailAddress` it stores; found by the live check).
pub fn norm(v: &Value) -> Value {
    match v {
        Value::Object(o) => {
            let mut out = serde_json::Map::new();
            for (k, x) in o {
                if k == "emailAddress" {
                    if let Some(a) = x.get("address").and_then(|a| a.as_str()) {
                        out.insert(k.clone(), json!({ "address": a.to_lowercase() }));
                        continue;
                    }
                }
                let n = norm(x);
                if n.is_null() || n == json!(false) || n == json!([]) || n == json!({}) { continue }
                out.insert(k.clone(), n);
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
        Value::String(s) => Value::String(s.to_lowercase()),
        other => other.clone(),
    }
}

/// Two rules do the same thing: same conditions and same actions (names, ids and positions are not compared).
pub fn same_effect(a: &Value, b: &Value) -> bool {
    norm(&a["conditions"]) == norm(&b["conditions"]) && norm(&a["actions"]) == norm(&b["actions"])
}

fn by_id<'a>(rules: &'a [Value], id: &str) -> Option<&'a Value> { rules.iter().find(|r| r["id"].as_str() == Some(id)) }

/// Everything except `skip` is unchanged (byte for byte, position included unless `ignore_sequence`).
fn others_unchanged(before: &[Value], after: &[Value], skip: &[&str], ignore_sequence: bool) -> Result<(), String> {
    for b in before {
        let id = b["id"].as_str().unwrap_or_default();
        if skip.contains(&id) { continue }
        let Some(a) = by_id(after, id) else { return Err(format!("rule \u{201c}{}\u{201d} disappeared", b["displayName"].as_str().unwrap_or(id))) };
        let same = if ignore_sequence { fingerprint(a) == fingerprint(b) } else { a == b };
        if !same { return Err(format!("rule \u{201c}{}\u{201d} changed unexpectedly", b["displayName"].as_str().unwrap_or(id))) }
    }
    for a in after {
        let id = a["id"].as_str().unwrap_or_default();
        if !skip.contains(&id) && by_id(before, id).is_none() { return Err(format!("an unexpected new rule appeared: {}", a["displayName"].as_str().unwrap_or(id))) }
    }
    Ok(())
}

/// After a create: exactly one new rule, shaped as asked (the server upper-cases condition text, so compare
/// ignoring case), and every old rule untouched. Returns the new rule's id.
pub fn verify_create(before: &[Value], after: &[Value], body: &Value) -> Result<String, String> {
    let new: Vec<&Value> = after.iter().filter(|a| by_id(before, a["id"].as_str().unwrap_or_default()).is_none()).collect();
    let [n] = new.as_slice() else { return Err(format!("expected one new rule, found {}", new.len())) };
    if n["displayName"].as_str().map(|x| x.to_lowercase()) != body["displayName"].as_str().map(|x| x.to_lowercase()) { return Err("the server stored the new rule under another name".into()) }
    for k in ["conditions", "actions"] {
        if norm(&n[k]) != norm(&body[k]) { return Err(format!("the server stored the new rule differently ({}): {} vs {}", k, n[k], body[k])) }
    }
    if n["isEnabled"].as_bool() != body["isEnabled"].as_bool() { return Err("the server stored the new rule with a different on/off state".into()) }
    let id = n["id"].as_str().unwrap_or_default().to_string();
    others_unchanged(before, after, &[&id], false)?;
    Ok(id)
}

pub fn verify_enable(before: &[Value], after: &[Value], id: &str, on: bool) -> Result<(), String> {
    let Some(r) = by_id(after, id) else { return Err("the rule disappeared".into()) };
    if r["isEnabled"].as_bool() != Some(on) { return Err("the server did not take the on/off change".into()) }
    let b = by_id(before, id).ok_or("the rule was not there before")?;
    if fingerprint(r).replace("\"isEnabled\":true", "").replace("\"isEnabled\":false", "") != fingerprint(b).replace("\"isEnabled\":true", "").replace("\"isEnabled\":false", "") {
        return Err("the rule changed in other ways too".into())
    }
    others_unchanged(before, after, &[id], false)
}

pub fn verify_delete(before: &[Value], after: &[Value], id: &str) -> Result<(), String> {
    if by_id(after, id).is_some() { return Err("the rule is still there".into()) }
    others_unchanged(before, after, &[id], false)
}

/// After a move to `to`: the rule sits at `to`, the set of rules is the same, only positions differ.
pub fn verify_move(before: &[Value], after: &[Value], id: &str, to: i64) -> Result<(), String> {
    if before.len() != after.len() { return Err("the number of rules changed".into()) }
    let Some(r) = by_id(after, id) else { return Err("the rule disappeared".into()) };
    if r["sequence"].as_i64() != Some(to) { return Err(format!("the rule is at position {:?}, not {}", r["sequence"].as_i64(), to)) }
    others_unchanged(before, after, &[], true)
}

/// Rules that would see a message from `matcher` BEFORE a new rule appended at the end, and stop it there.
/// (First match wins, and 30 of the user's 35 rules stop.) Returns one line per shadowing enabled rule.
pub fn shadowed_by(matcher: &Match, existing: &[Value]) -> Vec<String> {
    let probe = match matcher { Match::Address(a) => a.trim().to_lowercase(), Match::Domain(d) => format!("x@{}", d.trim().trim_start_matches('@').to_lowercase()) };
    let mut out = Vec::new();
    for r in existing {
        let s = summarize(r);
        if !s.enabled || s.read_only || !s.stops || !s.exceptions.is_empty() { continue }
        let c = &r["conditions"];
        // only single-condition rules are judged; anything else is left out rather than guessed
        let keys: Vec<&String> = c.as_object().map(|o| o.iter().filter(|(_, v)| is_set(v)).map(|(k, _)| k).collect()).unwrap_or_default();
        if keys.len() != 1 { continue }
        let hit = match keys[0].as_str() {
            "fromAddresses" => texts(&c["fromAddresses"]).iter().any(|a| a.to_lowercase() == probe),
            "senderContains" => texts(&c["senderContains"]).iter().any(|t| !t.is_empty() && probe.contains(&t.to_lowercase())),
            _ => false,
        };
        if hit {
            let what = if s.tags.is_empty() { s.actions.join(", ") } else { format!("tag {}", s.tags.join(", ")) };
            out.push(format!("rule {} ({}) comes first and stops: {}", s.sequence, what, s.conditions.join(", ")));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tag_rule(id: &str, seq: i64, from: Value, tag: &str, enabled: bool) -> Value {
        json!({ "id": id, "displayName": format!("For all messages from {id}"), "sequence": seq, "isEnabled": enabled,
                "isReadOnly": false, "hasError": false,
                "conditions": from, "actions": { "assignCategories": [tag], "stopProcessingRules": true } })
    }

    #[test]
    fn tag_rule_reads_in_plain_language() {
        let r = summarize(&tag_rule("a", 3, json!({ "senderContains": ["carmelbaptist.org"] }), "CARMEL", true));
        assert_eq!(r.kind, "tag");
        assert_eq!(r.tags, vec!["CARMEL"]);
        assert_eq!(r.conditions, vec!["sender contains \u{201c}carmelbaptist.org\u{201d}"]);
        assert!(r.stops && r.enabled && r.actions.is_empty() && r.unknown.is_empty());
    }

    #[test]
    fn from_addresses_objects_and_several_values() {
        let r = summarize(&tag_rule("b", 1, json!({ "fromAddresses": [
            { "emailAddress": { "address": "a@x.org", "name": "A" } }, { "emailAddress": { "address": "b@y.org" } }] }), "Family", true));
        assert_eq!(r.conditions, vec!["from a@x.org or b@y.org"]);
    }

    #[test]
    fn non_tag_rules_are_not_called_tag_rules() {
        let fwd = summarize(&json!({ "id": "f", "displayName": "fwd", "sequence": 5, "isEnabled": true,
            "conditions": { "fromAddresses": [{ "emailAddress": { "address": "news@s.com" } }] },
            "actions": { "forwardTo": [{ "emailAddress": { "address": "erika@x.net" } }], "stopProcessingRules": true } }));
        assert_eq!((fwd.kind, fwd.actions.clone()), ("other", vec!["forward to erika@x.net".to_string()]));
        let del = summarize(&json!({ "id": "d", "sequence": 6, "conditions": { "bodyContains": ["spam"] }, "actions": { "delete": true } }));
        assert_eq!(del.kind, "other");
        assert_eq!(del.actions, vec!["move to Deleted Items"]);
        let imp = summarize(&json!({ "id": "i", "sequence": 7, "conditions": { "senderContains": ["lifelock"] }, "actions": { "markImportance": "high" } }));
        assert_eq!(imp.actions, vec!["set importance to high"]);
        // tag plus another action: still listed with its tag, but as "other" so the extra action is visible
        let both = summarize(&json!({ "id": "t", "sequence": 8, "conditions": { "senderContains": ["x"] }, "actions": { "assignCategories": ["T"], "markAsRead": true } }));
        assert_eq!((both.kind, both.tags.clone(), both.actions.clone()), ("other", vec!["T".to_string()], vec!["mark as read".to_string()]));
    }

    #[test]
    fn read_only_and_empty_rules_are_system_and_unknown_parts_are_never_hidden() {
        let cp = summarize(&json!({ "id": "cp1", "displayName": "CP1", "sequence": 1, "isEnabled": true, "isReadOnly": true, "conditions": {}, "actions": {} }));
        assert_eq!(cp.kind, "system");
        let empty = summarize(&json!({ "id": "e", "sequence": 2, "conditions": { "senderContains": [] }, "actions": { "assignCategories": [] } }));
        assert_eq!(empty.kind, "system", "nothing set at all");
        let odd = summarize(&json!({ "id": "o", "sequence": 3, "conditions": { "fromAddresses": [{ "emailAddress": { "address": "a@b.c" } }], "frobnicate": ["x"] },
            "actions": { "assignCategories": ["T"], "playSound": true } }));
        assert_eq!(odd.kind, "other", "an action we cannot word is not a plain tag rule");
        assert_eq!(odd.unknown, vec!["frobnicate", "playSound"]);
        let off = summarize(&tag_rule("x", 4, json!({ "senderContains": ["q"] }), "IGOR", false));
        assert!(!off.enabled);
    }

    #[test]
    fn exceptions_are_kept_apart_from_conditions() {
        let r = summarize(&json!({ "id": "x", "sequence": 1, "conditions": { "senderContains": ["shop"] },
            "exceptions": { "subjectContains": ["receipt"] }, "actions": { "assignCategories": ["Shopping"] } }));
        assert_eq!(r.exceptions, vec!["subject contains \u{201c}receipt\u{201d}"]);
        assert_eq!(r.conditions.len(), 1);
    }

    #[test]
    fn organise_groups_by_tag_in_rule_order_and_keeps_the_rest_apart() {
        let rules: Vec<RuleSummary> = vec![
            tag_rule("r3", 3, json!({ "senderContains": ["c"] }), "SaintLucia", true),
            tag_rule("r1", 1, json!({ "senderContains": ["a"] }), "SaintLucia", true),
            tag_rule("r2", 2, json!({ "senderContains": ["b"] }), "Family", true),
            tag_rule("r4", 4, json!({ "senderContains": ["d"] }), "saintlucia", true),
            json!({ "id": "cp", "sequence": 9, "isReadOnly": true, "conditions": {}, "actions": {} }),
            json!({ "id": "del", "sequence": 8, "conditions": { "bodyContains": ["x"] }, "actions": { "delete": true } }),
        ].iter().map(summarize).collect();
        let o = organise(rules);
        assert_eq!(o.groups.iter().map(|g| (g.tag.as_str(), g.rules.len())).collect::<Vec<_>>(), vec![("SaintLucia", 3), ("Family", 1)]);
        assert_eq!(o.groups[0].rules.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec!["r1", "r3", "r4"], "rule order within a tag; spelling variants group together");
        assert_eq!(o.other.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec!["del"]);
        assert_eq!(o.system.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec!["cp"]);
    }

    // ---- phase 3: writing

    fn live_like() -> Vec<Value> {
        vec![
            json!({ "id": "cp1", "displayName": "CP1", "sequence": 1, "isEnabled": true, "isReadOnly": true, "conditions": {}, "actions": {} }),
            tag_rule("a", 2, json!({ "senderContains": ["CARMELBAPTIST.ORG"] }), "CARMEL", true),
            tag_rule("b", 3, json!({ "fromAddresses": [{ "emailAddress": { "address": "Venmo@Venmo.com" } }] }), "BBQ2022", true),
            json!({ "id": "fwd", "displayName": "fwd", "sequence": 4, "isEnabled": true, "isReadOnly": false,
                    "conditions": { "senderContains": ["TRADESMART"] }, "actions": { "forwardTo": [{ "emailAddress": { "address": "e@x.net" } }], "stopProcessingRules": true } }),
        ]
    }

    fn server_stores(body: &Value, id: &str, seq: i64) -> Value {
        // what Graph does: assigns id and sequence, upper-cases condition text
        let mut u = body.clone();
        u["id"] = json!(id); u["sequence"] = json!(seq); u["isReadOnly"] = json!(false); u["hasError"] = json!(false);
        if let Some(a) = u["conditions"].get_mut("fromAddresses").and_then(|v| v.as_array_mut()) { for x in a { let s = x["emailAddress"]["address"].as_str().unwrap().to_uppercase(); x["emailAddress"]["address"] = json!(s.clone()); x["emailAddress"]["name"] = json!(s); } }
        if let Some(a) = u["conditions"].get_mut("senderContains").and_then(|v| v.as_array_mut()) { for x in a { *x = json!(x.as_str().unwrap().to_uppercase()); } }
        u
    }

    #[test]
    fn new_rule_bodies_match_the_users_own_rule_shape() {
        let a = new_rule_body(&NewRule { tag: " Family ".into(), matcher: Match::Address("Erika@Macielinski.net".into()), enabled: true, stop: true });
        assert_eq!(a["conditions"]["fromAddresses"][0]["emailAddress"]["address"], "erika@macielinski.net");
        assert_eq!(a["actions"], json!({ "assignCategories": ["Family"], "stopProcessingRules": true }));
        assert_eq!(a["isEnabled"], true);
        assert_eq!(a["sequence"], 9999, "Graph requires a sequence; a big one is appended at the end");
        let d = new_rule_body(&NewRule { tag: "CARMEL".into(), matcher: Match::Domain("@Carmelbaptist.org".into()), enabled: false, stop: false });
        assert_eq!(d["conditions"], json!({ "senderContains": ["@carmelbaptist.org"] }));
        assert_eq!((d["isEnabled"].clone(), d["actions"]["stopProcessingRules"].clone()), (json!(false), json!(false)));
        assert_eq!(summarize(&d).kind, "tag");
    }

    #[test]
    fn input_validation() {
        assert!(valid_address("a@b.co") && valid_address(" A.B+c@x-y.org "));
        for bad in ["", "a", "a@b", "@b.co", "a@@b.co", "a b@c.co", "a@b.co,c@d.co", "a@.co", "<a@b.co>", "a@b.co;"] { assert!(!valid_address(bad), "{bad}"); }
        assert!(valid_domain("x.org") && valid_domain("@x.org") && valid_domain("mail.x-y.org"));
        for bad in ["", "x", "x.", ".org", "a b.org", "x.org/evil", "a@b.org"] { assert!(!valid_domain(bad), "{bad}"); }
        assert!(valid_tag_name("Saint Lucia") && !valid_tag_name("") && !valid_tag_name("a,b") && !valid_tag_name("a;b") && !valid_tag_name(&"x".repeat(65)));
    }

    #[test]
    fn only_plain_tag_rules_are_writable() {
        let r = live_like();
        assert!(writable(&r[1]).is_ok());
        assert!(writable(&r[0]).unwrap_err().contains("read-only"));
        assert!(writable(&r[3]).unwrap_err().contains("only plain tag rules"));
    }

    #[test]
    fn verify_create_accepts_what_graph_really_does_and_rejects_surprises() {
        let before = live_like();
        let body = new_rule_body(&NewRule { tag: "Family".into(), matcher: Match::Address("erika@x.net".into()), enabled: false, stop: true });
        let mut after = before.clone();
        after.push(server_stores(&body, "new1", 5));
        assert_eq!(verify_create(&before, &after, &body).unwrap(), "new1");
        // nothing arrived
        assert!(verify_create(&before, &before, &body).unwrap_err().contains("found 0"));
        // stored with another tag
        let mut wrong = after.clone();
        wrong[4]["actions"]["assignCategories"] = json!(["Other"]);
        assert!(verify_create(&before, &wrong, &body).unwrap_err().contains("actions"));
        // an old rule changed behind our back
        let mut moved = after.clone();
        moved[1]["isEnabled"] = json!(false);
        assert!(verify_create(&before, &moved, &body).unwrap_err().contains("changed unexpectedly"));
        // two new rules
        let mut two = after.clone();
        two.push(server_stores(&body, "new2", 6));
        assert!(verify_create(&before, &two, &body).unwrap_err().contains("found 2"));
    }

    #[test]
    fn verify_enable_delete_move() {
        let before = live_like();
        let mut after = before.clone();
        after[1]["isEnabled"] = json!(false);
        assert!(verify_enable(&before, &after, "a", false).is_ok());
        assert!(verify_enable(&before, &after, "a", true).unwrap_err().contains("did not take"));
        let mut also = after.clone();
        also[1]["conditions"] = json!({ "senderContains": ["OTHER"] });
        assert!(verify_enable(&before, &also, "a", false).unwrap_err().contains("other ways"));
        let mut hit_other = after.clone();
        hit_other[2]["isEnabled"] = json!(false);
        assert!(verify_enable(&before, &hit_other, "a", false).unwrap_err().contains("unexpectedly"));

        let del: Vec<Value> = before.iter().filter(|r| r["id"] != "b").cloned().collect();
        assert!(verify_delete(&before, &del, "b").is_ok(), "Graph leaves a gap in the sequence: nothing else moves");
        assert!(verify_delete(&before, &before, "b").unwrap_err().contains("still there"));
        let mut renum = del.clone();
        renum[2]["sequence"] = json!(3);
        assert!(verify_delete(&before, &renum, "b").unwrap_err().contains("unexpectedly"), "an unexpected renumbering is reported, not ignored");

        // move a (seq 2) down to b's slot (3): Graph swaps them
        let mut mv = before.clone();
        mv[1]["sequence"] = json!(3);
        mv[2]["sequence"] = json!(2);
        assert!(verify_move(&before, &mv, "a", 3).is_ok());
        assert!(verify_move(&before, &mv, "a", 4).unwrap_err().contains("not 4"));
        mv[3]["displayName"] = json!("renamed");
        assert!(verify_move(&before, &mv, "a", 3).unwrap_err().contains("unexpectedly"));
    }

    #[test]
    fn same_effect_ignores_what_graph_adds_and_case() {
        let ours = new_rule_body(&NewRule { tag: "Family".into(), matcher: Match::Address("erika@x.net".into()), enabled: true, stop: true });
        // exactly what the live server returned for a stored rule (name added to the address, other fields null)
        let theirs = json!({ "id": "r", "displayName": "omalook: Family <- erika@x.net", "sequence": 36, "isEnabled": true, "isReadOnly": false,
            "conditions": { "fromAddresses": [{ "emailAddress": { "address": "ERIKA@X.NET", "name": "erika@x.net" } }], "senderContains": null, "sentToAddresses": [] },
            "actions": { "assignCategories": ["Family"], "stopProcessingRules": true, "delete": false, "forwardTo": null } });
        assert!(same_effect(&ours, &theirs));
        let mut other_tag = theirs.clone();
        other_tag["actions"]["assignCategories"] = json!(["Work"]);
        assert!(!same_effect(&ours, &other_tag));
        let mut other_sender = theirs.clone();
        other_sender["conditions"]["fromAddresses"][0]["emailAddress"]["address"] = json!("OTHER@X.NET");
        assert!(!same_effect(&ours, &other_sender));
        let mut keeps_going = theirs.clone();
        keeps_going["actions"]["stopProcessingRules"] = json!(false);
        assert!(!same_effect(&ours, &keeps_going), "stop true vs not stopping is a real difference");
        let off = { let mut o = ours.clone(); o["actions"]["stopProcessingRules"] = json!(false); o };
        let mut absent = theirs.clone();
        absent["actions"].as_object_mut().unwrap().remove("stopProcessingRules");
        assert!(same_effect(&off, &absent), "an absent flag is a false flag");
    }

    #[test]
    fn fingerprint_ignores_position_only() {
        let a = tag_rule("a", 2, json!({ "senderContains": ["X"] }), "T", true);
        let mut b = a.clone();
        b["sequence"] = json!(9);
        assert_eq!(fingerprint(&a), fingerprint(&b));
        b["isEnabled"] = json!(false);
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn a_new_rule_at_the_end_can_be_shadowed_by_an_earlier_stopping_rule() {
        let r = live_like();
        let carmel = shadowed_by(&Match::Address("pastor@carmelbaptist.org".into()), &r);
        assert_eq!(carmel.len(), 1);
        assert!(carmel[0].contains("rule 2") && carmel[0].contains("CARMEL"), "{carmel:?}");
        assert_eq!(shadowed_by(&Match::Domain("carmelbaptist.org".into()), &r).len(), 1, "a domain probe is judged as an address at that domain");
        assert!(shadowed_by(&Match::Address("venmo@venmo.com".into()), &r)[0].contains("BBQ2022"), "address match ignores case");
        assert!(shadowed_by(&Match::Address("nobody@else.org".into()), &r).is_empty());
        // a disabled rule or one that does not stop shadows nothing
        let mut off = r.clone();
        off[1]["isEnabled"] = json!(false);
        assert!(shadowed_by(&Match::Address("pastor@carmelbaptist.org".into()), &off).is_empty());
        let mut goes_on = r.clone();
        goes_on[1]["actions"]["stopProcessingRules"] = json!(false);
        assert!(shadowed_by(&Match::Address("pastor@carmelbaptist.org".into()), &goes_on).is_empty());
    }
}
