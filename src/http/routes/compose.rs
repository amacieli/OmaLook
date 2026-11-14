//! Compose routes: queue, cancel and status of outgoing messages, the originals of a forward,
//! attachment limits, drafts (local copies the draft worker pushes to Drafts) and the send-as
//! identities. Moved from the legacy chain in `main.rs` (R3.1, group G4). Every answer is 200 with
//! JSON, as before; failures are reported inside the JSON. The picked-file checks
//! (`crate::picked`) run unchanged on the send and save paths.

use crate::http::router::{Ctx, Lock, Pattern, Response, Route};
use crate::providers::graph::GraphEmailProvider;
use crate::{AuthManager, Database};
use log::{info, warn};
use std::path::PathBuf;
use std::sync::Arc;
use super::messages::attachment_list;

pub fn routes() -> Vec<Route> {
    use Pattern::Exact;
    vec![
        Route::new("POST", Exact("/compose/send"), Lock::Exclusive, compose_send),
        Route::new("POST", Exact("/compose/cancel"), Lock::Exclusive, compose_cancel),
        Route::new("GET", Exact("/compose/status"), Lock::Exclusive, compose_status),
        Route::new("GET", Exact("/compose/originals"), Lock::None, compose_originals),
        // Ran on its own thread outside the dispatch lock before R3.1 (provider call, OL-051): still unlocked.
        Route::new("GET", Exact("/compose/identities"), Lock::None, identities),
        Route::new("POST", Exact("/compose/identity_default"), Lock::None, identity_default),
        Route::new("GET", Exact("/attach/limits"), Lock::Exclusive, attach_limits),
        Route::new("POST", Exact("/drafts/save"), Lock::Exclusive, drafts_save),
        Route::new("POST", Exact("/drafts/discard"), Lock::Exclusive, drafts_discard),
        Route::new("POST", Exact("/drafts/adopt"), Lock::Exclusive, drafts_adopt),
        Route::new("GET", Exact("/drafts/get"), Lock::None, drafts_get),
        Route::new("GET", Exact("/drafts"), Lock::None, drafts_list),
    ]
}

/// A 200 JSON answer (every route here answers 200, errors included).
fn answer(body: String) -> Response {
    Response::json(body)
}

fn err_ok_false(m: &str) -> String {
    serde_json::json!({ "ok": false, "error": m }).to_string()
}

/// The request body as UTF-8, capped at `limit`. Without a Content-Length header it is an error
/// (the body reader alone would call it empty), as the old `read_post_body` did.
fn post_body(c: &Ctx, limit: usize) -> Result<String, String> {
    if c.req.header("content-length").is_none() {
        return Err("missing Content-Length".into());
    }
    c.body_text(limit)
}

/// messages.db for these routes; a failure is reported as `database: ...`.
fn open_db(c: &Ctx) -> Result<Database, String> {
    Database::open(&c.db_path()).map_err(|e| format!("database: {}", e))
}

/// POST /compose/send — queue a composed message. The body is the JSON the compose pane builds
/// (`crate::compose::OutgoingMessage`). It is stored in the outbox to go out after the
/// `[mail] send_delay_secs` setting (0 = now) and the account's outbox worker is woken, so the
/// send never waits for a poll. Answers `{ok, id, delay_secs}` or `{ok:false, error}`.
fn compose_send(c: &Ctx) -> Response {
    answer(compose_send_answer(c))
}

