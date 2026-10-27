//! Microsoft Graph calendar provider (mirrors providers/graph.rs for mail)
//!
//! Pagination follows @odata.nextLink exactly like fetch_all_folder_messages.

use crate::auth::AuthManager;
use crate::errors::{OmarchyError, Result};
use crate::models::CalendarEvent;
use async_trait::async_trait;
use chrono::{Local, NaiveDate, TimeZone, Utc};
use log::{debug, error};
use tokio::sync::Mutex;

const SELECT: &str = "id,subject,body,start,end,isAllDay,type,seriesMasterId,location,attendees,organizer,isOrganizer,responseStatus,showAs,isOnlineMeeting,onlineMeeting,onlineMeetingProvider,isReminderOn,reminderMinutesBeforeStart,webLink,sensitivity,recurrence";

/// Graph attendees -> `[{name,email,role,response}]` JSON ("" when there are none).
pub(crate) fn attendees_json(attendees: &serde_json::Value) -> String {
    let list: Vec<serde_json::Value> = attendees
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| serde_json::json!({
            "name": a["emailAddress"]["name"].as_str().unwrap_or(""),
            "email": a["emailAddress"]["address"].as_str().unwrap_or(""),
            "role": a["type"].as_str().unwrap_or("required"),
            "response": a["status"]["response"].as_str().unwrap_or("none"),
        }))
        .collect();
    if list.is_empty() { String::new() } else { serde_json::Value::Array(list).to_string() }
}

#[async_trait]
pub trait CalendarProvider: Send + Sync {
    /// Fetch the most recent events (by start time, newest first)
    async fn fetch_events(&self, limit: usize) -> Result<Vec<CalendarEvent>>;

    /// Fetch ALL events with pagination, streaming batches (~50) to a channel.
    async fn fetch_all_events(
        &self,
        tx: tokio::sync::mpsc::Sender<Vec<CalendarEvent>>,
    ) -> Result<usize>;

    /// Every event overlapping `[from, to)` with recurring series EXPANDED into their individual
    /// occurrences (Graph `calendarView`), following all pages. `/me/events` only returns each
    /// series once, as its master.
    async fn fetch_window(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<CalendarEvent>>;

    async fn is_token_valid(&self) -> Result<bool>;

    /// Create an event (docs/PLAN-calendar-events.md section 4). Providers that cannot yet say so.
    async fn create_event(&self, _draft: &crate::calendar_write::EventDraft) -> Result<crate::calendar_write::CreatedEvent> {
        Err(OmarchyError::HttpError("creating events is not supported for this account type yet".to_string()))
    }
}

/// Local midnight of `date` as an RFC 3339 instant with its UTC offset.
pub(crate) fn local_midnight(date: NaiveDate) -> String {
    let naive = date.and_hms_opt(0, 0, 0).expect("midnight is valid");
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) | chrono::LocalResult::Ambiguous(dt, _) => dt.to_rfc3339(),
        chrono::LocalResult::None => Utc.from_utc_datetime(&naive).to_rfc3339(),
    }
}

/// `calendarView` URL for `[from, to)`. The offsets contain '+', so the instants are percent-encoded.
pub(crate) fn window_url(from: NaiveDate, to: NaiveDate) -> String {
    format!(
        "https://graph.microsoft.com/v1.0/me/calendarView?startDateTime={}&endDateTime={}&$select={}&$top=50&$orderby=start/dateTime",
        urlencoding::encode(&local_midnight(from)),
        urlencoding::encode(&local_midnight(to)),
        SELECT
    )
}

pub struct GraphCalendarProvider {
    auth: Mutex<AuthManager>,
    /// IANA zone requested from Graph so start/end arrive as local wall-clock
    time_zone: String,
}

impl GraphCalendarProvider {
    pub fn new(auth: AuthManager) -> Self {
        Self { auth: Mutex::new(auth), time_zone: local_time_zone() }
    }

    async fn get_token(&self) -> Result<String> {
        let mut auth = self.auth.lock().await;
        let token = auth
            .get_token()
            .map_err(|e| OmarchyError::AuthError(format!("Failed to get token: {}", e)))?;
        if token.is_empty() {
            return Err(OmarchyError::AuthError("No access token".to_string()));
        }
        Ok(token)
    }

