//! Message routes (R3.1, group G7): the list (`GET /messages`, with search and the provider-side
//! `fullbody:` terms), bodies, read/unread, categories, move/archive/delete with undo, invitations,
//! list headers, attachments and the render cache. Moved from the legacy chain in `main.rs`; answers
//! are the same JSON as before. The render helper path stays in `main.rs` (it needs `resolve_qml_dir`).

use crate::http::router::{Ctx, Lock, Pattern, Response, Route};
use crate::providers::graph::GraphEmailProvider;
use crate::{AuthManager, Database};
use log::{info, warn};
use std::path::PathBuf;
use super::calendar::refresh_calendar_note;

pub fn routes() -> Vec<Route> {
    use Pattern::Exact;
    vec![
        Route::new("POST", Exact("/messages/read"), Lock::Exclusive, message_read),
        Route::new("GET", Exact("/categories"), Lock::Exclusive, categories),
        Route::new("POST", Exact("/categories/create"), Lock::Exclusive, categories_create),
        Route::new("POST", Exact("/messages/category"), Lock::Exclusive, message_category),
        Route::new("GET", Exact("/messages/meta_progress"), Lock::Exclusive, meta_progress),
        Route::new("POST", Exact("/messages/action"), Lock::Exclusive, message_action),
        Route::new("POST", Exact("/messages/action/undo"), Lock::Exclusive, message_action_undo),
        Route::new("GET", Exact("/messages/list-info"), Lock::None, list_info),
        Route::new("GET", Exact("/messages/invite"), Lock::None, invite_info),
        // These two ran on their own thread outside the dispatch lock before R3.1 (provider calls): still unlocked.
        Route::new("POST", Exact("/messages/invite"), Lock::None, respond_invite),
        Route::new("GET", Exact("/messages/attachments"), Lock::None, message_attachments),
        Route::new("POST", Exact("/messages/attachments/save"), Lock::None, save_attachments),
        Route::new("GET", Exact("/render/get"), Lock::None, render_get),
        Route::new("POST", Exact("/render/adopt"), Lock::Exclusive, render_adopt),
        Route::new("GET", Exact("/messages/body"), Lock::None, message_body),
        Route::new("GET", Exact("/messages"), Lock::None, messages_list),
    ]
}

/// POST /messages/read?id=ID&read=true|false — mark a message read/unread.
/// Updates the local cache at once; the daemon pushes it to the provider.
fn message_read(c: &Ctx) -> Response {
    let id = c.query("id").unwrap_or_default();
    let read = c.query("read").as_deref() == Some("true");
    let ok = !id.is_empty()
        && Database::open(&c.db_path())
            .and_then(|db| db.set_message_read(&id, read))
            .map(|changed| if changed { crate::sync_state::request_read_push() })
            .is_ok();
    Response::json(if ok { "ok" } else { "error" })
}

/// GET /categories?account=ID — the account's categories (Exchange) / user labels (Gmail) with colours,
/// whether the Exchange category list is readable at all, and the colour palette for new ones.
fn categories(c: &Ctx) -> Response {
    let config_dir = c.config_dir;
    let db_path = config_dir.join("messages.db");
    let account = c.query("account").unwrap_or_default();
    let body = match rusqlite::Connection::open(&db_path) {
        Ok(conn) => {
            let cats: Vec<serde_json::Value> = conn
                .prepare("SELECT name, color, pending FROM category_defs WHERE account_id = ?1 AND delete_pending = 0 ORDER BY lower(name)")
                .and_then(|mut st| {
                    st.query_map(rusqlite::params![account], |r| Ok(serde_json::json!({
                        "name": r.get::<_, String>(0)?, "color": r.get::<_, String>(1)?, "pending": r.get::<_, i64>(2)? != 0,
                    }))).map(|it| it.flatten().collect())
                })
                .unwrap_or_default();
            let master: bool = conn
                .query_row("SELECT master_list FROM category_state WHERE account_id = ?1", rusqlite::params![account], |r| r.get::<_, i64>(0))
                .map(|v| v != 0)
                .unwrap_or(true);
            let palette: Vec<serde_json::Value> = crate::models::CATEGORY_PALETTE.iter()
                .map(|c| serde_json::json!({ "key": c.key, "label": c.label, "hex": c.hex }))
                .collect();
            serde_json::json!({ "categories": cats, "master_list": master, "palette": palette }).to_string()
        }
        Err(_) => serde_json::json!({ "categories": [], "master_list": true, "palette": [] }).to_string(),
    };
    Response::json(body)
}

/// POST /categories/create?account=ID&name=X&color=KEY — a new category / label.
/// Usable at once; the daemon creates it on the provider.
fn categories_create(c: &Ctx) -> Response {
    let account = c.query("account").unwrap_or_default();
    let name = c.query("name").unwrap_or_default().trim().to_string();
    let color = c.query("color").and_then(|k| crate::models::palette_color(&k));
    let body = match color {
        Some(pal) if !account.is_empty() && !name.is_empty() && name.chars().count() <= 64 && !name.chars().any(|ch| ch.is_control() || ch == ';' || ch == ',') => {
            match Database::open_for_account(&c.db_path(), &account)
                .and_then(|db| db.add_pending_category(&name, pal.hex))
            {
                Ok(created) => {
                    crate::sync_state::request_read_push();
                    serde_json::json!({ "created": created }).to_string()
                }
                Err(e) => serde_json::json!({ "error": format!("database: {}", e) }).to_string(),
            }
        }
        _ => serde_json::json!({ "error": "need account, a name (max 64 chars, no ; or ,) and a palette colour" }).to_string(),
    };
    Response::json(body)
}

/// POST /messages/category?ids=a,b&op=add|remove&name=X — set / clear a category on messages.
/// Stored at once; the daemon pushes it to the provider.
fn message_category(c: &Ctx) -> Response {
    let ids: Vec<String> = c.query("ids")
        .map(|v| v.split(',').filter(|s| !s.is_empty()).map(String::from).collect())
        .unwrap_or_default();
    let name = c.query("name").unwrap_or_default();
    let add = match c.query("op").as_deref() { Some("add") => Some(true), Some("remove") => Some(false), _ => None };
    let body = match add {
        Some(add) if !ids.is_empty() && !name.is_empty() => {
            match Database::open(&c.db_path())
                .and_then(|db| db.change_message_categories(&ids, add, &name))
            {
                Ok(changed) => {
                    if changed > 0 { crate::sync_state::request_read_push(); }
                    serde_json::json!({ "changed": changed }).to_string()
                }
                Err(e) => serde_json::json!({ "error": format!("database: {}", e) }).to_string(),
            }
        }
        _ => serde_json::json!({ "error": "need ids, name and op=add|remove" }).to_string(),
    };
    Response::json(body)
}

