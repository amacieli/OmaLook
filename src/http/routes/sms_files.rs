//! SMS cache and writes (KDE Connect), the picked-file allow-list (`/files/register`), the compose
//! file picker (`/fs/list`) and the search history. Moved from the legacy chain in `main.rs` (R3.1,
//! group G1). Answers are 200 with JSON as before; the SMS routes report failures inside the JSON.

use crate::http::router::{Ctx, Lock, Pattern, Response, Route};
use crate::Database;
use log::warn;
use std::collections::HashMap;
use std::path::PathBuf;

pub fn routes() -> Vec<Route> {
    use Pattern::Exact;
    vec![
        Route::new("GET", Exact("/sms/threads"), Lock::Exclusive, sms_threads),
        Route::new("GET", Exact("/sms/contacts"), Lock::Exclusive, sms_contacts),
        Route::new("GET", Exact("/sms/messages"), Lock::Exclusive, sms_messages),
        Route::new("GET", Exact("/sms/attachment"), Lock::Exclusive, sms_attachment),
        Route::new("GET", Exact("/sms/unread"), Lock::Exclusive, sms_unread),
        Route::new("POST", Exact("/sms/send"), Lock::Exclusive, sms_send),
        Route::new("POST", Exact("/sms/read"), Lock::Exclusive, sms_read),
        Route::new("POST", Exact("/sms/retry"), Lock::Exclusive, sms_retry),
        Route::new("POST", Exact("/sms/sync"), Lock::Exclusive, sms_sync),
        Route::new("POST", Exact("/files/register"), Lock::Exclusive, files_register),
        Route::new("POST", Exact("/files/picker"), Lock::Exclusive, files_picker),
        Route::new("GET", Exact("/fs/list"), Lock::Exclusive, fs_list),
        Route::new("GET", Exact("/search/history"), Lock::Exclusive, search_history_get),
        Route::new("POST", Exact("/search/history"), Lock::Exclusive, search_history_post),
    ]
}

/// `{"error": msg}` as a 200 answer (the SMS routes never answer with another status).
fn sms_error(msg: &str) -> Response {
    Response::json(serde_json::json!({ "error": msg }).to_string())
}

/// messages.db for the SMS routes; a failure is reported as `database: ...`.
fn open_db(c: &Ctx) -> Result<Database, String> {
    Database::open(&c.db_path()).map_err(|e| format!("database: {}", e))
}

/// A query's answer as JSON, or `{"error": ...}` when it failed.
fn answer(r: crate::errors::Result<serde_json::Value>) -> Response {
    match r {
        Ok(v) => Response::json(v.to_string()),
        Err(e) => sms_error(&e.to_string()),
    }
}

fn query_int(c: &Ctx, key: &str) -> Option<i64> {
    c.query(key).and_then(|v| v.parse::<i64>().ok())
}

/// The request body as UTF-8, read the way the old `read_post_body` did: without a Content-Length
/// header it is an error (the body reader alone would call it empty), and it is capped at `limit`.
fn post_body(c: &Ctx, limit: usize) -> Result<String, String> {
    if c.req.header("content-length").is_none() {
        return Err("missing Content-Length".into());
    }
    c.body_text(limit)
}

/// Contact name for each address ("" when the phone shared none).
fn sms_names_of(device: &str, addresses: &[String]) -> Vec<String> {
    addresses.iter().map(|a| crate::sms_names::lookup(device, a).unwrap_or_default()).collect()
}

/// Display title of a thread: contact names (falling back to the number), comma-joined.
fn sms_title(addresses: &[String], names: &[String]) -> String {
    let mut seen: Vec<String> = vec![];
    for (a, n) in addresses.iter().zip(names) {
        let shown = if n.is_empty() { a.clone() } else { n.clone() };
        if !seen.contains(&shown) {
            seen.push(shown);
        }
    }
    seen.join(", ")
}

