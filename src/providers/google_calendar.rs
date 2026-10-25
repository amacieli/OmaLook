//! Google Calendar provider (Calendar API v3, the account's primary calendar) behind the same
//! `CalendarProvider` trait as Graph.
//!
//! Mapping to the Graph-shaped model the daemon and UI use:
//! * `events.list` (singleEvents=false) returns each recurring series once → `seriesMaster`
//!   (stored, never displayed), exactly like `/me/events`.
//! * `events.list` with `singleEvents=true` over a window expands series server-side → the
//!   `occurrence` rows (`calendarView` on Graph).
//! * Times are converted to local wall-clock `YYYY-MM-DDTHH:MM:SS` in this machine's zone;
//!   all-day events keep their dates (end exclusive on both services).
//! Google event ids are only unique per calendar, so ids are stored as `<account id>:<id>`.

use super::google_api::GoogleApi;
use crate::accounts::scoped;
use crate::errors::Result;
use crate::models::CalendarEvent;
use async_trait::async_trait;
use chrono::{DateTime, Local, NaiveDate};
use log::debug;

const EVENTS: &str = "https://www.googleapis.com/calendar/v3/calendars/primary/events";
const PAGE_SIZE: usize = 250;

pub struct GoogleCalendarProvider {
    account_id: String,
    api: GoogleApi,
    time_zone: String,
}

/// `{dateTime|date}` → local wall-clock "YYYY-MM-DDTHH:MM:SS".
pub(crate) fn local_stamp(v: &serde_json::Value) -> Option<String> {
    if let Some(dt) = v["dateTime"].as_str() {
        let parsed = DateTime::parse_from_rfc3339(dt).ok()?;
        return Some(parsed.with_timezone(&Local).format("%Y-%m-%dT%H:%M:%S").to_string());
    }
    let d = NaiveDate::parse_from_str(v["date"].as_str()?, "%Y-%m-%d").ok()?;
    Some(format!("{}T00:00:00", d))
}

/// Google attendees -> `[{name,email,role,response}]` JSON in the same shape as the Graph provider.
fn google_attendees_json(attendees: &serde_json::Value) -> String {
    let list: Vec<serde_json::Value> = attendees.as_array().into_iter().flatten().map(|a| serde_json::json!({
        "name": a["displayName"].as_str().unwrap_or(""),
        "email": a["email"].as_str().unwrap_or(""),
        "role": if a["resource"].as_bool().unwrap_or(false) { "resource" } else if a["optional"].as_bool().unwrap_or(false) { "optional" } else { "required" },
        "response": match a["responseStatus"].as_str().unwrap_or("needsAction") { "needsAction" => "none", "tentative" => "tentativelyAccepted", o => o },
    })).collect();
    if list.is_empty() { String::new() } else { serde_json::Value::Array(list).to_string() }
}

/// Google event resource → `CalendarEvent` (None for cancelled or untimed entries).
/// `expanded` = the list call used `singleEvents=true`.
pub(crate) fn parse_event(account_id: &str, time_zone: &str, ev: &serde_json::Value) -> Option<CalendarEvent> {
    if ev["status"].as_str() == Some("cancelled") {
        return None;
    }
    let id = ev["id"].as_str().filter(|s| !s.is_empty())?;
    let (start, end) = (local_stamp(&ev["start"])?, local_stamp(&ev["end"])?);
    let master = ev["recurringEventId"].as_str().map(|m| scoped(account_id, m));
    let event_type = if master.is_some() {
        "occurrence" // instances (incl. modified ones) of a series
    } else if ev["recurrence"].is_array() {
        "seriesMaster"
    } else {
        "singleInstance"
    };
    Some(CalendarEvent {
        id: scoped(account_id, id),
        subject: ev["summary"].as_str().filter(|s| !s.is_empty()).unwrap_or("(no title)").to_string(),
        body: ev["description"].as_str().unwrap_or("").to_string(),
        start,
        end,
        is_all_day: ev["start"]["date"].is_string(),
        time_zone: time_zone.to_string(),
        event_type: event_type.to_string(),
        series_master_id: master,
        location: ev["location"].as_str().unwrap_or("").to_string(),
        organizer_name: ev["organizer"]["displayName"].as_str().unwrap_or("").to_string(),
        organizer_email: ev["organizer"]["email"].as_str().unwrap_or("").to_string(),
        is_organizer: ev["organizer"]["self"].as_bool().unwrap_or(false),
        attendees_json: google_attendees_json(&ev["attendees"]),
        my_response: ev["attendees"].as_array().into_iter().flatten()
            .find(|a| a["self"].as_bool().unwrap_or(false))
            .and_then(|a| a["responseStatus"].as_str())
            .map(|r| match r { "needsAction" => "notResponded", "tentative" => "tentativelyAccepted", o => o })
            .unwrap_or("").to_string(),
        show_as: match ev["transparency"].as_str() { Some("transparent") => "free", _ => "busy" }.to_string(),
        online_provider: if ev["hangoutLink"].is_string() { "googleMeet".to_string() } else { String::new() },
        online_url: ev["hangoutLink"].as_str().unwrap_or("").to_string(),
        reminder_minutes: None,
        web_link: ev["htmlLink"].as_str().unwrap_or("").to_string(),
        sensitivity: ev["visibility"].as_str().unwrap_or("").to_string(),
        recurrence_json: String::new(),
        calendar_id: String::new(),
    })
}

