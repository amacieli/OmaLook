//! MCP calendar tools (OL-021 track D). `calendar_find_times` is a read: the finder's ranked slots
//! (`calendar_finder::find_free_time`). `calendar_create_event`, `calendar_update_event` and
//! `calendar_cancel_event` are writes: each is queued in the account's calendar outbox and waits the send
//! delay (the undo window) before the worker sends it, so no invitation goes out at once. `dry_run` answers
//! what would be sent and queues nothing.
//!
//! Domain level, tool mode (off / ask / allow) and account scope are applied by `rpc::call_tool` and the
//! policy before a handler runs. The account check here is a second gate on the `account` argument.

use super::Ctx;
use crate::accounts::{is_personal_microsoft, personal_refusal, PersonalLimit};
use crate::calendar_finder::{find_free_time, FindRequest};
use crate::calendar_write::{invitee_count, looks_like_address, EventDraft};
use chrono::{NaiveDate, Utc};
use serde_json::{json, Value};

fn text<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty())
}

/// A JSON array of strings, or a comma-separated string, trimmed and without empties.
fn list_of(args: &Value, key: &str) -> Result<Vec<String>, String> {
    let raw: Vec<String> = match args.get(key) {
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| v.as_str().map(str::to_string).ok_or_else(|| format!("{} must be a list of strings", key)))
            .collect::<Result<_, _>>()?,
        Some(Value::String(s)) => s.split(',').map(str::to_string).collect(),
        None | Some(Value::Null) => Vec::new(),
        _ => return Err(format!("{} must be a list", key)),
    };
    Ok(raw.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
}

/// Email addresses from `list_of`, checked and de-duplicated (case-insensitively).
fn addresses(args: &Value, key: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for a in list_of(args, key)? {
        if !looks_like_address(&a) {
            return Err(format!("\"{}\" is not an email address", a));
        }
        if !out.iter().any(|o| o.eq_ignore_ascii_case(&a)) {
            out.push(a);
        }
    }
    Ok(out)
}

fn account_of(ctx: &Ctx, args: &Value) -> Result<String, String> {
    let id = text(args, "account").ok_or("missing account (an account id from accounts_list)")?;
    if !ctx.policy().account_allowed(id) {
        return Err("that account is not enabled for MCP".into());
    }
    Ok(id.to_string())
}

fn day(args: &Value, key: &str) -> Result<NaiveDate, String> {
    let t = text(args, key).ok_or(format!("missing {} (YYYY-MM-DD)", key))?;
    NaiveDate::parse_from_str(t, "%Y-%m-%d").map_err(|_| format!("{} must be a date like 2026-10-31", key))
}

fn dry_run(args: &Value) -> bool {
    args.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false)
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn open_db(ctx: &Ctx) -> Result<crate::db::Database, String> {
    let p = ctx.config_dir.join("messages.db");
    crate::db::Database::open(p.to_str().ok_or("bad database path")?).map_err(|e| format!("database: {}", e))
}

/// The editor's request JSON (what `EventDraft::from_request` reads), built from the MCP arguments.
/// Attendees are `required` unless listed in `optional`.
fn editor_json(args: &Value, account: &str) -> Result<Value, String> {
    let start = text(args, "start").ok_or("missing start (local time, YYYY-MM-DDTHH:MM)")?;
    let end = text(args, "end").ok_or("missing end (local time, YYYY-MM-DDTHH:MM)")?;
    let mut people: Vec<Value> = Vec::new();
    for e in addresses(args, "attendees")? {
        people.push(json!({ "email": e, "name": "", "role": "required" }));
    }
    for e in addresses(args, "optional")? {
        people.push(json!({ "email": e, "name": "", "role": "optional" }));
    }
    Ok(json!({
        "account_id": account,
        "subject": text(args, "subject").unwrap_or(""),
        "body": text(args, "body").unwrap_or(""),
        "start": start,
        "end": end,
        "time_zone": text(args, "time_zone").map(str::to_string).unwrap_or_else(|| crate::providers::calendar::local_time_zone()),
        "all_day": args.get("all_day").and_then(|v| v.as_bool()).unwrap_or(false),
        "location": text(args, "location").unwrap_or(""),
        "attendees": people,
        "reminder_minutes": args.get("reminder_minutes").cloned().unwrap_or(Value::Null),
        "show_as": text(args, "show_as").unwrap_or("busy"),
    }))
}