/// A short excerpt of `body` around the first word of `query` it contains.
fn sms_snippet(body: &str, query: &str) -> String {
    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = flat.chars().collect();
    let lower: String = flat.to_lowercase();
    let at = query
        .split_whitespace()
        .filter_map(|w| lower.find(&w.to_lowercase()))
        .min()
        .map(|b| lower[..b].chars().count())
        .unwrap_or(0);
    let start = at.saturating_sub(24);
    let end = (start + 90).min(chars.len());
    format!(
        "{}{}{}",
        if start > 0 { "\u{2026}" } else { "" },
        chars[start..end].iter().collect::<String>(),
        if end < chars.len() { "\u{2026}" } else { "" }
    )
}

/// GET /sms/threads?device=&limit=&q= — conversations, newest first. With `q`, every thread is
/// searched and the matches are kept (by contact name, phone number, or message text).
fn sms_threads(c: &Ctx) -> Response {
    let db = match open_db(c) {
        Ok(db) => db,
        Err(e) => return sms_error(&e),
    };
    let device = c.query("device");
    let query = c.query("q").unwrap_or_default().trim().to_string();
    let limit = if query.is_empty() { query_int(c, "limit").unwrap_or(500).clamp(1, 5000) } else { 5000 };
    let hits: HashMap<(String, i64), String> = if query.is_empty() {
        Default::default()
    } else {
        db.sms_search_bodies(device.as_deref(), &query, 20000)
            .unwrap_or_default()
            .into_iter()
            .map(|(d, t, b)| ((d, t), b))
            .collect()
    };
    let q_lower = query.to_lowercase();
    let q_digits: String = query.chars().filter(|ch| ch.is_ascii_digit()).collect();
    answer(db.sms_threads(device.as_deref(), limit).map(|r| {
        serde_json::Value::Array(
            r.into_iter()
                .filter_map(|t| {
                    let names = sms_names_of(&t.device_id, &t.addresses);
                    let title = sms_title(&t.addresses, &names);
                    let mut v = serde_json::json!(t);
                    v["title"] = serde_json::json!(title);
                    v["names"] = serde_json::json!(names);
                    if !query.is_empty() {
                        let by_name = names.iter().chain(std::iter::once(&title)).any(|n| n.to_lowercase().contains(&q_lower));
                        let by_number = q_digits.len() >= 3
                            && t.addresses.iter().any(|a| a.chars().filter(|ch| ch.is_ascii_digit()).collect::<String>().contains(&q_digits));
                        let body = hits.get(&(t.device_id.clone(), t.thread_id));
                        if by_name || by_number {
                            v["match"] = serde_json::json!(if by_name { "name" } else { "number" });
                        } else if let Some(b) = body {
                            v["match"] = serde_json::json!("text");
                            v["snippet"] = serde_json::json!(sms_snippet(b, &query));
                        } else {
                            return None;
                        }
                        if by_name || by_number {
                            if let Some(b) = body {
                                v["snippet"] = serde_json::json!(sms_snippet(b, &query));
                            }
                        }
                    }
                    Some(v)
                })
                .collect(),
        )
    }))
}

/// GET /sms/contacts?device=&q= — recipient suggestions: the phone's contacts whose name or number
/// contains `q`.
fn sms_contacts(c: &Ctx) -> Response {
    let db = match open_db(c) {
        Ok(db) => db,
        Err(e) => return sms_error(&e),
    };
    let q = c.query("q").unwrap_or_default().to_lowercase();
    let qd: String = q.chars().filter(|ch| ch.is_ascii_digit()).collect();
    let dev = c.query("device").or_else(|| db.sms_default_device().ok().flatten()).unwrap_or_default();
    let list: Vec<serde_json::Value> = crate::sms_names::contacts(&dev)
        .into_iter()
        .filter(|(n, num)| {
            q.is_empty()
                || n.to_lowercase().contains(&q)
                || (qd.len() >= 2 && num.chars().filter(|ch| ch.is_ascii_digit()).collect::<String>().contains(&qd))
        })
        .take(12)
        .map(|(n, num)| serde_json::json!({ "name": n, "number": num }))
        .collect();
    Response::json(serde_json::json!({ "device": dev, "contacts": list }).to_string())
}

