//! Calendar routes: the month list, event create / edit / cancel / undo / status, the outbox, tags,
//! the free-time finder and the online-meeting settings. Moved from the legacy chain in `main.rs`
//! (R3.1, group G3). Answers are 200 with JSON as before; failures are `{"ok":false,"error":...}`.

use crate::http::router::{Ctx, Lock, Pattern, Response, Route};
use crate::{AuthManager, Database};
use log::warn;
use std::sync::Arc;

pub fn routes() -> Vec<Route> {
    use Pattern::Exact;
    vec![
        Route::new("GET", Exact("/calendar/events"), Lock::Exclusive, events),
        Route::new("POST", Exact("/calendar/event"), Lock::Exclusive, event_create),
        Route::new("PATCH", Exact("/calendar/event"), Lock::Exclusive, event_edit),
        Route::new("POST", Exact("/calendar/event/cancel"), Lock::Exclusive, event_cancel),
        Route::new("POST", Exact("/calendar/event/undo"), Lock::Exclusive, event_undo),
        Route::new("GET", Exact("/calendar/event/status"), Lock::Exclusive, event_status),
        Route::new("GET", Exact("/calendar/outbox"), Lock::Exclusive, outbox_list),
        Route::new("POST", Exact("/calendar/tags"), Lock::Exclusive, tags),
        Route::new("GET", Exact("/calendar/find"), Lock::None, find),
        Route::new("GET", Exact("/calendar/meeting-options"), Lock::Exclusive, meeting_options),
        Route::new("GET", Exact("/settings/meetings"), Lock::Exclusive, meetings_get),
        Route::new("POST", Exact("/settings/meetings"), Lock::Exclusive, meetings_post),
    ]
}

fn event_create(c: &Ctx) -> Response {
    Response::json(calendar_event_response(c))
}
fn event_edit(c: &Ctx) -> Response {
    Response::json(calendar_change_response(c, true))
}
fn event_cancel(c: &Ctx) -> Response {
    Response::json(calendar_change_response(c, false))
}
fn event_undo(c: &Ctx) -> Response {
    Response::json(outbox_answer(c, true))
}
fn event_status(c: &Ctx) -> Response {
    Response::json(outbox_answer(c, false))
}
fn tags(c: &Ctx) -> Response {
    Response::json(calendar_tags_response(c))
}
fn find(c: &Ctx) -> Response {
    Response::json(calendar_find_response(c))
}

/// `{"ok":false,"error":msg}`.
fn err(m: &str) -> String {
    serde_json::json!({ "ok": false, "error": m }).to_string()
}

/// Re-read the account's calendar around `date` after an invitation changed it. Returns
/// `"refreshed"` or `"not refreshed: ..."`. (Also used by the invitation routes in `main.rs`.)
pub fn refresh_calendar_note(rt: &tokio::runtime::Runtime, db_path: &str, account: &str, date: Option<chrono::NaiveDate>) -> String {
    match Database::open_for_account(db_path, account) {
        Err(e) => format!("not refreshed: {}", e),
        Ok(cal_db) => {
            let provider = Arc::new(crate::providers::GraphCalendarProvider::new(AuthManager::for_account(account)));
            let cal = crate::calendar_daemon::CalendarDaemon::new(Default::default(), Arc::new(cal_db), provider);
            let near = rt.block_on(cal.refresh_near_window());
            let around = match date {
                Some(d) => rt.block_on(cal.refresh_around(d)).map(|_| ()),
                None => Ok(()),
            };
            match near.map(|_| ()).and(around) {
                Ok(()) => "refreshed".to_string(),
                Err(e) => { warn!("Calendar refresh after invitation change failed: {}", e); format!("not refreshed: {}", e) }
            }
        }
    }
}