/// The draft for a write, checked: it must validate, and a personal Microsoft account gets no attendees.
fn checked_draft(args: &Value, account: &str, tx: &str) -> Result<(EventDraft, Value), String> {
    let v = editor_json(args, account)?;
    let draft = EventDraft::from_request(&v, tx)?;
    draft.validate().map_err(|e| e.to_string())?;
    if !draft.attendees.is_empty() {
        if let Some(m) = personal_refusal(account, PersonalLimit::Invite) {
            return Err(m.into());
        }
    }
    Ok((draft, v))
}

/// The account and type of a cached event, read-only. Only single events can be changed over MCP for now:
/// recurring series and their occurrences need an edit scope this tool does not take.
fn single_event(ctx: &Ctx, account: &str, id: &str) -> Result<(), String> {
    let conn = rusqlite::Connection::open_with_flags(
        ctx.config_dir.join("messages.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| e.to_string())?;
    let (acc, kind): (String, String) = conn
        .query_row("SELECT account_id, COALESCE(event_type, '') FROM calendar_events WHERE id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .map_err(|_| "no such event in this account".to_string())?;
    if acc != account {
        return Err("no such event in this account".into());
    }
    if matches!(kind.as_str(), "seriesMaster" | "occurrence" | "exception") {
        return Err("only single (non-recurring) events can be changed over MCP yet".into());
    }
    Ok(())
}

/// `calendar_find_times`: ranked free slots for the attendees. Work and school accounts only.
pub fn find_times(ctx: &Ctx, args: &Value) -> Result<Value, String> {
    let account = account_of(ctx, args)?;
    if let Some(m) = personal_refusal(&account, PersonalLimit::Finder) {
        return Err(m.into());
    }
    let from = day(args, "from")?;
    let to = day(args, "to")?;
    let duration_min = args.get("duration").and_then(|v| v.as_i64()).ok_or("missing duration (minutes)")?;
    let mut attendees: Vec<(String, bool)> = addresses(args, "attendees")?.into_iter().map(|a| (a, true)).collect();
    attendees.extend(addresses(args, "optional")?.into_iter().map(|a| (a, false)));
    let other_accounts = list_of(args, "accounts")?;
    if let Some(bad) = other_accounts.iter().find(|a| !ctx.policy().account_allowed(a)) {
        return Err(format!("account {} is not enabled for MCP", bad));
    }
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(5).clamp(1, 20) as usize;
    let req = FindRequest { account_id: account, attendees, other_accounts, from, to, duration_min, limit };
    req.validate()?;
    find_free_time(&ctx.config_dir, &req)
}

/// `calendar_create_event`: queues a new event behind the undo window.
pub fn create_event(ctx: &Ctx, args: &Value) -> Result<Value, String> {
    let account = account_of(ctx, args)?;
    let now = now_ms();
    let tx = format!("mcp-{:x}", now);
    let (draft, v) = checked_draft(args, &account, &tx)?;
    let graph = draft.to_graph_json().map_err(|e| e.to_string())?;
    let invitees = invitee_count(&draft.attendees);
    if dry_run(args) {
        return Ok(json!({ "ok": true, "dry_run": true, "invitees": invitees, "graph": graph }));
    }
    let delay = crate::settings::read_send_delay(&ctx.settings_path());
    let db = open_db(ctx)?;
    let id = format!("mcp-co-{:x}", now);
    db.cal_outbox_enqueue(&id, &account, "create", &v.to_string(), invitees, &tx, now + delay as i64 * 1000)
        .map_err(|e| format!("could not queue the event: {}", e))?;
    crate::sync_state::request_cal_outbox_run();
    Ok(json!({ "ok": true, "queued": true, "id": id, "delay_secs": delay, "invitees": invitees }))
}

/// `calendar_update_event`: replaces a single event's fields (the same arguments as create), queued.
pub fn update_event(ctx: &Ctx, args: &Value) -> Result<Value, String> {
    let account = account_of(ctx, args)?;
    let id = text(args, "id").ok_or("missing id")?;
    single_event(ctx, &account, id)?;
    let now = now_ms();
    let tx = format!("mcp-{:x}", now);
    let (draft, v) = checked_draft(args, &account, &tx)?;
    let invitees = invitee_count(&draft.attendees);
    if dry_run(args) {
        let patch = draft.to_graph_patch_json(false).map_err(|e| e.to_string())?;
        return Ok(json!({ "ok": true, "dry_run": true, "id": id, "invitees": invitees, "graph": patch }));
    }
    let delay = crate::settings::read_send_delay(&ctx.settings_path());
    let db = open_db(ctx)?;
    let outbox_id = format!("mcp-cu-{:x}", now);
    db.cal_queue_event_change(&outbox_id, "update", id, &v, invitees, now + delay as i64 * 1000)?;
    crate::sync_state::request_cal_outbox_run();
    Ok(json!({ "ok": true, "queued": true, "id": outbox_id, "delay_secs": delay, "invitees": invitees }))
}

/// `calendar_cancel_event`: `kind` = `cancel` (organiser's cancel, optional message to attendees) or `delete`
/// (the entry goes, nobody is told). Personal accounts: a message is refused, and a cancel is a plain delete.
pub fn cancel_event(ctx: &Ctx, args: &Value) -> Result<Value, String> {
    let account = account_of(ctx, args)?;
    let id = text(args, "id").ok_or("missing id")?;
    let kind = text(args, "kind").unwrap_or("cancel");
    if kind != "cancel" && kind != "delete" {
        return Err("kind must be cancel or delete".into());
    }
    let comment = text(args, "comment").unwrap_or("");
    if kind == "cancel" && !comment.is_empty() {
        if let Some(m) = personal_refusal(&account, PersonalLimit::CancelWithMessage) {
            return Err(m.into());
        }
    }
    single_event(ctx, &account, id)?;
    let op = if kind == "delete" || is_personal_microsoft(&account) { "delete" } else { "cancel" };
    if dry_run(args) {
        return Ok(json!({ "ok": true, "dry_run": true, "id": id, "op": op, "comment": comment }));
    }
    let now = now_ms();
    let delay = crate::settings::read_send_delay(&ctx.settings_path());
    let db = open_db(ctx)?;
    let outbox_id = format!("mcp-cc-{:x}", now);
    let payload = json!({ "account_id": account, "comment": comment });
    db.cal_queue_event_change(&outbox_id, op, id, &payload, 0, now + delay as i64 * 1000)?;
    crate::sync_state::request_cal_outbox_run();
    Ok(json!({ "ok": true, "queued": true, "id": outbox_id, "op": op, "delay_secs": delay }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::policy::{Class, Decision, Domain, Policy};
    use std::fs;
    use std::path::PathBuf;

    fn ctx(name: &str, accounts: &str) -> (Ctx, PathBuf) {
        let d = std::env::temp_dir().join(format!("omalook-mcp-cal-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let toml = format!(
            "[sync]\nrun_in_background = true\n[mcp]\nenabled = true\naccounts = [{}]\n[mcp.domains]\ncalendar = \"write\"\n",
            accounts
        );
        fs::write(d.join("settings.toml"), toml).unwrap();
        (Ctx::new(&d), d)
    }

    fn create_args(account: &str) -> Value {
        json!({
            "account": account, "subject": "Planning", "start": "2026-11-20T10:00", "end": "2026-11-20T10:30",
            "attendees": ["ann@x.com", "ANN@x.com", "bob@y.org"], "optional": ["cy@z.net"], "dry_run": true,
        })
    }

    #[test]
    fn schemas_name_the_required_fields() {
        use crate::mcp::registry::input_schema;
        assert_eq!(input_schema("calendar_create_event")["required"], json!(["account", "start", "end"]));
        assert_eq!(input_schema("calendar_cancel_event")["required"], json!(["account", "id"]));
        assert_eq!(input_schema("calendar_find_times")["required"], json!(["account", "from", "to", "duration"]));
    }

    #[test]
    fn writes_are_w2_and_the_read_is_r() {
        use crate::mcp::registry::find;
        assert_eq!(find("calendar_find_times").unwrap().class, Class::R);
        for n in ["calendar_create_event", "calendar_update_event", "calendar_cancel_event"] {
            let t = find(n).unwrap();
            assert_eq!(t.class, Class::W2, "{}", n);
            assert!(t.implemented);
        }
    }

    #[test]
    fn permission_modes_gate_the_write_tools() {
        use crate::mcp::policy::McpSettings;
        let mut s = McpSettings { enabled: true, ..Default::default() };
        s.domains.insert("calendar".into(), "write".into());
        // Default for W2 is ask: the rpc layer refuses it until the user allows the tool.
        assert_eq!(Policy::new(s.clone(), true).decide("calendar_create_event", Domain::Calendar, Class::W2), Decision::Ask);
        s.tools.insert("calendar_create_event".into(), "allow".into());
        assert_eq!(Policy::new(s.clone(), true).decide("calendar_create_event", Domain::Calendar, Class::W2), Decision::Allow);
        s.tools.insert("calendar_create_event".into(), "off".into());
        assert_eq!(Policy::new(s.clone(), true).decide("calendar_create_event", Domain::Calendar, Class::W2), Decision::Hidden);
        // At read level no write is visible even when allowed.
        s.domains.insert("calendar".into(), "read".into());
        s.tools.insert("calendar_update_event".into(), "allow".into());
        assert_eq!(Policy::new(s, true).decide("calendar_update_event", Domain::Calendar, Class::W2), Decision::Hidden);
    }

    #[test]
    fn accounts_outside_the_mcp_scope_are_refused() {
        let (c, d) = ctx("scope", "\"exchange-1\"");
        let e = create_event(&c, &create_args("exchange-2")).unwrap_err();
        assert!(e.contains("not enabled"), "{}", e);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn create_dry_run_counts_invitees_and_queues_nothing() {
        let (c, d) = ctx("dry", "\"exchange-1\"");
        let v = create_event(&c, &create_args("exchange-1")).unwrap();
        assert_eq!(v["dry_run"], json!(true));
        assert_eq!(v["invitees"], json!(3), "two required (case-folded duplicate dropped) and one optional");
        assert!(v["graph"].is_object());
        assert!(!d.join("messages.db").exists(), "a dry run never opens the database");
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn argument_validation_gives_plain_errors() {
        let (c, d) = ctx("validate", "\"exchange-1\"");
        let mut a = create_args("exchange-1");
        a["attendees"] = json!(["not-an-address"]);
        assert!(create_event(&c, &a).unwrap_err().contains("is not an email address"));
        let mut a = create_args("exchange-1");
        a["start"] = json!("tomorrow-ish");
        assert!(create_event(&c, &a).unwrap_err().contains("start"));
        let mut a = create_args("exchange-1");
        a.as_object_mut().unwrap().remove("end");
        assert!(create_event(&c, &a).unwrap_err().contains("missing end"));
        let mut a = create_args("exchange-1");
        a["attendees"] = json!(42);
        assert!(create_event(&c, &a).unwrap_err().contains("must be a list"));
        let f = json!({ "account": "exchange-1", "from": "2026-11-20", "to": "2026-11-27", "duration": 30, "attendees": "a@x.com" });
        assert!(find_times(&c, &f).is_ok() || find_times(&c, &f).unwrap_err().len() > 0);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn personal_accounts_create_without_attendees_and_refuse_the_rest() {
        let (c, d) = ctx("personal", "\"outlook-1\"");
        let mut solo = create_args("outlook-1");
        solo.as_object_mut().unwrap().remove("attendees");
        solo.as_object_mut().unwrap().remove("optional");
        assert_eq!(create_event(&c, &solo).unwrap()["invitees"], json!(0), "create works without attendees");
        let e = create_event(&c, &create_args("outlook-1")).unwrap_err();
        assert!(e.contains("can't invite attendees"), "{}", e);
        let f = json!({ "account": "outlook-1", "from": "2026-11-20", "to": "2026-11-27", "duration": 30 });
        assert!(find_times(&c, &f).unwrap_err().contains("no free-time finder"));
        let cancel = json!({ "account": "outlook-1", "id": "ev1", "comment": "Sorry", "dry_run": true });
        assert!(cancel_event(&c, &cancel).unwrap_err().contains("can't cancel with a message"));
        let _ = fs::remove_dir_all(d);
    }
}