/// GET /messages/meta_progress — {"active":bool,"remaining":N}: the one-time repopulate of message
/// details (sender name, To/Cc/Bcc, categories…).
fn meta_progress(c: &Ctx) -> Response {
    let remaining: i64 = rusqlite::Connection::open(c.config_dir.join("messages.db"))
        .and_then(|c| c.query_row("SELECT COUNT(*) FROM messages WHERE meta_ok = 0", [], |r| r.get(0)))
        .unwrap_or(0);
    Response::json(serde_json::json!({ "active": crate::sync_state::meta_active(), "remaining": remaining }).to_string())
}

/// POST /messages/action?ids=a,b&op=archive|trash|delete|move[&dest=FOLDER] — archive / trash /
/// delete / move messages. The rows are hidden at once; the daemon tells the provider when the undo
/// window (the mail "send delay") is over.
fn message_action(c: &Ctx) -> Response {
    message_action_answer(c, false)
}

/// POST /messages/action/undo?ids=a,b takes them back while that window is open.
fn message_action_undo(c: &Ctx) -> Response {
    message_action_answer(c, true)
}

fn message_action_answer(c: &Ctx, undo: bool) -> Response {
    let config_dir = c.config_dir;
    let db_path = config_dir.join("messages.db");
    let ids: Vec<String> = c.query("ids")
        .map(|v| v.split(',').filter(|s| !s.is_empty()).map(String::from).collect())
        .unwrap_or_default();
    let delay = crate::settings::read_send_delay(&config_dir.join("settings.toml"));
    let body = match Database::open(db_path.to_str().unwrap_or("messages.db")) {
        Ok(db) if undo => {
            let undone = db.cancel_message_actions(&ids).unwrap_or(0);
            if undone > 0 { crate::sync_state::bump_mail("message action undone"); }
            serde_json::json!({ "undone": undone }).to_string()
        }
        Ok(db) => {
            use crate::models::MessageAction;
            let op = c.query("op").unwrap_or_default();
            let action = match op.as_str() {
                "archive" => Some(MessageAction::Archive),
                "trash" => Some(MessageAction::Trash),
                "delete" => Some(MessageAction::Delete),
                "move" => c.query("dest").map(MessageAction::Move),
                _ => None,
            };
            match action {
                Some(action) if !ids.is_empty() => {
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
                    let queued = db.queue_message_actions(&ids, &action, now + delay as i64).unwrap_or(0);
                    // Wake the daemons the moment the window closes.
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(delay * 1000 + 400));
                        crate::sync_state::request_read_push();
                    });
                    serde_json::json!({ "queued": queued, "delay_secs": delay }).to_string()
                }
                _ => serde_json::json!({ "error": "bad request: need ids and op (archive|trash|delete|move+dest)" }).to_string(),
            }
        }
        Err(e) => serde_json::json!({ "error": format!("database: {}", e) }).to_string(),
    };
    Response::json(body)
}

/// GET /messages/list-info?id=ID — List-Id / List-Post of a message (lazy provider fetch, cached).
fn list_info(c: &Ctx) -> Response {
    let id = c.query("id").unwrap_or_default();
    Response::json(list_info_json(c.config_dir, &id))
}

/// GET /messages/invite?id=ID — meeting type of a message and whether it can be answered. Cached after the first look.
fn invite_info(c: &Ctx) -> Response {
    let id = c.query("id").unwrap_or_default();
    Response::json(invite_info_json(c.config_dir, &id))
}

/// POST /messages/invite?id=ID&choice=accept|tentative|decline|remove — answer an invitation (remove: drop
/// a cancelled meeting from the calendar) on Graph (silent reply), then refresh the calendar for
/// accept/tentative. Slow, so it holds the dispatch lock while it runs.
fn respond_invite(c: &Ctx) -> Response {
    let id = c.query("id").unwrap_or_default();
    let choice = c.query("choice").unwrap_or_default();
    Response::json(respond_invite_json(c.config_dir, &id, &choice))
}

/// GET /messages/attachments?id=ID — the message's attachments (cached after the first fetch).
fn message_attachments(c: &Ctx) -> Response {
    let id = c.query("id").unwrap_or_default();
    Response::json(message_attachments_json(c.config_dir, &id))
}

/// POST /messages/attachments/save?id=ID[&att=ATT] — save one, or all non-inline ones, as individual
/// files into the download folder.
fn save_attachments(c: &Ctx) -> Response {
    let id = c.query("id").unwrap_or_default();
    let att = c.query("att");
    Response::json(save_attachments_json(c.config_dir, &id, att.as_deref()))
}

/// GET /render/get?id&width&scale&images (render cache, OL-028).
fn render_get(c: &Ctx) -> Response {
    Response::json(render_get_json(c))
}

/// POST /render/adopt?id&width&scale&images&file (render cache, OL-028).
fn render_adopt(c: &Ctx) -> Response {
    Response::json(render_adopt_json(c))
}

/// GET /messages/body?id=ID — full body of one message: {"type","content"}. Served from the cache;
/// otherwise fetched from the provider and cached.
fn message_body(c: &Ctx) -> Response {
    let id = c.query("id").unwrap_or_default();
    Response::json(message_body_json(c.config_dir, &id))
}

/// GET /messages — messages as JSON (optional ?folder_id=, ?q=, ?limit=, ?offset=, ?account=).
fn messages_list(c: &Ctx) -> Response {
    messages_response(c, std::time::Instant::now())
}

/// Invitation lookup of a message as JSON (see `InviteInfo::to_json`), or `{"error":"..."}`.
/// Ordinary mail is cached for good; calendar mail for a few minutes, because the answer can
/// change elsewhere (Outlook, phone) and the event can be cancelled or deleted.
fn invite_info_json(config_dir: &std::path::Path, id: &str) -> String {
    use crate::invites::{cache_ttl_secs, InviteInfo};
    let err = |m: &str| format!("{{\"error\":{}}}", serde_json::to_string(m).unwrap());
    if id.is_empty() {
        return err("missing id");
    }
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let account = match db.message_account(id) {
        Ok(Some(a)) => a,
        _ => return err("unknown message"),
    };
    if crate::accounts::provider_of(&account) != "exchange" {
        return InviteInfo::none().to_json().to_string();
    }
    let cached = db.invite_info(id).ok().flatten();
    if let Some((json, at)) = &cached {
        let kind = serde_json::from_str::<serde_json::Value>(json)
            .ok().and_then(|v| v["type"].as_str().map(str::to_string)).unwrap_or_default();
        if !kind.is_empty() && chrono::Utc::now().timestamp() - at < cache_ttl_secs(&kind) {
            return json.clone();
        }
    }
    match crate::runtime::block_on(GraphEmailProvider::new(AuthManager::for_account(&account)).fetch_invite_info(id)) {
        Ok(info) => {
            let json = info.to_json().to_string();
            if let Err(e) = db.set_invite_info(id, &json) {
                warn!("Could not cache invite info of {}: {}", id, e);
            }
            json
        }
        Err(e) => {
            warn!("Invite lookup failed for {}: {}", id, e);
            // A stale answer beats none (offline, token trouble).
            match cached {
                Some((json, _)) => json,
                None => err("could not read this message from the server"),
            }
        }
    }
}

