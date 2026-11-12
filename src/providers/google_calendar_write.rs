//! Google Calendar write side (OL-021 track C; docs/PLAN-calendar-events.md sections 4, 6, 7.1).
//!
//! Pure request builders (golden-tested, no network) and the thin calls that send them through the
//! account's `GoogleApi`. `GoogleCalendarProvider`'s `CalendarProvider` methods delegate here, and the
//! calendar outbox worker calls those, so the undo window, claim, retry and dry run are the same as Graph's.
//!
//! Google rules this module follows:
//! * `sendUpdates=all` only when the event has people on it; `none` otherwise (create, edit, personal delete).
//!   A cancel of a meeting always says `all` (Google sends nothing when there is nobody to tell).
//! * `responseStatus` is never sent on create or edit: Google gives each invitee `needsAction` and the
//!   organiser their own answer. Only a decline sets a response (the attendee's own entry, `declined`).
//! * Series: the master id changes the whole series; an occurrence (instance) id makes an exception.
//! * Ids: Google ids are per calendar and the cache stores them as `<account>:<id>`; `unscoped` on the way in,
//!   `scoped` on the way out. A create's id is derived from the transaction id, so a retry hits 409 (same
//!   event) instead of making a duplicate.
//! * Google's delete has no message field, so the cancellation message is not sent.

use super::google_api::GoogleApi;
use crate::accounts::{scoped, unscoped};
use crate::calendar_write::{Attendee, CreatedEvent, DraftError, EventDraft, Pattern, Range, Recurrence};
use crate::errors::{OmarchyError, Result};
use crate::google_meet;
use crate::providers::calendar::ScheduleAnswer;
use chrono::{DateTime, Duration, FixedOffset, Local, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc, Weekday};
use serde_json::{json, Value};

const EVENTS: &str = "https://www.googleapis.com/calendar/v3/calendars/primary/events";
/// Google's free/busy endpoint (answers busy intervals per calendar).
pub(crate) const FREEBUSY: &str = "https://www.googleapis.com/calendar/v3/freeBusy";

fn enc(id: &str) -> String {
    urlencoding::encode(id).into_owned()
}

fn stamp(dt: NaiveDateTime) -> String {
    dt.format("%Y-%m-%dT%H:%M:%S").to_string()
}

fn summary(draft: &EventDraft) -> String {
    if draft.subject.trim().is_empty() { "(no title)".to_string() } else { draft.subject.clone() }
}

/// `sendUpdates` for a write: `all` only when there are people to tell.
pub fn send_updates(has_people: bool) -> &'static str {
    if has_people { "all" } else { "none" }
}

/// `start` / `end` objects. Timed: local wall-clock plus the IANA zone. All-day: dates, end exclusive
/// (the day after the last day), the same rule the Graph builder uses.
pub fn when(draft: &EventDraft) -> (Value, Value) {
    if draft.is_all_day {
        let s = draft.start.date();
        let e = draft.end.date();
        let e = if e > s { e } else { s + Duration::days(1) };
        (json!({ "date": s.format("%Y-%m-%d").to_string() }), json!({ "date": e.format("%Y-%m-%d").to_string() }))
    } else {
        (
            json!({ "dateTime": stamp(draft.start), "timeZone": draft.time_zone }),
            json!({ "dateTime": stamp(draft.end), "timeZone": draft.time_zone }),
        )
    }
}

/// Google has no tentative / out-of-office / working-elsewhere show-as; those become busy.
pub fn transparency(show_as: &str) -> &'static str {
    if show_as == "free" { "transparent" } else { "opaque" }
}

fn reminders(minutes: Option<u32>) -> Value {
    match minutes {
        Some(m) => json!({ "useDefault": false, "overrides": [{ "method": "popup", "minutes": m }] }),
        None => json!({ "useDefault": false, "overrides": [] }),
    }
}

/// Google attendee: `email`, optional `displayName`, `optional` / `resource` flags. No `responseStatus` (see module doc).
fn attendee_json(a: &Attendee) -> Value {
    let mut v = json!({ "email": a.email });
    if !a.name.trim().is_empty() {
        v["displayName"] = json!(a.name);
    }
    if a.role == "optional" {
        v["optional"] = json!(true);
    }
    if a.role == "resource" {
        v["resource"] = json!(true);
    }
    v
}

