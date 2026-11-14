//! Settings routes: mail, render cache, calendar, UI state, senders, signatures, keys, view state,
//! the MCP admin page, background running; the sign-in / account trigger routes; and POST
//! /mail/sync. Moved from the legacy chain in `main.rs` (R3.1, group G6). Answers are 200 with
//! JSON as before (the `ok` answers are `Response::json("ok")`, the same bytes).
//!
//! The MCP admin routes call `crate::mcp::admin::handle` unchanged: that is the agent-widening
//! confirm path (R1.6), so its body handling and call stay as they were.

use crate::http::router::{Ctx, Lock, Pattern, Response, Route};
use crate::models::Settings;
use log::{debug, error, info, warn};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

/// Cap of the JSON body of the MCP admin and background routes (as `mcp_json_body` had).
const MCP_BODY_LIMIT: usize = 64 * 1024;
/// Cap of the bare-value bodies (the reader already caps every body at 1 MiB).
const VALUE_BODY_LIMIT: usize = 1024 * 1024;

pub fn routes() -> Vec<Route> {
    use Pattern::Exact;
    let x = Lock::Exclusive;
    vec![
        Route::new("GET", Exact("/settings/mail"), x, get_mail),
        Route::new("POST", Exact("/settings/mail"), x, post_mail),
        Route::new("GET", Exact("/settings/render"), x, get_render),
        Route::new("POST", Exact("/settings/render"), x, post_render),
        Route::new("POST", Exact("/auth/confirm"), x, auth_confirm),
        Route::new("POST", Exact("/accounts/login"), x, accounts_login),
        Route::new("GET", Exact("/settings/senders"), x, senders_list),
        Route::new("POST", Exact("/settings/senders/add"), x, senders_add),
        Route::new("POST", Exact("/settings/senders/set"), x, senders_set),
        Route::new("POST", Exact("/settings/senders/remove"), x, senders_remove),
        Route::new("GET", Exact("/settings/keys"), x, settings_keys),
        Route::new("GET", Exact("/settings/view_state"), x, view_state_get),
        Route::new("POST", Exact("/settings/view_state"), x, view_state_post),
        Route::new("GET", Exact("/settings/signatures"), x, signatures_get),
        Route::new("POST", Exact("/settings/signature"), x, signature_post),
        Route::new("GET", Exact("/settings/ui"), x, settings_ui),
        Route::new("GET", Exact("/settings/mcp"), Lock::None, mcp_admin),
        Route::new("POST", Exact("/settings/mcp/enabled"), Lock::None, mcp_admin),
        Route::new("POST", Exact("/settings/mcp/domain"), Lock::None, mcp_admin),
        Route::new("POST", Exact("/settings/mcp/tool"), Lock::None, mcp_admin),
        Route::new("POST", Exact("/settings/mcp/account"), Lock::None, mcp_admin),
        Route::new("POST", Exact("/settings/mcp/preset"), Lock::None, mcp_admin),
        Route::new("POST", Exact("/settings/mcp/token/regenerate"), Lock::None, mcp_admin),
        Route::new("POST", Exact("/settings/mcp/copy"), Lock::None, mcp_admin),
        Route::new("POST", Exact("/settings/mcp/confirm"), Lock::None, mcp_admin),
        Route::new("POST", Exact("/settings/mcp/cancel"), Lock::None, mcp_admin),
        Route::new("GET", Exact("/settings/background"), x, background),
        Route::new("POST", Exact("/settings/background"), x, background),
        Route::new("POST", Exact("/settings/calendar_pane_offset"), x, calendar_pane_offset),
        Route::new("GET", Exact("/settings/calendar"), x, get_calendar),
        Route::new("POST", Exact("/settings/calendar"), x, post_calendar),
        Route::new("POST", Exact("/settings/scale_factor"), x, scale_factor),
        Route::new("POST", Exact("/settings/message_rendering"), x, message_rendering),
        Route::new("POST", Exact("/settings/sidebar_expanded"), x, sidebar_expanded),
        Route::new("POST", Exact("/settings/folder_pane"), x, folder_pane),
        Route::new("POST", Exact("/settings/list_width_offset"), x, list_width_offset),
        Route::new("POST", Exact("/auth/login"), x, auth_login),
        Route::new("POST", Exact("/auth/cancel"), x, auth_cancel),
        Route::new("POST", Exact("/auth/logout"), x, auth_logout),
        Route::new("POST", Exact("/accounts/remove"), x, accounts_remove),
        Route::new("POST", Exact("/mail/sync"), x, mail_sync),
    ]
}

/// `"ok"` answers (the old `200 OK` with `Content-Length: 2`).
fn ok_text() -> Response {
    Response::json("ok")
}

/// Path of settings.toml under the config dir.
fn settings_path(c: &Ctx) -> std::path::PathBuf {
    c.config_dir.join("settings.toml")
}

