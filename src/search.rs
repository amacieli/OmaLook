//! Keyword search patterns for the mail list: `from:stripe is:unread after:30d "exact phrase" -newsletter`.
//!
//! `parse()` turns the text into a [`Query`]; `Query::to_sql()` turns that into WHERE conditions over
//! the `messages m` table (every value is a bound parameter, never spliced into the SQL).
//! All terms are ANDed; a leading `-` negates a term.
//!
//! | term | matches |
//! |---|---|
//! | `word`, `"a phrase"`, `wor*` | subject, sender name or preview body (FTS5, stemmed), or the sender address |
//! | `from:` `to:` `cc:` `bcc:` `subject:` | case-insensitive substring of that field |
//! | `body:` | the locally cached body *preview* (FTS5) |
//! | `fullbody:` | the full body, searched at the provider (not local; see `Query::fullbody`) |
//! | `cat:` / `category:` | exact category name |
//! | `is:unread` `is:read` `is:important` | read state / importance |
//! | `has:attachment` | has a real (non-inline) attachment |
//! | `after:` `before:` `date:` | received date: `2026-10-01`, `today`, `yesterday`, or relative `7d` `2w` `3m` `1y` (`date:` takes a day only) |

use chrono::{DateTime, Duration, Local, NaiveDate, SecondsFormat, TimeZone, Utc};
use rusqlite::types::Value;

const FIELDS: &str =
    "from, to, cc, bcc, subject, body, fullbody, cat, category, is, has, after, before, date";

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    /// Bare word / phrase: FTS over subject, from_name, body — or the sender address.
    Text(String),
    Body(String),
    Subject(String),
    From(String),
    To(String),
    Cc(String),
    Bcc(String),
    Cat(String),
    Unread,
    Read,
    Important,
    Attachment,
    /// received_at >= this UTC timestamp
    OnOrAfter(String),
    /// received_at < this UTC timestamp
    Before(String),
    /// start <= received_at < end
    Between(String, String),
}

#[derive(Debug, Clone, PartialEq)]
struct Clause {
    neg: bool,
    kind: Kind,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    clauses: Vec<Clause>,
    /// `fullbody:` terms as (negated, text). They cannot be answered from the local cache; the
    /// caller must run them against the provider (Phase D) or refuse the query.
    pub fullbody: Vec<(bool, String)>,
}

impl Query {
    /// True when the pattern holds nothing to filter on.
    pub fn is_empty(&self) -> bool {
        self.clauses.is_empty() && self.fullbody.is_empty()
    }

    /// WHERE conditions (to be ANDed) over `messages m`. Bound values are appended to `args`;
    /// placeholders are numbered `?N` from the current length of `args`, so this composes with
    /// conditions the caller has already added. `fullbody` terms are not included.
    pub fn to_sql(&self, args: &mut Vec<Value>) -> Vec<String> {
        let mut out = Vec::new();
        for c in &self.clauses {
            let mut bind = |v: String| -> usize {
                args.push(Value::Text(v));
                args.len()
            };
            let cond = match &c.kind {
                Kind::Text(t) => {
                    let n = bind(fts_term(t, None));
                    let m = bind(like(t));
                    format!(
                        "(m.id IN (SELECT id FROM messages_fts WHERE messages_fts MATCH ?{n}) \
                         OR m.from_email LIKE ?{m} ESCAPE '\\')"
                    )
                }
                Kind::Body(t) => {
                    let n = bind(fts_term(t, Some("body")));
                    format!("m.id IN (SELECT id FROM messages_fts WHERE messages_fts MATCH ?{n})")
                }
                Kind::Subject(t) => {
                    let n = bind(like(t));
                    format!("m.subject LIKE ?{n} ESCAPE '\\'")
                }
                Kind::From(t) => {
                    let n = bind(like(t));
                    format!("(m.from_email LIKE ?{n} ESCAPE '\\' OR COALESCE(m.from_name,'') LIKE ?{n} ESCAPE '\\')")
                }
                Kind::To(t) => {
                    let n = bind(like(t));
                    format!("COALESCE(m.to_text,'') LIKE ?{n} ESCAPE '\\'")
                }
                Kind::Cc(t) => {
                    let n = bind(like(t));
                    format!("COALESCE(m.cc_text,'') LIKE ?{n} ESCAPE '\\'")
                }
                Kind::Bcc(t) => {
                    let n = bind(like(t));
                    format!("COALESCE(m.bcc_text,'') LIKE ?{n} ESCAPE '\\'")
                }
                Kind::Cat(t) => {
                    // categories is a JSON array of names: ["Saint Lucia","Family"]. Substring,
                    // ignoring case and spaces, so `cat:lucia` and `cat:saintlucia` both find "Saint Lucia".
                    let squeezed: String = t.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
                    let n = bind(format!("%{}%", escape_like(&squeezed)));
                    format!("replace(lower(COALESCE(m.categories,'')), ' ', '') LIKE ?{n} ESCAPE '\\'")
                }
                Kind::Unread => "COALESCE(m.is_read,0) = 0".to_string(),
                Kind::Read => "COALESCE(m.is_read,0) = 1".to_string(),
                Kind::Important => "m.importance = 'high'".to_string(),
                Kind::Attachment => "COALESCE((SELECT c.has_real FROM attachment_cache c WHERE c.message_id = m.id), m.has_attachments, 0) = 1".to_string(),
                Kind::OnOrAfter(ts) => {
                    let n = bind(ts.clone());
                    format!("m.received_at >= ?{n}")
                }
                Kind::Before(ts) => {
                    let n = bind(ts.clone());
                    format!("m.received_at < ?{n}")
                }
                Kind::Between(a, b) => {
                    let n = bind(a.clone());
                    let k = bind(b.clone());
                    format!("(m.received_at >= ?{n} AND m.received_at < ?{k})")
                }
            };
            out.push(if c.neg { format!("NOT ({cond})") } else { cond });
        }
        out
    }
}