/// List lookup of a message for reply-to-list (OL-053): `{is_list, list_id, post, no_post}` plus a `refusal`
/// sentence when a reply cannot go to the list, or `{"error"}`. The headers are fetched from the provider once
/// per message (the cache keeps no List-* headers) and the answer is cached for good in `list_info`.
fn list_info_json(config_dir: &std::path::Path, id: &str) -> String {
    use crate::list_headers::{refusal, ListInfo};
    let err = |m: &str| format!("{{\"error\":{}}}", serde_json::to_string(m).unwrap());
    if id.is_empty() {
        return err("missing id");
    }
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let with_refusal = |info: ListInfo| {
        let mut j = info.to_json();
        j["refusal"] = serde_json::json!(refusal(&info));
        j.to_string()
    };
    if let Some(cached) = db.list_info_get(id).ok().flatten() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&cached) {
            return with_refusal(ListInfo::from_json(&v));
        }
    }
    let account = match db.message_account(id) {
        Ok(Some(a)) => a,
        _ => return err("unknown message"),
    };
    let headers = match crate::accounts::provider_of(&account) {
        "gmail" => crate::runtime::block_on(crate::providers::gmail::GmailProvider::new(&account, None).fetch_list_headers(id)),
        "exchange" => crate::runtime::block_on(GraphEmailProvider::new(AuthManager::for_account(&account)).fetch_list_headers(id)),
        _ => return err("this account type cannot tell whether a message came from a list"),
    };
    match headers {
        Ok(h) => {
            let info = ListInfo::from_headers(&h);
            if let Err(e) = db.list_info_set(id, &info.to_json().to_string()) {
                warn!("Could not cache list info of {}: {}", id, e);
            }
            with_refusal(info)
        }
        Err(e) => {
            warn!("List lookup failed for {}: {}", id, e);
            err("could not read this message's headers from the server")
        }
    }
}