fn weekday_code(d: Weekday) -> &'static str {
    match d {
        Weekday::Mon => "MO",
        Weekday::Tue => "TU",
        Weekday::Wed => "WE",
        Weekday::Thu => "TH",
        Weekday::Fri => "FR",
        Weekday::Sat => "SA",
        Weekday::Sun => "SU",
    }
}

/// UNTIL value. All-day: a date. Timed: the end of that day in UTC, taken in this machine's zone when the
/// event is in it (the editor's default). Another zone has no zone database here, so its end of day is taken
/// as UTC: a late-evening last occurrence in a far-off zone can then be dropped. Known limitation.
fn until_value(end: NaiveDate, zone: &str, all_day: bool) -> String {
    if all_day {
        return end.format("%Y%m%d").to_string();
    }
    let local_end = end.and_hms_opt(23, 59, 59).expect("end of day");
    if zone == crate::providers::calendar::local_time_zone() {
        if let LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) = Local.from_local_datetime(&local_end) {
            return dt.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ").to_string();
        }
    }
    local_end.format("%Y%m%dT%H%M%SZ").to_string()
}

/// The RRULE line for a series (`recurrence: ["RRULE:..."]`). Pattern and range are the editor's, already validated.
pub fn rrule(rec: &Recurrence, zone: &str, all_day: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    match &rec.pattern {
        Pattern::Daily { interval } => {
            parts.push("FREQ=DAILY".into());
            parts.push(format!("INTERVAL={interval}"));
        }
        Pattern::Weekly { interval, days } => {
            parts.push("FREQ=WEEKLY".into());
            parts.push(format!("INTERVAL={interval}"));
            let by: Vec<&str> = days.iter().map(|d| weekday_code(*d)).collect();
            parts.push(format!("BYDAY={}", by.join(",")));
        }
        Pattern::AbsoluteMonthly { interval, day_of_month } => {
            parts.push("FREQ=MONTHLY".into());
            parts.push(format!("INTERVAL={interval}"));
            parts.push(format!("BYMONTHDAY={day_of_month}"));
        }
        Pattern::RelativeMonthly { interval, day, index } => {
            let ord = match index.as_str() {
                "first" => "1",
                "second" => "2",
                "third" => "3",
                "fourth" => "4",
                _ => "-1", // "last" (validated upstream)
            };
            parts.push("FREQ=MONTHLY".into());
            parts.push(format!("INTERVAL={interval}"));
            parts.push(format!("BYDAY={}{}", ord, weekday_code(*day)));
        }
    }
    match &rec.range {
        Range::NoEnd => {}
        Range::EndDate(end) => parts.push(format!("UNTIL={}", until_value(*end, zone, all_day))),
        Range::Numbered(n) => parts.push(format!("COUNT={n}")),
    }
    format!("RRULE:{}", parts.join(";"))
}

/// Google event id derived from the transaction id: lowercase `a-v` and `0-9` only (Google's rule), always
/// prefixed so it is at least 5 characters. The same draft always gets the same id, so a retried create meets 409.
pub fn event_id_for(transaction_id: &str) -> String {
    let clean: String = transaction_id
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .filter(|c| matches!(c, 'a'..='v' | '0'..='9'))
        .take(900)
        .collect();
    format!("omalook{clean}")
}

/// The `events.insert` body. Pure: same draft, same JSON.
pub fn create_body(draft: &EventDraft) -> std::result::Result<Value, DraftError> {
    draft.validate()?;
    let (start, end) = when(draft);
    let mut b = json!({
        "id": event_id_for(&draft.transaction_id),
        "summary": summary(draft),
        "description": draft.body_text,
        "start": start,
        "end": end,
        "transparency": transparency(&draft.show_as),
        "reminders": reminders(draft.reminder_minutes),
    });
    if let Some(loc) = draft.location.as_deref().filter(|l| !l.trim().is_empty()) {
        b["location"] = json!(loc);
    }
    if !draft.attendees.is_empty() {
        b["attendees"] = Value::Array(draft.attendees.iter().map(attendee_json).collect());
    }
    if let Some(rec) = &draft.recurrence {
        b["recurrence"] = json!([rrule(rec, &draft.time_zone, draft.is_all_day)]);
    }
    if google_meet::wants_meet(draft.online_provider.as_deref()) {
        b["conferenceData"] = google_meet::create_request(&draft.transaction_id);
    }
    Ok(b)
}

