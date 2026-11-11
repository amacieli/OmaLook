//! Tags on calendar events (docs/PLAN-tags-and-rules.md, OL-040 phase 6): pure helpers, no I/O.
//!
//! Where the tags live (decision Q5 for Google, plan §6 for Exchange):
//!   - Exchange: the event's Graph `categories`, the same shared master list as mail tags.
//!   - Google: `extendedProperties.private.omalook_tags` (invisible in Google's own UI, syncs between OmaLook
//!     installs, server-side filterable). The value is a JSON array of names, `["Family","Saint Lucia"]`, so a
//!     name may hold a comma; a comma-separated value (what the phase 0 spike wrote) is still read.

use crate::db::tags::{squash, tag_key};
use serde_json::Value;

/// The property OmaLook keeps Google event tags in.
pub const GOOGLE_TAGS_KEY: &str = "omalook_tags";

/// Names with blanks dropped and case-insensitive duplicates removed, first spelling kept.
pub fn clean(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in names {
        let n = n.trim().to_string();
        if n.is_empty() || out.iter().any(|o| tag_key(o) == tag_key(&n)) { continue }
        out.push(n);
    }
    out
}

/// Graph `categories` (an array of names) -> tags.
pub fn from_graph(categories: &Value) -> Vec<String> {
    clean(categories.as_array().into_iter().flatten().filter_map(|c| c.as_str().map(String::from)))
}

/// The Google private property value -> tags (JSON array, else comma-separated).
pub fn from_google(value: &str) -> Vec<String> {
    let v = value.trim();
    if v.starts_with('[') {
        if let Ok(arr) = serde_json::from_str::<Vec<String>>(v) { return clean(arr) }
    }
    clean(v.split(',').map(String::from))
}

/// Tags -> the Google property value ("" when there are none; the caller then removes the property).
pub fn to_google(tags: &[String]) -> String {
    let t = clean(tags.iter().cloned());
    if t.is_empty() { String::new() } else { serde_json::to_string(&t).unwrap_or_default() }
}

/// `tag:` semantics for the calendar, same as mail search: `lucia` = a tag whose name contains it (case, spaces
/// and punctuation ignored, "saint" = "st"), `=Family` = exactly that tag (ignoring case), `none` = untagged,
/// a leading `-` negates. Stars are accepted and ignored. An empty query matches everything.
pub fn matches(tags: &[String], query: &str) -> bool {
    let q = query.trim();
    let (neg, q) = match q.strip_prefix('-') { Some(r) => (true, r.trim()), None => (false, q) };
    let q = q.trim_matches('*').trim().trim_matches('"');
    if q.is_empty() { return !neg }
    let hit = if q.eq_ignore_ascii_case("none") {
        tags.is_empty()
    } else if let Some(exact) = q.strip_prefix('=') {
        let want = tag_key(exact.trim().trim_matches('"'));
        tags.iter().any(|t| tag_key(t) == want)
    } else {
        let want = squash(q);
        !want.is_empty() && tags.iter().any(|t| squash(t).contains(&want))
    };
    hit != neg
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn t(v: &[&str]) -> Vec<String> { v.iter().map(|x| x.to_string()).collect() }

    #[test]
    fn graph_and_google_values_read_and_clean() {
        assert_eq!(from_graph(&json!(["Family", " family ", "", "Saint Lucia"])), t(&["Family", "Saint Lucia"]));
        assert!(from_graph(&json!(null)).is_empty());
        assert_eq!(from_google(r#"["Family","Saint, Lucia"]"#), t(&["Family", "Saint, Lucia"]), "a comma inside a name survives");
        assert_eq!(from_google("Family,SaintLucia"), t(&["Family", "SaintLucia"]), "the spike's format is still read");
        assert!(from_google("").is_empty() && from_google("[]").is_empty());
        assert_eq!(from_google("[not json"), t(&["[not json"]), "garbage is kept as text, never a crash");
    }

    #[test]
    fn a_google_event_carries_its_tags_in_the_private_property() {
        let ev = |props: serde_json::Value| crate::providers::google_calendar::parse_event("gmail-a", "UTC",
            &json!({"id": "g1", "summary": "s", "start": {"dateTime": "2026-10-05T09:00:00Z"}, "end": {"dateTime": "2026-10-05T10:00:00Z"}, "extendedProperties": props})).unwrap();
        assert_eq!(ev(json!({"private": {"omalook_tags": r#"["Family"]"#}})).tags, t(&["Family"]));
        assert_eq!(ev(json!({"private": {"omalook_tags": "Family,Panama"}})).tags, t(&["Family", "Panama"]));
        assert!(ev(json!({"private": {"other": "x"}, "shared": {"omalook_tags": "Nope"}})).tags.is_empty(), "only the private property counts");
        assert!(ev(json!(null)).tags.is_empty());
    }

    #[test]
    fn tags_survive_the_cache_and_the_month_query() {
        use crate::models::CalendarEvent;
        let db = crate::db::Database::open_for_account(":memory:", "ex").unwrap();
        let ev = |id: &str, tags: &[&str]| CalendarEvent { id: id.into(), subject: id.into(), start: "2026-10-05T09:00:00".into(), end: "2026-10-05T10:00:00".into(),
            event_type: "singleInstance".into(), tags: t(tags), ..Default::default() };
        db.upsert_event(&ev("a", &["Family", "Saint, Lucia"])).unwrap();
        db.upsert_event(&ev("b", &[])).unwrap();
        let got = db.get_events_for_month_by_account("2026-10", None).unwrap();
        let tags_of = |id: &str| got.iter().find(|(e, _, _)| e.id == id).unwrap().0.tags.clone();
        assert_eq!(tags_of("a"), t(&["Family", "Saint, Lucia"]));
        assert!(tags_of("b").is_empty());
        // a later sync that removed the tag on the server clears it here too
        db.upsert_event(&ev("a", &[])).unwrap();
        assert!(db.get_events_for_month_by_account("2026-10", None).unwrap().iter().find(|(e, _, _)| e.id == "a").unwrap().0.tags.is_empty());
    }

    #[test]
    fn google_value_round_trips() {
        let v = to_google(&t(&["Family", "Saint, Lucia", "family"]));
        assert_eq!(v, r#"["Family","Saint, Lucia"]"#);
        assert_eq!(from_google(&v), t(&["Family", "Saint, Lucia"]));
        assert_eq!(to_google(&[]), "");
    }

    #[test]
    fn tag_filter_matches_like_mail_search() {
        let tags = t(&["Saint Lucia", "Family"]);
        for q in ["lucia", "LUCIA", "*lucia*", "saintlucia", "st lucia", "\"saint lucia\"", "fam", "=Family", "=family", "=\"Saint Lucia\""] { assert!(matches(&tags, q), "{q}") }
        for q in ["=Fam", "=lucia", "panama", "none", "=SaintLucia"] { assert!(!matches(&tags, q), "{q}") }
        assert!(matches(&[], "none") && !matches(&tags, "none"));
        assert!(!matches(&tags, "-lucia") && matches(&tags, "-panama") && matches(&[], "-lucia"));
        assert!(matches(&tags, "") && matches(&[], "  "), "no filter");
        assert!(!matches(&[], "family"));
    }
}