/// GET /sms/messages?thread=&device=&before=&limit= — one page of a conversation, newest first.
fn sms_messages(c: &Ctx) -> Response {
    let db = match open_db(c) {
        Ok(db) => db,
        Err(e) => return sms_error(&e),
    };
    let device = c.query("device");
    match query_int(c, "thread") {
        None => sms_error("missing thread"),
        Some(t) => answer(
            db.sms_messages(device.as_deref(), t, query_int(c, "before"), query_int(c, "limit").unwrap_or(100).clamp(1, 1000))
                .map(|r| {
                    serde_json::Value::Array(
                        r.into_iter()
                            .map(|m| {
                                let names = sms_names_of(&m.device_id, &m.addresses);
                                let mut v = serde_json::json!(m);
                                v["names"] = serde_json::json!(names);
                                v
                            })
                            .collect(),
                    )
                }),
        ),
    }
}

/// GET /sms/attachment?device=&part=&uid= — state of one MMS picture (the first call asks the phone).
fn sms_attachment(c: &Ctx) -> Response {
    if let Err(e) = open_db(c) {
        return sms_error(&e);
    }
    match (c.query("device"), query_int(c, "part"), c.query("uid")) {
        (Some(d), Some(part), Some(uid)) if !d.is_empty() && !uid.is_empty() => {
            Response::json(crate::sms_daemon::attachment_status(&d, part, &uid).to_string())
        }
        _ => sms_error("need device, part and uid"),
    }
}

/// GET /sms/unread — unread message and thread counts.
fn sms_unread(c: &Ctx) -> Response {
    let db = match open_db(c) {
        Ok(db) => db,
        Err(e) => return sms_error(&e),
    };
    answer(db.sms_unread_totals().map(|(m, t)| serde_json::json!({ "messages": m, "threads": t })))
}

/// POST /sms/send — send a text through KDE Connect. JSON body:
/// `{"device"?: id, "thread"?: id, "to"?: ["number", ...], "text": "..."}`. With `thread` it replies in
/// that conversation; with `to` it starts one (or reuses the thread that already has exactly those
/// recipients). The text is stored as a pending row at once (queued -> the phone's echo replaces it,
/// or it turns into a failure). Answers `{ok, device, thread, uid, new_thread}` or `{ok:false, error, ...}`.
fn sms_send(c: &Ctx) -> Response {
    Response::json(sms_send_answer(c))
}

fn sms_send_answer(c: &Ctx) -> String {
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let raw = match post_body(c, 64 * 1024) {
        Ok(b) => b,
        Err(e) => return err(&e),
    };
    let req: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return err(&format!("unreadable request: {}", e)),
    };
    let text = req["text"].as_str().unwrap_or("").trim_end().to_string();
    if text.trim().is_empty() {
        return err("nothing to send");
    }
    if text.chars().count() > 5000 {
        return err("message too long");
    }
    let db = match open_db(c) {
        Ok(d) => d,
        Err(e) => return err(&e),
    };
    let device = match req["device"].as_str().map(String::from).or_else(|| db.sms_default_device().ok().flatten()) {
        Some(d) => d,
        None => return err("no phone paired (Settings > SMS)"),
    };
    let to: Vec<String> = req["to"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let (mut thread, addresses) = match req["thread"].as_i64() {
        Some(t) => match db.sms_messages(Some(&device), t, None, 1).ok().and_then(|r| r.into_iter().next()) {
            Some(m) => (Some(t), m.addresses),
            None => return err("unknown conversation"),
        },
        None => {
            if to.is_empty() {
                return err("add at least one recipient");
            }
            if let Some(bad) = to.iter().find(|a| a.chars().filter(|ch| ch.is_ascii_digit()).count() < 3) {
                return err(&format!("not a phone number: {}", bad));
            }
            (db.sms_thread_for_addresses(&device, &to).ok().flatten(), to)
        }
    };
    if matches!(thread, Some(t) if t <= 0) {
        thread = None; // a stand-in thread: send as new
    }
    let new_thread = thread.is_none();
    let shown_thread = thread.unwrap_or_else(|| crate::db::sms::pending_thread_id(&addresses));
    let uid = match db.sms_add_pending(&device, shown_thread, &addresses, &text, false) {
        Ok(u) => u,
        Err(e) => return err(&e.to_string()),
    };
    crate::sync_state::bump_sms("send queued");
    match crate::sms_daemon::send_text(&device, thread, &addresses, &text) {
        Ok(()) => serde_json::json!({ "ok": true, "device": device, "thread": shown_thread, "uid": uid, "new_thread": new_thread }).to_string(),
        Err(e) => {
            warn!("SMS send failed: {}", e);
            let _ = db.sms_set_pending_state(&device, uid, true);
            crate::sync_state::bump_sms("send failed");
            serde_json::json!({ "ok": false, "error": e, "device": device, "thread": shown_thread, "uid": uid, "new_thread": new_thread }).to_string()
        }
    }
}