impl GoogleCalendarProvider {
    pub fn new(account_id: &str) -> Self {
        Self { account_id: account_id.to_string(), api: GoogleApi::new(account_id), time_zone: super::calendar::local_time_zone() }
    }

    fn parse_page(&self, json: &serde_json::Value) -> Vec<CalendarEvent> {
        json["items"].as_array().into_iter().flatten().filter_map(|e| parse_event(&self.account_id, &self.time_zone, e)).collect()
    }
}

#[async_trait]
impl super::CalendarProvider for GoogleCalendarProvider {
    /// The most recently modified events (Google cannot order series masters by start time).
    async fn fetch_events(&self, limit: usize) -> Result<Vec<CalendarEvent>> {
        let url = format!("{}?maxResults={}&orderBy=updated&singleEvents=false&showDeleted=false", EVENTS, limit.clamp(1, 2500));
        Ok(self.parse_page(&self.api.get(&url).await?))
    }

    async fn fetch_all_events(&self, tx: tokio::sync::mpsc::Sender<Vec<CalendarEvent>>) -> Result<usize> {
        let (mut total, mut token) = (0usize, None::<String>);
        loop {
            let mut url = format!("{}?maxResults={}&singleEvents=false&showDeleted=false", EVENTS, PAGE_SIZE);
            if let Some(t) = &token {
                url.push_str(&format!("&pageToken={}", urlencoding::encode(t)));
            }
            let json = self.api.get(&url).await?;
            let batch = self.parse_page(&json);
            total += batch.len();
            if !batch.is_empty() && tx.send(batch).await.is_err() {
                debug!("Google calendar pagination receiver dropped, stopping");
                break;
            }
            match json["nextPageToken"].as_str() {
                Some(t) => token = Some(t.to_string()),
                None => break,
            }
            tokio::task::yield_now().await;
        }
        debug!("Completed full Google calendar sync: {} events", total);
        Ok(total)
    }