/// The request body as text for the routes that read a bare value (`true`, an integer, a JSON
/// object). Invalid UTF-8 reads as empty, which each caller already answers as "not valid".
fn body_str(c: &Ctx) -> String {
    c.body_text(VALUE_BODY_LIMIT).unwrap_or_default()
}

/// The JSON body of a Settings -> MCP / background request. GET has no body (Null). POST: it must
/// carry Content-Length (as `read_post_body` required) and be at most 64 KiB; an empty or
/// unparsable body is Null. An error is the message to show.
fn settings_json(c: &Ctx) -> Result<Value, String> {
    if c.req.method != "POST" {
        return Ok(Value::Null);
    }
    if c.req.header("content-length").is_none() {
        return Err("missing Content-Length".into());
    }
    c.json_body(MCP_BODY_LIMIT)
}

/// Apply `change` to the parsed settings.toml and write it back. Answers whether it was done.
fn update_settings(path: &Path, change: impl FnOnce(&mut Settings)) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str::<Settings>(&s).ok())
        .and_then(|mut settings| {
            change(&mut settings);
            toml::to_string_pretty(&settings).ok().map(|content| std::fs::write(path, content))
        })
        .is_some()
}

// ── /settings/mail, /settings/render ───────────────────────────────────────────────────────────

fn get_mail(c: &Ctx) -> Response {
    mail_settings(c, false)
}

fn post_mail(c: &Ctx) -> Response {
    mail_settings(c, true)
}

/// `GET|POST /settings/mail?send_delay=N&large_audience=N` — seconds a sent message waits before it
/// goes; recipient count above which Send asks to confirm (0 = never).
fn mail_settings(c: &Ctx, post: bool) -> Response {
    use crate::models::MailSettings;
    let path = settings_path(c);
    let mut secs = crate::settings::read_send_delay(&path);
    let mut large = crate::settings::read_large_audience(&path);
    if post {
        if let Some(wanted) = c.query("send_delay").and_then(|v| v.parse::<i32>().ok()) {
            match crate::settings::write_send_delay(&path, wanted) {
                Ok(stored) => secs = stored,
                Err(e) => warn!("POST /settings/mail: could not save: {}", e),
            }
        }
        if let Some(wanted) = c.query("large_audience").and_then(|v| v.parse::<i32>().ok()) {
            match crate::settings::write_large_audience(&path, wanted) {
                Ok(stored) => large = stored,
                Err(e) => warn!("POST /settings/mail: could not save: {}", e),
            }
        }
    }
    Response::json(
        json!({
            "send_delay_secs": secs,
            "max_send_delay_secs": MailSettings::MAX_SEND_DELAY_SECS,
            "large_audience_confirm": large,
            "max_large_audience_confirm": MailSettings::MAX_LARGE_AUDIENCE,
        })
        .to_string(),
    )
}

fn get_render(c: &Ctx) -> Response {
    render_settings(c, false)
}

fn post_render(c: &Ctx) -> Response {
    render_settings(c, true)
}

/// `GET|POST /settings/render?cache_mb=N` — size cap of the rendered-HTML cache; lowering it trims
/// the cache at once.
fn render_settings(c: &Ctx, post: bool) -> Response {
    let path = settings_path(c);
    if post {
        if let Some(wanted) = c.query("cache_mb").and_then(|v| v.parse::<i64>().ok()) {
            match crate::settings::write_render_cache_mb(&path, wanted) {
                Ok(_) => {
                    crate::render_cache::open_configured(c.config_dir).trim();
                }
                Err(e) => warn!("POST /settings/render: could not save: {}", e),
            }
        }
    }
    let cache = crate::render_cache::open_configured(c.config_dir);
    Response::json(
        json!({
            "cache_mb": crate::settings::read_render_cache_mb(&path),
            "min_mb": crate::settings::MIN_RENDER_CACHE_MB,
            "max_mb": crate::settings::MAX_RENDER_CACHE_MB,
            "used_bytes": cache.total_bytes(),
        })
        .to_string(),
    )
}

// ── /auth/confirm, /accounts/login (answers; the sign-in work is in the scheduler) ─────────────

/// POST /auth/confirm?pending=ID&decision=replace|cancel — answer the "this mailbox is already
/// signed in — replace its sign-in?" prompt.
fn auth_confirm(c: &Ctx) -> Response {
    let pending_id = c.query("pending").unwrap_or_default();
    let replace = c.query("decision").as_deref() == Some("replace");
    let ok = crate::scheduler::GLOBAL
        .get()
        .map(|sch| crate::account_ops::resolve_reauth(c.config_dir, &pending_id, replace, sch))
        .map(|r| r.map_err(|e| error!("❌ re-authentication answer failed: {}", e)).is_ok())
        .unwrap_or(false);
    Response::json(if ok { "ok" } else { "error" })
}