/// POST /sms/read?thread=&read=true|false[&uid=&device=] — local read state only (KDE Connect cannot
/// mark messages read on the phone; the best-effort phone sync is in `sms_daemon`).
fn sms_read(c: &Ctx) -> Response {
    let db = match open_db(c) {
        Ok(db) => db,
        Err(e) => return sms_error(&e),
    };
    let device = c.query("device");
    let t = match query_int(c, "thread") {
        None => return sms_error("missing thread"),
        Some(t) => t,
    };
    let read = c.query("read").as_deref() != Some("false");
    // Unread follows mail: only the newest received message of the thread goes back to unread.
    let changed = if read || query_int(c, "uid").is_some() {
        db.sms_set_read(device.as_deref(), t, query_int(c, "uid"), read)
    } else {
        db.sms_mark_thread_unread(device.as_deref(), t)
    };
    answer(changed.map(|n| {
        if n > 0 {
            crate::sync_state::bump_sms("read flag");
        }
        if n > 0 && read {
            // Best effort: ask the phone's SMS app to mark the thread read (see sms_daemon).
            if let Some(dev) = device.clone().or_else(|| db.sms_default_device().ok().flatten()) {
                let mut needles: Vec<String> = vec![];
                if let Ok(rows) = db.sms_messages(Some(&dev), t, None, 1) {
                    if let Some(m) = rows.first() {
                        needles.extend(sms_names_of(&dev, &m.addresses));
                        needles.extend(m.addresses.clone());
                    }
                }
                crate::sms_daemon::sync_read_to_phone(dev, needles);
            }
        }
        serde_json::json!({ "ok": true, "changed": n })
    }))
}

/// POST /sms/retry?uid=&thread=&device= — send a failed text again: one (`uid`) or every failed text
/// of the thread.
fn sms_retry(c: &Ctx) -> Response {
    let db = match open_db(c) {
        Ok(db) => db,
        Err(e) => return sms_error(&e),
    };
    let dev = c.query("device").or_else(|| db.sms_default_device().ok().flatten()).unwrap_or_default();
    let uids = match (query_int(c, "uid"), query_int(c, "thread")) {
        (Some(u), _) => vec![u],
        (None, Some(t)) => db.sms_failed_in_thread(&dev, t).unwrap_or_default(),
        _ => vec![],
    };
    let (mut sent, mut failed) = (0, 0);
    for u in uids {
        let Ok(Some((thread, addrs, body))) = db.sms_failed_row(&dev, u) else { continue };
        let _ = db.sms_set_pending_state(&dev, u, false);
        match crate::sms_daemon::send_text(&dev, Some(thread), &addrs, &body) {
            Ok(()) => sent += 1,
            Err(e) => {
                warn!("SMS retry failed: {}", e);
                let _ = db.sms_set_pending_state(&dev, u, true);
                failed += 1;
            }
        }
    }
    crate::sync_state::bump_sms("retry");
    Response::json(serde_json::json!({ "ok": failed == 0, "sent": sent, "failed": failed }).to_string())
}