/// POST /calendar/event — queue a new calendar event from the editor (docs/PLAN-calendar-events.md
/// sections 6 and 10). Body: the editor's JSON (`EventDraft::from_request`) plus `account_id` and an
/// optional `transaction_id` (the editor makes one per draft, so a repeated press is one event).
/// The write waits `[mail] send_delay_secs` (the undo window) and the account's calendar outbox worker
/// is woken. `?dry_run=1` answers the exact Graph body and queues nothing. Answers
/// `{ok, id, delay_secs, attendees}` or `{ok:false, error}`.
fn calendar_event_response(c: &Ctx) -> String {
    let config_dir = c.config_dir;
    use crate::calendar_write::EventDraft;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();

    let raw = match c.body_text(256 * 1024) {
        Ok(b) => b,
        Err(e) => return err(&e),
    };
    let mut req: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return err(&format!("unreadable event: {}", e)),
    };
    let account_id = req["account_id"].as_str().unwrap_or("").to_string();
    if account_id.is_empty() {
        return err("no calendar account selected");
    }
    if req["time_zone"].as_str().unwrap_or("").is_empty() {
        req["time_zone"] = serde_json::json!(crate::providers::calendar::local_time_zone());
    }
    let now = Database::now_ms();
    let tx = match req["transaction_id"].as_str().filter(|t| !t.is_empty()) {
        Some(t) => t.to_string(),
        None => format!("ol-{:x}-{:03x}", now, COUNTER.fetch_add(1, Ordering::Relaxed) & 0xfff),
    };
    // Online meeting (OL-021 track B): a personal / Jitsi link becomes text in the body and location here, once, so the
    // queued payload (and a retry) carries the same link; Teams / Meet stay as `online_provider` for the body builders.
    let meeting_url = match crate::online_meeting::prepare_request(&mut req, &account_id, &crate::online_meeting::read_config(&config_dir.join("settings.toml")), &tx) {
        Ok(u) => u,
        Err(e) => return err(&e.to_string()),
    };
    let draft = match EventDraft::from_request(&req, &tx) {
        Ok(d) => d,
        Err(e) => return err(&e),
    };
    if !draft.attendees.is_empty() {
        if let Some(m) = crate::accounts::personal_refusal(&account_id, crate::accounts::PersonalLimit::Invite) {
            return err(m);
        }
    }
    // Google accounts get Google's body (OL-021 track C). The dry run names the body by provider.
    let google = crate::accounts::provider_of(&account_id) == "gmail";
    let graph_body = match if google { crate::providers::google_calendar_write::create_body(&draft) } else { draft.to_graph_json() } {
        Ok(b) => b,
        Err(e) => return err(&e.to_string()),
    };
    if c.query("dry_run").as_deref() == Some("1") {
        let key = if google { "google" } else { "graph" };
        return serde_json::json!({ "ok": true, "dry_run": true, "provider": key, key: graph_body, "online_provider": draft.online_provider, "join_url": meeting_url }).to_string();
    }
    // A signed-out account has no worker, so the event would sit unsaved without anyone knowing.
    if !crate::scheduler::is_running(&account_id) {
        return err("that account is not signed in, so it cannot save events");
    }
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let delay = crate::settings::read_send_delay(&config_dir.join("settings.toml"));
    let id = format!("co-{:x}-{:03x}", now, COUNTER.fetch_add(1, Ordering::Relaxed) & 0xfff);
    let payload = req.to_string();
    if let Err(e) = db.cal_outbox_enqueue(&id, &account_id, "create", &payload, draft.attendees.len(), &tx, now + delay as i64 * 1000) {
        return err(&format!("could not queue the event: {}", e));
    }
    crate::sync_state::request_cal_outbox_run();
    log::info!("Calendar: queued {} ({}, {} attendee(s)), saves in {}s", id, account_id, draft.attendees.len(), delay);
    serde_json::json!({ "ok": true, "id": id, "delay_secs": delay, "attendees": draft.attendees.len(), "join_url": meeting_url }).to_string()
}