fn compose_send_answer(c: &Ctx) -> String {
    use crate::compose::{OutgoingMessage, MAX_BODY_BYTES};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let err = |m: &str| err_ok_false(m);
    let config_dir = c.config_dir;

    let raw = match post_body(c, MAX_BODY_BYTES + 64 * 1024) {
        Ok(b) => b,
        Err(e) => return err(&e),
    };
    let mut body: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return err(&format!("unreadable message: {}", e)),
    };
    let db = match open_db(c) {
        Ok(db) => db,
        Err(e) => return err(&e),
    };
    // Attachments travel as handles (R3.2): each resolves to the path the backend holds before
    // anything opens a file. The draft's own stored attachments supply the original of a spooled copy.
    let stored = body["draft_id"].as_str().filter(|d| !d.is_empty()).map(|d| crate::draft_sync::stored_attachments(&db, d)).unwrap_or_default();
    if let Err(e) = crate::app::files::resolve_request(&mut body, &stored) {
        return err(&e);
    }
    let mut msg: OutgoingMessage = match serde_json::from_value(body) {
        Ok(m) => m,
        Err(e) => return err(&format!("unreadable message: {}", e)),
    };
    // Distribution lists become their members here, at queue time: the chip in the compose pane
    // stays one chip, and what is stored in the outbox is the final address list.
    let lists = db.lists_all().unwrap_or_default();
    let expanded = match msg.expand_lists(&|id| {
        lists.iter().find(|l| l.id == id).map(|l| (l.name.clone(), l.members.iter().map(|m| crate::compose::Address { email: m.email.clone(), name: m.name.clone(), list: 0 }).collect()))
    }) {
        Ok(r) => r,
        Err(e) => return err(&e),
    };
    // Pictures the body does not show go, and so does any `<img>` that would not load for the
    // recipient (a local path); see `settle_inline`.
    msg.settle_inline();
    if let Err(e) = msg.validate() {
        return err(&e);
    }
    // Sent as an alias (OL-051): it must be one of the account's identities; the account's own address needs no override.
    if !msg.from.trim().is_empty() {
        let acct_email = db.account_email(&msg.account_id).ok().flatten().unwrap_or_default();
        match crate::identities::choose_from(&identities_for(&db, &msg.account_id, false), &acct_email, &msg.from) {
            Ok(f) => msg.from = f,
            Err(e) => return err(&e),
        }
    }
    // A signed-out account has no worker, so the message would sit unsent without anyone knowing.
    if !crate::scheduler::GLOBAL.get().map(|s| s.is_running(&msg.account_id)).unwrap_or(false) {
        return err("that account is not signed in, so it cannot send");
    }
    let delay = crate::settings::read_send_delay(&config_dir.join("settings.toml"));
    let now = Database::now_ms();
    let id = format!("ob-{:x}-{:03x}", now, COUNTER.fetch_add(1, Ordering::Relaxed) & 0xfff);
    // Copy the attached files aside so the message goes out with exactly these bytes, whatever
    // happens to the originals during the undo window (docs/PLAN-compose-attachments.md 3.2).
    let spool_root = crate::attach_spool::default_root();
    let mut msg = msg;
    msg.attachments = match crate::attach_spool::snapshot(&spool_root, &id, &msg.attachments) {
        Ok(a) => a,
        Err(e) => return err(&e),
    };
    let payload = match serde_json::to_string(&msg) {
        Ok(p) => p,
        Err(e) => {
            crate::attach_spool::remove(&spool_root, &id);
            return err(&e.to_string());
        }
    };
    if let Err(e) = db.outbox_enqueue(&id, &msg.account_id, &payload, now + delay as i64 * 1000) {
        crate::attach_spool::remove(&spool_root, &id);
        return err(&format!("could not queue the message: {}", e));
    }
    crate::sync_state::request_outbox_run();
    info!("Compose: queued {} ({} → {} recipient(s)), sends in {}s", id, msg.account_id, msg.to.len() + msg.cc.len() + msg.bcc.len(), delay);
    serde_json::json!({
        "ok": true, "id": id, "delay_secs": delay,
        "recipients": msg.to.len() + msg.cc.len() + msg.bcc.len(),
        "lists": expanded.iter().map(|(n, c)| serde_json::json!({ "name": n, "count": c })).collect::<Vec<_>>(),
    }).to_string()
}

/// GET /compose/originals?id=MESSAGE — the files attached to the message being forwarded, for the
/// forward's chips: `{"originals":[{"id","name","size"}]}` (ids: Graph attachment id, Gmail MIME part
/// id) or `{"error"}`. Pictures embedded in the HTML are left out: they were never the user's to pick.
fn compose_originals(c: &Ctx) -> Response {
    answer(compose_originals_json(c.config_dir, &c.query("id").unwrap_or_default()))
}

/// POST /compose/cancel?id=ID — take a queued message back while it is still inside its delay.
fn compose_cancel(c: &Ctx) -> Response {
    answer(outbox_answer(c, true))
}