/// `y` on a meeting cancellation: remove the cancelled meeting (one occurrence or a whole series,
/// as the cancellation says) from the user's calendar, then refresh the calendar. A cancellation
/// whose meeting is no longer on the calendar succeeds without touching anything, so the caller
/// can still mark the mail read. Returns the lookup JSON plus `"ok":true`, `"removed":bool`,
/// `"series":bool`, `"calendar":...`, or `{"error":...}`.
fn remove_cancelled_json(config_dir: &std::path::Path, id: &str) -> String {
    use crate::invites::status_of;
    let err = |m: &str| format!("{{\"error\":{}}}", serde_json::to_string(m).unwrap());
    let db_path = config_dir.join("messages.db");
    let db = match Database::open(db_path.to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let account = match db.message_account(id) {
        Ok(Some(a)) => a,
        _ => return err("unknown message"),
    };
    if crate::accounts::provider_of(&account) != "exchange" {
        return err("removing cancelled meetings is only supported for Microsoft accounts");
    }
    let graph = GraphEmailProvider::new(AuthManager::for_account(&account));
    // Fresh lookup, never the cache: a stale answer here could delete the wrong thing.
    let mut info = match crate::runtime::block_on(graph.fetch_invite_info(id)) {
        Ok(i) => i,
        Err(e) => {
            warn!("Invite lookup before removing failed for {}: {}", id, e);
            return err("could not read this message from the server");
        }
    };
    if info.kind != "meetingCancelled" {
        return err("this message is not a meeting cancellation");
    }
    let mut removed = false;
    if let Some(event_id) = info.event_id.clone().filter(|_| info.removable()) {
        match crate::runtime::block_on(graph.delete_event(&event_id)) {
            Ok(()) => removed = true,
            // Already gone (removed in Outlook, or the server dropped it): the goal is met.
            Err(e) if status_of(&e.to_string()) == Some(404) => {}
            Err(e) => {
                warn!("Removing cancelled meeting of {} failed: {}", id, e);
                return match status_of(&e.to_string()) {
                    Some(403) => err("not allowed: grant Calendars.ReadWrite to this account (sign in again after the Azure consent)"),
                    _ => err("the server refused to remove the meeting"),
                };
            }
        }
    }
    if removed {
        info.event_id = None;
        info.response = "none".to_string();
    }
    let mut out = info.to_json();
    if let Err(e) = db.set_invite_info(id, &out.to_string()) {
        warn!("Could not cache invite info of {}: {}", id, e);
    }
    let note = if removed {
        refresh_calendar_note(db_path.to_str().unwrap_or("messages.db"), &account, info.start_date())
    } else {
        "refreshed".to_string()
    };
    out["ok"] = serde_json::json!(true);
    out["removed"] = serde_json::json!(removed);
    out["calendar"] = serde_json::json!(note);
    out.to_string()
}

/// Answer an invitation (choice = accept | tentative | decline) on its Graph event, then
/// refresh the calendar so the change shows at once. Returns the new `InviteInfo` JSON plus
/// `"ok":true` and `"calendar":"refreshed"|"not refreshed: ..."`, or `{"error":...}`.
/// A 403 means the account has not granted Calendars.ReadWrite yet.
fn respond_invite_json(config_dir: &std::path::Path, id: &str, choice: &str) -> String {
    use crate::invites::{response_op, status_of};
    let err = |m: &str| format!("{{\"error\":{}}}", serde_json::to_string(m).unwrap());
    if id.is_empty() {
        return err("missing id");
    }
    if choice == "remove" {
        return remove_cancelled_json(config_dir, id);
    }
    let Some((op, new_response)) = response_op(choice) else {
        return err("unknown response");
    };
    let db_path = config_dir.join("messages.db");
    let db = match Database::open(db_path.to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let account = match db.message_account(id) {
        Ok(Some(a)) => a,
        _ => return err("unknown message"),
    };
    if crate::accounts::provider_of(&account) != "exchange" {
        return err("responding to invitations is only supported for Microsoft accounts");
    }
    let graph = GraphEmailProvider::new(AuthManager::for_account(&account));
    // Always look the invitation up afresh: the cache may predate a cancellation.
    let mut info = match crate::runtime::block_on(graph.fetch_invite_info(id)) {
        Ok(i) => i,
        Err(e) => {
            warn!("Invite lookup before responding failed for {}: {}", id, e);
            return err("could not read this message from the server");
        }
    };
    let Some(event_id) = info.event_id.clone().filter(|_| info.actionable()) else {
        let why = if info.is_organizer { "you are the organizer of this meeting" }
            else if info.is_cancelled || info.kind == "meetingCancelled" { "this meeting was cancelled" }
            else if info.kind == "meetingRequest" { "the meeting is no longer on your calendar" }
            else { "this message is not an open invitation" };
        return err(&format!("cannot respond: {}", why));
    };
    if let Err(e) = crate::runtime::block_on(graph.respond_to_event(&event_id, op)) {
        warn!("Respond {} to {} failed: {}", op, id, e);
        return match status_of(&e.to_string()) {
            Some(403) => err("not allowed: grant Calendars.ReadWrite to this account (sign in again after the Azure consent)"),
            Some(404) => err("the meeting no longer exists on the server"),
            _ => err("the server rejected the response"),
        };
    }
    info.response = new_response.to_string();
    let mut out = info.to_json();
    if let Err(e) = db.set_invite_info(id, &out.to_string()) {
        warn!("Could not cache invite info of {}: {}", id, e);
    }
    // Every answer changes the calendar: accept/tentative add or update the event, decline removes it.
    let note = refresh_calendar_note(db_path.to_str().unwrap_or("messages.db"), &account, info.start_date());
    out["ok"] = serde_json::json!(true);
    out["calendar"] = serde_json::json!(note);
    out.to_string()
}

/// GET /render/get?id&width&scale&images -> {"hit":false} or {"hit":true,"file":PNG,"meta":{width,height,scale,bg,links}}.
/// Also remembers the width and scale so the background pre-render draws at the size the pane really uses.
fn render_get_json(c: &Ctx) -> String {
    let config_dir = c.config_dir;
    let miss = "{\"hit\":false}".to_string();
    let id = c.query("id").unwrap_or_default();
    let width: u32 = c.query("width").and_then(|v| v.parse().ok()).unwrap_or(0);
    let scale: f64 = c.query("scale").and_then(|v| v.parse().ok()).unwrap_or(1.0);
    let images = c.query("images").as_deref() == Some("1");
    if id.is_empty() || width < 100 { return miss; }
    let cache = crate::render_cache::open_configured(config_dir);
    cache.write_hint(crate::render_cache::Hint { width, scale });
    let Ok(db) = Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) else { return miss };
    let Ok(Some((t, body))) = db.cached_body(&id) else { return miss };
    if t != "html" { return miss; }
    let k = crate::render_cache::key(&id, &body, width, scale, images);
    match cache.lookup(&k) {
        Some((png, meta)) => {
            let m: serde_json::Value = serde_json::from_str(&meta).unwrap_or(serde_json::Value::Null);
            serde_json::json!({ "hit": true, "file": png.to_string_lossy(), "meta": m }).to_string()
        }
        None => miss,
    }
}

/// POST /render/adopt?id&width&scale&images&file=PNG: the pane just rendered a message itself (helper output beside the
/// PNG as PNG.json); keep a copy in the render cache. Only the pane's own temp files (XDG_RUNTIME_DIR/omalook-render-N.png).
fn render_adopt_json(c: &Ctx) -> String {
    let config_dir = c.config_dir;
    let err = |m: &str| format!("{{\"ok\":false,\"error\":{}}}", serde_json::to_string(m).unwrap());
    let id = c.query("id").unwrap_or_default();
    let width: u32 = c.query("width").and_then(|v| v.parse().ok()).unwrap_or(0);
    let scale: f64 = c.query("scale").and_then(|v| v.parse().ok()).unwrap_or(1.0);
    let images = c.query("images").as_deref() == Some("1");
    let file = PathBuf::from(c.query("file").unwrap_or_default());
    let run = PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into()));
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if file.parent() != Some(run.as_path()) || !name.starts_with("omalook-render-") || !name.ends_with(".png") { return err("not a render output"); }
    let Ok(json) = std::fs::read_to_string(format!("{}.json", file.display())) else { return err("no helper output") };
    let Ok(db) = Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) else { return err("database") };
    let Ok(Some((t, body))) = db.cached_body(&id) else { return err("no cached body") };
    if t != "html" { return err("not html"); }
    let received: String = rusqlite::Connection::open(config_dir.join("messages.db")).ok()
        .and_then(|c| c.query_row("SELECT COALESCE(received_at,'') FROM messages WHERE id = ?1", [&id], |r| r.get(0)).ok()).unwrap_or_default();
    let cache = crate::render_cache::open_configured(config_dir);
    let k = crate::render_cache::key(&id, &body, width, scale, images);
    match cache.store(&k, &id, &received, &file, &json) {
        Ok(kept) => format!("{{\"ok\":true,\"kept\":{}}}", kept),
        Err(e) => err(&e.to_string()),
    }
}

