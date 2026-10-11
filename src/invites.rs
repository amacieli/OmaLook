//! Meeting invitations (Microsoft Graph only): what kind of calendar message a mail is, what
//! the user's answer to it currently is, and answering it.
//!
//! Graph keeps the calendar item (`event`) separate from the mail (`eventMessage`). The mail only
//! says what *kind* of message it is (`meetingMessageType`); the answer lives on the event
//! (`responseStatus`) and is changed with `POST /me/events/{id}/accept|tentativelyAccept|decline`.
//! There is no accept action on messages.

use serde_json::{json, Value};

/// The Graph `$select`/`$expand` that returns the message kind and its event in one request. The
/// `microsoft.graph.eventMessage/` cast is required: plain `$select=meetingMessageType` is a 400.
pub const INFO_QUERY: &str = "$select=id,microsoft.graph.eventMessage/meetingMessageType\
&$expand=microsoft.graph.eventMessage/event($select=id,subject,start,end,isAllDay,location,\
isCancelled,isOrganizer,responseStatus,organizer)";

/// The response words the UI uses, mapped to the Graph event action and the `responseStatus` the
/// event is left with. `None` for anything else.
pub fn response_op(choice: &str) -> Option<(&'static str, &'static str)> {
    match choice {
        "accept" => Some(("accept", "accepted")),
        "tentative" => Some(("tentativelyAccept", "tentativelyAccepted")),
        "decline" => Some(("decline", "declined")),
        _ => None,
    }
}

/// What the backend knows about one message as a calendar item.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InviteInfo {
    /// `meetingRequest` | `meetingCancelled` | `meetingAccepted` | `meetingTentativelyAccepted` |
    /// `meetingDeclined` | `none` (ordinary mail).
    pub kind: String,
    pub event_id: Option<String>,
    /// The user's own answer on the event: `notResponded` | `accepted` | `tentativelyAccepted` |
    /// `declined` | `organizer` | `none` | `` (no event).
    pub response: String,
    pub is_organizer: bool,
    pub is_cancelled: bool,
    pub all_day: bool,
    pub subject: String,
    pub organizer: String,
    pub location: String,
    /// Raw Graph start/end (`dateTime`, `timeZone`), kept so the display text is built on demand.
    pub start: Option<(String, String)>,
    pub end: Option<(String, String)>,
}

fn dt(v: &Value) -> Option<(String, String)> {
    let d = v["dateTime"].as_str()?;
    Some((d.to_string(), v["timeZone"].as_str().unwrap_or("UTC").to_string()))
}

impl InviteInfo {
    pub fn none() -> Self {
        Self { kind: "none".into(), ..Default::default() }
    }

    /// Parse the reply of `GET /me/messages/{id}?INFO_QUERY`.
    pub fn from_graph(v: &Value) -> Self {
        let kind = v["meetingMessageType"].as_str().filter(|k| !k.is_empty()).unwrap_or("none").to_string();
        let mut info = Self { kind, ..Default::default() };
        let ev = &v["event"];
        if ev.is_object() {
            info.event_id = ev["id"].as_str().map(str::to_string);
            info.response = ev["responseStatus"]["response"].as_str().unwrap_or("none").to_string();
            info.is_organizer = ev["isOrganizer"].as_bool().unwrap_or(false);
            info.is_cancelled = ev["isCancelled"].as_bool().unwrap_or(false);
            info.all_day = ev["isAllDay"].as_bool().unwrap_or(false);
            info.subject = ev["subject"].as_str().unwrap_or("").to_string();
            info.location = ev["location"]["displayName"].as_str().unwrap_or("").to_string();
            let o = &ev["organizer"]["emailAddress"];
            let (name, addr) = (o["name"].as_str().unwrap_or(""), o["address"].as_str().unwrap_or(""));
            info.organizer = if name.is_empty() { addr.to_string() } else { name.to_string() };
            info.start = dt(&ev["start"]);
            info.end = dt(&ev["end"]);
        }
        info
    }

    /// Anything other than ordinary mail.
    pub fn is_calendar_mail(&self) -> bool {
        self.kind != "none"
    }