/// GET /compose/status?id=ID.
fn compose_status(c: &Ctx) -> Response {
    answer(outbox_answer(c, false))
}

fn outbox_answer(c: &Ctx, cancelling: bool) -> String {
    let err = |m: &str| err_ok_false(m);
    let id = match c.query("id") {
        Some(i) => i,
        None => return err("missing id"),
    };
    let db = match open_db(c) {
        Ok(db) => db,
        Err(e) => return err(&e),
    };
    let cancelled = if cancelling {
        match db.outbox_cancel(&id) {
            Ok(c) => c,
            Err(e) => return err(&e.to_string()),
        }
    } else {
        false
    };
    if cancelled {
        // The UI restores its own chips (original paths); the copies are no longer needed.
        crate::attach_spool::remove(&crate::attach_spool::default_root(), &id);
    }
    match db.outbox_status(&id) {
        Ok(Some((state, error, attempts))) => serde_json::json!({
            "ok": true, "cancelled": cancelled, "state": state, "error": error, "attempts": attempts
        }).to_string(),
        Ok(None) => serde_json::json!({ "ok": false, "cancelled": false, "state": "unknown", "error": "no such message" }).to_string(),
        Err(e) => err(&e.to_string()),
    }
}

/// GET /attach/limits?account=ID — what the account's provider accepts on one message.
fn attach_limits(c: &Ctx) -> Response {
    let provider = c
        .query("account")
        .map(|a| crate::accounts::provider_of(&a).to_string())
        .unwrap_or_default();
    answer(serde_json::to_string(&crate::compose::attachment_limits(&provider)).unwrap_or_else(|_| "{}".into()))
}

/// POST /drafts/save?state=editing|saved — the compose pane's draft (`OutgoingMessage` JSON). An empty
/// `draft_id` = a new draft. Writes the local copy (`draft_sync::save_local`); the draft worker pushes it
/// to Drafts. Answers `{ok, id, state, attachments, missing}` or `{ok:false, error}`.
fn drafts_save(c: &Ctx) -> Response {
    answer(drafts_save_answer(c))
}

fn drafts_save_answer(c: &Ctx) -> String {
    use crate::compose::{OutgoingMessage, MAX_BODY_BYTES};
    let err = |m: &str| err_ok_false(m);
    let raw = match post_body(c, MAX_BODY_BYTES + 64 * 1024) {
        Ok(b) => b,
        Err(e) => return err(&e),
    };
    let mut body: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return err(&format!("unreadable draft: {}", e)),
    };
    let account_id = body["account_id"].as_str().unwrap_or("").to_string();
    if account_id.is_empty() {
        return err("the draft has no account");
    }
    let state = c.query("state").unwrap_or_else(|| "saved".to_string());
    let db = match Database::open_for_account(&c.db_path(), &account_id) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    // Attachments travel as handles (R3.2). The draft's stored attachments are the trusted ones: they
    // supply the original behind a spooled copy, and a handle for a gone copy still resolves.
    let draft_id = body["draft_id"].as_str().unwrap_or("").to_string();
    let stored = crate::draft_sync::stored_attachments(&db, &draft_id);
    if let Err(e) = crate::app::files::resolve_request(&mut body, &stored) {
        return err(&e);
    }
    let mut msg: OutgoingMessage = match serde_json::from_value(body) {
        Ok(m) => m,
        Err(e) => return err(&format!("unreadable draft: {}", e)),
    };
    match crate::draft_sync::save_local(&db, &crate::draft_sync::default_root(), &mut msg, &state) {
        Ok(id) => serde_json::json!({
            "ok": true, "id": id, "state": state, "attachments": crate::app::files::attachments_for_ui(&msg.attachments),
            "missing": crate::attach_spool::missing_copies(&msg.attachments),
        }).to_string(),
        Err(e) => err(&e),
    }
}

/// GET /drafts[?account=ID] — the local drafts (every account without `account`).
fn drafts_list(c: &Ctx) -> Response {
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let db = match open_db(c) {
        Ok(db) => db,
        Err(e) => return answer(err(&e)),
    };
    let rows = match c.query("account") {
        Some(a) => db.draft_list(&a),
        None => db.draft_list_all(),
    };
    answer(match rows {
        Ok(rows) => serde_json::json!({
            "ok": true,
            "drafts": rows.iter().map(crate::draft_sync::draft_json).collect::<Vec<_>>(),
        }).to_string(),
        Err(e) => err(&e.to_string()),
    })
}