/// Parse `input` using the current time for relative dates.
pub fn parse(input: &str) -> Result<Query, String> {
    parse_at(input, Utc::now())
}

/// Parse with an explicit "now" (tests).
pub fn parse_at(input: &str, now: DateTime<Utc>) -> Result<Query, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut q = Query::default();
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let mut neg = false;
        if chars[i] == '-' && i + 1 < chars.len() && !chars[i + 1].is_whitespace() {
            neg = true;
            i += 1;
        }
        // optional `field:` prefix (letters, directly followed by ':')
        let mut field: Option<String> = None;
        let mut j = i;
        while j < chars.len() && chars[j].is_ascii_alphabetic() {
            j += 1;
        }
        if j > i && j < chars.len() && chars[j] == ':' {
            let name = chars[i..j].iter().collect::<String>().to_lowercase();
            i = j + 1;
            if FIELDS.split(", ").any(|f| f == name) {
                // `cat: lucia` (space after the colon) reads the same as `cat:lucia`
                while i < chars.len() && chars[i].is_whitespace() { i += 1; }
            }
            field = Some(name);
        }
        // value: "quoted" or a run up to whitespace
        let (value, quoted) = if i < chars.len() && chars[i] == '"' {
            let start = i + 1;
            let mut k = start;
            while k < chars.len() && chars[k] != '"' {
                k += 1;
            }
            if k >= chars.len() {
                return Err("unterminated quote".to_string());
            }
            i = k + 1;
            (chars[start..k].iter().collect::<String>(), true)
        } else {
            let start = i;
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            (chars[start..i].iter().collect::<String>(), false)
        };
        let value = value.trim().to_string();
        if value.is_empty() {
            match field {
                Some(f) => return Err(format!("{f}: needs a value")),
                None if quoted => continue, // "" — nothing to match
                None => continue,
            }
        }
        match field.as_deref() {
            None => q.clauses.push(Clause { neg, kind: Kind::Text(value) }),
            Some("from") => q.clauses.push(Clause { neg, kind: Kind::From(value) }),
            Some("to") => q.clauses.push(Clause { neg, kind: Kind::To(value) }),
            Some("cc") => q.clauses.push(Clause { neg, kind: Kind::Cc(value) }),
            Some("bcc") => q.clauses.push(Clause { neg, kind: Kind::Bcc(value) }),
            Some("subject") => q.clauses.push(Clause { neg, kind: Kind::Subject(value) }),
            Some("body") => q.clauses.push(Clause { neg, kind: Kind::Body(value) }),
            Some("fullbody") => q.fullbody.push((neg, value)),
            Some("cat") | Some("category") => q.clauses.push(Clause { neg, kind: Kind::Cat(value) }),
            Some("is") => {
                let kind = match value.to_lowercase().as_str() {
                    "unread" => Kind::Unread,
                    "read" => Kind::Read,
                    "important" | "high" => Kind::Important,
                    other => return Err(format!("is:{other} — expected unread, read or important")),
                };
                q.clauses.push(Clause { neg, kind });
            }
            Some("has") => match value.to_lowercase().as_str() {
                "attachment" | "attachments" | "attach" => {
                    q.clauses.push(Clause { neg, kind: Kind::Attachment })
                }
                other => return Err(format!("has:{other} — expected attachment")),
            },
            Some("after") => {
                let (start, _) = resolve_date(&value, now)?;
                q.clauses.push(Clause { neg, kind: Kind::OnOrAfter(start) });
            }
            Some("before") => {
                let (start, _) = resolve_date(&value, now)?;
                q.clauses.push(Clause { neg, kind: Kind::Before(start) });
            }
            Some("date") => {
                let (start, end) = resolve_date(&value, now)?;
                let end = end.ok_or_else(|| {
                    format!("date:{value} — use a day (2026-10-01, today, yesterday); for a span use after:/before:")
                })?;
                q.clauses.push(Clause { neg, kind: Kind::Between(start, end) });
            }
            Some(other) => {
                return Err(format!(
                    "unknown field \"{other}:\" (fields: {FIELDS}); quote the text to search for it literally"
                ))
            }
        }
    }
    Ok(q)
}