/// JSON for GET /messages/body: `{"type":"html"|"text","content":"..."}`, or
/// `{"error":"..."}` when the body cannot be had.
pub(crate) fn message_body_json(config_dir: &std::path::Path, id: &str) -> String {
    let err = |m: &str| format!("{{\"error\":{}}}", serde_json::to_string(m).unwrap());
    if id.is_empty() {
        return err("missing id");
    }
    let path = config_dir.join("messages.db");
    let db = match Database::open(path.to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let ok = |t: &str, c: &str| format!("{{\"type\":\"{}\",\"content\":{}}}", t, serde_json::to_string(c).unwrap());
    if let Ok(Some((t, c))) = db.cached_body(id) {
        return ok(&t, &c);
    }
    let account = match db.message_account(id) {
        Ok(Some(a)) => a,
        _ => return err("unknown message"),
    };
    let fetched = if crate::accounts::provider_of(&account) == "gmail" {
        crate::runtime::block_on(crate::providers::GmailProvider::new(&account, None).fetch_message_body(id))
    } else {
        crate::runtime::block_on(GraphEmailProvider::new(AuthManager::for_account(&account)).fetch_message_body(id))
    };
    match fetched {
        Ok((t, c)) => {
            if let Err(e) = db.store_body(id, &t, &c) {
                warn!("Could not cache body of {}: {}", id, e);
            }
            ok(&t, &c)
        }
        Err(e) => {
            warn!("Body fetch failed for {}: {}", id, e);
            // The cached preview still lets the pane show something.
            let reason = if e.to_string().contains("404") {
                "this message no longer exists on the server"
            } else {
                "could not fetch the message body"
            };
            format!(
                "{{\"error\":{},\"preview\":{}}}",
                serde_json::to_string(reason).unwrap(),
                serde_json::to_string(&db.message_preview(id).unwrap_or_default()).unwrap()
            )
        }
    }
}

/// The Graph provider for message `id`, or a user-facing reason it cannot be had.
pub(crate) fn graph_for_message(db: &Database, id: &str) -> Result<GraphEmailProvider, String> {
    let account = match db.message_account(id) {
        Ok(Some(a)) => a,
        _ => return Err("unknown message".into()),
    };
    if crate::accounts::provider_of(&account) == "gmail" {
        return Err("attachments are not supported for Gmail accounts yet".into());
    }
    Ok(GraphEmailProvider::new(AuthManager::for_account(&account)))
}

/// The attachment list of a message: cached, else fetched (and cached). The HTML body is
/// consulted so pictures embedded in it are not listed as attachments.
pub(crate) fn attachment_list(config_dir: &std::path::Path, db: &Database, id: &str) -> Result<Vec<crate::attachments::Attachment>, String> {
    if let Ok(Some(list)) = db.cached_attachments(id) {
        return Ok(list);
    }
    let provider = graph_for_message(db, id)?;
    if !matches!(db.cached_body(id), Ok(Some(_))) {
        let _ = message_body_json(config_dir, id); // fetches and caches the body
    }
    let body = match db.cached_body(id) { Ok(Some((t, c))) if t == "html" => Some(c), _ => None };
    match crate::runtime::block_on(provider.fetch_attachments(id, body.as_deref())) {
        Ok(list) => {
            if let Err(e) = db.store_attachments(id, &list) {
                warn!("Could not cache attachments of {}: {}", id, e);
            }
            Ok(list)
        }
        Err(e) => {
            warn!("Attachment list failed for {}: {}", id, e);
            Err(if e.to_string().contains("404") { "this message no longer exists on the server".into() } else { "could not fetch the attachment list".into() })
        }
    }
}

/// JSON for GET /messages/attachments?id=ID: `{"attachments":[...],"dir":"~/Downloads"}` or `{"error":"..."}`.
fn message_attachments_json(config_dir: &std::path::Path, id: &str) -> String {
    let err = |m: &str| format!("{{\"error\":{}}}", serde_json::to_string(m).unwrap());
    if id.is_empty() { return err("missing id"); }
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    match attachment_list(config_dir, &db, id) {
        Ok(list) => {
            let dir = crate::attachments::download_dir(&config_dir.join("settings.toml"));
            serde_json::json!({ "attachments": list, "dir": crate::attachments::display_dir(&dir) }).to_string()
        }
        Err(e) => err(&e),
    }
}

/// JSON for POST /messages/attachments/save?id=ID&att=ATT (one) or no `att` (every attachment
/// that is not inline): `{"saved":[{"id","name","path"}],"failed":[{"id","name","error"}],"dir":"~/Downloads"}`.
/// One failure never stops the others.
fn save_attachments_json(config_dir: &std::path::Path, id: &str, att: Option<&str>) -> String {
    let err = |m: &str| format!("{{\"error\":{}}}", serde_json::to_string(m).unwrap());
    if id.is_empty() { return err("missing id"); }
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let list = match attachment_list(config_dir, &db, id) { Ok(l) => l, Err(e) => return err(&e) };
    let wanted: Vec<_> = match att {
        Some(a) => list.iter().filter(|x| x.id == a).cloned().collect(),
        None => list.iter().filter(|x| !x.inline).cloned().collect(),
    };
    if wanted.is_empty() { return err("no such attachment"); }
    let provider = match graph_for_message(&db, id) { Ok(p) => p, Err(e) => return err(&e) };
    let dir = crate::attachments::download_dir(&config_dir.join("settings.toml"));
    let (mut saved, mut failed) = (Vec::new(), Vec::new());
    for a in &wanted {
        match crate::runtime::block_on(provider.download_attachment(id, a, &dir)) {
            Ok(path) => saved.push(serde_json::json!({ "id": a.id, "name": a.name, "path": path.display().to_string() })),
            Err(e) => {
                warn!("Attachment save failed ({}): {}", a.name, e);
                failed.push(serde_json::json!({ "id": a.id, "name": a.name, "error": e.to_string() }));
            }
        }
    }
    serde_json::json!({ "saved": saved, "failed": failed, "dir": crate::attachments::display_dir(&dir) }).to_string()
}

/// Local drafts that are not sent yet, as message rows for the Drafts folder (`GET /messages`, first page).
/// `id` is `draft:<local id>`, `draft_id` the local id, `draft_pushed` true once the provider holds the copy.
/// The provider's own copy of a draft we hold is hidden from the same list (dedupe in `GET /messages`).
fn local_draft_rows(conn: &rusqlite::Connection, account: Option<&str>, folder: &str) -> Vec<String> {
    let mut sql = String::from(
        "SELECT d.local_id, d.account_id, COALESCE(a.email, ''), d.remote_id, d.payload_json, d.updated_at, d.dirty \
         FROM drafts d LEFT JOIN accounts a ON a.id = d.account_id WHERE d.state != 'sent'",
    );
    let mut args: Vec<String> = Vec::new();
    if let Some(a) = account {
        sql.push_str(" AND (d.account_id = ?1 OR lower(a.email) = lower(?1))");
        args.push(a.to_string());
    }
    sql.push_str(" ORDER BY d.updated_at DESC");
    let Ok(mut stmt) = conn.prepare(&sql) else { return Vec::new() };
    #[allow(clippy::type_complexity)] // one-off row tuple for a single query_map
    let found: Vec<(String, String, String, Option<String>, String, i64, bool)> = stmt
        .query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get::<_, i64>(6)? != 0))
        })
        .map(|it| it.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();
    let names = |p: &serde_json::Value, k: &str| -> String {
        p[k].as_array().map(|v| v.iter().map(|a| {
            let e = a["email"].as_str().unwrap_or("");
            let n = a["name"].as_str().unwrap_or("");
            if n.is_empty() { e.to_string() } else { format!("{} <{}>", n, e) }
        }).collect::<Vec<_>>().join(", ")).unwrap_or_default()
    };
    found.into_iter().map(|(local, account_id, email, remote, payload, updated, dirty)| {
        let p: serde_json::Value = serde_json::from_str(&payload).unwrap_or(serde_json::Value::Null);
        let to = names(&p, "to");
        let from = p["from"].as_str().filter(|s| !s.is_empty()).map(|s| s.to_string()).unwrap_or_else(|| email.clone());
        let received = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(updated).unwrap_or_else(chrono::Utc::now).to_rfc3339();
        serde_json::json!({
            "id": format!("draft:{}", local),
            "draft_id": local,
            "draft_pushed": remote.is_some() && !dirty,
            "subject": p["subject"].as_str().unwrap_or(""),
            "from_email": from,
            "from_name": if to.is_empty() { "(no recipient)".to_string() } else { format!("To: {}", to) },
            "received_at": received,
            "is_read": true,
            "account_id": account_id,
            "account_email": email,
            "to_text": to,
            "cc_text": names(&p, "cc"),
            "bcc_text": names(&p, "bcc"),
            "sent_at": "",
            "cats": "[]",
            "importance": "",
            "has_attachments": false,
            "conversation_id": "",
            "folder_id": folder,
            "is_drafts": true,
        }).to_string()
    }).collect()
}