/// GET /drafts/get?id=ID — one local draft.
fn drafts_get(c: &Ctx) -> Response {
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let db = match open_db(c) {
        Ok(db) => db,
        Err(e) => return answer(err(&e)),
    };
    let id = match c.query("id") {
        Some(i) => i,
        None => return answer(err("missing id")),
    };
    answer(match db.draft_get(&id) {
        Ok(Some(r)) => serde_json::json!({ "ok": true, "draft": crate::draft_sync::draft_json(&r) }).to_string(),
        Ok(None) => err("no such draft"),
        Err(e) => err(&e.to_string()),
    })
}

/// POST /drafts/discard?id=ID — deletes the provider's copy first and the local row only after that
/// succeeded.
fn drafts_discard(c: &Ctx) -> Response {
    answer(drafts_discard_answer(c))
}

fn drafts_discard_answer(c: &Ctx) -> String {
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let db_str = c.db_path();
    let db = match open_db(c) {
        Ok(db) => db,
        Err(e) => return err(&e),
    };
    let id = match c.query("id") {
        Some(i) => i,
        None => return err("missing id"),
    };
    let row = match db.draft_get(&id) {
        Ok(Some(r)) => r,
        Ok(None) => return serde_json::json!({ "ok": true, "discarded": false }).to_string(),
        Err(e) => return err(&e.to_string()),
    };
    let account_db = match Database::open_for_account(&db_str, &row.account_id) {
        Ok(d) => d,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let provider: Arc<dyn crate::providers::EmailProvider> = if crate::accounts::provider_of(&row.account_id) == "gmail" {
        Arc::new(crate::providers::gmail::GmailProvider::new(&row.account_id, Some(c.config_dir.join("messages.db"))))
    } else {
        Arc::new(GraphEmailProvider::new(AuthManager::for_account(&row.account_id)))
    };
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => return err(&format!("runtime: {}", e)),
    };
    let spool = crate::draft_sync::default_root();
    match rt.block_on(crate::draft_sync::discard(&account_db, provider.as_ref(), &spool, &id)) {
        Ok(()) => serde_json::json!({ "ok": true, "discarded": true, "id": id }).to_string(),
        Err(e) => err(&e.to_string()),
    }
}

/// POST /drafts/adopt?id=<message id>: a draft that lives only on the provider (a row in the Drafts folder with no
/// local copy) gets a local copy built from the cached message, so it resumes like any other draft. The local copy
/// keeps the provider's id and is marked clean, so nothing is pushed until the user edits it. Answers {ok, id}.
/// The cache has no In-Reply-To, so an adopted reply opens as a new message.
fn drafts_adopt(c: &Ctx) -> Response {
    answer(drafts_adopt_answer(c.config_dir, c.query("id")))
}