fn ts(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Start of the local calendar day, as a UTC timestamp string.
fn local_day_start(d: NaiveDate) -> Option<DateTime<Utc>> {
    let naive = d.and_hms_opt(0, 0, 0)?;
    Local.from_local_datetime(&naive).earliest().map(|l| l.with_timezone(&Utc))
}

/// A date word → (start, end-of-that-day if it names a whole day). Days are local-time days;
/// relative spans (`7d`) are exact offsets from `now` and have no day end.
fn resolve_date(v: &str, now: DateTime<Utc>) -> Result<(String, Option<String>), String> {
    let low = v.to_lowercase();
    let today = now.with_timezone(&Local).date_naive();
    let day = |d: NaiveDate| -> Result<(String, Option<String>), String> {
        let s = local_day_start(d).ok_or_else(|| format!("bad date \"{v}\""))?;
        let e = local_day_start(d.succ_opt().ok_or_else(|| format!("bad date \"{v}\""))?)
            .ok_or_else(|| format!("bad date \"{v}\""))?;
        Ok((ts(s), Some(ts(e))))
    };
    match low.as_str() {
        "today" => return day(today),
        "yesterday" => return day(today.pred_opt().ok_or("bad date")?),
        _ => {}
    }
    if let Some(unit) = low.chars().last().filter(|c| matches!(c, 'd' | 'w' | 'm' | 'y')) {
        if let Ok(n) = low[..low.len() - 1].parse::<i64>() {
            if n < 0 || n > 100_000 {
                return Err(format!("bad date \"{v}\""));
            }
            let days = match unit {
                'd' => n,
                'w' => n * 7,
                'm' => n * 30,
                _ => n * 365,
            };
            return Ok((ts(now - Duration::days(days)), None));
        }
    }
    for fmt in ["%Y-%m-%d", "%Y/%m/%d"] {
        if let Ok(d) = NaiveDate::parse_from_str(&low, fmt) {
            return day(d);
        }
    }
    Err(format!("bad date \"{v}\" (use 2026-10-01, today, yesterday, 7d, 2w, 3m, 1y)"))
}

fn escape_like(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '%' | '_') {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

/// `%text%` for LIKE ... ESCAPE '\'.
fn like(s: &str) -> String {
    format!("%{}%", escape_like(s))
}

/// FTS5 MATCH string for one user term: always a quoted phrase (so user punctuation and FTS
/// keywords like AND/NOT/NEAR are inert); a trailing `*` makes it a prefix match.
fn fts_term(t: &str, column: Option<&str>) -> String {
    let (core, prefix) = match t.strip_suffix('*') {
        Some(c) if !c.is_empty() => (c, true),
        _ => (t, false),
    };
    let phrase = format!("\"{}\"{}", core.replace('"', "\"\""), if prefix { "*" } else { "" });
    match column {
        Some(c) => format!("{c} : {phrase}"),
        None => format!("{{subject from_name body}} : {phrase}"),
    }
}

// ---- provider-side search (`fullbody:`) ------------------------------------------------------

/// Graph `$search` rounds at most (each round reaches further back in time).
pub const GRAPH_SEARCH_MAX_ROUNDS: usize = 12;
/// A round that returns fewer hits than this was not cut short, so nothing older matches.
pub const GRAPH_SEARCH_SHORT_ROUND: usize = 100;

/// Words of a `fullbody:` value with characters that mean something to the providers' query
/// languages removed.
fn provider_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| w.chars().filter(|c| !matches!(c, '"' | '(' | ')' | ':' | '\\' | '\'')).collect::<String>())
        .filter(|w| !w.is_empty())
        .collect()
}