/// The `events.insert` URL: `sendUpdates` from the attendees, `conferenceDataVersion=1` when a Meet room is asked for.
pub fn create_url(draft: &EventDraft) -> String {
    let mut q = format!("sendUpdates={}", send_updates(!draft.attendees.is_empty()));
    if google_meet::wants_meet(draft.online_provider.as_deref()) {
        q.push('&');
        q.push_str(google_meet::CONFERENCE_QUERY);
    }
    format!("{EVENTS}?{q}")
}

/// The `events.patch` body for an edit. Always sends `attendees` (an empty list removes everyone) and the
/// location (empty clears it). `recurrence` only for the series master. Never the Meet request (an edit does not add a room).
pub fn patch_body(draft: &EventDraft, series: bool) -> std::result::Result<Value, DraftError> {
    draft.validate()?;
    let (start, end) = when(draft);
    let mut b = json!({
        "summary": summary(draft),
        "description": draft.body_text,
        "start": start,
        "end": end,
        "transparency": transparency(&draft.show_as),
        "reminders": reminders(draft.reminder_minutes),
        "location": draft.location.as_deref().unwrap_or("").trim(),
        "attendees": draft.attendees.iter().map(attendee_json).collect::<Vec<_>>(),
    });
    if series {
        if let Some(rec) = &draft.recurrence {
            b["recurrence"] = json!([rrule(rec, &draft.time_zone, draft.is_all_day)]);
        }
    }
    Ok(b)
}

/// `events.patch` URL for an edit: `sendUpdates` is `all` when the edited event has people on it.
pub fn patch_url(account_id: &str, target: &str, draft: &EventDraft) -> String {
    format!("{EVENTS}/{}?sendUpdates={}", enc(unscoped(account_id, target)), send_updates(!draft.attendees.is_empty()))
}

/// The `events.patch` body for an attendee's decline: the event's attendee list as it is, with the calendar
/// owner's own entry set to `declined` (and the comment on it). None when the owner is not on the guest list.
pub fn decline_body(event: &Value, comment: &str) -> Option<Value> {
    let list = event["attendees"].as_array()?;
    let me = list.iter().position(|a| a["self"].as_bool() == Some(true))?;
    let attendees: Vec<Value> = list
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let mut v = json!({ "email": a["email"].as_str().unwrap_or("") });
            for key in ["displayName", "optional", "resource", "additionalGuests"] {
                if let Some(x) = a.get(key).filter(|x| !x.is_null()) {
                    v[key] = x.clone();
                }
            }
            if i == me {
                v["responseStatus"] = json!("declined");
                if !comment.trim().is_empty() {
                    v["comment"] = json!(comment);
                }
            } else if let Some(r) = a["responseStatus"].as_str() {
                v["responseStatus"] = json!(r);
            }
            v
        })
        .collect();
    Some(json!({ "attendees": attendees }))
}

/// A 404 or 410 on delete: the event is already gone, which is the state asked for.
pub fn is_gone(e: &OmarchyError) -> bool {
    let s = e.to_string();
    s.contains("error: 404") || s.contains("error: 410")
}

fn is_conflict(e: &OmarchyError) -> bool {
    e.to_string().contains("error: 409")
}

fn created(account_id: &str, raw: Value) -> CreatedEvent {
    CreatedEvent {
        id: scoped(account_id, raw["id"].as_str().unwrap_or("")),
        web_link: raw["htmlLink"].as_str().unwrap_or("").to_string(),
        join_url: google_meet::join_link(&raw).unwrap_or_default(),
        is_series: raw["recurrence"].is_array(),
        raw,
    }
}

/// Create an event on the primary calendar. A 409 (the same transaction id again) returns the event that exists.
pub async fn create(api: &GoogleApi, account_id: &str, draft: &EventDraft) -> Result<CreatedEvent> {
    let body = create_body(draft).map_err(|e| OmarchyError::HttpError(format!("Invalid event: {e}")))?;
    let raw = match api.post(&create_url(draft), &body).await {
        Ok(raw) => raw,
        Err(e) if is_conflict(&e) => {
            api.get(&format!("{EVENTS}/{}", enc(&event_id_for(&draft.transaction_id)))).await?
        }
        Err(e) => return Err(e),
    };
    Ok(created(account_id, raw))
}