    /// Can the user answer it: an open request whose event still exists and is not their own
    /// meeting. (`meetingAccepted` and friends are *other people's* answers to the user's own
    /// meetings, and a cancelled meeting cannot be answered.)
    pub fn actionable(&self) -> bool {
        self.kind == "meetingRequest" && self.event_id.is_some() && !self.is_organizer && !self.is_cancelled
    }

    /// Start date of the event in local time, for refreshing the calendar around it.
    pub fn start_date(&self) -> Option<chrono::NaiveDate> {
        self.start.as_ref().and_then(|(d, z)| local_datetime(d, z, self.all_day)).map(|d| d.date())
    }

    /// `Tue Oct 14, 2:00 – 3:00 PM` (local time), or `Tue Oct 14 · all day`.
    pub fn when_text(&self) -> String {
        let (Some((s, sz)), Some((e, ez))) = (&self.start, &self.end) else { return String::new() };
        let (Some(start), Some(end)) = (local_datetime(s, sz, self.all_day), local_datetime(e, ez, self.all_day)) else {
            return String::new();
        };
        if self.all_day {
            return format!("{} · all day", start.format("%a %b %-d"));
        }
        if start.date() == end.date() {
            format!("{}, {} – {}", start.format("%a %b %-d"), start.format("%-I:%M"), end.format("%-I:%M %p"))
        } else {
            format!("{} – {}", start.format("%a %b %-d, %-I:%M %p"), end.format("%a %b %-d, %-I:%M %p"))
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "type": self.kind,
            "actionable": self.actionable(),
            "response": self.response,
            "organizer": self.organizer,
            "isOrganizer": self.is_organizer,
            "cancelled": self.is_cancelled || self.kind == "meetingCancelled",
            "hasEvent": self.event_id.is_some(),
            "subject": self.subject,
            "when": self.when_text(),
            "location": short(&self.location, 60),
        })
    }
}

/// `s` cut to `max` characters with an ellipsis (online-meeting locations are long URLs).
fn short(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { format!("{}…", s.chars().take(max - 1).collect::<String>()) }
}

/// Parse a Graph `dateTime` (`2026-09-11T01:00:00.0000000`) in `zone` into local wall-clock time.
/// Only UTC needs converting: events are returned in UTC unless a `Prefer` zone is sent. All-day
/// events are floating dates and are never shifted.
fn local_datetime(s: &str, zone: &str, all_day: bool) -> Option<chrono::NaiveDateTime> {
    use chrono::{Local, NaiveDateTime, TimeZone};
    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").ok()?;
    if all_day || !zone.eq_ignore_ascii_case("UTC") {
        return Some(naive);
    }
    Some(Local.from_utc_datetime(&naive).naive_local())
}

/// HTTP status carried by a provider error message (`... Graph API error: 403 Forbidden — ...`).
pub fn status_of(error_text: &str) -> Option<u16> {
    let rest = error_text.split("Graph API error: ").nth(1)?;
    rest.split_whitespace().next()?.parse().ok()
}

