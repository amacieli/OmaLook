//! Calendar service (R3.5): events from the local cache (read), and the write path: drafts checked
//! here and queued through the calendar outbox. Callers pass typed inputs; no MCP argument shapes.

use super::{clip, Page, MAX_TEXT, NOT_FOUND};
use crate::db::Database;
use chrono::{Duration, Local, NaiveDate};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Event {
    pub id: String,
    pub subject: String,
    pub start: String,
    pub end: String,
    pub is_all_day: bool,
    pub time_zone: String,
    pub account_id: String,
    pub account_email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// The date range a list covers: `from` defaults to today, `to` to 14 days after `from`.
pub fn resolve_range(from: Option<NaiveDate>, to: Option<NaiveDate>) -> Result<(NaiveDate, NaiveDate), String> {
    let from = from.unwrap_or_else(|| Local::now().date_naive());
    let to = to.unwrap_or(from + Duration::days(14));
    if to < from { return Err("to is before from".into()); }
    if (to - from).num_days() > 366 { return Err("the range may not exceed 366 days".into()); }
    Ok((from, to))
}

pub fn list_events(db: &Database, accounts: &[String], from: NaiveDate, to: NaiveDate, limit: i64, offset: i64) -> Result<Page<Event>, String> {
    // Events overlapping [from, to + 1 day).
    let from_ts = format!("{}T00:00:00", from);
    let to_ts = format!("{}T00:00:00", to + Duration::days(1));
    let rows = db.mcp_events(accounts, &from_ts, &to_ts, limit, offset).map_err(|e| e.to_string())?;
    let items = rows.into_iter().map(|r| Event {
        id: r.id, subject: r.subject, start: r.start, end: r.end, is_all_day: r.is_all_day,
        time_zone: r.time_zone, account_id: r.account_id, account_email: r.account_email, body: None,
    }).collect();
    Ok(Page::of(items, limit, offset))
}

/// One event with its stored body (clipped). Visible only in an account `allowed` accepts.
pub fn get_event(db: &Database, allowed: &dyn Fn(&str) -> bool, id: &str) -> Result<Event, String> {
    let r = db.mcp_event(id).ok().flatten().ok_or_else(|| NOT_FOUND.to_string())?;
    if !allowed(&r.account_id) { return Err(NOT_FOUND.into()); }
    let body = r.body.ok_or_else(|| NOT_FOUND.to_string())?;
    Ok(Event {
        id: r.id, subject: r.subject, start: r.start, end: r.end, is_all_day: r.is_all_day,
        time_zone: r.time_zone, account_id: r.account_id, account_email: r.account_email,
        body: Some(clip(&body, MAX_TEXT)),
    })
}

// ---- write side: drafts queued through the calendar outbox -----------------------------------

use crate::accounts::{is_personal_microsoft, personal_refusal, PersonalLimit};
use crate::calendar_finder::{find_free_time, FindRequest};
use crate::calendar_write::{invitee_count, EventDraft};
use serde_json::{json, Value};
use std::path::Path;

/// A new or changed event, typed. `attendees` is `(address, required)`; addresses are already
/// checked and de-duplicated by the caller.
#[derive(Debug, Clone)]
pub struct EventInput {
    pub account_id: String,
    pub subject: String,
    pub body: String,
    pub start: String,
    pub end: String,
    pub time_zone: String,
    pub all_day: bool,
    pub location: String,
    pub attendees: Vec<(String, bool)>,
    pub reminder_minutes: Option<u64>,
    pub show_as: String,
}

/// A draft that passed validation, with what the outbox needs.
pub struct Checked {
    pub draft: EventDraft,
    /// The editor JSON the draft was read from; this is what the outbox stores.
    pub editor: Value,
    pub invitees: usize,
}

impl EventInput {
    fn editor_json(&self) -> Value {
        let people: Vec<Value> = self
            .attendees
            .iter()
            .map(|(e, required)| json!({ "email": e, "name": "", "role": if *required { "required" } else { "optional" } }))
            .collect();
        json!({
            "account_id": self.account_id,
            "subject": self.subject,
            "body": self.body,
            "start": self.start,
            "end": self.end,
            "time_zone": self.time_zone,
            "all_day": self.all_day,
            "location": self.location,
            "attendees": people,
            "reminder_minutes": self.reminder_minutes.map(Value::from).unwrap_or(Value::Null),
            "show_as": self.show_as,
        })
    }
}

/// Build and check the draft: it must validate, and a personal Microsoft account gets no attendees.
pub fn check_draft(input: &EventInput, tx: &str) -> Result<Checked, String> {
    let editor = input.editor_json();
    let draft = EventDraft::from_request(&editor, tx)?;
    draft.validate().map_err(|e| e.to_string())?;
    if !draft.attendees.is_empty() {
        if let Some(m) = personal_refusal(&input.account_id, PersonalLimit::Invite) {
            return Err(m.into());
        }
    }
    let invitees = invitee_count(&draft.attendees);
    Ok(Checked { draft, editor, invitees })
}

impl Checked {
    /// The Graph body a create would send (dry run).
    pub fn create_graph(&self) -> Result<Value, String> {
        self.draft.to_graph_json().map_err(|e| e.to_string())
    }
    /// The Graph patch an update would send (dry run).
    pub fn update_graph(&self) -> Result<Value, String> {
        self.draft.to_graph_patch_json(false).map_err(|e| e.to_string())
    }
}

/// Only a single (non-recurring) event of `account` can be changed over MCP for now: recurring
/// series and their occurrences need an edit scope this tool does not take.
pub fn check_single_event(db: &Database, account: &str, id: &str) -> Result<(), String> {
    let (acc, kind) = db.cal_event_kind(id).ok().flatten().ok_or("no such event in this account")?;
    if acc != account {
        return Err("no such event in this account".into());
    }
    if matches!(kind.as_str(), "seriesMaster" | "occurrence" | "exception") {
        return Err("only single (non-recurring) events can be changed over MCP yet".into());
    }
    Ok(())
}

/// The outbox operation for a cancel request: `kind` is `cancel` (organiser's cancel, optional
/// message) or `delete` (nobody is told). Personal accounts: a message is refused, and a cancel is a delete.
pub fn cancel_op(account: &str, kind: &str, comment: &str) -> Result<&'static str, String> {
    if kind != "cancel" && kind != "delete" {
        return Err("kind must be cancel or delete".into());
    }
    if kind == "cancel" && !comment.is_empty() {
        if let Some(m) = personal_refusal(account, PersonalLimit::CancelWithMessage) {
            return Err(m.into());
        }
    }
    Ok(if kind == "delete" || is_personal_microsoft(account) { "delete" } else { "cancel" })
}