/// POST /accounts/login?account=ID — log in an existing account. Reuses the kept token when there
/// is one; otherwise tells the UI to run a device flow.
fn accounts_login(c: &Ctx) -> Response {
    use crate::account_ops::ResumeOutcome;
    let id = c.query("account").unwrap_or_default();
    let body = match crate::scheduler::GLOBAL.get() {
        Some(scheduler) => match crate::account_ops::log_in_existing(c.config_dir, &id, scheduler) {
            Ok(ResumeOutcome::Resumed) => "{\"result\":\"resumed\"}".to_string(),
            Ok(ResumeOutcome::LoginRequired(provider)) => {
                json!({ "result": "login_required", "provider": provider }).to_string()
            }
            Err(e) => {
                error!("❌ log in {} failed: {}", id, e);
                "{\"result\":\"error\"}".to_string()
            }
        },
        None => "{\"result\":\"error\"}".to_string(),
    };
    Response::json(body)
}

// ── /settings/senders ──────────────────────────────────────────────────────────────────────────

fn senders_list(c: &Ctx) -> Response {
    Response::json(sender_prefs(c, "list"))
}

fn senders_add(c: &Ctx) -> Response {
    Response::json(sender_prefs(c, "add"))
}

fn senders_set(c: &Ctx) -> Response {
    Response::json(sender_prefs(c, "set"))
}

fn senders_remove(c: &Ctx) -> Response {
    Response::json(sender_prefs(c, "remove"))
}

/// The sender-preference routes' JSON. `op` is list | add | set | remove.
fn sender_prefs(c: &Ctx, op: &str) -> String {
    use crate::db::SenderAdd;
    let err = |m: &str| format!("{{\"ok\":false,\"error\":{}}}", serde_json::to_string(m).unwrap_or_default());
    let db = match crate::Database::open(&c.db_path()) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let email = c.query("email").unwrap_or_default();
    let result = match op {
        "add" => db.add_sender(&email).map(|r| {
            let status = match r {
                SenderAdd::Added => "added",
                SenderAdd::AlreadyListed => "exists",
                SenderAdd::Invalid => "invalid",
            };
            format!("{{\"ok\":{},\"status\":\"{}\"}}", r != SenderAdd::Invalid, status)
        }),
        "set" => {
            let field = c.query("field").unwrap_or_default();
            let value = c.query("value").as_deref() == Some("true");
            db.set_sender_pref(&email, &field, value).map(|ok| format!("{{\"ok\":{}}}", ok))
        }
        "remove" => db.remove_sender(&email).map(|ok| format!("{{\"ok\":{}}}", ok)),
        _ => db.list_senders().map(|rows| {
            let items: Vec<String> = rows
                .iter()
                .map(|s| {
                    format!(
                        "{{\"email\":{},\"always_html\":{},\"always_images\":{}}}",
                        serde_json::to_string(&s.email).unwrap_or_default(),
                        s.always_html,
                        s.always_images
                    )
                })
                .collect();
            format!("[{}]", items.join(","))
        }),
    };
    result.unwrap_or_else(|e| err(&e.to_string()))
}

// ── /settings/keys, /settings/view_state, /settings/signatures, /settings/ui ───────────────────

/// GET /settings/keys — the `[keys]` table of settings.toml:
/// {"serial":N,"preset":"","bindings":{"mail.archive":["e"]},"error":""}. `serial` is the file's
/// mtime; the UI polls this and re-applies when it moves.
fn settings_keys(c: &Ctx) -> Response {
    let path = settings_path(c);
    let serial: u128 = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let body = match std::fs::read_to_string(&path) {
        Ok(text) => match toml::from_str::<Settings>(&text) {
            Ok(st) => {
                let bindings: serde_json::Map<String, Value> =
                    st.keys.bindings.iter().map(|(k, v)| (k.clone(), json!(v.keys()))).collect();
                json!({ "serial": serial.to_string(), "preset": st.keys.preset, "bindings": bindings, "error": "" })
            }
            Err(e) => json!({ "serial": serial.to_string(), "preset": "", "bindings": {}, "error": format!("settings.toml: {}", e) }),
        },
        Err(_) => json!({ "serial": serial.to_string(), "preset": "", "bindings": {}, "error": "" }),
    };
    Response::json(body.to_string())
}

fn view_state_get(c: &Ctx) -> Response {
    Response::json(crate::view_state::load(&c.config_dir.join("view_state.json")))
}

/// POST /settings/view_state — the body is a flat JSON object; anything else is a 400.
fn view_state_post(c: &Ctx) -> Response {
    let path = c.config_dir.join("view_state.json");
    let body = body_str(c);
    if crate::view_state::save(&path, body.trim()) {
        Response::json("{}")
    } else {
        Response::json_status(400, "{\"error\":\"not a flat JSON object\"}")
    }
}

fn signatures_get(c: &Ctx) -> Response {
    signatures(c, false)
}

fn signature_post(c: &Ctx) -> Response {
    signatures(c, true)
}