/// PATCH /calendar/event?id=ID&scope=occurrence|series|following[&dry_run=1] — edit an event (phase 3): the body is the editor's
/// JSON (as for a create) with `account_id`. `scope=series` changes the whole series (the id resolves to its master);
/// `occurrence` makes one exception; `following` ends the series the day before the picked occurrence and starts a new
/// series from it (plan 5.7). POST /calendar/event/cancel?id=ID&scope=…&kind=cancel|decline|delete[&dry_run=1] —
/// cancel a meeting you organise (sends the cancellation), decline one you were invited to, or delete an entry; the
/// body is `{account_id, comment}`. Every one waits the undo window (`POST /calendar/event/undo?id=`) and is sent by
/// the account's calendar outbox. `invitees` counts the people the change reaches, shown before the user confirms.
/// `?dry_run=1` answers what would be sent and queues nothing. Answers `{ok, id, target, series, invitees, delay_secs}`.
fn calendar_change_response(c: &Ctx, update: bool) -> String {
    let config_dir = c.config_dir;
    use crate::calendar_write::{invitee_count, EventDraft};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();

    let raw = match c.body_text(256 * 1024) {
        Ok(b) => b,
        Err(e) => return err(&e),
    };
    let mut req: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return err(&format!("unreadable change: {}", e)),
    };
    let account_id = req["account_id"].as_str().unwrap_or("").to_string();
    if account_id.is_empty() {
        return err("no calendar account selected");
    }
    let event_id = c.query("id").unwrap_or_default();
    if event_id.is_empty() {
        return err("missing id");
    }
    let scope = c.query("scope").unwrap_or_default();
    let whole_series = scope == "series";
    let following = scope == "following";
    let kind = if update { "update".to_string() } else { c.query("kind").unwrap_or_else(|| "cancel".into()) };
    if !matches!(kind.as_str(), "update" | "cancel" | "decline" | "delete") {
        return err("unknown change");
    }
    let now = Database::now_ms();
    let tx = format!("ol-{:x}-{:03x}", now, COUNTER.fetch_add(1, Ordering::Relaxed) & 0xfff);
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let target = match db.cal_event_ref(&event_id, whole_series || following) {
        Ok(Some(r)) => r,
        Ok(None) => return err("that event is not in the calendar cache; refresh the calendar and try again"),
        Err(e) => return err(&e),
    };
    // "This and following" (plan 5.7): the target is the series master; the picked occurrence's date is where the
    // new series starts. Only an edit and a personal delete split a series; a cancel of the rest is not built yet.
    let occurrence = if following {
        let picked = match db.cal_event_ref(&event_id, false) {
            Ok(Some(r)) => r,
            _ => return err("that event is not in the calendar cache; refresh the calendar and try again"),
        };
        if !target.series {
            return err("this event does not repeat");
        }
        if crate::accounts::provider_of(&account_id) == "gmail" {
            return err("Google calendars cannot split a series yet: change this occurrence or the whole series");
        }
        if kind == "cancel" || kind == "decline" {
            return err("cancelling the rest of a series is not built yet: cancel this occurrence or the whole series");
        }
        if kind == "delete" && target.invitees > 0 {
            return err("this series has invitees: cancel this occurrence or the whole series");
        }
        picked.start.get(..10).unwrap_or_default().to_string()
    } else {
        String::new()
    };
    // What the change reaches, and (for an edit) what it will send to the provider, checked before anything queues.
    let (invitees, graph) = if update {
        if req["time_zone"].as_str().unwrap_or("").is_empty() {
            req["time_zone"] = serde_json::json!(crate::providers::calendar::local_time_zone());
        }
        // An edit never adds a Teams / Meet room (the patch builders drop it); a personal / Jitsi link is text, so it can change.
        if let Err(e) = crate::online_meeting::prepare_request(&mut req, &account_id, &crate::online_meeting::read_config(&config_dir.join("settings.toml")), &tx) {
            return err(&e.to_string());
        }
        let draft = match EventDraft::from_request(&req, &tx) {
            Ok(d) => d,
            Err(e) => return err(&e),
        };
        if !draft.attendees.is_empty() {
            if let Some(m) = crate::accounts::personal_refusal(&account_id, crate::accounts::PersonalLimit::Invite) {
                return err(m);
            }
        }
        let google = crate::accounts::provider_of(&account_id) == "gmail";
        let graph = match if google { crate::providers::google_calendar_write::patch_body(&draft, target.series) } else if following { draft.to_graph_json() } else { draft.to_graph_patch_json(target.series) } {
            Ok(b) => b,
            Err(e) => return err(&e.to_string()),
        };
        (invitee_count(&draft.attendees), Some(graph))
    } else {
        // A decline tells only the organiser; a cancel or a delete of a meeting tells every cached invitee.
        (if kind == "decline" { 0 } else { target.invitees }, None)
    };
    if c.query("dry_run").as_deref() == Some("1") {
        let google = crate::accounts::provider_of(&account_id) == "gmail";
        let key = if google { "google" } else { "graph" };
        return serde_json::json!({ "ok": true, "dry_run": true, "provider": key, "kind": kind, "id": event_id, "target": target.target,
            "series": target.series, "invitees": invitees, "following": following, "occurrence": occurrence, key: graph }).to_string();
    }
    if !crate::scheduler::is_running(&account_id) {
        return err("that account is not signed in, so it cannot change events");
    }
    let delay = crate::settings::read_send_delay(&config_dir.join("settings.toml"));
    let out_id = format!("cu-{:x}-{:03x}", now, COUNTER.fetch_add(1, Ordering::Relaxed) & 0xfff);
    let payload = if update {
        req["account_id"] = serde_json::json!(account_id);
        req["transaction_id"] = serde_json::json!(tx);
        req["series"] = serde_json::json!(target.series);
        req["following"] = serde_json::json!(following);
        req["occurrence"] = serde_json::json!(occurrence);
        req.clone()
    } else {
        serde_json::json!({ "account_id": account_id, "comment": req["comment"].as_str().unwrap_or(""),
            "subject": target.subject, "start": target.start, "end": target.end, "all_day": target.all_day,
            "location": target.location, "transaction_id": tx, "series": target.series,
            "following": following, "occurrence": occurrence })
    };
    if let Err(e) = db.cal_queue_event_change(&out_id, &kind, &target.target, &payload, invitees, now + delay as i64 * 1000) {
        return err(&e);
    }
    crate::sync_state::request_cal_outbox_run();
    log::info!("Calendar: queued {} {} of {} ({} invitee(s)), runs in {}s", kind, out_id, target.target, invitees, delay);
    serde_json::json!({ "ok": true, "id": out_id, "kind": kind, "target": target.target, "series": target.series,
        "following": following, "invitees": invitees, "delay_secs": delay }).to_string()
}