/// POST /sms/sync — ask the phone for a full re-pull. The DB is opened first, as before, so a broken
/// DB answers the same error.
fn sms_sync(c: &Ctx) -> Response {
    if let Err(e) = open_db(c) {
        return sms_error(&e);
    }
    Response::json(serde_json::json!({ "ok": crate::sms_daemon::request_sync() }).to_string())
}

/// POST /files/register — `{"purpose": "attach"|"photo"|"import", "paths": [...]}`. Each file the user
/// picked, pasted, dropped or typed gets a handle (`app::files::register`). Answers
/// `{ok, files: [{handle, name, size}], refused: [{path, error}]}`: no path comes back, and a refused
/// entry echoes the path as sent and names no other file.
fn files_register(c: &Ctx) -> Response {
    use crate::app::files::{self, Purpose};
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let raw = match post_body(c, 256 * 1024) {
        Ok(b) => b,
        Err(e) => return Response::json(err(&e)),
    };
    let req: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return Response::json(err("unreadable request")),
    };
    let purpose = match Purpose::parse(req["purpose"].as_str().unwrap_or("")) {
        Some(p) => p,
        None => return Response::json(err("unknown purpose")),
    };
    let Some(list) = req["paths"].as_array() else { return Response::json(err("no files named")) };
    if list.len() > files::MAX_PER_REQUEST {
        return Response::json(err("too many files in one request"));
    }
    let mut registered = Vec::new();
    let mut refused = Vec::new();
    for p in list {
        let as_sent = p.as_str().unwrap_or("").to_string();
        match files::register(purpose, &as_sent) {
            Ok(f) => registered.push(f),
            Err(e) => refused.push(serde_json::json!({ "path": as_sent, "error": e })),
        }
    }
    Response::json(serde_json::json!({ "ok": true, "files": registered, "refused": refused }).to_string())
}

/// POST /files/picker `{"purpose": "attach"|"photo"|"import"}` — the folder browser opens: answers
/// `{ok, picker}`, the id every `GET /fs/list` of that browser must carry. A picker expires 30 minutes
/// after its last listing; at most 16 are live.
fn files_picker(c: &Ctx) -> Response {
    use crate::app::files::{self, Purpose};
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let raw = match post_body(c, 4 * 1024) {
        Ok(b) => b,
        Err(e) => return Response::json(err(&e)),
    };
    let req: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return Response::json(err("unreadable request")),
    };
    let Some(purpose) = Purpose::parse(req["purpose"].as_str().unwrap_or("")) else {
        return Response::json(err("unknown purpose"));
    };
    Response::json(match files::open_picker(purpose) {
        Ok(id) => serde_json::json!({ "ok": true, "picker": id }).to_string(),
        Err(e) => err(&e),
    })
}

/// GET /fs/list?picker=ID&path=DIR[&hidden=1] — one directory for the compose file picker. The picker
/// must be live (else `{error: "picker expired"}`), and folders under the secret roots are not listed.
fn fs_list(c: &Ctx) -> Response {
    use crate::app::files;
    let err = |m: &str| serde_json::json!({ "error": m }).to_string();
    if !files::picker_live(&c.query("picker").unwrap_or_default()) {
        return Response::json(err("picker expired"));
    }
    let path = c.query("path").unwrap_or_default();
    let hidden = c.query("hidden").as_deref() == Some("1");
    // List the canonical folder, so the check and the listing look at the same directory.
    let target = crate::fs_browse::expand(&path);
    let dir = if target.is_file() { target.parent().map(|p| p.to_path_buf()).unwrap_or(target.clone()) } else { target.clone() };
    let list_path = match std::fs::canonicalize(&dir) {
        Ok(canon) if files::in_secret_root(&canon) => return Response::json(err("that folder cannot be listed")),
        Ok(canon) => canon.to_string_lossy().into_owned(),
        Err(_) => path,
    };
    Response::json(match crate::fs_browse::list(&list_path, hidden) {
        Ok(l) => serde_json::to_string(&l).unwrap_or_else(|_| "{\"error\":\"could not list\"}".into()),
        Err(e) => err(&e),
    })
}