/// GET /settings/signatures — every account's signature: {account_id: text} (OL-050).
/// POST /settings/signature?account=ID with body {"text": "..."} — set or clear one (empty).
fn signatures(c: &Ctx, post: bool) -> Response {
    let path = settings_path(c);
    let result: Result<std::collections::BTreeMap<String, String>, String> = if post {
        let account = c.query("account").unwrap_or_default();
        let body = body_str(c);
        let text = serde_json::from_str::<Value>(body.trim())
            .ok()
            .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()));
        match (account.is_empty(), text) {
            (true, _) => Err("missing account".to_string()),
            (false, None) => Err("body must be {\"text\": \"...\"}".to_string()),
            (false, Some(t)) => crate::settings::write_signature(&path, &account, &t).map_err(|e| e.to_string()),
        }
    } else {
        Ok(crate::settings::read_signatures(&path))
    };
    match result {
        Ok(map) => Response::json(serde_json::to_string(&map).unwrap_or_else(|_| "{}".into())),
        Err(e) => Response::json_status(400, json!({ "error": e }).to_string()),
    }
}

/// GET /settings/ui — the UI settings as JSON.
fn settings_ui(c: &Ctx) -> Response {
    let body = match std::fs::read_to_string(settings_path(c)).ok().and_then(|s| toml::from_str::<Settings>(&s).ok()) {
        Some(settings) => format!(
            "{{\"sidebar_expanded\":{},\"window_width\":{},\"window_height\":{},\"message_rendering\":\"{}\",\"folder_pane\":{},\"list_width_offset\":{},\"calendar_pane_offset\":{},\"scale_factor\":{}}}",
            settings.ui.sidebar_expanded,
            settings.ui.window_width,
            settings.ui.window_height,
            crate::settings::normalize_message_rendering(&settings.ui.message_rendering).unwrap_or("system_sender"),
            settings.ui.folder_pane,
            settings.ui.list_width_offset,
            settings.ui.calendar_pane_offset,
            crate::settings::normalize_scale_factor(settings.font.scale_factor),
        ),
        None => "{\"sidebar_expanded\":true,\"window_width\":1280,\"window_height\":800,\"message_rendering\":\"system_sender\",\"folder_pane\":false,\"list_width_offset\":0,\"calendar_pane_offset\":0,\"scale_factor\":1.0}".to_string(),
    };
    Response::json(body)
}

// ── /settings/mcp (the MCP admin page; see src/mcp/admin.rs) ───────────────────────────────────

/// Every `/settings/mcp` route. GET is the state; the POSTs are the changes `admin::handle` takes.
/// Its answer is passed on as it is; `unknown route` only if it returns None.
fn mcp_admin(c: &Ctx) -> Response {
    let body = match settings_json(c) {
        Ok(v) => v,
        Err(e) => return Response::json(json!({ "error": e }).to_string()),
    };
    let answer = crate::mcp::admin::handle(c.config_dir, &c.req.method, c.path(), &body)
        .unwrap_or_else(|| "{\"error\":\"unknown route\"}".to_string());
    Response::json(answer)
}

// ── /settings/background ───────────────────────────────────────────────────────────────────────

/// GET|POST /settings/background {"enabled": bool} — keep the backend running after the window
/// closes. Takes effect when the window closes; no restart.
fn background(c: &Ctx) -> Response {
    let body_json = match settings_json(c) {
        Ok(v) => v,
        Err(e) => json!({ "error": e }),
    };
    Response::json(crate::mcp::admin::handle_background(c.config_dir, &c.req.method, &body_json))
}

// ── /settings/calendar_pane_offset, /settings/calendar ─────────────────────────────────────────

/// POST /settings/calendar_pane_offset (body: integer, characters) — calendar sidebar width. It
/// writes only the `[ui]` offset; the calendar window settings are a different route.
fn calendar_pane_offset(c: &Ctx) -> Response {
    let cols: i32 = body_str(c).trim().parse::<i32>().unwrap_or(0).clamp(-200, 400);
    let ok = update_settings(&settings_path(c), |s| s.ui.calendar_pane_offset = cols);
    debug!("POST /settings/calendar_pane_offset {} → {}", cols, ok);
    ok_text()
}

fn get_calendar(c: &Ctx) -> Response {
    calendar_settings(c, false)
}

fn post_calendar(c: &Ctx) -> Response {
    calendar_settings(c, true)
}