/// POST /calendar/tags?account=A&id=EVENT&tags=["Family","Panama"] (OL-040 phase 6b): set the tags of an event. A recurring
/// event is tagged as a whole (an occurrence resolves to its series). The change waits `[mail] send_delay_secs`, the same
/// undo window as every calendar write (`POST /calendar/event/undo?id=`), then the account's calendar outbox writes it
/// (Exchange categories / Google private property) and reads it back. `?dry_run=1` answers what would be written and queues
/// nothing. Answers `{ok, id, event_id, series, tags, delay_secs}` or `{ok:false, error}`.
fn calendar_tags_response(c: &Ctx) -> String {
    let config_dir = c.config_dir;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let account = c.query("account").unwrap_or_default();
    let event = c.query("id").unwrap_or_default();
    if account.is_empty() || event.is_empty() { return err("missing account or id") }
    let tags: Vec<String> = match serde_json::from_str::<Vec<String>>(&c.query("tags").unwrap_or_else(|| "[]".into())) {
        Ok(t) => crate::calendar_tags::clean(t),
        Err(_) => return err("tags must be a JSON list of names"),
    };
    if tags.len() > 25 { return err("too many tags on one event (25 at most)") }
    if let Some(bad) = tags.iter().find(|t| !crate::rules::valid_tag_name(t)) { return err(&format!("\"{}\" is not a usable tag name", bad)) }
    let db = match Database::open_for_account(config_dir.join("messages.db").to_str().unwrap_or("messages.db"), &account) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let now = Database::now_ms();
    let id = format!("ct-{:x}-{:03x}", now, COUNTER.fetch_add(1, Ordering::Relaxed) & 0xfff);
    let dry = c.query("dry_run").as_deref() == Some("1");
    if !dry && !crate::scheduler::is_running(&account) {
        return err("that account is not signed in, so it cannot save tags");
    }
    let delay = crate::settings::read_send_delay(&config_dir.join("settings.toml"));
    // a dry run queues far in the future and is cancelled at once: it exercises the same lookups without a write
    let queued = db.cal_queue_event_tags(&id, &event, &tags, if dry { i64::MAX / 2 } else { now + delay as i64 * 1000 });
    let (target, series) = match queued { Ok(v) => v, Err(e) => return err(&e) };
    if dry {
        let _ = db.cal_outbox_cancel(&id);
        return serde_json::json!({ "ok": true, "dry_run": true, "event_id": target, "series": series, "tags": tags }).to_string();
    }
    crate::sync_state::request_cal_outbox_run();
    log::info!("Calendar: queued tags {:?} on {} ({}), saves in {}s", tags, target, account, delay);
    serde_json::json!({ "ok": true, "id": id, "event_id": target, "series": series, "tags": tags, "delay_secs": delay }).to_string()
}