/// How long a cached lookup stays valid. Ordinary mail never turns into an invitation; anything
/// else can change (answered elsewhere, cancelled, event deleted).
pub fn cache_ttl_secs(kind: &str) -> i64 {
    if kind == "none" { i64::MAX } else { 300 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(kind: &str, response: &str, organizer: bool, cancelled: bool, with_event: bool) -> Value {
        let mut v = json!({ "meetingMessageType": kind });
        if with_event {
            v["event"] = json!({
                "id": "EV1", "subject": "Sync", "isAllDay": false,
                "isOrganizer": organizer, "isCancelled": cancelled,
                "responseStatus": { "response": response },
                "start": { "dateTime": "2026-09-11T01:00:00.0000000", "timeZone": "UTC" },
                "end":   { "dateTime": "2026-09-11T02:00:00.0000000", "timeZone": "UTC" },
                "location": { "displayName": "Room 4" },
                "organizer": { "emailAddress": { "name": "Pat", "address": "pat@x.org" } },
            });
        }
        v
    }

    #[test]
    fn maps_each_response_to_graph_event_action_and_state() {
        assert_eq!(response_op("accept"), Some(("accept", "accepted")));
        assert_eq!(response_op("tentative"), Some(("tentativelyAccept", "tentativelyAccepted")));
        assert_eq!(response_op("decline"), Some(("decline", "declined")));
        assert_eq!(response_op("maybe"), None);
    }

    #[test]
    fn only_an_open_request_from_someone_else_is_actionable() {
        assert!(InviteInfo::from_graph(&sample("meetingRequest", "notResponded", false, false, true)).actionable());
        // already answered: still answerable (change of mind)
        assert!(InviteInfo::from_graph(&sample("meetingRequest", "accepted", false, false, true)).actionable());
        // own meeting, cancelled, event gone, other people's answers, ordinary mail
        assert!(!InviteInfo::from_graph(&sample("meetingRequest", "organizer", true, false, true)).actionable());
        assert!(!InviteInfo::from_graph(&sample("meetingRequest", "notResponded", false, true, true)).actionable());
        assert!(!InviteInfo::from_graph(&sample("meetingRequest", "", false, false, false)).actionable());
        assert!(!InviteInfo::from_graph(&sample("meetingAccepted", "organizer", true, false, true)).actionable());
        assert!(!InviteInfo::from_graph(&sample("meetingCancelled", "notResponded", false, true, true)).actionable());
        assert!(!InviteInfo::from_graph(&json!({})).actionable());
    }

    #[test]
    fn ordinary_mail_is_not_calendar_mail() {
        let i = InviteInfo::from_graph(&json!({ "id": "x" }));
        assert_eq!(i.kind, "none");
        assert!(!i.is_calendar_mail());
        assert!(InviteInfo::from_graph(&sample("meetingCancelled", "notResponded", false, true, true)).is_calendar_mail());
    }

    #[test]
    fn parses_event_details() {
        let i = InviteInfo::from_graph(&sample("meetingRequest", "tentativelyAccepted", false, false, true));
        assert_eq!(i.event_id.as_deref(), Some("EV1"));
        assert_eq!(i.response, "tentativelyAccepted");
        assert_eq!(i.organizer, "Pat");
        assert_eq!(i.location, "Room 4");
        assert_eq!(i.to_json()["response"], "tentativelyAccepted");
        assert_eq!(i.to_json()["actionable"], true);
        assert!(i.start_date().is_some());
        assert!(!i.when_text().is_empty());
    }

    #[test]
    fn all_day_events_are_not_shifted_by_the_timezone() {
        let mut v = sample("meetingRequest", "notResponded", false, false, true);
        v["event"]["isAllDay"] = json!(true);
        v["event"]["start"] = json!({ "dateTime": "2026-09-11T00:00:00.0000000", "timeZone": "UTC" });
        v["event"]["end"] = json!({ "dateTime": "2026-09-12T00:00:00.0000000", "timeZone": "UTC" });
        let i = InviteInfo::from_graph(&v);
        assert_eq!(i.start_date(), chrono::NaiveDate::from_ymd_opt(2026, 9, 11));
        assert!(i.when_text().ends_with("all day"));
    }

    #[test]
    fn long_locations_are_cut_for_the_banner() {
        let mut v = sample("meetingRequest", "notResponded", false, false, true);
        v["event"]["location"]["displayName"] = json!("https://zoom.example/".to_string() + &"x".repeat(200));
        let loc = InviteInfo::from_graph(&v).to_json()["location"].as_str().unwrap().to_string();
        assert_eq!(loc.chars().count(), 60);
        assert!(loc.ends_with('…'));
    }

    #[test]
    fn status_is_read_from_the_error_text() {
        assert_eq!(status_of("HTTP error: Graph API error: 403 Forbidden — {\"error\":\"x\"}"), Some(403));
        assert_eq!(status_of("Graph API error: 404 Not Found — {}"), Some(404));
        // a stray 403 elsewhere in the text is not a status
        assert_eq!(status_of("could not parse id 4031"), None);
    }

    #[test]
    fn cache_keeps_ordinary_mail_forever_and_invitations_briefly() {
        assert_eq!(cache_ttl_secs("none"), i64::MAX);
        assert_eq!(cache_ttl_secs("meetingRequest"), 300);
    }
}