    async fn get_page(&self, client: &reqwest::Client, token: &str, url: &str) -> Result<serde_json::Value> {
        let response = client
            .get(url)
            .header("Authorization", format!("Bearer {}", token))
            // local-time start/end + plain-text body
            .header(
                "Prefer",
                format!("outlook.timezone=\"{}\", outlook.body-content-type=\"text\"", self.time_zone),
            )
            .send()
            .await
            .map_err(|e| OmarchyError::HttpError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!("Graph calendar API returned status: {}", status);
            error!("Graph calendar API error body: {}", body);
            return Err(OmarchyError::HttpError(format!("Graph API error: {} — {}", status, body)));
        }
        let text = response.text().await.map_err(|e| OmarchyError::HttpError(e.to_string()))?;
        serde_json::from_str(&text).map_err(|e| OmarchyError::HttpError(e.to_string()))
    }

    fn parse_event(&self, ev: &serde_json::Value) -> CalendarEvent {
        // Graph returns "2026-10-03T09:00:00.0000000" — keep "YYYY-MM-DDTHH:MM:SS"
        let stamp = |v: &serde_json::Value| -> String {
            v["dateTime"].as_str().unwrap_or("").chars().take(19).collect()
        };
        CalendarEvent {
            id: ev["id"].as_str().unwrap_or("").to_string(),
            subject: ev["subject"].as_str().unwrap_or("(no title)").to_string(),
            body: ev["body"]["content"].as_str().unwrap_or("").to_string(),
            start: stamp(&ev["start"]),
            end: stamp(&ev["end"]),
            is_all_day: ev["isAllDay"].as_bool().unwrap_or(false),
            time_zone: ev["start"]["timeZone"].as_str().unwrap_or(&self.time_zone).to_string(),
            event_type: ev["type"].as_str().unwrap_or("singleInstance").to_string(),
            series_master_id: ev["seriesMasterId"].as_str().map(|s| s.to_string()),
            location: ev["location"]["displayName"].as_str().unwrap_or("").to_string(),
            organizer_name: ev["organizer"]["emailAddress"]["name"].as_str().unwrap_or("").to_string(),
            organizer_email: ev["organizer"]["emailAddress"]["address"].as_str().unwrap_or("").to_string(),
            is_organizer: ev["isOrganizer"].as_bool().unwrap_or(false),
            attendees_json: attendees_json(&ev["attendees"]),
            my_response: ev["responseStatus"]["response"].as_str().unwrap_or("").to_string(),
            show_as: ev["showAs"].as_str().unwrap_or("").to_string(),
            online_provider: if ev["isOnlineMeeting"].as_bool().unwrap_or(false) || ev["onlineMeeting"].is_object() {
                ev["onlineMeetingProvider"].as_str().unwrap_or("").to_string()
            } else { String::new() },
            online_url: ev["onlineMeeting"]["joinUrl"].as_str().unwrap_or("").to_string(),
            reminder_minutes: if ev["isReminderOn"].as_bool().unwrap_or(false) { ev["reminderMinutesBeforeStart"].as_i64() } else { None },
            web_link: ev["webLink"].as_str().unwrap_or("").to_string(),
            sensitivity: ev["sensitivity"].as_str().unwrap_or("").to_string(),
            recurrence_json: if ev["recurrence"].is_object() { ev["recurrence"].to_string() } else { String::new() },
            calendar_id: String::new(),
        }
    }

    fn parse_page(&self, json: &serde_json::Value) -> Vec<CalendarEvent> {
        json["value"]
            .as_array()
            .map(|vals| vals.iter().map(|v| self.parse_event(v)).filter(|e| !e.id.is_empty() && !e.start.is_empty()).collect())
            .unwrap_or_default()
    }
}