/// GET /calendar/find?account=A&attendees=a@x,b@y&optional=c@z&accounts=A2,A3&from=YYYY-MM-DD&to=YYYY-MM-DD&duration=MIN[&limit=5]
/// (OL-021 track A, docs/PLAN-calendar-events.md 5.6): ranked free slots. `account` is the account the answer is for
/// (Exchange; its getSchedule covers you and the attendees); `attendees` are required, `optional` optional; `accounts`
/// are other signed-in accounts whose calendars count as your busy time; `to` is exclusive. Read-only: nothing is
/// queued or sent. Answers the finder JSON (`calendar_finder::find_free_time`) or `{ok:false, error}`.
fn calendar_find_response(c: &Ctx) -> String {
    let config_dir = c.config_dir;
    use chrono::NaiveDate;
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let list = |key: &str| -> Vec<String> {
        c.query(key)
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };
    let date = |key: &str| c.query(key).and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok());
    let (from, to) = match (date("from"), date("to")) {
        (Some(f), Some(t)) => (f, t),
        _ => return err("from and to must be dates (YYYY-MM-DD, to exclusive)"),
    };
    let duration_min = match c.query("duration").and_then(|s| s.parse::<i64>().ok()) {
        Some(d) => d,
        None => return err("duration (minutes) is missing"),
    };
    let account_id = c.query("account").unwrap_or_default();
    if account_id.is_empty() {
        return err("missing account");
    }
    let mut attendees: Vec<(String, bool)> = list("attendees").into_iter().map(|a| (a, true)).collect();
    attendees.extend(list("optional").into_iter().map(|a| (a, false)));
    let req = crate::calendar_finder::FindRequest {
        account_id,
        attendees,
        other_accounts: list("accounts"),
        from,
        to,
        duration_min,
        limit: c.query("limit").and_then(|s| s.parse().ok()).unwrap_or(5),
    };
    match crate::calendar_finder::find_free_time(config_dir, &req) {
        Ok(v) => v.to_string(),
        Err(e) => err(&e),
    }
}

/// GET /calendar/events?month=YYYY-MM[&account=][&tag=] — cached calendar events.
fn events(c: &Ctx) -> Response {
    let month: String = c.query("month").unwrap_or_default();

    // `?tag=` keeps events whose tags match (same syntax as mail search: lucia, =Family, none, -lucia)
    let tag_filter = c.query("tag").unwrap_or_default();
    let body = match Database::open(c.config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => match db.get_events_for_month_by_account(&month, c.query("account").as_deref()) {
            Ok(events) => serde_json::to_string(
                &events.iter().filter(|(e, _, _)| crate::calendar_tags::matches(&e.tags, &tag_filter)).map(|(e, account_id, account_email)| serde_json::json!({
                    "tags": e.tags,
                    "account_id": account_id,
                    "account_email": account_email,
                    "id": e.id,
                    "subject": e.subject,
                    "body": e.body,
                    "start": e.start,
                    "end": e.end,
                    "is_all_day": e.is_all_day,
                    "time_zone": e.time_zone,
                    "event_type": e.event_type,
                    "series_master_id": e.series_master_id,
                    "location": e.location,
                    "organizer_name": e.organizer_name,
                    "organizer_email": e.organizer_email,
                    "is_organizer": e.is_organizer,
                    "attendees": serde_json::from_str::<serde_json::Value>(&e.attendees_json).unwrap_or_else(|_| serde_json::json!([])),
                    "my_response": e.my_response,
                    "show_as": e.show_as,
                    "online_provider": e.online_provider,
                    "reminder_minutes": e.reminder_minutes,
                    "web_link": e.web_link,
                    "sensitivity": e.sensitivity,
                    "join_url": crate::join_link::detect(&e.online_url, &e.location, &e.body).map(|j| j.url),
                    "join_provider": crate::join_link::detect(&e.online_url, &e.location, &e.body).map(|j| j.provider),
                })).collect::<Vec<_>>()
            ).unwrap_or_else(|_| "[]".to_string()),
            Err(e) => {
                warn!("GET /calendar/events: query failed: {}", e);
                "[]".to_string()
            }
        },
        Err(e) => {
            warn!("GET /calendar/events: DB open failed: {}", e);
            "[]".to_string()
        }
    };
    Response::json(body)
}