fn drafts_adopt_answer(config_dir: &PathBuf, mid: Option<String>) -> String {
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let Some(mid) = mid else { return err("missing id") };
    let db_path = config_dir.join("messages.db");
    let db_str = db_path.to_str().unwrap_or("messages.db").to_string();
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let row: rusqlite::Result<(String, String, String, String, String, String, String, String)> = conn.query_row(
        "SELECT COALESCE(m.account_id, ''), COALESCE(m.subject, ''), COALESCE(m.from_email, ''), COALESCE(m.to_text, ''), \
                COALESCE(m.cc_text, ''), COALESCE(m.bcc_text, ''), COALESCE(m.body_preview, ''), COALESCE(a.email, '') \
         FROM messages m LEFT JOIN accounts a ON a.id = m.account_id WHERE m.id = ?1",
        [&mid],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
    );
    let Ok((account, subject, from_email, to, cc, bcc, preview, account_email)) = row else {
        return err("no such message");
    };
    if account.is_empty() {
        return err("the message has no account");
    }
    let db = match Database::open_for_account(&db_str, &account) {
        Ok(d) => d,
        Err(e) => return err(&format!("database: {}", e)),
    };
    if let Ok(Some(local)) = db.draft_local_for_remote(&account, &mid) {
        return serde_json::json!({ "ok": true, "id": local, "existing": true }).to_string();
    }
    let (mode, format, content) = match db.cached_body(&mid) {
        Ok(Some((t, c))) if t.eq_ignore_ascii_case("html") => ("html", "html", c),
        Ok(Some((_, c))) => ("system", "text", c),
        _ => ("system", "text", preview),
    };
    // An alias the draft was written from stays the From; the account's own address is the default.
    let from = if !from_email.is_empty() && !from_email.eq_ignore_ascii_case(&account_email) { from_email } else { String::new() };
    let addrs = |t: &str| -> Vec<serde_json::Value> {
        crate::addresses::parse_addresses(t).into_iter()
            .map(|(name, email)| serde_json::json!({ "email": email, "name": name })).collect()
    };
    let mut msg: crate::compose::OutgoingMessage = match serde_json::from_value(serde_json::json!({
        "kind": "new", "account_id": account, "in_reply_to": "",
        "to": addrs(&to), "cc": addrs(&cc), "bcc": addrs(&bcc),
        "subject": subject, "from": from, "mode": mode,
        "body": { "format": format, "content": content }, "attachments": [],
    })) {
        Ok(m) => m,
        Err(e) => return err(&format!("draft: {}", e)),
    };
    let local = match crate::draft_sync::save_local(&db, &crate::draft_sync::default_root(), &mut msg, "saved") {
        Ok(l) => l,
        Err(e) => return err(&e),
    };
    let sig = crate::draft_sync::push_signature(&msg);
    let read_at = match db.draft_get(&local) { Ok(Some(r)) => r.updated_at, _ => 0 };
    if let Err(e) = db.draft_mark_pushed(&local, read_at, &mid, None, &sig) {
        return err(&e.to_string());
    }
    serde_json::json!({ "ok": true, "id": local, "existing": false }).to_string()
}

/// GET /compose/identities?account=ID[&refresh=1] -> `{identities:[{email,name,primary}], default}`.
fn identities(c: &Ctx) -> Response {
    answer(identities_answer(c, false))
}

/// POST /compose/identity_default?account=ID&email=ADDR remembers the account's default From (empty email clears it).
fn identity_default(c: &Ctx) -> Response {
    answer(identities_answer(c, true))
}

fn identities_answer(c: &Ctx, set_default: bool) -> String {
    let err = |m: &str| serde_json::json!({ "error": m }).to_string();
    let account = c.query("account").unwrap_or_default();
    if account.is_empty() {
        return err("missing account");
    }
    let db = match open_db(c) {
        Ok(db) => db,
        Err(e) => return err(&e),
    };
    let email = db.account_email(&account).ok().flatten().unwrap_or_default();
    let list = identities_for(&db, &account, c.query("refresh").as_deref() == Some("1"));
    if set_default {
        let want = c.query("email").unwrap_or_default();
        let chosen = match crate::identities::choose_from(&list, &email, &want) {
            Ok(c) => c,
            Err(e) => return err(&e),
        };
        // The account address itself is stored as "no preference", like the provider default.
        return match db.identity_pref_set(&account, if chosen.is_empty() && !want.trim().is_empty() { &email } else { &chosen }) {
            Ok(()) => serde_json::json!({ "ok": true }).to_string(),
            Err(e) => err(&e.to_string()),
        };
    }
    let pref = db.identity_pref_get(&account).ok().flatten();
    serde_json::json!({
        "identities": list.iter().map(|i| serde_json::json!({ "email": i.email, "name": i.name, "primary": i.primary })).collect::<Vec<_>>(),
        "default": crate::identities::default_identity(&list, pref.as_deref(), &email),
    }).to_string()
}

// ---- helpers moved from main.rs (verbatim apart from `crate::` paths) ----