#[async_trait]
impl CalendarProvider for GraphCalendarProvider {
    async fn fetch_events(&self, limit: usize) -> Result<Vec<CalendarEvent>> {
        debug!("Fetching {} calendar events from Graph API", limit);
        let token = self.get_token().await?;
        let client = reqwest::Client::new();
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/events?$top={}&$select={}&$orderby=start/dateTime desc",
            limit, SELECT
        );
        let json = self.get_page(&client, &token, &url).await?;
        Ok(self.parse_page(&json))
    }

    async fn fetch_all_events(
        &self,
        tx: tokio::sync::mpsc::Sender<Vec<CalendarEvent>>,
    ) -> Result<usize> {
        let token = self.get_token().await?;
        let client = reqwest::Client::new();

        let mut next_url = Some(format!(
            "https://graph.microsoft.com/v1.0/me/events?$top=50&$select={}&$orderby=start/dateTime desc",
            SELECT
        ));
        let mut total = 0usize;

        while let Some(url) = next_url.take() {
            let json = self.get_page(&client, &token, &url).await?;
            let batch = self.parse_page(&json);
            total += batch.len();

            if !batch.is_empty() && tx.send(batch).await.is_err() {
                debug!("Calendar pagination receiver dropped, stopping");
                break;
            }

            next_url = json["@odata.nextLink"].as_str().map(|s| s.to_string());
            tokio::task::yield_now().await;
        }

        debug!("Completed full calendar sync: {} events total", total);
        Ok(total)
    }

    async fn fetch_window(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<CalendarEvent>> {
        let token = self.get_token().await?;
        let client = reqwest::Client::new();
        let mut next_url = Some(window_url(from, to));
        let mut all = Vec::new();
        while let Some(url) = next_url.take() {
            let json = self.get_page(&client, &token, &url).await?;
            all.extend(self.parse_page(&json));
            next_url = json["@odata.nextLink"].as_str().map(|s| s.to_string());
            tokio::task::yield_now().await;
        }
        debug!("calendarView {}..{}: {} events", from, to, all.len());
        Ok(all)
    }

    async fn create_event(&self, draft: &crate::calendar_write::EventDraft) -> Result<crate::calendar_write::CreatedEvent> {
        let body = draft.to_graph_json().map_err(|e| OmarchyError::HttpError(format!("Invalid event: {}", e)))?;
        let token = self.get_token().await?;
        let response = reqwest::Client::new()
            .post("https://graph.microsoft.com/v1.0/me/events")
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .header("Prefer", format!("outlook.timezone=\"{}\", outlook.body-content-type=\"text\"", self.time_zone))
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| OmarchyError::HttpError(e.to_string()))?;
        let status = response.status();
        let text = response.text().await.map_err(|e| OmarchyError::HttpError(e.to_string()))?;
        if !status.is_success() {
            // Phase 0: the same transactionId with a different body is a 400 that names the event the
            // first attempt made. That event exists, so a retried create is a success, not an error.
            if let Some(id) = crate::calendar_write::existing_event_id(&text) {
                return Ok(crate::calendar_write::CreatedEvent { id, ..Default::default() });
            }
            return Err(OmarchyError::HttpError(format!("Graph API error: {} — {}", status, text)));
        }
        let raw: serde_json::Value = serde_json::from_str(&text).map_err(|e| OmarchyError::HttpError(e.to_string()))?;
        Ok(crate::calendar_write::CreatedEvent {
            id: raw["id"].as_str().unwrap_or("").to_string(),
            web_link: raw["webLink"].as_str().unwrap_or("").to_string(),
            join_url: raw["onlineMeeting"]["joinUrl"].as_str().unwrap_or("").to_string(),
            is_series: raw["type"].as_str() == Some("seriesMaster"),
            raw,
        })
    }

    async fn is_token_valid(&self) -> Result<bool> {
        let mut auth = self.auth.lock().await;
        let token = auth
            .get_token()
            .map_err(|_| OmarchyError::AuthError("Token check failed".to_string()));
        Ok(token.is_ok() && !token.unwrap_or_default().is_empty())
    }
}