/// Edit an event. `series` says the target is the master (the whole series changes).
pub async fn update(api: &GoogleApi, account_id: &str, target: &str, draft: &EventDraft, series: bool) -> Result<CreatedEvent> {
    let body = patch_body(draft, series).map_err(|e| OmarchyError::HttpError(format!("Invalid event: {e}")))?;
    let raw = api.patch(&patch_url(account_id, target, draft), &body).await?;
    Ok(created(account_id, raw))
}

/// Delete an event (or one occurrence, or a whole series when given the master). `notify` = tell the attendees.
/// Already gone counts as done.
pub async fn delete(api: &GoogleApi, account_id: &str, target: &str, notify: bool) -> Result<()> {
    let url = format!("{EVENTS}/{}?sendUpdates={}", enc(unscoped(account_id, target)), if notify { "all" } else { "none" });
    match api.delete(&url).await {
        Ok(_) => Ok(()),
        Err(e) if is_gone(&e) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Attendee: decline (the owner is told). Reads the event first, so the guest list is the live one.
pub async fn decline(api: &GoogleApi, account_id: &str, target: &str, comment: &str) -> Result<()> {
    let id = enc(unscoped(account_id, target));
    let event = api.get(&format!("{EVENTS}/{id}")).await?;
    let body = decline_body(&event, comment)
        .ok_or_else(|| OmarchyError::HttpError("you are not on this event's guest list".to_string()))?;
    api.patch(&format!("{EVENTS}/{id}?sendUpdates=all"), &body).await?;
    Ok(())
}

/// Local wall-clock -> UTC instant (a time that does not exist in this zone is read as UTC).
fn local_to_utc(t: NaiveDateTime) -> DateTime<Utc> {
    match Local.from_local_datetime(&t) {
        LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => dt.with_timezone(&Utc),
        LocalResult::None => t.and_utc(),
    }
}

/// `freeBusy` request for the given people over `[from, to)`.
pub fn freebusy_body(emails: &[String], from: DateTime<Utc>, to: DateTime<Utc>) -> Value {
    json!({
        "timeMin": from.to_rfc3339(),
        "timeMax": to.to_rfc3339(),
        "items": emails.iter().map(|e| json!({ "id": e })).collect::<Vec<_>>(),
    })
}

/// Google's `freeBusy` answer -> the finder's `ScheduleAnswer` (one digit per `interval_min` slot, `2` busy,
/// `0` free). A calendar that is missing or reports errors gets `error` and no view, so it is unknown, not free.
pub fn parse_freebusy(json: &Value, emails: &[String], start: NaiveDateTime, end: NaiveDateTime, interval_min: u32) -> Vec<ScheduleAnswer> {
    let span = (end - start).num_minutes();
    let slots = if span <= 0 || interval_min == 0 { 0 } else { ((span + interval_min as i64 - 1) / interval_min as i64) as usize };
    let slot_at = |i: usize| local_to_utc(start + Duration::minutes(i as i64 * interval_min as i64));
    let calendars = json["calendars"].as_object();
    emails
        .iter()
        .map(|email| {
            // Google echoes the id as sent; compare case-insensitively in case the case differs.
            let cal = calendars.and_then(|m| m.iter().find(|(k, _)| k.eq_ignore_ascii_case(email)).map(|(_, v)| v));
            let Some(cal) = cal else {
                return ScheduleAnswer { schedule_id: email.clone(), error: Some("no free/busy answer for this address".into()), ..Default::default() };
            };
            if let Some(first) = cal["errors"].as_array().and_then(|e| e.first()) {
                let reason = first["reason"].as_str().or_else(|| first["domain"].as_str()).unwrap_or("error");
                return ScheduleAnswer { schedule_id: email.clone(), error: Some(format!("free/busy not available ({reason})")), ..Default::default() };
            }
            let busy: Vec<(DateTime<Utc>, DateTime<Utc>)> = cal["busy"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|b| {
                    let s = DateTime::<FixedOffset>::parse_from_rfc3339(b["start"].as_str()?).ok()?.with_timezone(&Utc);
                    let e = DateTime::<FixedOffset>::parse_from_rfc3339(b["end"].as_str()?).ok()?.with_timezone(&Utc);
                    Some((s, e))
                })
                .collect();
            let view: String = (0..slots)
                .map(|i| {
                    let (s0, s1) = (slot_at(i), slot_at(i + 1));
                    if busy.iter().any(|(bs, be)| *bs < s1 && *be > s0) { '2' } else { '0' }
                })
                .collect();
            ScheduleAnswer { schedule_id: email.clone(), availability_view: view, error: None, working_hours: None }
        })
        .collect()
}

/// Free/busy for up to 50 people over `[start, end)` local wall-clock (the finder's shape: one digit per slot).
pub async fn schedule(api: &GoogleApi, emails: &[String], start: NaiveDateTime, end: NaiveDateTime, interval_min: u32) -> Result<Vec<ScheduleAnswer>> {
    let body = freebusy_body(emails, local_to_utc(start), local_to_utc(end));
    let raw = api.post(FREEBUSY, &body).await?;
    Ok(parse_freebusy(&raw, emails, start, end, interval_min))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar_write::{Pattern, Range};
    use chrono::NaiveDate;

    fn dt(y: i32, m: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(h, mi, 0).unwrap()
    }

    fn base() -> EventDraft {
        EventDraft {
            subject: "Planning".into(),
            body_text: "agenda".into(),
            start: dt(2026, 11, 16, 9, 0),
            end: dt(2026, 11, 16, 9, 30),
            time_zone: "America/New_York".into(),
            is_all_day: false,
            location: None,
            attendees: vec![],
            reminder_minutes: Some(15),
            show_as: "busy".into(),
            recurrence: None,
            transaction_id: "ol-1f-002".into(),
            online_provider: None,
        }
    }

    fn att(email: &str, role: &str, name: &str) -> Attendee {
        Attendee { name: name.into(), email: email.into(), role: role.into() }
    }

    #[test]
    fn personal_timed_create_golden() {
        let body = create_body(&base()).unwrap();
        assert_eq!(body, json!({
            "id": "omalookol1f002",
            "summary": "Planning",
            "description": "agenda",
            "start": {"dateTime": "2026-11-16T09:00:00", "timeZone": "America/New_York"},
            "end": {"dateTime": "2026-11-16T09:30:00", "timeZone": "America/New_York"},
            "transparency": "opaque",
            "reminders": {"useDefault": false, "overrides": [{"method": "popup", "minutes": 15}]},
        }));
        assert_eq!(create_url(&base()), format!("{EVENTS}?sendUpdates=none"));
    }

    #[test]
    fn blank_title_no_reminder_location_and_free_show_as() {
        let mut d = base();
        d.subject = "  ".into();
        d.reminder_minutes = None;
        d.location = Some("Room 4".into());
        d.show_as = "free".into();
        let b = create_body(&d).unwrap();
        assert_eq!((b["summary"].as_str(), b["transparency"].as_str()), (Some("(no title)"), Some("transparent")));
        assert_eq!(b["reminders"], json!({"useDefault": false, "overrides": []}));
        assert_eq!(b["location"], "Room 4");
        assert!(b.get("attendees").is_none() && b.get("recurrence").is_none());
    }

    #[test]
    fn meeting_create_sends_updates_and_attendee_roles_without_response_status() {
        let mut d = base();
        d.attendees = vec![att("bo@x.com", "required", "Bo"), att("cy@x.com", "optional", ""), att("room@x.com", "resource", "Room")];
        let b = create_body(&d).unwrap();
        assert_eq!(b["attendees"], json!([
            {"email": "bo@x.com", "displayName": "Bo"},
            {"email": "cy@x.com", "optional": true},
            {"email": "room@x.com", "displayName": "Room", "resource": true},
        ]));
        assert!(b["attendees"].as_array().unwrap().iter().all(|a| a.get("responseStatus").is_none()));
        assert_eq!(create_url(&d), format!("{EVENTS}?sendUpdates=all"));
    }

    #[test]
    fn all_day_uses_dates_with_end_exclusive_and_a_one_day_minimum() {
        let mut d = base();
        d.is_all_day = true;
        d.start = dt(2026, 11, 16, 0, 0);
        d.end = dt(2026, 11, 18, 0, 0);
        let (s, e) = when(&d);
        assert_eq!((s, e), (json!({"date": "2026-11-16"}), json!({"date": "2026-11-18"})));
        d.end = d.start;
        assert_eq!(when(&d).1, json!({"date": "2026-11-17"}));
    }

    #[test]
    fn meet_request_goes_in_the_body_and_the_query() {
        let mut d = base();
        d.online_provider = Some("googleMeet".into());
        let b = create_body(&d).unwrap();
        assert_eq!(b["conferenceData"]["createRequest"]["requestId"], "ol-1f-002");
        assert!(create_url(&d).ends_with("&conferenceDataVersion=1"));
        d.online_provider = Some("teamsForBusiness".into());
        assert!(create_body(&d).unwrap().get("conferenceData").is_none());
    }

    #[test]
    fn rrule_table_matches_the_editor_patterns() {
        let z = "America/New_York";
        let rule = |p: Pattern, r: Range| rrule(&Recurrence { pattern: p, range: r }, z, false);
        assert_eq!(rule(Pattern::Daily { interval: 1 }, Range::NoEnd), "RRULE:FREQ=DAILY;INTERVAL=1");
        assert_eq!(
            rule(Pattern::Weekly { interval: 2, days: vec![Weekday::Tue, Weekday::Thu] }, Range::Numbered(10)),
            "RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=TU,TH;COUNT=10"
        );
        assert_eq!(
            rule(Pattern::AbsoluteMonthly { interval: 1, day_of_month: 2 }, Range::NoEnd),
            "RRULE:FREQ=MONTHLY;INTERVAL=1;BYMONTHDAY=2"
        );
        assert_eq!(
            rule(Pattern::RelativeMonthly { interval: 1, day: Weekday::Tue, index: "second".into() }, Range::NoEnd),
            "RRULE:FREQ=MONTHLY;INTERVAL=1;BYDAY=2TU"
        );
        assert_eq!(
            rule(Pattern::RelativeMonthly { interval: 1, day: Weekday::Fri, index: "last".into() }, Range::NoEnd),
            "RRULE:FREQ=MONTHLY;INTERVAL=1;BYDAY=-1FR"
        );
        // all-day series end: a plain date
        let all_day = rrule(&Recurrence { pattern: Pattern::Daily { interval: 1 }, range: Range::EndDate(NaiveDate::from_ymd_opt(2027, 1, 5).unwrap()) }, z, true);
        assert_eq!(all_day, "RRULE:FREQ=DAILY;INTERVAL=1;UNTIL=20270105");
    }

    #[test]
    fn series_create_carries_the_rrule_and_the_patch_sends_it_only_for_the_master() {
        let mut d = base();
        d.recurrence = Some(Recurrence { pattern: Pattern::Weekly { interval: 1, days: vec![Weekday::Mon] }, range: Range::NoEnd });
        assert_eq!(create_body(&d).unwrap()["recurrence"], json!(["RRULE:FREQ=WEEKLY;INTERVAL=1;BYDAY=MO"]));
        assert!(patch_body(&d, false).unwrap().get("recurrence").is_none(), "an occurrence cannot take a new pattern");
        assert!(patch_body(&d, true).unwrap().get("recurrence").is_some());
    }

    #[test]
    fn patch_always_sends_attendees_and_clears_location_and_targets_the_unscoped_id() {
        let d = base();
        let b = patch_body(&d, false).unwrap();
        assert_eq!((b["attendees"].clone(), b["location"].clone()), (json!([]), json!("")));
        assert!(b.get("conferenceData").is_none() && b.get("id").is_none());
        assert_eq!(patch_url("gmail-45ed5f", "gmail-45ed5f:abc123", &d), format!("{EVENTS}/abc123?sendUpdates=none"));
        let mut m = base();
        m.attendees = vec![att("bo@x.com", "required", "")];
        assert!(patch_url("gmail-45ed5f", "abc123", &m).ends_with("sendUpdates=all"));
    }

    #[test]
    fn event_id_is_google_safe_and_stable() {
        assert_eq!(event_id_for("ol-1f-002"), "omalookol1f002");
        assert_eq!(event_id_for("tx-7"), "omalookt7"); // x is outside a-v and is dropped
        assert!(event_id_for("").len() >= 5);
        assert_eq!(event_id_for("OL-AB"), event_id_for("ol-ab"));
        // characters outside a-v are dropped (w, x are fine, z is not)
        assert_eq!(event_id_for("zz-1"), "omalook1");
    }

    #[test]
    fn decline_sets_only_the_owners_response() {
        let ev = json!({"id": "x", "attendees": [
            {"email": "ann@x.com", "responseStatus": "accepted", "organizer": true, "self": false},
            {"email": "me@x.com", "self": true, "responseStatus": "needsAction", "optional": true},
            {"email": "bo@x.com", "responseStatus": "tentative", "self": false}]});
        let b = decline_body(&ev, "sorry, travelling").unwrap();
        assert_eq!(b["attendees"], json!([
            {"email": "ann@x.com", "responseStatus": "accepted"},
            {"email": "me@x.com", "optional": true, "responseStatus": "declined", "comment": "sorry, travelling"},
            {"email": "bo@x.com", "responseStatus": "tentative"},
        ]));
        assert!(decline_body(&json!({"attendees": [{"email": "a@x.com"}]}), "").is_none(), "not on the guest list");
    }

    #[test]
    fn already_gone_is_done_and_other_errors_are_not() {
        let gone = OmarchyError::HttpError("Google API error: 410 Gone — {}".into());
        let missing = OmarchyError::HttpError("Google API error: 404 Not Found — {}".into());
        let other = OmarchyError::HttpError("Google API error: 403 Forbidden — insufficientPermissions".into());
        assert!(is_gone(&gone) && is_gone(&missing) && !is_gone(&other));
        assert!(is_conflict(&OmarchyError::HttpError("Google API error: 409 Conflict — {}".into())));
    }

    #[test]
    fn freebusy_request_is_utc_and_lists_the_people() {
        let from = Utc.with_ymd_and_hms(2026, 11, 16, 14, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2026, 11, 17, 4, 0, 0).unwrap();
        let b = freebusy_body(&["a@x.com".to_string()], from, to);
        assert_eq!(b, json!({"timeMin": "2026-11-16T14:00:00+00:00", "timeMax": "2026-11-17T04:00:00+00:00", "items": [{"id": "a@x.com"}]}));
    }

    #[test]
    fn freebusy_answer_becomes_a_digit_per_slot_and_errors_are_unknown() {
        // Compare in UTC so the test does not depend on the machine's zone: the parser's local times
        // are converted the same way the request is.
        let start = dt(2026, 11, 16, 9, 0);
        let end = dt(2026, 11, 16, 10, 0);
        let s = local_to_utc(start);
        let at = |m: i64| (s + Duration::minutes(m)).to_rfc3339();
        let json = json!({"calendars": {
            "A@x.com": {"busy": [{"start": at(15), "end": at(30)}, {"start": at(40), "end": at(50)}]},
            "b@x.com": {"busy": [], "errors": [{"domain": "global", "reason": "notFound"}]},
        }});
        let out = parse_freebusy(&json, &["a@x.com".to_string(), "b@x.com".to_string(), "c@x.com".to_string()], start, end, 15);
        assert_eq!(out[0].availability_view, "0222", "busy 15-30 and 40-50 touch slots 1, 2, 3; case-insensitive match");
        assert!(out[0].error.is_none());
        assert_eq!(out[1].error.as_deref(), Some("free/busy not available (notFound)"));
        assert!(out[1].availability_view.is_empty());
        assert!(out[2].error.is_some(), "no answer at all is unknown");
    }

    #[test]
    fn freebusy_view_feeds_the_finder_busy_reader() {
        use crate::calendar_finder::busy_from_view;
        let start = dt(2026, 11, 16, 9, 0);
        let end = dt(2026, 11, 16, 10, 0);
        let s = local_to_utc(start);
        let json = json!({"calendars": {"a@x.com": {"busy": [{"start": (s + Duration::minutes(15)).to_rfc3339(), "end": (s + Duration::minutes(45)).to_rfc3339()}]}}});
        let view = parse_freebusy(&json, &["a@x.com".to_string()], start, end, 15)[0].availability_view.clone();
        assert_eq!(view, "0220");
        let busy = busy_from_view(&view, start, 15, end).unwrap();
        assert_eq!(busy.len(), 1, "two busy slots coalesce into one interval");
    }
}