/// The send-as identities of an account (OL-051): cached for six hours, read from the provider when older (or `force`),
/// the stale copy when the provider cannot be reached, and just the account address when nothing was ever read.
fn identities_for(db: &Database, account: &str, force: bool) -> Vec<crate::identities::Identity> {
    use crate::identities::{ensure_primary, Identity};
    const TTL_SECS: i64 = 6 * 3600;
    let email = db.account_email(account).ok().flatten().unwrap_or_default();
    let cached = db.identities_get(account).ok().flatten();
    if let Some((list, at)) = &cached {
        if !force && chrono::Utc::now().timestamp() - at < TTL_SECS {
            return list.clone();
        }
    }
    let fetched: Option<Result<Vec<Identity>, String>> = tokio::runtime::Builder::new_current_thread().enable_all().build().ok().map(|rt| {
        match crate::accounts::provider_of(account) {
            "gmail" => rt.block_on(crate::providers::gmail::GmailProvider::new(account, None).fetch_identities(&email)).map_err(|e| e.to_string()),
            "exchange" => rt.block_on(GraphEmailProvider::new(AuthManager::for_account(account)).fetch_identities(&email)).map_err(|e| e.to_string()),
            _ => Ok(Vec::new()),
        }
    });
    match fetched {
        Some(Ok(list)) => {
            let list = ensure_primary(list, &email);
            if let Err(e) = db.identities_replace(account, &list) {
                warn!("Could not cache identities of {}: {}", account, e);
            }
            list
        }
        other => {
            if let Some(Err(e)) = other {
                warn!("Identities of {} not read: {}", account, e);
            }
            match cached {
                Some((list, _)) => list,
                None => ensure_primary(Vec::new(), &email),
            }
        }
    }
}