/// GET|POST /settings/calendar — recurring-meeting window (years back / ahead) and the defaults.
fn calendar_settings(c: &Ctx, post: bool) -> Response {
    use crate::models::CalendarSettings;
    let path = settings_path(c);
    let current = if post {
        let cur = crate::settings::read_calendar_settings(&path);
        let wanted = CalendarSettings {
            recurrence_years_back: c.query("back").and_then(|v| v.parse().ok()).unwrap_or(cur.recurrence_years_back),
            recurrence_years_ahead: c.query("ahead").and_then(|v| v.parse().ok()).unwrap_or(cur.recurrence_years_ahead),
            default_minutes: c.query("minutes").and_then(|v| v.parse().ok()).unwrap_or(cur.default_minutes),
            default_reminder_minutes: c.query("reminder").and_then(|v| v.parse().ok()).unwrap_or(cur.default_reminder_minutes),
            default_series_months: c.query("series_months").and_then(|v| v.parse().ok()).unwrap_or(cur.default_series_months),
            default_account: c.query("account").unwrap_or(cur.default_account.clone()),
        };
        match crate::settings::write_calendar_settings(&path, &wanted) {
            Ok(stored) => stored,
            Err(e) => {
                warn!("POST /settings/calendar: could not save: {}", e);
                cur
            }
        }
    } else {
        crate::settings::read_calendar_settings(&path)
    };
    Response::json(
        json!({
            "recurrence_years_back": current.recurrence_years_back,
            "recurrence_years_ahead": current.recurrence_years_ahead,
            "default_minutes": current.default_minutes,
            "default_reminder_minutes": current.default_reminder_minutes,
            "default_series_months": current.default_series_months,
            "default_account": current.default_account,
            "max_years_back": CalendarSettings::MAX_YEARS_BACK,
            "max_years_ahead": CalendarSettings::MAX_YEARS_AHEAD,
        })
        .to_string(),
    )
}

// ── POST /settings/{scale_factor, message_rendering, sidebar_expanded, folder_pane, list_width_offset}

/// POST /settings/scale_factor?value=1.1 — UI scale, clamped, stored in [font].
fn scale_factor(c: &Ctx) -> Response {
    let wanted = c.query("value").and_then(|v| v.parse::<f64>().ok());
    let body = match wanted.map(|w| crate::settings::write_scale_factor(&settings_path(c), w)) {
        Some(Ok(stored)) => format!("{{\"ok\":true,\"scale_factor\":{}}}", stored),
        other => {
            if let Some(Err(e)) = other {
                warn!("POST /settings/scale_factor {:?}: {}", wanted, e);
            }
            "{\"ok\":false}".to_string()
        }
    };
    Response::json(body)
}

/// POST /settings/message_rendering?value=html|system|system_sender.
fn message_rendering(c: &Ctx) -> Response {
    let wanted = c.query("value").unwrap_or_default();
    let body = match crate::settings::write_message_rendering(&settings_path(c), &wanted) {
        Ok(stored) => format!("{{\"ok\":true,\"message_rendering\":\"{}\"}}", stored),
        Err(e) => {
            warn!("POST /settings/message_rendering {}: {}", wanted, e);
            "{\"ok\":false}".to_string()
        }
    };
    Response::json(body)
}

/// POST /settings/sidebar_expanded — body "true" or "false".
fn sidebar_expanded(c: &Ctx) -> Response {
    let expanded = body_str(c).trim() == "true";
    let ok = update_settings(&settings_path(c), |s| s.ui.sidebar_expanded = expanded);
    debug!("POST /settings/sidebar_expanded {} → {}", expanded, ok);
    ok_text()
}

/// POST /settings/folder_pane (body "true"|"false") — show/hide the folder pane.
fn folder_pane(c: &Ctx) -> Response {
    let shown = body_str(c).trim() == "true";
    let ok = update_settings(&settings_path(c), |s| s.ui.folder_pane = shown);
    debug!("POST /settings/folder_pane {} → {}", shown, ok);
    ok_text()
}

/// POST /settings/list_width_offset (body: integer, characters) — message list width.
fn list_width_offset(c: &Ctx) -> Response {
    let cols: i32 = body_str(c).trim().parse::<i32>().unwrap_or(0).clamp(-200, 400);
    let ok = update_settings(&settings_path(c), |s| s.ui.list_width_offset = cols);
    debug!("POST /settings/list_width_offset {} → {}", cols, ok);
    ok_text()
}

// ── POST trigger routes: answer first, then act (Response::then, still under the lock) ────────

fn auth_login(c: &Ctx) -> Response {
    // New account: its id is generated as <provider>-<suffix>.
    let provider = c.query("provider").unwrap_or_else(|| "exchange".to_string());
    let email = c.query("email");
    let dir = c.config_dir.clone();
    ok_text().then(move || {
        info!("🔔 HTTP trigger: add {} account requested", provider);
        if let Some(scheduler) = crate::scheduler::GLOBAL.get() {
            let scheduler = Arc::clone(scheduler);
            std::thread::spawn(move || {
                if let Err(e) = crate::account_ops::begin_add_account_with_hint(&dir, &provider, email.as_deref(), scheduler) {
                    error!("❌ Add account failed: {}", e);
                }
            });
        }
    })
}

fn auth_cancel(c: &Ctx) -> Response {
    let dir = c.config_dir.clone();
    ok_text().then(move || {
        info!("🔔 HTTP trigger: cancel sign-in");
        crate::google_auth::cancel_login(&dir);
    })
}

fn auth_logout(c: &Ctx) -> Response {
    account_action(c, false)
}

fn accounts_remove(c: &Ctx) -> Response {
    account_action(c, true)
}