fn history_path(c: &Ctx) -> PathBuf {
    c.config_dir.join("search_history.json")
}

/// GET /search/history — recent search patterns, newest first (JSON array).
fn search_history_get(c: &Ctx) -> Response {
    let list = crate::search::load_history(&history_path(c));
    Response::json(serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string()))
}

/// POST /search/history?q=PATTERN — record one (called after it ran fine). Without `q` it only
/// answers the list.
fn search_history_post(c: &Ctx) -> Response {
    let path = history_path(c);
    let list = match c.query("q") {
        Some(q) => crate::search::push_history(&path, &q),
        None => crate::search::load_history(&path),
    };
    Response::json(serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::request::{read_request, test_stream, Limits};

    /// One raw request, read the way the server reads it, through one handler.
    fn run(dir: &PathBuf, raw: String, f: fn(&Ctx) -> Response) -> Response {
        let req = read_request(&mut test_stream(raw.into_bytes()), &Limits::default()).expect("request parses");
        f(&Ctx::new(dir, &req))
    }

    fn get(target: &str) -> String {
        format!("GET {} HTTP/1.1\r\nHost: 127.0.0.1:1\r\n\r\n", target)
    }

    fn post(target: &str, body: &str) -> String {
        format!("POST {} HTTP/1.1\r\nHost: 127.0.0.1:1\r\nContent-Length: {}\r\n\r\n{}", target, body.len(), body)
    }

    fn temp_dir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("omalook-sms-files-{}-{}", test, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn json_of(r: &Response) -> serde_json::Value {
        serde_json::from_slice(r.body()).expect("JSON body")
    }

    #[test]
    fn the_table_lists_each_moved_path_exclusive() {
        let routes = routes();
        let mut seen: Vec<(&str, &str)> = routes.iter().map(|r| (r.method, r.pattern_str())).collect();
        seen.sort();
        assert_eq!(
            seen,
            vec![
                ("GET", "/fs/list"),
                ("GET", "/search/history"),
                ("GET", "/sms/attachment"),
                ("GET", "/sms/contacts"),
                ("GET", "/sms/messages"),
                ("GET", "/sms/threads"),
                ("GET", "/sms/unread"),
                ("POST", "/files/picker"),
                ("POST", "/files/register"),
                ("POST", "/search/history"),
                ("POST", "/sms/read"),
                ("POST", "/sms/retry"),
                ("POST", "/sms/send"),
                ("POST", "/sms/sync"),
            ]
        );
        assert!(routes.iter().all(|r| r.lock == Lock::Exclusive));
    }

    #[test]
    fn files_register_refuses_an_unknown_purpose_and_non_array_paths() {
        let dir = temp_dir("files-register");
        let r = run(&dir, post("/files/register", r#"{"purpose":"nope","paths":["/etc/hostname"]}"#), files_register);
        assert_eq!(r.status(), 200);
        assert_eq!(json_of(&r), serde_json::json!({ "ok": false, "error": "unknown purpose" }));
        let r = run(&dir, post("/files/register", r#"{"purpose":"attach","paths":"/etc/hostname"}"#), files_register);
        assert_eq!(json_of(&r), serde_json::json!({ "ok": false, "error": "no files named" }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_sms_read_on_an_empty_db_answers_json_200() {
        let dir = temp_dir("sms-empty");
        let r = run(&dir, get("/sms/threads?limit=5"), sms_threads);
        assert_eq!(r.status(), 200);
        assert_eq!(json_of(&r), serde_json::json!([]));
        let r = run(&dir, get("/sms/messages?device=dev-1"), sms_messages);
        assert_eq!(json_of(&r), serde_json::json!({ "error": "missing thread" }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_register_answers_handles_and_no_paths() {
        let dir = temp_dir("files-register-ok");
        let f = dir.join("notes.txt");
        std::fs::write(&f, b"12345").unwrap();
        let body = serde_json::json!({ "purpose": "attach", "paths": [f.to_string_lossy(), "relative/x"] }).to_string();
        let r = run(&dir, post("/files/register", &body), files_register);
        let v = json_of(&r);
        assert_eq!(v["ok"], true);
        let files = v["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0]["handle"].as_str().unwrap().starts_with("fh-"));
        assert_eq!((files[0]["name"].as_str(), files[0]["size"].as_u64()), (Some("notes.txt"), Some(5)));
        assert!(files[0].get("path").is_none(), "no canonical path goes back");
        assert_eq!(v["refused"][0]["path"], "relative/x", "a refused entry echoes the path as sent");
        // The handle is what the rest of the backend resolves.
        let h = files[0]["handle"].as_str().unwrap();
        assert!(crate::app::files::resolve(crate::app::files::Purpose::Attach, h).is_some());
        assert!(crate::app::files::resolve(crate::app::files::Purpose::Import, h).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn picker_id(dir: &PathBuf, purpose: &str) -> String {
        let body = serde_json::json!({ "purpose": purpose }).to_string();
        let v = json_of(&run(dir, post("/files/picker", &body), files_picker));
        assert_eq!(v["ok"], true, "{}", v);
        v["picker"].as_str().unwrap().to_string()
    }

    #[test]
    fn fs_list_needs_a_live_picker_and_refuses_secret_roots() {
        let dir = temp_dir("fs-list");
        std::fs::write(dir.join("a.txt"), b"1").unwrap();
        let target = dir.to_string_lossy().into_owned();
        // No picker, or a made-up one: expired.
        let r = run(&dir, get(&format!("/fs/list?path={}", target)), fs_list);
        assert_eq!(r.status(), 200);
        assert_eq!(json_of(&r), serde_json::json!({ "error": "picker expired" }));
        let r = run(&dir, get(&format!("/fs/list?picker=fp-nope&path={}", target)), fs_list);
        assert_eq!(json_of(&r), serde_json::json!({ "error": "picker expired" }));
        // With a picker: the folder lists.
        let id = picker_id(&dir, "attach");
        assert!(id.starts_with("fp-") && id.len() == 35);
        let r = run(&dir, get(&format!("/fs/list?picker={}&path={}", id, target)), fs_list);
        assert!(json_of(&r)["entries"].as_array().unwrap().iter().any(|e| e["name"] == "a.txt"));
        // A secret root is refused even with a live picker (/proc is one on every Linux).
        if std::path::Path::new("/proc/self").exists() {
            let r = run(&dir, get(&format!("/fs/list?picker={}&path=/proc", id)), fs_list);
            assert_eq!(json_of(&r), serde_json::json!({ "error": "that folder cannot be listed" }));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_picker_refuses_an_unknown_purpose() {
        let dir = temp_dir("picker-purpose");
        let r = run(&dir, post("/files/picker", r#"{"purpose":"nope"}"#), files_picker);
        assert_eq!(json_of(&r), serde_json::json!({ "ok": false, "error": "unknown purpose" }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_history_post_then_get_round_trips() {
        let dir = temp_dir("search-history");
        let r = run(&dir, post("/search/history?q=hello", ""), search_history_post);
        assert_eq!(r.status(), 200);
        assert_eq!(json_of(&r), serde_json::json!(["hello"]));
        let r = run(&dir, get("/search/history"), search_history_get);
        assert_eq!(json_of(&r), serde_json::json!(["hello"]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