/// GET /compose/originals?id=MESSAGE — the files attached to the message being forwarded, for the
/// forward's chips: `{"originals":[{"id","name","size"}]}` (ids: Graph attachment id, Gmail MIME part
/// id) or `{"error"}`. Pictures embedded in the HTML are left out: they were never the user's to pick.
fn compose_originals_json(config_dir: &PathBuf, id: &str) -> String {
    let err = |m: &str| serde_json::json!({ "error": m }).to_string();
    if id.is_empty() { return err("missing id"); }
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let account = match db.message_account(id) { Ok(Some(a)) => a, _ => return err("unknown message") };
    let list: Result<Vec<(String, String, i64)>, String> = if crate::accounts::provider_of(&account) == "gmail" {
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() { Ok(rt) => rt, Err(e) => return err(&format!("runtime: {}", e)) };
        let p = crate::providers::gmail::GmailProvider::new(&account, None);
        rt.block_on(p.list_attachments(id)).map(|v| v.into_iter().map(|(i, n, s)| (i, n, s as i64)).collect()).map_err(|e| { warn!("Forward: attachment list failed for {}: {}", id, e); "could not read the original's attachments".to_string() })
    } else {
        attachment_list(config_dir, &db, id).map(|v| v.into_iter().filter(|a| !a.inline && a.saveable()).map(|a| (a.id, a.name, a.size)).collect())
    };
    match list {
        Ok(v) => serde_json::json!({ "originals": v.into_iter().map(|(i, n, s)| serde_json::json!({ "id": i, "name": n, "size": s })).collect::<Vec<_>>() }).to_string(),
        Err(e) => err(&e),
    }
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
        let dir = std::env::temp_dir().join(format!("omalook-compose-{}-{}", test, std::process::id()));
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
        let mut seen: Vec<(&str, &str, Lock)> = routes.iter().map(|r| (r.method, r.pattern_str(), r.lock)).collect();
        seen.sort_by_key(|t| (t.0, t.1));
        assert_eq!(
            seen,
            vec![
                ("GET", "/attach/limits", Lock::Exclusive),
                ("GET", "/compose/identities", Lock::None),
                ("GET", "/compose/originals", Lock::None),
                ("GET", "/compose/status", Lock::Exclusive),
                ("GET", "/drafts", Lock::None),
                ("GET", "/drafts/get", Lock::None),
                ("POST", "/compose/cancel", Lock::Exclusive),
                ("POST", "/compose/identity_default", Lock::None),
                ("POST", "/compose/send", Lock::Exclusive),
                ("POST", "/drafts/adopt", Lock::Exclusive),
                ("POST", "/drafts/discard", Lock::Exclusive),
                ("POST", "/drafts/save", Lock::Exclusive),
            ]
        );
    }

    /// An attachment that names a path and no handle is refused before anything opens it, with the
    /// same text as before. The file exists here; only the handle registry can say it was picked.
    #[test]
    fn compose_send_refuses_an_attachment_with_a_path_and_no_handle() {
        let dir = temp_dir("send-unregistered");
        let file = dir.join("never-registered.txt");
        std::fs::write(&file, b"not picked").unwrap();
        let body = serde_json::json!({
            "kind": "new", "account_id": "acct-a", "to": [{ "email": "a@example.com" }],
            "subject": "s", "body": { "format": "text", "content": "hi" },
            "attachments": [{ "path": file.to_string_lossy(), "name": "never-registered.txt" }],
        })
        .to_string();
        let r = run(&dir, post("/compose/send", &body), compose_send);
        assert_eq!(r.status(), 200);
        assert_eq!(
            json_of(&r),
            serde_json::json!({ "ok": false, "error": "never-registered.txt was not picked in this session; attach it again" })
        );
        // A forged handle is refused the same way.
        let body = serde_json::json!({
            "kind": "new", "account_id": "acct-a", "to": [{ "email": "a@example.com" }],
            "subject": "s", "body": { "format": "text", "content": "hi" },
            "attachments": [{ "handle": "fh-00000000000000000000000000000000", "name": "never-registered.txt" }],
        })
        .to_string();
        let r = run(&dir, post("/compose/send", &body), compose_send);
        assert_eq!(json_of(&r)["ok"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn drafts_save_answers_bad_json_as_before() {
        let dir = temp_dir("save-bad-json");
        let r = run(&dir, post("/drafts/save?state=saved", "{not json"), drafts_save);
        assert_eq!(r.status(), 200);
        let v = json_of(&r);
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap_or("").starts_with("unreadable draft: "), "{}", v);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn drafts_list_on_an_empty_db_is_an_empty_list() {
        let dir = temp_dir("drafts-empty");
        let r = run(&dir, get("/drafts"), drafts_list);
        assert_eq!(r.status(), 200);
        assert_eq!(json_of(&r), serde_json::json!({ "ok": true, "drafts": [] }));
        let r = run(&dir, get("/drafts/get?id=nope"), drafts_get);
        assert_eq!(json_of(&r), serde_json::json!({ "ok": false, "error": "no such draft" }));
        let r = run(&dir, get("/drafts/get"), drafts_get);
        assert_eq!(json_of(&r), serde_json::json!({ "ok": false, "error": "missing id" }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn attach_limits_answers_the_provider_limits_json() {
        let dir = temp_dir("attach-limits");
        let r = run(&dir, get("/attach/limits?account=acct-a"), attach_limits);
        assert_eq!(r.status(), 200);
        let expected = serde_json::to_string(&crate::compose::attachment_limits(crate::accounts::provider_of("acct-a"))).unwrap();
        assert_eq!(String::from_utf8(r.body().to_vec()).unwrap(), expected);
        let r = run(&dir, get("/attach/limits"), attach_limits);
        let expected = serde_json::to_string(&crate::compose::attachment_limits("")).unwrap();
        assert_eq!(String::from_utf8(r.body().to_vec()).unwrap(), expected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_parameters_answer_their_messages() {
        let dir = temp_dir("missing-params");
        assert_eq!(json_of(&run(&dir, get("/compose/identities"), identities)), serde_json::json!({ "error": "missing account" }));
        assert_eq!(json_of(&run(&dir, post("/compose/identity_default", ""), identity_default)), serde_json::json!({ "error": "missing account" }));
        assert_eq!(json_of(&run(&dir, get("/compose/status"), compose_status)), serde_json::json!({ "ok": false, "error": "missing id" }));
        assert_eq!(json_of(&run(&dir, post("/compose/cancel", ""), compose_cancel)), serde_json::json!({ "ok": false, "error": "missing id" }));
        assert_eq!(json_of(&run(&dir, get("/compose/originals"), compose_originals)), serde_json::json!({ "error": "missing id" }));
        assert_eq!(json_of(&run(&dir, post("/drafts/discard", ""), drafts_discard)), serde_json::json!({ "ok": false, "error": "missing id" }));
        assert_eq!(json_of(&run(&dir, post("/drafts/adopt", ""), drafts_adopt)), serde_json::json!({ "ok": false, "error": "missing id" }));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
