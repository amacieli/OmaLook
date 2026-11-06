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
}
