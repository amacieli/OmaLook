//! Calendar read service (R3.5): events from the local cache. The write side (drafts queued
//! through the outbox) is added below in the calendar write commit.

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