/// GET /calendar/outbox (queued, running and failed writes, and the ones that finished in the last
/// minute: the editor draws them as pending marks).
fn outbox_list(c: &Ctx) -> Response {
    let db = match Database::open(c.config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return Response::json(err(&format!("database: {}", e))),
    };
    Response::json(match db.cal_outbox_list() {
        Ok(rows) => serde_json::json!({ "ok": true, "items": rows.iter().map(|(id, account, op, state, attendees, error, due_at, payload, result)| {
            let p: serde_json::Value = serde_json::from_str(payload).unwrap_or(serde_json::Value::Null);
            serde_json::json!({
                "id": id, "account_id": account, "op": op, "state": state, "attendees": attendees,
                "error": error, "due_at": due_at, "result_id": result,
                "subject": p["subject"], "start": p["start"], "end": p["end"], "all_day": p["all_day"],
                "location": p["location"], "transaction_id": p["transaction_id"],
                "target": p["target"], "series": p["series"], "following": p["following"], "occurrence": p["occurrence"],
            })
        }).collect::<Vec<_>>() }).to_string(),
        Err(e) => err(&e.to_string()),
    })
}

/// POST /calendar/event/undo?id=ID (take a queued write back while it is inside its undo window;
/// `undoing`) and GET /calendar/event/status?id=ID: the write's state.
fn outbox_answer(c: &Ctx, undoing: bool) -> String {
    let db = match Database::open(c.config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let id = match c.query("id") { Some(i) => i, None => return err("missing id") };
    let cancelled = if undoing {
        match db.cal_outbox_cancel(&id) { Ok(c) => c, Err(e) => return err(&e.to_string()) }
    } else { false };
    match db.cal_outbox_status(&id) {
        Ok(Some((state, error, result))) => serde_json::json!({ "ok": true, "cancelled": cancelled, "state": state, "error": error, "result_id": result }).to_string(),
        Ok(None) => serde_json::json!({ "ok": false, "cancelled": false, "state": "unknown", "error": "no such event write" }).to_string(),
        Err(e) => err(&e.to_string()),
    }
}

/// GET /calendar/meeting-options?account=ID answers `{ok, options:[{id,label,kind,available,reason,default}]}` for that
/// calendar account (docs/PLAN-calendar-events.md 7.2). GET /settings/meetings answers the `[meetings]` settings;
/// POST /settings/meetings (JSON body, the same keys) stores them and answers what was stored, or `{ok:false,error}`.
fn meeting_options(c: &Ctx) -> Response {
    use crate::online_meeting as om;
    let account = c.query("account").unwrap_or_default();
    if account.is_empty() {
        return Response::json(err("no calendar account selected"));
    }
    let path = c.config_dir.join("settings.toml");
    Response::json(serde_json::json!({ "ok": true, "options": om::options(&account, &om::read_config(&path)) }).to_string())
}

fn meeting_cfg_json(cfg: &crate::online_meeting::MeetingConfig) -> String {
    use crate::online_meeting as om;
    serde_json::json!({
        "ok": true, "teams_enabled": cfg.teams_enabled, "meet_enabled": cfg.meet_enabled, "jitsi_enabled": cfg.jitsi_enabled,
        "personal_enabled": cfg.personal_enabled, "personal_label": cfg.personal_label, "personal_link": cfg.personal_link,
        "jitsi_server": cfg.jitsi_server, "default_provider": cfg.default_provider,
        "providers": om::registry().iter().map(|p| serde_json::json!({ "id": p.id(), "label": p.label() })).collect::<Vec<_>>(),
    }).to_string()
}

fn meetings_get(c: &Ctx) -> Response {
    use crate::online_meeting as om;
    Response::json(meeting_cfg_json(&om::read_config(&c.config_dir.join("settings.toml"))))
}

fn meetings_post(c: &Ctx) -> Response {
    use crate::online_meeting as om;
    let path = c.config_dir.join("settings.toml");
    let raw = match c.body_text(16 * 1024) {
        Ok(b) => b,
        Err(e) => return Response::json(err(&e)),
    };
    let v: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return Response::json(err(&format!("unreadable settings: {}", e))),
    };
    let cur = om::read_config(&path);
    let flag = |k: &str, d: bool| v[k].as_bool().unwrap_or(d);
    let text = |k: &str, d: &str| v[k].as_str().map(String::from).unwrap_or_else(|| d.to_string());
    let wanted = om::MeetingConfig {
        teams_enabled: flag("teams_enabled", cur.teams_enabled),
        meet_enabled: flag("meet_enabled", cur.meet_enabled),
        jitsi_enabled: flag("jitsi_enabled", cur.jitsi_enabled),
        personal_enabled: flag("personal_enabled", cur.personal_enabled),
        personal_label: text("personal_label", &cur.personal_label),
        personal_link: text("personal_link", &cur.personal_link),
        jitsi_server: text("jitsi_server", &cur.jitsi_server),
        default_provider: match v["default_provider"].as_object() {
            Some(o) => o.iter().filter_map(|(k, x)| Some((k.clone(), x.as_str()?.to_string()))).collect(),
            None => cur.default_provider.clone(),
        },
    };
    Response::json(match om::write_config(&path, &wanted) {
        Ok(stored) => meeting_cfg_json(&stored),
        Err(e) => err(&e.to_string()),
    })
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::request::{read_request, test_stream, Limits};
    use std::path::PathBuf;

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
        let dir = std::env::temp_dir().join(format!("omalook-calendar-{}-{}", test, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn text_of(r: &Response) -> String {
        String::from_utf8(r.body().to_vec()).expect("UTF-8 body")
    }

    #[test]
    fn the_table_lists_each_path_separately() {
        let routes = routes();
        let mut seen: Vec<(&str, &str, bool)> =
            routes.iter().map(|r| (r.method, r.pattern_str(), r.lock == Lock::None)).collect();
        seen.sort();
        assert_eq!(
            seen,
            vec![
                ("GET", "/calendar/event/status", false),
                ("GET", "/calendar/events", false),
                ("GET", "/calendar/find", true),
                ("GET", "/calendar/meeting-options", false),
                ("GET", "/calendar/outbox", false),
                ("GET", "/settings/meetings", false),
                ("PATCH", "/calendar/event", false),
                ("POST", "/calendar/event", false),
                ("POST", "/calendar/event/cancel", false),
                ("POST", "/calendar/event/undo", false),
                ("POST", "/calendar/tags", false),
                ("POST", "/settings/meetings", false),
            ]
        );
        assert!(routes.iter().all(|r| matches!(r.pattern, Pattern::Exact(_))));
    }

    #[test]
    fn events_on_an_empty_db_is_an_empty_list() {
        let dir = temp_dir("events-empty");
        let r = run(&dir, get("/calendar/events?month=2026-11"), events);
        assert_eq!(r.status(), 200);
        assert_eq!(text_of(&r), "[]");
        let r = run(&dir, get("/calendar/events"), events);
        assert_eq!(text_of(&r), "[]");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_with_bad_json_answers_unreadable_event() {
        let dir = temp_dir("create-bad-json");
        let r = run(&dir, post("/calendar/event", "{not json"), event_create);
        assert_eq!(r.status(), 200);
        let want = serde_json::from_str::<serde_json::Value>("{not json").unwrap_err();
        assert_eq!(text_of(&r), err(&format!("unreadable event: {}", want)));
        let r = run(&dir, post("/calendar/event", "{}"), event_create);
        assert_eq!(text_of(&r), err("no calendar account selected"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn change_needs_account_and_id() {
        let dir = temp_dir("change-missing");
        let r = run(&dir, post("/calendar/event/cancel?id=x", "{bad"), event_cancel);
        assert!(text_of(&r).contains("unreadable change: "));
        let r = run(&dir, post("/calendar/event/cancel?id=x", "{}"), event_cancel);
        assert_eq!(text_of(&r), err("no calendar account selected"));
        let r = run(&dir, post("/calendar/event/cancel", "{\"account_id\":\"a\"}"), event_cancel);
        assert_eq!(text_of(&r), err("missing id"));
        let r = run(&dir, post("/calendar/event/cancel?id=x&kind=nope", "{\"account_id\":\"a\"}"), event_cancel);
        assert_eq!(text_of(&r), err("unknown change"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_of_an_unknown_id_and_missing_id() {
        let dir = temp_dir("status-unknown");
        let r = run(&dir, get("/calendar/event/status?id=nope"), event_status);
        assert_eq!(
            text_of(&r),
            "{\"cancelled\":false,\"error\":\"no such event write\",\"ok\":false,\"state\":\"unknown\"}"
        );
        let r = run(&dir, get("/calendar/event/status"), event_status);
        assert_eq!(text_of(&r), err("missing id"));
        let r = run(&dir, post("/calendar/event/undo?id=nope", ""), event_undo);
        assert_eq!(
            text_of(&r),
            "{\"cancelled\":false,\"error\":\"no such event write\",\"ok\":false,\"state\":\"unknown\"}"
        );
        let r = run(&dir, get("/calendar/outbox"), outbox_list);
        assert_eq!(text_of(&r), "{\"items\":[],\"ok\":true}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tags_and_find_check_their_parameters() {
        let dir = temp_dir("tags-find");
        let r = run(&dir, post("/calendar/tags?account=a", ""), tags);
        assert_eq!(text_of(&r), err("missing account or id"));
        let r = run(&dir, post("/calendar/tags?account=a&id=e&tags=nope", ""), tags);
        assert_eq!(text_of(&r), err("tags must be a JSON list of names"));
        let r = run(&dir, get("/calendar/find?account=a"), find);
        assert_eq!(text_of(&r), err("from and to must be dates (YYYY-MM-DD, to exclusive)"));
        let r = run(&dir, get("/calendar/find?account=a&from=2026-11-16&to=2026-11-17"), find);
        assert_eq!(text_of(&r), err("duration (minutes) is missing"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn meetings_defaults_then_post_then_get() {
        let dir = temp_dir("meetings");
        let r = run(&dir, get("/settings/meetings"), meetings_get);
        let v: serde_json::Value = serde_json::from_str(&text_of(&r)).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["teams_enabled"], true);
        assert_eq!(v["personal_label"], "");
        assert_eq!(v["jitsi_server"], crate::online_meeting::MeetingConfig::default().jitsi_server);
        assert!(v["providers"].as_array().map(|a| !a.is_empty()).unwrap_or(false));

        let r = run(&dir, post("/settings/meetings", "{bad"), meetings_post);
        assert!(text_of(&r).contains("unreadable settings: "));

        let r = run(&dir, post("/settings/meetings", "{\"teams_enabled\":false,\"personal_label\":\"Room\"}"), meetings_post);
        let stored: serde_json::Value = serde_json::from_str(&text_of(&r)).unwrap();
        assert_eq!(stored["teams_enabled"], false);
        let r = run(&dir, get("/settings/meetings"), meetings_get);
        let v: serde_json::Value = serde_json::from_str(&text_of(&r)).unwrap();
        assert_eq!(v["teams_enabled"], false);
        assert_eq!(v["personal_label"], "Room");

        let r = run(&dir, get("/calendar/meeting-options"), meeting_options);
        assert_eq!(text_of(&r), err("no calendar account selected"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