/// Queue a new event (`create`) to be sent at `due_ms`, then wake the outbox worker.
pub fn queue_create(db: &Database, outbox_id: &str, checked: &Checked, account: &str, tx: &str, due_ms: i64) -> Result<(), String> {
    db.cal_outbox_enqueue(outbox_id, account, "create", &checked.editor.to_string(), checked.invitees, tx, due_ms)
        .map_err(|e| format!("could not queue the event: {}", e))?;
    crate::sync_state::request_cal_outbox_run();
    Ok(())
}

/// Queue a replacement of event `target` (`update`), then wake the outbox worker.
pub fn queue_update(db: &Database, outbox_id: &str, target: &str, checked: &Checked, due_ms: i64) -> Result<(), String> {
    db.cal_queue_event_change(outbox_id, "update", target, &checked.editor, checked.invitees, due_ms)?;
    crate::sync_state::request_cal_outbox_run();
    Ok(())
}

/// Queue a `cancel` or `delete` of event `target`, then wake the outbox worker.
pub fn queue_cancel(db: &Database, outbox_id: &str, op: &str, target: &str, account: &str, comment: &str, due_ms: i64) -> Result<(), String> {
    let payload = json!({ "account_id": account, "comment": comment });
    db.cal_queue_event_change(outbox_id, op, target, &payload, 0, due_ms)?;
    crate::sync_state::request_cal_outbox_run();
    Ok(())
}

/// Refuse the free-time finder for personal accounts.
pub fn check_finder_allowed(account: &str) -> Result<(), String> {
    match personal_refusal(account, PersonalLimit::Finder) {
        Some(m) => Err(m.into()),
        None => Ok(()),
    }
}

/// Ranked free slots for the attendees (the finder checks the request itself).
pub fn find_times(config_dir: &Path, req: &FindRequest) -> Result<Value, String> {
    req.validate()?;
    find_free_time(config_dir, req)
}