/// IANA time zone of this machine (from the /etc/localtime symlink), else UTC.
pub fn local_time_zone() -> String {
    std::fs::read_link("/etc/localtime")
        .ok()
        .and_then(|p| {
            let s = p.to_string_lossy().to_string();
            s.split_once("zoneinfo/").map(|(_, tz)| tz.to_string())
        })
        .filter(|tz| !tz.is_empty())
        .unwrap_or_else(|| "UTC".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_url_encodes_offsets_and_selects_type_fields() {
        let url = window_url(NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(), NaiveDate::from_ymd_opt(2026, 12, 30).unwrap());
        assert!(url.starts_with("https://graph.microsoft.com/v1.0/me/calendarView?startDateTime=2026-10-01T00%3A00%3A00"), "{}", url);
        let query = url.split_once('?').unwrap().1;
        assert!(!query.contains('+'), "a raw '+' would be read as a space: {}", url);
        assert!(url.contains("type,seriesMasterId"));
    }

    #[test]
    fn graph_event_types_are_parsed() {
        let p = GraphCalendarProvider::new(AuthManager::new());
        let occ = p.parse_event(&serde_json::json!({
            "id": "o", "subject": "Standup", "start": {"dateTime": "2026-10-05T09:00:00.0000000", "timeZone": "Eastern Standard Time"},
            "end": {"dateTime": "2026-10-05T09:30:00.0000000"}, "isAllDay": false, "type": "occurrence", "seriesMasterId": "M"
        }));
        assert_eq!((occ.event_type.as_str(), occ.series_master_id.as_deref()), ("occurrence", Some("M")));
        let single = p.parse_event(&serde_json::json!({"id": "s", "start": {"dateTime": "2026-10-05T09:00:00"}, "end": {"dateTime": "2026-10-05T10:00:00"}}));
        assert_eq!(single.event_type, "singleInstance");
    }

    #[test]
    fn graph_detail_fields_are_parsed() {
        let p = GraphCalendarProvider::new(AuthManager::new());
        let e = p.parse_event(&serde_json::json!({
            "id": "x", "subject": "Review", "start": {"dateTime": "2026-10-05T09:00:00.0000000"}, "end": {"dateTime": "2026-10-05T10:00:00.0000000"},
            "location": {"displayName": "Room 4"}, "isOrganizer": false, "responseStatus": {"response": "tentativelyAccepted"},
            "organizer": {"emailAddress": {"name": "Ann", "address": "ann@contoso.com"}},
            "attendees": [{"type": "required", "status": {"response": "accepted"}, "emailAddress": {"name": "Bob", "address": "bob@contoso.com"}},
                          {"type": "optional", "status": {"response": "none"}, "emailAddress": {"address": "cy@contoso.com"}}],
            "showAs": "busy", "isOnlineMeeting": true, "onlineMeetingProvider": "teamsForBusiness",
            "onlineMeeting": {"joinUrl": "https://teams.microsoft.com/l/meetup-join/q"},
            "isReminderOn": true, "reminderMinutesBeforeStart": 15, "webLink": "https://outlook.office.com/x", "sensitivity": "normal",
            "recurrence": {"pattern": {"type": "weekly"}}
        }));
        assert_eq!((e.location.as_str(), e.organizer_email.as_str(), e.my_response.as_str()), ("Room 4", "ann@contoso.com", "tentativelyAccepted"));
        assert_eq!((e.show_as.as_str(), e.online_provider.as_str(), e.reminder_minutes), ("busy", "teamsForBusiness", Some(15)));
        assert_eq!(e.online_url, "https://teams.microsoft.com/l/meetup-join/q");
        let att: serde_json::Value = serde_json::from_str(&e.attendees_json).unwrap();
        assert_eq!(att.as_array().unwrap().len(), 2);
        assert_eq!((att[0]["email"].as_str(), att[0]["response"].as_str()), (Some("bob@contoso.com"), Some("accepted")));
        assert_eq!((att[1]["name"].as_str(), att[1]["role"].as_str()), (Some(""), Some("optional")));
        assert!(e.recurrence_json.contains("weekly"));
        // a reminder switched off, no attendees, no online meeting: all empty
        let bare = p.parse_event(&serde_json::json!({"id": "y", "start": {"dateTime": "2026-10-05T09:00:00"}, "end": {"dateTime": "2026-10-05T10:00:00"},
            "isReminderOn": false, "reminderMinutesBeforeStart": 15, "isOnlineMeeting": false, "onlineMeeting": null, "attendees": []}));
        assert_eq!((bare.reminder_minutes, bare.attendees_json.as_str(), bare.online_provider.as_str(), bare.online_url.as_str()), (None, "", "", ""));
    }
}