    async fn fetch_window(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<CalendarEvent>> {
        let (mut all, mut token) = (Vec::new(), None::<String>);
        loop {
            let mut url = format!(
                "{}?singleEvents=true&orderBy=startTime&showDeleted=false&maxResults={}&timeMin={}&timeMax={}",
                EVENTS, PAGE_SIZE,
                urlencoding::encode(&super::calendar::local_midnight(from)),
                urlencoding::encode(&super::calendar::local_midnight(to)),
            );
            if let Some(t) = &token {
                url.push_str(&format!("&pageToken={}", urlencoding::encode(t)));
            }
            let json = self.api.get(&url).await?;
            all.extend(self.parse_page(&json));
            match json["nextPageToken"].as_str() {
                Some(t) => token = Some(t.to_string()),
                None => break,
            }
            tokio::task::yield_now().await;
        }
        debug!("Google events {}..{}: {}", from, to, all.len());
        Ok(all)
    }

    async fn is_token_valid(&self) -> Result<bool> {
        self.api.is_token_valid().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn timed_event_is_converted_to_local_wall_clock() {
        let ev = json!({"id": "e1", "summary": "Standup", "description": "daily",
            "start": {"dateTime": "2026-10-05T13:00:00Z"}, "end": {"dateTime": "2026-10-05T13:30:00Z"}});
        let e = parse_event("gmail-aaaaaa", "America/New_York", &ev).unwrap();
        let expect = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Local).format("%Y-%m-%dT%H:%M:%S").to_string();
        assert_eq!(e.start, expect("2026-10-05T13:00:00Z"));
        assert_eq!(e.end, expect("2026-10-05T13:30:00Z"));
        assert_eq!((e.id.as_str(), e.subject.as_str(), e.body.as_str()), ("gmail-aaaaaa:e1", "Standup", "daily"));
        assert!(!e.is_all_day);
        assert_eq!(e.event_type, "singleInstance");
    }

    #[test]
    fn google_detail_fields_are_parsed() {
        let ev = json!({"id": "e3", "summary": "Sync", "location": "HQ", "start": {"dateTime": "2026-10-05T13:00:00Z"}, "end": {"dateTime": "2026-10-05T13:30:00Z"},
            "organizer": {"email": "me@gmail.com", "self": true}, "hangoutLink": "https://meet.google.com/abc-defg-hij", "transparency": "transparent",
            "htmlLink": "https://calendar.google.com/e", "attendees": [
                {"email": "me@gmail.com", "self": true, "responseStatus": "needsAction"},
                {"email": "r@x.com", "displayName": "Room", "resource": true, "responseStatus": "accepted"},
                {"email": "o@x.com", "optional": true, "responseStatus": "tentative"}]});
        let e = parse_event("g-1", "UTC", &ev).unwrap();
        assert_eq!((e.location.as_str(), e.is_organizer, e.my_response.as_str(), e.show_as.as_str()), ("HQ", true, "notResponded", "free"));
        assert_eq!((e.online_provider.as_str(), e.online_url.as_str()), ("googleMeet", "https://meet.google.com/abc-defg-hij"));
        let a: serde_json::Value = serde_json::from_str(&e.attendees_json).unwrap();
        assert_eq!((a[1]["role"].as_str(), a[2]["role"].as_str(), a[2]["response"].as_str()), (Some("resource"), Some("optional"), Some("tentativelyAccepted")));
    }

    #[test]
    fn all_day_event_keeps_its_dates() {
        let ev = json!({"id": "e2", "start": {"date": "2026-10-08"}, "end": {"date": "2026-10-09"}});
        let e = parse_event("a-1", "UTC", &ev).unwrap();
        assert_eq!((e.start.as_str(), e.end.as_str(), e.is_all_day), ("2026-10-08T00:00:00", "2026-10-09T00:00:00", true));
        assert_eq!(e.subject, "(no title)");
    }

    #[test]
    fn series_masters_and_occurrences_are_typed_like_graph() {
        let t = json!({"dateTime": "2026-10-05T13:00:00Z"});
        let master = parse_event("a-1", "UTC", &json!({"id": "S", "recurrence": ["RRULE:FREQ=WEEKLY"], "start": t, "end": t})).unwrap();
        assert_eq!((master.event_type.as_str(), master.series_master_id.as_deref()), ("seriesMaster", None));
        let occ = parse_event("a-1", "UTC", &json!({"id": "S_20261012T130000Z", "recurringEventId": "S", "start": t, "end": t})).unwrap();
        assert_eq!((occ.event_type.as_str(), occ.series_master_id.as_deref()), ("occurrence", Some("a-1:S")));
    }

    #[test]
    fn cancelled_or_untimed_events_are_dropped_and_ids_are_per_account() {
        let t = json!({"dateTime": "2026-10-05T13:00:00Z"});
        assert!(parse_event("a-1", "UTC", &json!({"id": "x", "status": "cancelled", "start": t, "end": t})).is_none());
        assert!(parse_event("a-1", "UTC", &json!({"id": "x"})).is_none());
        let a = parse_event("gmail-aaaaaa", "UTC", &json!({"id": "x", "start": t, "end": t})).unwrap();
        let b = parse_event("gmail-bbbbbb", "UTC", &json!({"id": "x", "start": t, "end": t})).unwrap();
        assert_ne!(a.id, b.id);
    }
}