/// POST /auth/logout?account=ID and POST /accounts/remove?account=ID. The answer goes first; the
/// account work runs after it.
fn account_action(c: &Ctx, remove: bool) -> Response {
    let id = c.query("account").unwrap_or_else(|| crate::token_store::DEFAULT_ACCOUNT.to_string());
    let action = if remove { "remove" } else { "logout" };
    let dir = c.config_dir.clone();
    ok_text().then(move || {
        info!("🔔 HTTP trigger: {} account {}", action, id);
        if let Some(scheduler) = crate::scheduler::GLOBAL.get() {
            let result = if remove {
                crate::account_ops::remove_account(&dir, &id, scheduler)
            } else {
                crate::account_ops::sign_out_account(&dir, &id, scheduler)
            };
            match result {
                Ok(()) => info!("✅ {} succeeded for {}", action, id),
                Err(e) => error!("❌ {} failed for {}: {}", action, id, e),
            }
        }
    })
}

// ── POST /mail/sync ────────────────────────────────────────────────────────────────────────────

/// POST /mail/sync — the Refresh button / F5: poll the provider now, not at the next interval.
/// Returns at once; new mail reaches the UI through the mail sync serial.
fn mail_sync(_: &Ctx) -> Response {
    crate::sync_state::request_mail_sync();
    ok_text()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::request::{read_request, test_stream, Limits, Request};
    use crate::http::router::{table, Found};
    use crate::mcp::admin;
    use std::path::PathBuf;

    /// A request parsed the way the server parses one off the socket.
    fn request(method: &str, target: &str, body: &str) -> Request {
        let text = if method == "POST" {
            format!("POST {} HTTP/1.1\r\nHost: 127.0.0.1:1\r\nContent-Length: {}\r\n\r\n{}", target, body.len(), body)
        } else {
            format!("{} {} HTTP/1.1\r\nHost: 127.0.0.1:1\r\n\r\n", method, target)
        };
        read_request(&mut test_stream(text.into_bytes()), &Limits::default()).expect("request parses")
    }

    /// Dispatch through the real route table, the way the server does; panics if not routed.
    fn dispatch(dir: &PathBuf, method: &str, target: &str, body: &str) -> Response {
        let req = request(method, target, body);
        match table().find(method, req.path()) {
            Found::Route(r) => (r.handler)(&Ctx::new(dir, &req)),
            _ => panic!("{} {} is not routed", method, target),
        }
    }

    fn temp_dir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("omalook-settings-{}-{}", test, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn json_of(r: &Response) -> Value {
        serde_json::from_slice(r.body()).expect("JSON body")
    }

    fn body_of(r: &Response) -> String {
        String::from_utf8(r.body().to_vec()).expect("UTF-8 body")
    }

    /// The `pending.id` is random per request; everything else must match exactly.
    fn without_pending_id(mut v: Value) -> Value {
        if let Some(p) = v.get_mut("pending").and_then(|p| p.as_object_mut()) {
            p.remove("id");
        }
        v
    }

    #[test]
    fn every_moved_route_is_exclusive_and_unknown_paths_fall_through() {
        let all = [
            ("GET", "/settings/mail"), ("POST", "/settings/mail"), ("GET", "/settings/render"),
            ("POST", "/settings/render"), ("POST", "/auth/confirm"), ("POST", "/accounts/login"),
            ("GET", "/settings/senders"), ("POST", "/settings/senders/add"), ("POST", "/settings/senders/set"),
            ("POST", "/settings/senders/remove"), ("GET", "/settings/keys"), ("GET", "/settings/view_state"),
            ("POST", "/settings/view_state"), ("GET", "/settings/signatures"), ("POST", "/settings/signature"),
            ("GET", "/settings/ui"), ("GET", "/settings/mcp"), ("POST", "/settings/mcp/enabled"),
            ("POST", "/settings/mcp/confirm"), ("POST", "/settings/mcp/cancel"), ("GET", "/settings/background"),
            ("POST", "/settings/background"), ("POST", "/settings/calendar_pane_offset"), ("GET", "/settings/calendar"),
            ("POST", "/settings/calendar"), ("POST", "/settings/scale_factor"), ("POST", "/settings/message_rendering"),
            ("POST", "/settings/sidebar_expanded"), ("POST", "/settings/folder_pane"),
            ("POST", "/settings/list_width_offset"), ("POST", "/auth/login"), ("POST", "/auth/cancel"),
            ("POST", "/auth/logout"), ("POST", "/accounts/remove"), ("POST", "/mail/sync"),
        ];
        for (method, path) in all {
            match table().find(method, path) {
                Found::Route(r) if path.starts_with("/settings/mcp") => {
                    assert!(matches!(r.lock, Lock::None), "{} {} must run outside the lock", method, path)
                }
                Found::Route(r) => assert!(matches!(r.lock, Lock::Exclusive), "{} {} must be exclusive", method, path),
                _ => panic!("{} {} is not routed", method, path),
            }
        }
        // Wrong method on a known path is 405-shaped, not a route; unknown MCP sub-paths are not routed.
        assert!(matches!(table().find("GET", "/settings/mcp/confirm"), Found::WrongMethod));
        assert!(matches!(table().find("POST", "/settings/mcp/no-such"), Found::NotFound));
        assert!(matches!(table().find("GET", "/auth/login"), Found::WrongMethod));
    }

    #[test]
    fn calendar_pane_offset_does_not_change_the_calendar_settings() {
        // Hazard 12: the pane-offset POST used to be caught by the calendar prefix.
        let dir = temp_dir("pane-offset");
        dispatch(&dir, "POST", "/settings/calendar?back=3&ahead=2&minutes=45&reminder=10&series_months=6&account=acct-a", "");
        let before = json_of(&dispatch(&dir, "GET", "/settings/calendar", ""));
        assert_eq!(before["recurrence_years_back"], 3);

        let r = dispatch(&dir, "POST", "/settings/calendar_pane_offset", "37");
        assert_eq!(body_of(&r), "ok");

        assert_eq!(json_of(&dispatch(&dir, "GET", "/settings/calendar", "")), before, "calendar settings changed");
        assert_eq!(json_of(&dispatch(&dir, "GET", "/settings/ui", ""))["calendar_pane_offset"], 37);
    }

    #[test]
    fn mail_settings_default_in_an_empty_config() {
        let dir = temp_dir("mail-defaults");
        let v = json_of(&dispatch(&dir, "GET", "/settings/mail", ""));
        assert_eq!(v["send_delay_secs"], crate::models::MailSettings::DEFAULT_SEND_DELAY_SECS);
        assert_eq!(v["large_audience_confirm"], crate::models::MailSettings::DEFAULT_LARGE_AUDIENCE);
        assert_eq!(v["max_send_delay_secs"], crate::models::MailSettings::MAX_SEND_DELAY_SECS);
        assert_eq!(v["max_large_audience_confirm"], crate::models::MailSettings::MAX_LARGE_AUDIENCE);
    }

    #[test]
    fn mail_and_render_post_then_get_read_back() {
        let dir = temp_dir("mail-render");
        dispatch(&dir, "POST", "/settings/mail?send_delay=7&large_audience=55", "");
        let v = json_of(&dispatch(&dir, "GET", "/settings/mail", ""));
        assert_eq!(v["send_delay_secs"], 7);
        assert_eq!(v["large_audience_confirm"], 55);

        dispatch(&dir, "POST", "/settings/render?cache_mb=128", "");
        let v = json_of(&dispatch(&dir, "GET", "/settings/render", ""));
        assert_eq!(v["cache_mb"], 128);
        assert_eq!(v["min_mb"], crate::settings::MIN_RENDER_CACHE_MB);
    }

    #[test]
    fn ui_posts_read_back_through_settings_ui() {
        let dir = temp_dir("ui");
        dispatch(&dir, "POST", "/settings/scale_factor?value=1.25", "");
        dispatch(&dir, "POST", "/settings/message_rendering?value=html", "");
        dispatch(&dir, "POST", "/settings/sidebar_expanded", "false");
        dispatch(&dir, "POST", "/settings/folder_pane", "true");
        dispatch(&dir, "POST", "/settings/list_width_offset", "12");
        dispatch(&dir, "POST", "/settings/calendar_pane_offset", "-5");

        let v = json_of(&dispatch(&dir, "GET", "/settings/ui", ""));
        assert_eq!(v["scale_factor"], 1.25);
        assert_eq!(v["message_rendering"], "html");
        assert_eq!(v["sidebar_expanded"], false);
        assert_eq!(v["folder_pane"], true);
        assert_eq!(v["list_width_offset"], 12);
        assert_eq!(v["calendar_pane_offset"], -5);
    }

    #[test]
    fn signature_post_then_get_reads_back() {
        let dir = temp_dir("signature");
        let r = dispatch(&dir, "POST", "/settings/signature?account=acct-a", r#"{"text":"Thanks, Adam"}"#);
        assert_eq!(r.status(), 200);
        let v = json_of(&dispatch(&dir, "GET", "/settings/signatures", ""));
        assert_eq!(v["acct-a"], "Thanks, Adam");

        let missing = dispatch(&dir, "POST", "/settings/signature", r#"{"text":"x"}"#);
        assert_eq!(missing.status(), 400);
    }

    #[test]
    fn view_state_post_then_get_reads_back_and_rejects_non_objects() {
        let dir = temp_dir("view-state");
        assert_eq!(body_of(&dispatch(&dir, "POST", "/settings/view_state", r#"{"sort":"date"}"#)), "{}");
        assert_eq!(json_of(&dispatch(&dir, "GET", "/settings/view_state", ""))["sort"], "date");
        assert_eq!(dispatch(&dir, "POST", "/settings/view_state", "[1,2]").status(), 400);
    }

    #[test]
    fn senders_add_set_remove_round_trip() {
        let dir = temp_dir("senders");
        assert_eq!(json_of(&dispatch(&dir, "POST", "/settings/senders/add?email=a@example.com", ""))["status"], "added");
        assert_eq!(json_of(&dispatch(&dir, "POST", "/settings/senders/set?email=a@example.com&field=html&value=true", "")), json!({"ok": true}));
        let list = json_of(&dispatch(&dir, "GET", "/settings/senders", ""));
        let row = list.as_array().unwrap().iter().find(|r| r["email"] == "a@example.com").expect("listed");
        assert_eq!(row["always_html"], true);
        assert_eq!(json_of(&dispatch(&dir, "POST", "/settings/senders/remove?email=a@example.com", "")), json!({"ok": true}));
        let list = json_of(&dispatch(&dir, "GET", "/settings/senders", ""));
        assert!(list.as_array().unwrap().iter().all(|r| r["email"] != "a@example.com"));
    }

    #[test]
    fn mcp_admin_routes_match_admin_handle_exactly() {
        // The widening confirm path: the table must answer what admin::handle answers, unchanged.
        let a = temp_dir("mcp-table");
        let b = temp_dir("mcp-direct");
        // Background on, MCP off: turning the server on is a widening and parks (as in admin's own test).
        for d in [&a, &b] {
            crate::settings::write_run_in_background(&d.join("settings.toml"), true).unwrap();
        }
        let body = json!({"value": true});
        let via_table = json_of(&dispatch(&a, "POST", "/settings/mcp/enabled", &body.to_string()));
        let direct = serde_json::from_str::<Value>(&admin::handle(&b, "POST", "/settings/mcp/enabled", &body).expect("admin answers")).unwrap();
        assert_eq!(without_pending_id(via_table.clone()), without_pending_id(direct));

        // The change is a widening: it is parked for UI confirmation and not applied.
        assert!(via_table["pending"]["id"].is_string(), "a widening change must stay parked: {}", via_table);
        assert_eq!(via_table["enabled"], false);
        let state = json_of(&dispatch(&a, "GET", "/settings/mcp", ""));
        assert_eq!(state["enabled"], false, "the parked change must not apply");

        // Confirm reads the id from the JSON body (the query string is never read).
        let unknown_body = json!({"id": "nope"});
        let via_table = json_of(&dispatch(&a, "POST", "/settings/mcp/confirm", &unknown_body.to_string()));
        let direct = serde_json::from_str::<Value>(&admin::handle(&b, "POST", "/settings/mcp/confirm", &unknown_body).expect("admin answers")).unwrap();
        assert_eq!(without_pending_id(via_table), without_pending_id(direct));
    }

    #[test]
    fn mcp_unlisted_post_path_is_not_routed() {
        // Only the listed MCP paths are routes; anything else falls through to the 404 in main.rs.
        assert!(matches!(table().find("POST", "/settings/mcp/no-such-thing"), Found::NotFound));
    }

    #[test]
    fn background_get_and_post_disable() {
        let dir = temp_dir("background");
        let body = json!({"enabled": false});
        let direct = admin::handle_background(&dir, "POST", &body);
        let via_table = body_of(&dispatch(&dir, "POST", "/settings/background", &body.to_string()));
        assert_eq!(
            without_pending_id(serde_json::from_str(&via_table).unwrap()),
            without_pending_id(serde_json::from_str(&direct).unwrap())
        );
        let get = json_of(&dispatch(&dir, "GET", "/settings/background", ""));
        assert!(get.get("run_in_background").is_some());
    }

    #[test]
    fn auth_confirm_and_accounts_login_unknown_ids_answer_error_without_scheduler() {
        let dir = temp_dir("auth-unknown");
        assert_eq!(body_of(&dispatch(&dir, "POST", "/auth/confirm?pending=nope&decision=cancel", "")), "error");
        assert_eq!(body_of(&dispatch(&dir, "POST", "/accounts/login?account=nope", "")), "{\"result\":\"error\"}");
    }

    #[test]
    fn auth_cancel_answers_ok_and_runs_after_the_answer() {
        let dir = temp_dir("auth-cancel");
        let (bytes, after) = dispatch(&dir, "POST", "/auth/cancel", "").into_parts();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{}", text);
        assert!(text.contains("Content-Length: 2\r\n"));
        assert!(text.ends_with("\r\n\r\nok"));
        // Not run here: cancel_login bumps the process-wide LOGIN_GEN and would abort the login a
        // google_auth test is waiting on in a parallel thread.
        assert!(after.is_some(), "cancel runs after the answer");
    }

    #[test]
    fn accounts_remove_answers_ok_then_runs_its_action() {
        let dir = temp_dir("accounts-remove");
        let (bytes, after) = dispatch(&dir, "POST", "/accounts/remove?account=acct-x", "").into_parts();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{}", text);
        assert!(text.contains("Content-Type: application/json\r\n"));
        assert!(text.ends_with("\r\n\r\nok"));
        // No scheduler is set in tests, so the action is a no-op; it must still run without panicking.
        after.expect("remove runs after the answer")();
    }

    #[test]
    fn mail_sync_answers_ok() {
        let dir = temp_dir("mail-sync");
        assert_eq!(body_of(&dispatch(&dir, "POST", "/mail/sync", "")), "ok");
    }
}