/// Most message ids taken from one provider search (per account); beyond it the result is partial.
const REMOTE_SEARCH_CAP: usize = 1000;

/// Provider search results are reused for a few minutes: the list re-asks on every page and
/// refresh, and each ask would otherwise be a round trip to Graph / Gmail.
#[allow(clippy::type_complexity)] // single process-wide cache; an alias would only be used here
static REMOTE_SEARCH_CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, Vec<String>, bool)>>> = std::sync::OnceLock::new();

const REMOTE_SEARCH_TTL: std::time::Duration = std::time::Duration::from_secs(300);

/// Ids (as cached locally) of the messages whose full text matches `term`, asked of the provider
/// of every account in scope: the folder (`all:<kind>` = that kind of folder in every account),
/// or the whole mailbox when no folder is given; `account` (id or address) narrows it further.
/// The flag is true when any account hit the cap.
fn remote_body_ids(conn: &rusqlite::Connection, folder_id: Option<&str>, account: Option<&str>, term: &str) -> Result<(Vec<String>, bool), String> {
    use crate::providers::EmailProvider;
    let db_err = |e: rusqlite::Error| format!("database: {e}");
    // (account id, provider folder id)
    let mut scopes: Vec<(String, Option<String>)> = Vec::new();
    match folder_id {
        Some(f) if f.starts_with("all:") => {
            let mut st = conn.prepare("SELECT account_id, id FROM folders WHERE well_known_name = ?1 AND account_id IS NOT NULL").map_err(db_err)?;
            let rows = st.query_map([&f[4..]], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(db_err)?;
            scopes.extend(rows.flatten().map(|(a, f)| (a, Some(f))));
        }
        Some(f) => {
            let acct: Option<String> = conn.query_row("SELECT account_id FROM folders WHERE id = ?1", [f], |r| r.get(0)).map_err(db_err)?;
            scopes.push((acct.ok_or("unknown folder")?, Some(f.to_string())));
        }
        None => {
            let mut st = conn.prepare("SELECT id FROM accounts WHERE enabled = 1").map_err(db_err)?;
            let rows = st.query_map([], |r| r.get::<_, String>(0)).map_err(db_err)?;
            scopes.extend(rows.flatten().map(|a| (a, None)));
        }
    }
    if let Some(acct) = account {
        let wanted: Option<String> = conn
            .query_row("SELECT id FROM accounts WHERE id = ?1 OR lower(email) = lower(?1)", [acct], |r| r.get(0))
            .ok();
        match wanted {
            Some(id) => scopes.retain(|(a, _)| *a == id),
            None => scopes.clear(),
        }
    }
    if scopes.is_empty() {
        return Err("no account to search".into());
    }

    let cache = REMOTE_SEARCH_CACHE.get_or_init(Default::default);
    let (mut all, mut more) = (Vec::<String>::new(), false);
    for (acct, folder) in scopes {
        let key = format!("{}|{}|{}", acct, folder.as_deref().unwrap_or(""), term.to_lowercase());
        let hit = cache.lock().ok().and_then(|c| c.get(&key).filter(|(t, _, _)| t.elapsed() < REMOTE_SEARCH_TTL).cloned());
        let (ids, m) = match hit {
            Some((_, ids, m)) => (ids, m),
            None => {
                let found = if crate::accounts::provider_of(&acct) == "gmail" {
                    crate::runtime::block_on(crate::providers::GmailProvider::new(&acct, None).search_message_ids(folder.as_deref(), term, REMOTE_SEARCH_CAP))
                } else {
                    crate::runtime::block_on(GraphEmailProvider::new(AuthManager::for_account(&acct)).search_message_ids(folder.as_deref(), term, REMOTE_SEARCH_CAP))
                };
                let (ids, m) = found.map_err(|e| {
                    warn!("fullbody search failed for {}: {}", acct, e);
                    format!("the search at {} failed ({})", acct, e.to_string().chars().take(160).collect::<String>())
                })?;
                info!("fullbody search {:?} at {} ({:?}): {} ids{}", term, acct, folder.as_deref().map(|f| f.chars().take(12).collect::<String>()), ids.len(), if m { " (capped)" } else { "" });
                if let Ok(mut c) = cache.lock() {
                    c.insert(key, (std::time::Instant::now(), ids.clone(), m));
                }
                (ids, m)
            }
        };
        all.extend(ids);
        more |= m;
    }
    Ok((all, more))
}

/// The whole HTTP response for `GET /messages` (list, paging, `?q=` search). Runs on its own
/// thread: a `fullbody:` search waits on the mail provider and must not stall the server loop.
fn messages_response(c: &Ctx, req_t: std::time::Instant) -> Response {
    let config_dir = c.config_dir;
    // The raw request line: `folder_id=` is split out of it as it always was.
    let first_line = c.req.first_line();
                    // Optional ?q= keyword pattern (see src/search.rs). A bad pattern is a 400 with
                    // {"error": "..."} so the UI can show why; `fullbody:` needs the provider, which
                    // is not wired up yet, so it is refused rather than silently ignored.
                    let search_q = match c.query("q").map(|q| crate::search::parse(&q)) {
                        None => Ok(None),
                        Some(Ok(q)) => Ok(Some(q)),
                        Some(Err(e)) => Err(e),
                    };
                    let search_q = match search_q {
                        Ok(q) => q,
                        Err(e) => {
                            return Response::json_error(400, &e);
                        }
                    };
                    // Parse optional ?folder_id= query parameter
                    let folder_id: Option<String> = first_line
                        .split_once("folder_id=")
                        .map(|(_, rest)| {
                            let raw = rest.split([' ', '&', '\r', '\n'])
                                .next()
                                .unwrap_or("");
                            // URL-decode: folder IDs contain = and / which QML encodeURIComponent encodes
                            crate::http::router::url_decode(raw)
                        })
                        .filter(|s| !s.is_empty());

                    let db_path = config_dir.join("messages.db");
                    let mut truncated = false;   // a fullbody: result hit the provider-side cap
                    let body = match rusqlite::Connection::open(&db_path) {
                        Ok(conn) => {
                            // Category colours per (account, lower-cased name), so each row can say
                            // which categories it carries and how to draw them.
                            let mut cat_colors: std::collections::HashMap<(String, String), String> = std::collections::HashMap::new();
                            if let Ok(mut st) = conn.prepare("SELECT account_id, name, color FROM category_defs") {
                                if let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))) {
                                    for (a, n, c) in rows.flatten() { cat_colors.insert((a, n.to_lowercase()), c); }
                                }
                            }
                            let map_row = |row: &rusqlite::Row| {
                                let account_id: String = row.get(6)?;
                                let cats: Vec<serde_json::Value> = crate::db::categories_from_json(row.get::<_, Option<String>>(12)?.as_deref())
                                    .into_iter()
                                    .map(|name| {
                                        let color = cat_colors.get(&(account_id.clone(), name.to_lowercase())).cloned()
                                            .unwrap_or_else(|| crate::models::fallback_category_color(&name).to_string());
                                        serde_json::json!([name, color])
                                    })
                                    .collect();
                                Ok(serde_json::json!({
                                    "id": row.get::<_, String>(0)?,
                                    "subject": row.get::<_, String>(1)?,
                                    "from_email": row.get::<_, String>(2)?,
                                    "from_name": row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                                    "received_at": row.get::<_, String>(4)?,
                                    "is_read": row.get::<_, bool>(5)?,
                                    "account_id": account_id,
                                    "account_email": row.get::<_, String>(7)?,
                                    "to_text": row.get::<_, Option<String>>(8)?.unwrap_or_default(),
                                    "cc_text": row.get::<_, Option<String>>(9)?.unwrap_or_default(),
                                    "bcc_text": row.get::<_, Option<String>>(10)?.unwrap_or_default(),
                                    "sent_at": row.get::<_, Option<String>>(11)?.unwrap_or_default(),
                                    // JSON text: [["Name","#rrggbb"],…] (a list model cannot hold nested lists cheaply)
                                    "cats": serde_json::Value::Array(cats).to_string(),
                                    "importance": row.get::<_, Option<String>>(13)?.unwrap_or_default(),
                                    "has_attachments": row.get::<_, Option<bool>>(14)?.unwrap_or(false),
                                    "conversation_id": row.get::<_, Option<String>>(15)?.unwrap_or_default(),
                                    "folder_id": row.get::<_, String>(16)?,
                                    // True for a row in the Drafts folder: Enter / `e` resume it (see POST /drafts/adopt).
                                    "is_drafts": row.get::<_, i64>(17)? != 0,
                                }).to_string())
                            };

                            // Paged: ?limit= (default 200, max 1000) & ?offset=. The cache holds
                            // tens of thousands of rows, so the UI pulls them a page at a time.
                            // Optional filters: ?folder_id= and ?account=<id or email address>.
                            let limit: i64 = c.query("limit")
                                .and_then(|v| v.parse().ok())
                                .map(|n: i64| n.clamp(1, 1000))
                                .unwrap_or(200);
                            let offset: i64 = c.query("offset")
                                .and_then(|v| v.parse().ok())
                                .map(|n: i64| n.max(0))
                                .unwrap_or(0);
                            let mut conds: Vec<String> = Vec::new();
                            let mut args: Vec<rusqlite::types::Value> = Vec::new();
                            // The Drafts folder also lists the local drafts that are not sent yet (first page only).
                            let drafts_view = match folder_id.as_deref() {
                                Some("all:drafts") => true,
                                Some(f) => conn.query_row("SELECT 1 FROM folders WHERE id = ?1 AND well_known_name = 'drafts'", [f], |_| Ok(())).is_ok(),
                                None => false,
                            };
                            if let Some(ref fid) = folder_id {
                                // "all:<kind>" = that kind of folder across every account (Inbox - all accounts).
                                if let Some(kind) = fid.strip_prefix("all:") {
                                    args.push(kind.to_string().into());
                                    conds.push(format!("m.folder_id IN (SELECT id FROM folders WHERE well_known_name = ?{})", args.len()));
                                } else {
                                    args.push(fid.clone().into());
                                    conds.push(format!("m.folder_id = ?{}", args.len()));
                                }
                            }
                            if let Some(acct) = c.query("account") {
                                args.push(acct.into());
                                let n = args.len();
                                conds.push(format!("(m.account_id = ?{n} OR lower(a.email) = lower(?{n}))"));
                            }
                            if let Some(ref sq) = search_q {
                                conds.extend(sq.to_sql(&mut args));
                                // `fullbody:` terms are answered by the mail provider(s); the ids that
                                // come back (messages in scope, as cached here) become IN / NOT IN.
                                let account = c.query("account");
                                for (neg, term) in &sq.fullbody {
                                    match remote_body_ids(&conn, folder_id.as_deref(), account.as_deref(), term) {
                                        Ok((ids, more)) => {
                                            if more && *neg {
                                                return Response::json_error(400, &format!("-fullbody:{term} matches more than {REMOTE_SEARCH_CAP} messages, too many to exclude"));
                                            }
                                            truncated |= more;
                                            if ids.is_empty() {
                                                if !*neg { conds.push("0".to_string()); }
                                                continue;
                                            }
                                            let marks: Vec<String> = ids.into_iter().map(|id| { args.push(id.into()); format!("?{}", args.len()) }).collect();
                                            conds.push(format!("m.id {}IN ({})", if *neg { "NOT " } else { "" }, marks.join(",")));
                                        }
                                        Err(e) => return Response::json_error(400, &format!("fullbody: {e}")),
                                    }
                                }
                            }
                            // Archived / deleted / moved messages wait out their undo window hidden.
                            conds.push("m.action_pending IS NULL".to_string());
                            // A draft that this app holds locally is shown from the local copy (GET /drafts), never twice.
                            conds.push("m.id NOT IN (SELECT remote_id FROM drafts WHERE remote_id IS NOT NULL AND state != 'sent')".to_string());
                            let where_sql = if conds.is_empty() { String::new() } else { format!(" WHERE {}", conds.join(" AND ")) };
                            args.push(limit.into());
                            args.push(offset.into());
                            let sql = format!(
                                "SELECT m.id, m.subject, m.from_email, m.from_name, m.received_at, m.is_read, \
                                        COALESCE(m.account_id, ''), COALESCE(a.email, ''), \
                                        m.to_text, m.cc_text, m.bcc_text, m.sent_at, m.categories, m.importance, \
                                        COALESCE((SELECT c.has_real FROM attachment_cache c WHERE c.message_id = m.id), m.has_attachments), \
                                        m.conversation_id, COALESCE(m.folder_id, ''), COALESCE((SELECT f.well_known_name = 'drafts' FROM folders f WHERE f.id = m.folder_id), 0) \
                                 FROM messages m LEFT JOIN accounts a ON a.id = m.account_id{} \
                                 ORDER BY m.received_at DESC LIMIT ?{} OFFSET ?{}",
                                where_sql, args.len() - 1, args.len()
                            );
                            let rows: Vec<String> = match conn.prepare(&sql) {
                                Ok(mut stmt) => stmt.query_map(rusqlite::params_from_iter(args.iter()), map_row)
                                    .map(|it| it.filter_map(|r| r.ok()).collect())
                                    .unwrap_or_default(),
                                Err(e) => { warn!("GET /messages: bad query: {}", e); Vec::new() }
                            };
                            let mut rows = rows;
                            if drafts_view && offset == 0 && search_q.is_none() {
                                let mut local = local_draft_rows(&conn, c.query("account").as_deref(), folder_id.as_deref().unwrap_or(""));
                                local.extend(rows);
                                rows = local;
                            }
                            format!("[{}]", rows.join(","))
                        }
                        Err(e) => {
                            warn!("GET /messages: DB open failed: {}", e);
                            "[]".to_string()
                        }
                    };
                    // Same bytes as before, including the header order (the flag goes before Content-Type).
                    let response = format!(
                        "HTTP/1.1 200 OK\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        if truncated { "X-Search-Truncated: 1\r\n" } else { "" }, body.len(), body
                    );
                    crate::perf::mark(&format!("http -> GET /messages answered ({} bytes, handler {:.1}ms)", body.len(), req_t.elapsed().as_secs_f64() * 1000.0));
                    Response::preformatted(response)
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
        let dir = std::env::temp_dir().join(format!("omalook-messages-{}-{}", test, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn json_of(r: &Response) -> serde_json::Value {
        serde_json::from_slice(r.body()).expect("JSON body")
    }

    #[test]
    fn the_table_lists_each_moved_path_with_its_lock() {
        let routes = routes();
        let mut seen: Vec<(&str, &str, bool)> = routes
            .iter()
            .map(|r| (r.method, r.pattern_str(), r.lock == Lock::None))
            .collect();
        seen.sort();
        assert_eq!(
            seen,
            vec![
                ("GET", "/categories", false),
                ("GET", "/messages", true),
                ("GET", "/messages/attachments", true),
                ("GET", "/messages/body", true),
                ("GET", "/messages/invite", true),
                ("GET", "/messages/list-info", true),
                ("GET", "/messages/meta_progress", false),
                ("GET", "/render/get", true),
                ("POST", "/categories/create", false),
                ("POST", "/messages/action", false),
                ("POST", "/messages/action/undo", false),
                ("POST", "/messages/attachments/save", true),
                ("POST", "/messages/category", false),
                ("POST", "/messages/invite", true),
                ("POST", "/messages/read", false),
                ("POST", "/render/adopt", false),
            ]
        );
    }

    #[test]
    fn a_message_list_on_an_empty_db_answers_an_empty_array() {
        let dir = temp_dir("list-empty");
        let r = run(&dir, get("/messages?folder=x"), |c| messages_response(c, std::time::Instant::now()));
        assert_eq!(r.status(), 200);
        assert_eq!(json_of(&r), serde_json::json!([]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_body_for_an_unknown_id_answers_unknown_message() {
        let dir = temp_dir("body-unknown");
        let r = run(&dir, get("/messages/body?id=nope"), message_body);
        assert_eq!(r.status(), 200);
        assert_eq!(json_of(&r), serde_json::json!({ "error": "unknown message" }));
        let r = run(&dir, get("/messages/body"), message_body);
        assert_eq!(json_of(&r), serde_json::json!({ "error": "missing id" }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_action_with_a_bad_body_answers_the_bad_request_error() {
        let dir = temp_dir("action-bad");
        // The action reads its arguments from the query; a JSON-looking body is ignored, as before.
        let r = run(&dir, post("/messages/action", "{not json"), message_action);
        assert_eq!(r.status(), 200);
        assert_eq!(
            json_of(&r),
            serde_json::json!({ "error": "bad request: need ids and op (archive|trash|delete|move+dest)" })
        );
        let r = run(&dir, post("/messages/action?ids=a&op=archive", ""), message_action);
        // a db with no such ids: nothing queued, delay reported
        assert_eq!(r.status(), 200);
        assert_eq!(json_of(&r)["queued"], serde_json::json!(0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn categories_on_an_empty_db_answer_empty_lists() {
        let dir = temp_dir("categories-empty");
        let r = run(&dir, get("/categories?account=acc-1"), categories);
        assert_eq!(r.status(), 200);
        let v = json_of(&r);
        assert_eq!(v["categories"], serde_json::json!([]));
        assert_eq!(v["master_list"], serde_json::json!(true));
        assert!(v["palette"].as_array().map(|a| !a.is_empty()).unwrap_or(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn category_create_refuses_bad_names_and_missing_fields() {
        let dir = temp_dir("categories-create");
        let r = run(&dir, post("/categories/create?account=acc-1&name=a;b&color=nope", ""), categories_create);
        assert_eq!(r.status(), 200);
        assert_eq!(
            json_of(&r),
            serde_json::json!({ "error": "need account, a name (max 64 chars, no ; or ,) and a palette colour" })
        );
        let r = run(&dir, post("/categories/create?account=acc-1", ""), categories_create);
        assert!(json_of(&r).get("error").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_category_and_meta_answer_on_an_empty_db() {
        let dir = temp_dir("read-cat-meta");
        let r = run(&dir, post("/messages/read?id=nope&read=true", ""), message_read);
        assert_eq!(r.status(), 200);
        assert_eq!(r.body(), b"ok".as_slice());
        let r = run(&dir, post("/messages/read", ""), message_read);
        assert_eq!(r.body(), b"error".as_slice());
        let r = run(&dir, post("/messages/category?ids=a&op=add", ""), message_category);
        assert_eq!(json_of(&r), serde_json::json!({ "error": "need ids, name and op=add|remove" }));
        let r = run(&dir, get("/messages/meta_progress"), meta_progress);
        assert_eq!(json_of(&r)["remaining"], serde_json::json!(0));
        assert!(json_of(&r)["active"].is_boolean());
        let r = run(&dir, post("/messages/action/undo?ids=a", ""), message_action_undo);
        assert_eq!(json_of(&r), serde_json::json!({ "undone": 0 }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_id_routes_refuse_a_missing_id() {
        let dir = temp_dir("missing-id");
        assert_eq!(json_of(&run(&dir, get("/messages/list-info"), list_info)), serde_json::json!({ "error": "missing id" }));
        assert_eq!(json_of(&run(&dir, get("/messages/invite"), invite_info)), serde_json::json!({ "error": "missing id" }));
        assert_eq!(json_of(&run(&dir, get("/messages/attachments"), message_attachments)), serde_json::json!({ "error": "missing id" }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_cache_refuses_a_miss_and_a_file_outside_the_run_dir() {
        let dir = temp_dir("render");
        let r = run(&dir, get("/render/get?id=nope&width=800&scale=1&images=0"), render_get);
        assert_eq!(r.status(), 200);
        assert_eq!(json_of(&r), serde_json::json!({ "hit": false }));
        let r = run(&dir, post("/render/adopt?id=nope&width=800&scale=1&images=0&file=/etc/passwd", ""), render_adopt);
        assert_eq!(r.status(), 200);
        assert!(json_of(&r).get("error").is_some() || json_of(&r)["ok"] == serde_json::json!(false));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