/// KQL for Graph's `$search` on messages: every word must appear in the body (`body:a AND body:b`).
/// A trailing `*` stays (prefix match).
pub fn graph_kql(text: &str) -> String {
    provider_words(text).iter().map(|w| format!("body:{w}")).collect::<Vec<_>>().join(" AND ")
}

/// Gmail `q=`: all words, a multi-word value as one quoted phrase. Gmail has no body-only
/// operator, so subject and sender hits count too.
pub fn gmail_q(text: &str) -> String {
    let words = provider_words(text);
    match words.len() {
        0 => String::new(),
        1 => words[0].clone(),
        _ => format!("\"{}\"", words.join(" ")),
    }
}

// ---- search history -------------------------------------------------------------------------
// A small JSON array of patterns (newest first) in the config dir; not part of settings.toml
// because it is data the app writes, not configuration the user edits.

pub const HISTORY_MAX: usize = 50;

/// Saved patterns, newest first. A missing or unreadable file is an empty history.
pub fn load_history(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
        .unwrap_or_default()
}

/// Put `pattern` at the front (dropping an identical older entry), keep `HISTORY_MAX`, write the
/// file atomically, and return the new list. Blank patterns are ignored.
pub fn push_history(path: &std::path::Path, pattern: &str) -> Vec<String> {
    let pattern = pattern.trim();
    let mut list = load_history(path);
    if pattern.is_empty() {
        return list;
    }
    list.retain(|p| p != pattern);
    list.insert(0, pattern.to_string());
    list.truncate(HISTORY_MAX);
    if let Ok(json) = serde_json::to_string(&list) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use rusqlite::Connection;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 12, 15, 0, 0).unwrap()
    }

    fn p(s: &str) -> Query {
        parse_at(s, now()).unwrap()
    }

    #[test]
    fn history_orders_dedupes_caps_and_survives_garbage() {
        let path = std::env::temp_dir().join(format!("omalook-hist-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert!(load_history(&path).is_empty());
        push_history(&path, "from:a");
        push_history(&path, "  is:unread ");
        push_history(&path, "");
        let l = push_history(&path, "from:a");
        assert_eq!(l, ["from:a", "is:unread"]);
        assert_eq!(load_history(&path), l);
        for i in 0..80 {
            push_history(&path, &format!("q{i}"));
        }
        let l = load_history(&path);
        assert_eq!(l.len(), HISTORY_MAX);
        assert_eq!(l[0], "q79");
        std::fs::write(&path, "not json").unwrap();
        assert!(load_history(&path).is_empty());
        assert_eq!(push_history(&path, "x"), ["x"]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn provider_query_builders() {
        assert_eq!(graph_kql("budget"), "body:budget");
        assert_eq!(graph_kql("grab lunch"), "body:grab AND body:lunch");
        assert_eq!(graph_kql("inv* \"x\" (a:b)"), "body:inv* AND body:x AND body:ab");
        assert_eq!(graph_kql("  "), "");
        assert_eq!(gmail_q("budget"), "budget");
        assert_eq!(gmail_q("grab lunch"), "\"grab lunch\"");
        assert_eq!(gmail_q("\"\""), "");
    }

    // ---- parser ----

    #[test]
    fn parses_terms_and_negation() {
        let q = p(r#"invoice -newsletter from:stripe "two words""#);
        assert_eq!(
            q.clauses,
            vec![
                Clause { neg: false, kind: Kind::Text("invoice".into()) },
                Clause { neg: true, kind: Kind::Text("newsletter".into()) },
                Clause { neg: false, kind: Kind::From("stripe".into()) },
                Clause { neg: false, kind: Kind::Text("two words".into()) },
            ]
        );
    }

    #[test]
    fn quoted_value_after_field_and_colon_inside_quotes() {
        let q = p(r#"subject:"weekly report" "re: hello""#);
        assert_eq!(q.clauses[0].kind, Kind::Subject("weekly report".into()));
        assert_eq!(q.clauses[1].kind, Kind::Text("re: hello".into()));
    }

    #[test]
    fn space_after_colon() {
        assert_eq!(p("cat: lucia").clauses[0].kind, Kind::Cat("lucia".into()));
        assert_eq!(p("from: bob is: unread").clauses.len(), 2);
        assert!(parse_at("from:", now()).is_err());
        assert!(parse_at("from:   ", now()).is_err());
    }

    #[test]
    fn lone_dash_and_empty_input() {
        assert!(p("").is_empty());
        assert!(p("   ").is_empty());
        assert_eq!(p("-").clauses[0].kind, Kind::Text("-".into()));
    }

    #[test]
    fn fullbody_is_kept_separate() {
        let q = p("from:bob fullbody:budget -fullbody:draft");
        assert_eq!(q.clauses.len(), 1);
        assert_eq!(q.fullbody, vec![(false, "budget".to_string()), (true, "draft".to_string())]);
    }

    #[test]
    fn errors() {
        assert!(parse_at("from:", now()).unwrap_err().contains("needs a value"));
        assert!(parse_at("foo:bar", now()).unwrap_err().contains("unknown field"));
        assert!(parse_at("http://x.com", now()).unwrap_err().contains("unknown field"));
        assert!(parse_at("\"open", now()).unwrap_err().contains("unterminated"));
        assert!(parse_at("is:flagged", now()).is_err());
        assert!(parse_at("has:cheese", now()).is_err());
        assert!(parse_at("after:someday", now()).is_err());
        assert!(parse_at("date:7d", now()).is_err());
    }

    #[test]
    fn relative_dates() {
        let q = p("after:7d before:1w");
        assert_eq!(q.clauses[0].kind, Kind::OnOrAfter("2026-10-05T15:00:00Z".into()));
        assert_eq!(q.clauses[1].kind, Kind::Before("2026-10-05T15:00:00Z".into()));
    }

    #[test]
    fn absolute_day_spans_24h_ish() {
        match &p("date:2026-10-01").clauses[0].kind {
            Kind::Between(a, b) => assert!(a < b),
            k => panic!("{k:?}"),
        }
    }

    #[test]
    fn sql_is_parameterised() {
        let mut args = vec![Value::Text("preexisting".into())];
        let q = p("from:x') OR 1=1 -- b");
        let conds = q.to_sql(&mut args);
        let all = conds.join(" ");
        assert!(!all.contains("OR 1=1"), "{all}");
        assert!(all.contains("?2"), "placeholders continue after existing args: {all}");
    }

    // ---- against a real schema ----

    fn fixture() -> (Connection, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "omalook-search-test-{}-{:?}.db",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(path.to_str().unwrap()).unwrap();
        drop(db);
        let conn = Connection::open(&path).unwrap();
        let rows: &[(&str, &str, &str, &str, &str, &str, bool, &str, &str, i32, &str)] = &[
            // id, subject, from_email, from_name, body, received_at, is_read, to, categories, attach, importance
            ("1", "Your Stripe receipt", "receipts@stripe.com", "Stripe", "Thanks for your payment of $20", "2026-10-10T12:00:00Z", false, "Adam <adam@x.net>", "[\"Billing\"]", 1, "normal"),
            ("2", "Lunch on Friday?", "bob@example.com", "Bob Smith", "Want to grab lunch", "2026-10-11T12:00:00Z", true, "Adam <adam@x.net>, Cara <cara@x.net>", "[\"Saint Lucia\"]", 0, "high"),
            ("3", "Weekly newsletter", "news@example.org", "The Newsletter", "Invoices explained, 100% free", "2026-09-01T12:00:00Z", false, "Adam <adam@x.net>", "[\"Reading\"]", 0, "normal"),
            ("4", "Re: budget", "cara@x.net", "Cara", "the 50%_off thing", "2025-01-15T12:00:00Z", true, "Adam <adam@x.net>", "[]", 1, "normal"),
        ];
        for r in rows {
            conn.execute(
                "INSERT INTO messages (id, subject, from_email, from_name, body, received_at, is_read, to_text, categories, has_attachments, importance, folder_id)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'f1')",
                rusqlite::params![r.0, r.1, r.2, r.3, r.4, r.5, r.6, r.7, r.8, r.9, r.10],
            )
            .unwrap();
        }
        (conn, path)
    }

    fn run(conn: &Connection, pattern: &str) -> Vec<String> {
        let q = parse_at(pattern, now()).unwrap();
        let mut args = Vec::new();
        let conds = q.to_sql(&mut args);
        let w = if conds.is_empty() { String::new() } else { format!("WHERE {}", conds.join(" AND ")) };
        let sql = format!("SELECT m.id FROM messages m {w} ORDER BY m.id");
        let mut st = conn.prepare(&sql).unwrap_or_else(|e| panic!("{pattern}: {e}\n{sql}"));
        st.query_map(rusqlite::params_from_iter(args.iter()), |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    #[test]
    fn matches_against_real_schema() {
        let (c, path) = fixture();
        assert_eq!(run(&c, ""), ["1", "2", "3", "4"]);
        // bare word: sender address / name / subject / stemmed body
        assert_eq!(run(&c, "stripe"), ["1"]);
        assert_eq!(run(&c, "stripe.com"), ["1"]); // address substring path
        assert_eq!(run(&c, "invoice"), ["3"]); // porter: invoices -> invoice
        assert_eq!(run(&c, "newslet*"), ["3"]); // prefix
        assert_eq!(run(&c, "\"grab lunch\""), ["2"]);
        // fields
        assert_eq!(run(&c, "from:bob"), ["2"]);
        assert_eq!(run(&c, "from:\"Bob Smith\""), ["2"]);
        assert_eq!(run(&c, "to:cara"), ["2"]);
        assert_eq!(run(&c, "subject:lunch"), ["2"]);
        assert_eq!(run(&c, "body:payment"), ["1"]);
        assert_eq!(run(&c, "cat:billing"), ["1"]);
        assert_eq!(run(&c, "cat:bill"), ["1"]); // substring
        assert_eq!(run(&c, "cat:lucia"), ["2"]);
        assert_eq!(run(&c, "cat:saintlucia"), ["2"]); // spaces in the name don't matter
        assert_eq!(run(&c, "cat:\"saint lucia\""), ["2"]);
        assert_eq!(run(&c, "cat: BILL"), ["1"]); // space after the colon, any case
        // flags
        assert_eq!(run(&c, "is:unread"), ["1", "3"]);
        assert_eq!(run(&c, "is:read"), ["2", "4"]);
        assert_eq!(run(&c, "is:important"), ["2"]);
        assert_eq!(run(&c, "has:attachment"), ["1", "4"]);
        // negation & combination
        assert_eq!(run(&c, "is:unread -newsletter"), ["1"]);
        assert_eq!(run(&c, "-has:attachment -is:unread"), ["2"]);
        // dates (now = 2026-10-12 15:00Z)
        assert_eq!(run(&c, "after:7d"), ["1", "2"]);
        assert_eq!(run(&c, "before:30d"), ["3", "4"]);
        assert_eq!(run(&c, "after:2026-10-11"), ["2"]);
        assert_eq!(run(&c, "before:2026-01-01"), ["4"]);
        assert_eq!(run(&c, "date:2026-10-10"), ["1"]);
        assert_eq!(run(&c, "after:2026-09-01 before:2026-10-11"), ["1", "3"]);
        // literal wildcards and quotes are inert
        assert_eq!(run(&c, "subject:%"), Vec::<String>::new());
        assert_eq!(run(&c, "50%_off"), ["4"]);
        assert_eq!(run(&c, "from:\"'; DROP TABLE messages; --\""), Vec::<String>::new());
        assert_eq!(run(&c, "NOT"), Vec::<String>::new()); // FTS keyword is just a word
        assert_eq!(run(&c, "a\"\"b"), Vec::<String>::new()); // stray quotes don't break FTS syntax
        let _ = std::fs::remove_file(&path);
    }
}
