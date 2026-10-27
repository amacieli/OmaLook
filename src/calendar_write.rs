//! Calendar write side, phase 2: the neutral `EventDraft` and the pure Graph
//! request-body builder (`POST /me/events`). No network here; the outbox worker
//! sends what this module builds.
//!
//! Rules taken from docs/PLAN-calendar-events.md (sections 3, 4, 5.7) and the
//! phase 0 spike (section 15): Graph is lenient about recurrence, so the builder
//! makes the range start equal the event date (the series starts on the event's
//! own date) and refuses a weekly pattern whose start weekday is not listed,
//! instead of silently moving the first occurrence.

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, Weekday};
use serde_json::{json, Value};

/// Graph's documented maximum for `transactionId`.
const MAX_TRANSACTION_ID: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub struct Attendee {
    pub name: String,
    pub email: String,
    /// "required" | "optional" | "resource"
    pub role: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Daily { interval: u32 },
    /// `days` must contain the series start weekday (checked in `validate`).
    Weekly { interval: u32, days: Vec<Weekday> },
    AbsoluteMonthly { interval: u32, day_of_month: u32 },
    /// e.g. the second Tuesday: `index` is "first" | "second" | "third" | "fourth" | "last".
    RelativeMonthly { interval: u32, day: Weekday, index: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Range {
    NoEnd,
    EndDate(NaiveDate),
    Numbered(u32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recurrence {
    pub pattern: Pattern,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventDraft {
    pub subject: String,
    pub body_text: String,
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    /// IANA zone, e.g. "America/New_York". Graph also accepts Windows names.
    pub time_zone: String,
    pub is_all_day: bool,
    pub location: Option<String>,
    pub attendees: Vec<Attendee>,
    pub reminder_minutes: Option<u32>,
    /// "busy" | "free" | "tentative" | "oof" | "workingElsewhere"
    pub show_as: String,
    pub recurrence: Option<Recurrence>,
    /// Generated once per draft; the idempotency key for retries.
    pub transaction_id: String,
    /// Online meeting request, e.g. Some("teamsForBusiness") for a Teams link.
    pub online_provider: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DraftError {
    EmptyTransactionId,
    TransactionIdTooLong,
    EndNotAfterStart,
    BadAttendeeAddress(String),
    WeekdayNotInPattern(Weekday),
    EmptyWeeklyDays,
    BadInterval,
    BadDayOfMonth(u32),
    BadNumberOfOccurrences,
    EndBeforeStart,
    BadRelativeIndex(String),
}

impl std::fmt::Display for DraftError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyTransactionId => write!(f, "transaction id is empty"),
            Self::TransactionIdTooLong => write!(f, "transaction id is longer than {MAX_TRANSACTION_ID} characters"),
            Self::EndNotAfterStart => write!(f, "the event must end after it starts"),
            Self::BadAttendeeAddress(a) => write!(f, "not a valid address: {a}"),
            Self::WeekdayNotInPattern(d) => write!(f, "a weekly repeat must include the start day ({d:?})"),
            Self::EmptyWeeklyDays => write!(f, "a weekly repeat needs at least one day"),
            Self::BadInterval => write!(f, "the repeat interval must be at least 1"),
            Self::BadDayOfMonth(d) => write!(f, "day of month {d} is not 1-31"),
            Self::BadNumberOfOccurrences => write!(f, "a repeat count must be at least 1"),
            Self::EndBeforeStart => write!(f, "the repeat ends before the first occurrence"),
            Self::BadRelativeIndex(i) => write!(f, "unknown week index: {i}"),
        }
    }
}

/// Minimal address check: one '@', a non-empty local part and a dotted domain, no spaces.
pub fn looks_like_address(addr: &str) -> bool {
    let mut parts = addr.split('@');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(local), Some(domain), None) => {
            !local.is_empty()
                && !addr.contains(char::is_whitespace)
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        }
        _ => false,
    }
}

impl EventDraft {
    /// Everything the builder would reject, checked before anything is queued.
    pub fn validate(&self) -> Result<(), DraftError> {
        if self.transaction_id.is_empty() {
            return Err(DraftError::EmptyTransactionId);
        }
        if self.transaction_id.len() > MAX_TRANSACTION_ID {
            return Err(DraftError::TransactionIdTooLong);
        }
        if self.end <= self.start {
            return Err(DraftError::EndNotAfterStart);
        }
        for a in &self.attendees {
            if !looks_like_address(&a.email) {
                return Err(DraftError::BadAttendeeAddress(a.email.clone()));
            }
        }
        if let Some(rec) = &self.recurrence {
            validate_recurrence(rec, self.start.date())?;
        }
        Ok(())
    }

    /// The `POST /me/events` JSON body. Pure: same draft, same JSON.
    pub fn to_graph_json(&self) -> Result<Value, DraftError> {
        self.validate()?;

        let (start, end) = if self.is_all_day {
            // All-day: midnight to midnight, end exclusive (the day after the last day).
            let s = self.start.date().and_time(NaiveTime::MIN);
            let e = self.end.date().and_time(NaiveTime::MIN);
            (s, if e > s { e } else { s + chrono::Duration::days(1) })
        } else {
            (self.start, self.end)
        };

        let mut body = json!({
            "subject": if self.subject.trim().is_empty() { "(no title)".to_string() } else { self.subject.clone() },
            "body": { "contentType": "text", "content": self.body_text },
            "start": { "dateTime": fmt_graph_dt(start), "timeZone": self.time_zone },
            "end": { "dateTime": fmt_graph_dt(end), "timeZone": self.time_zone },
            "isAllDay": self.is_all_day,
            "showAs": self.show_as,
            "transactionId": self.transaction_id,
        });

        if let Some(loc) = self.location.as_deref().filter(|l| !l.trim().is_empty()) {
            body["location"] = json!({ "displayName": loc });
        }
        match self.reminder_minutes {
            Some(m) => {
                body["isReminderOn"] = json!(true);
                body["reminderMinutesBeforeStart"] = json!(m);
            }
            None => body["isReminderOn"] = json!(false),
        }
        if !self.attendees.is_empty() {
            body["attendees"] = Value::Array(
                self.attendees
                    .iter()
                    .map(|a| json!({
                        "emailAddress": { "address": a.email, "name": a.name },
                        "type": a.role,
                    }))
                    .collect(),
            );
        }
        if let Some(p) = &self.online_provider {
            body["isOnlineMeeting"] = json!(true);
            body["onlineMeetingProvider"] = json!(p);
        }
        if let Some(rec) = &self.recurrence {
            body["recurrence"] = recurrence_json(rec, self.start.date(), &self.time_zone);
        }
        Ok(body)
    }
}

fn validate_recurrence(rec: &Recurrence, start: NaiveDate) -> Result<(), DraftError> {
    match &rec.pattern {
        Pattern::Daily { interval } => check_interval(*interval)?,
        Pattern::Weekly { interval, days } => {
            check_interval(*interval)?;
            if days.is_empty() {
                return Err(DraftError::EmptyWeeklyDays);
            }
            if !days.contains(&start.weekday()) {
                return Err(DraftError::WeekdayNotInPattern(start.weekday()));
            }
        }
        Pattern::AbsoluteMonthly { interval, day_of_month } => {
            check_interval(*interval)?;
            if !(1..=31).contains(day_of_month) {
                return Err(DraftError::BadDayOfMonth(*day_of_month));
            }
        }
        Pattern::RelativeMonthly { interval, index, .. } => {
            check_interval(*interval)?;
            week_index(index)?;
        }
    }
    match &rec.range {
        Range::NoEnd => Ok(()),
        Range::EndDate(end) if *end < start => Err(DraftError::EndBeforeStart),
        Range::EndDate(_) => Ok(()),
        Range::Numbered(0) => Err(DraftError::BadNumberOfOccurrences),
        Range::Numbered(_) => Ok(()),
    }
}

fn check_interval(i: u32) -> Result<(), DraftError> {
    if i == 0 { Err(DraftError::BadInterval) } else { Ok(()) }
}

fn week_index(index: &str) -> Result<&str, DraftError> {
    match index {
        "first" | "second" | "third" | "fourth" | "last" => Ok(index),
        other => Err(DraftError::BadRelativeIndex(other.to_string())),
    }
}

fn recurrence_json(rec: &Recurrence, start: NaiveDate, zone: &str) -> Value {
    let pattern = match &rec.pattern {
        Pattern::Daily { interval } => json!({ "type": "daily", "interval": interval }),
        Pattern::Weekly { interval, days } => json!({
            "type": "weekly",
            "interval": interval,
            "daysOfWeek": days.iter().map(|d| graph_weekday(*d)).collect::<Vec<_>>(),
            "firstDayOfWeek": "sunday",
        }),
        Pattern::AbsoluteMonthly { interval, day_of_month } => json!({
            "type": "absoluteMonthly",
            "interval": interval,
            "dayOfMonth": day_of_month,
        }),
        Pattern::RelativeMonthly { interval, day, index } => json!({
            "type": "relativeMonthly",
            "interval": interval,
            "daysOfWeek": [graph_weekday(*day)],
            "index": index,
        }),
    };
    let fmt_date = |d: NaiveDate| d.format("%Y-%m-%d").to_string();
    let range = match &rec.range {
        Range::NoEnd => json!({ "type": "noEnd", "startDate": fmt_date(start) }),
        Range::EndDate(end) => json!({
            "type": "endDate",
            "startDate": fmt_date(start),
            "endDate": fmt_date(*end),
        }),
        Range::Numbered(n) => json!({
            "type": "numbered",
            "startDate": fmt_date(start),
            "numberOfOccurrences": n,
        }),
    };
    let mut range = range;
    range["recurrenceTimeZone"] = json!(zone);
    json!({ "pattern": pattern, "range": range })
}

fn graph_weekday(d: Weekday) -> &'static str {
    match d {
        Weekday::Mon => "monday",
        Weekday::Tue => "tuesday",
        Weekday::Wed => "wednesday",
        Weekday::Thu => "thursday",
        Weekday::Fri => "friday",
        Weekday::Sat => "saturday",
        Weekday::Sun => "sunday",
    }
}

/// Graph wants `YYYY-MM-DDTHH:MM:SS` with no offset; the zone travels in `timeZone`.
fn fmt_graph_dt(dt: NaiveDateTime) -> String {
    dt.format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// Weekly Mon-Fri, the "weekdays" preset of the Repeat row.
pub fn weekdays() -> Vec<Weekday> {
    vec![Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri]
}

/// What the provider answers to a create: enough to show the event and open it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CreatedEvent {
    pub id: String,
    pub web_link: String,
    pub join_url: String,
    /// True when it is a series (the answer is the master; occurrences come from `calendarView`).
    pub is_series: bool,
    /// The full provider answer, so the caller can cache the event without another request.
    pub raw: Value,
}

/// The event id Graph names in an `ErrorDuplicateTransactionId` answer (`@event.existingEventId`).
pub fn existing_event_id(error_body: &str) -> Option<String> {
    if !error_body.contains("ErrorDuplicateTransactionId") {
        return None;
    }
    let key = "existingEventId";
    let rest = &error_body[error_body.find(key)? + key.len()..];
    let rest = &rest[rest.find(':')? + 1..];
    let rest = &rest[rest.find('"')? + 1..];
    let id = &rest[..rest.find('"')?];
    if id.is_empty() { None } else { Some(id.to_string()) }
}

fn weekday_from_name(n: &str) -> Option<Weekday> {
    Some(match n.to_ascii_lowercase().as_str() {
        "monday" | "mon" => Weekday::Mon,
        "tuesday" | "tue" => Weekday::Tue,
        "wednesday" | "wed" => Weekday::Wed,
        "thursday" | "thu" => Weekday::Thu,
        "friday" | "fri" => Weekday::Fri,
        "saturday" | "sat" => Weekday::Sat,
        "sunday" | "sun" => Weekday::Sun,
        _ => return None,
    })
}

fn parse_local_dt(s: &str) -> Option<NaiveDateTime> {
    let s = s.trim();
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M"))
        .ok()
}

fn parse_recurrence(v: &Value) -> Result<Option<Recurrence>, String> {
    if v.is_null() {
        return Ok(None);
    }
    let p = &v["pattern"];
    let interval = p["interval"].as_u64().unwrap_or(1) as u32;
    let days = || -> Result<Vec<Weekday>, String> {
        p["daysOfWeek"]
            .as_array()
            .map(|a| a.iter().filter_map(|d| d.as_str()).map(|d| weekday_from_name(d).ok_or(format!("unknown weekday: {d}"))).collect())
            .unwrap_or_else(|| Ok(vec![]))
    };
    let pattern = match p["type"].as_str().unwrap_or("") {
        "daily" => Pattern::Daily { interval },
        "weekly" => Pattern::Weekly { interval, days: days()? },
        "absoluteMonthly" => Pattern::AbsoluteMonthly { interval, day_of_month: p["dayOfMonth"].as_u64().unwrap_or(0) as u32 },
        "relativeMonthly" => {
            let d = days()?;
            Pattern::RelativeMonthly {
                interval,
                day: *d.first().ok_or("a relative monthly repeat needs one weekday")?,
                index: p["index"].as_str().unwrap_or("").to_string(),
            }
        }
        other => return Err(format!("unsupported repeat type: {other}")),
    };
    let r = &v["range"];
    let range = match r["type"].as_str().unwrap_or("") {
        "noEnd" => Range::NoEnd,
        "endDate" => Range::EndDate(
            NaiveDate::parse_from_str(r["endDate"].as_str().unwrap_or(""), "%Y-%m-%d").map_err(|_| "the repeat end date is not a date".to_string())?,
        ),
        "numbered" => Range::Numbered(r["numberOfOccurrences"].as_u64().unwrap_or(0) as u32),
        other => return Err(format!("unsupported repeat end: {other}")),
    };
    Ok(Some(Recurrence { pattern, range }))
}

impl EventDraft {
    /// Build a draft from the JSON the event editor posts (`POST /calendar/event`):
    /// `{subject, body, start, end, time_zone, all_day, location, attendees:[{name,email,role}],
    ///   reminder_minutes, show_as, online_provider, recurrence:{pattern,range}}`.
    /// Times are local wall-clock `YYYY-MM-DDTHH:MM[:SS]`. Does not validate; `validate()` does.
    pub fn from_request(v: &Value, transaction_id: &str) -> Result<EventDraft, String> {
        let start = parse_local_dt(v["start"].as_str().unwrap_or("")).ok_or("start is not a date and time")?;
        let end = parse_local_dt(v["end"].as_str().unwrap_or("")).ok_or("end is not a date and time")?;
        let attendees = v["attendees"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|x| Attendee {
                        name: x["name"].as_str().unwrap_or("").to_string(),
                        email: x["email"].as_str().unwrap_or("").trim().to_string(),
                        role: match x["role"].as_str() {
                            Some("optional") => "optional".to_string(),
                            Some("resource") => "resource".to_string(),
                            _ => "required".to_string(),
                        },
                    })
                    .collect()
            })
            .unwrap_or_default();
        let show_as = match v["show_as"].as_str() {
            Some(s @ ("busy" | "free" | "tentative" | "oof" | "workingElsewhere")) => s.to_string(),
            _ => "busy".to_string(),
        };
        Ok(EventDraft {
            subject: v["subject"].as_str().unwrap_or("").to_string(),
            body_text: v["body"].as_str().unwrap_or("").to_string(),
            start,
            end,
            time_zone: v["time_zone"].as_str().filter(|z| !z.is_empty()).unwrap_or("UTC").to_string(),
            is_all_day: v["all_day"].as_bool().unwrap_or(false),
            location: v["location"].as_str().map(|s| s.to_string()),
            attendees,
            reminder_minutes: v["reminder_minutes"].as_u64().map(|m| m as u32),
            show_as,
            recurrence: parse_recurrence(&v["recurrence"])?,
            transaction_id: transaction_id.to_string(),
            online_provider: v["online_provider"].as_str().filter(|s| !s.is_empty()).map(|s| s.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(h, min, 0).unwrap()
    }

    fn base() -> EventDraft {
        EventDraft {
            subject: "Quarterly planning".into(),
            body_text: String::new(),
            start: dt(2026, 10, 27, 10, 0),
            end: dt(2026, 10, 27, 10, 30),
            time_zone: "America/New_York".into(),
            is_all_day: false,
            location: None,
            attendees: vec![],
            reminder_minutes: Some(15),
            show_as: "busy".into(),
            recurrence: None,
            transaction_id: "tx-1".into(),
            online_provider: None,
        }
    }

    #[test]
    fn personal_single_event_has_no_attendees_or_meeting() {
        let j = base().to_graph_json().unwrap();
        assert_eq!(j["subject"], "Quarterly planning");
        assert_eq!(j["start"]["dateTime"], "2026-10-27T10:00:00");
        assert_eq!(j["end"]["dateTime"], "2026-10-27T10:30:00");
        assert_eq!(j["start"]["timeZone"], "America/New_York");
        assert_eq!(j["isReminderOn"], true);
        assert_eq!(j["reminderMinutesBeforeStart"], 15);
        assert_eq!(j["transactionId"], "tx-1");
        assert!(j.get("attendees").is_none(), "personal entry sends no invitations");
        assert!(j.get("isOnlineMeeting").is_none());
        assert!(j.get("recurrence").is_none());
    }

    #[test]
    fn blank_title_becomes_no_title_and_no_reminder_is_off() {
        let mut d = base();
        d.subject = "   ".into();
        d.reminder_minutes = None;
        let j = d.to_graph_json().unwrap();
        assert_eq!(j["subject"], "(no title)");
        assert_eq!(j["isReminderOn"], false);
        assert!(j.get("reminderMinutesBeforeStart").is_none());
    }

    #[test]
    fn meeting_carries_attendee_types_and_teams_provider() {
        let mut d = base();
        d.attendees = vec![
            Attendee { name: "Jen".into(), email: "jen@x.org".into(), role: "required".into() },
            Attendee { name: "".into(), email: "erika@x.org".into(), role: "optional".into() },
        ];
        d.online_provider = Some("teamsForBusiness".into());
        d.location = Some("Room 4B".into());
        let j = d.to_graph_json().unwrap();
        assert_eq!(j["attendees"][0]["type"], "required");
        assert_eq!(j["attendees"][0]["emailAddress"]["address"], "jen@x.org");
        assert_eq!(j["attendees"][1]["type"], "optional");
        assert_eq!(j["isOnlineMeeting"], true);
        assert_eq!(j["onlineMeetingProvider"], "teamsForBusiness");
        assert_eq!(j["location"]["displayName"], "Room 4B");
    }

    #[test]
    fn all_day_runs_midnight_to_next_midnight() {
        let mut d = base();
        d.is_all_day = true;
        d.start = dt(2026, 10, 27, 0, 0);
        d.end = dt(2026, 10, 27, 0, 0);
        // end not after start is invalid even for all-day; the editor sets end = start + 1 day
        assert_eq!(d.to_graph_json(), Err(DraftError::EndNotAfterStart));
        d.end = dt(2026, 10, 28, 0, 0);
        let j = d.to_graph_json().unwrap();
        assert_eq!(j["isAllDay"], true);
        assert_eq!(j["start"]["dateTime"], "2026-10-27T00:00:00");
        assert_eq!(j["end"]["dateTime"], "2026-10-28T00:00:00");
    }

    #[test]
    fn weekly_series_sets_range_start_to_event_date() {
        let mut d = base(); // 2026-10-27 is a Tuesday
        d.recurrence = Some(Recurrence {
            pattern: Pattern::Weekly { interval: 1, days: vec![Weekday::Tue] },
            range: Range::EndDate(NaiveDate::from_ymd_opt(2026, 12, 29).unwrap()),
        });
        let j = d.to_graph_json().unwrap();
        assert_eq!(j["recurrence"]["pattern"]["type"], "weekly");
        assert_eq!(j["recurrence"]["pattern"]["daysOfWeek"], json!(["tuesday"]));
        assert_eq!(j["recurrence"]["range"]["type"], "endDate");
        assert_eq!(j["recurrence"]["range"]["startDate"], "2026-10-27");
        assert_eq!(j["recurrence"]["range"]["endDate"], "2026-12-29");
        assert_eq!(j["recurrence"]["range"]["recurrenceTimeZone"], "America/New_York");
    }

    #[test]
    fn weekly_pattern_must_include_start_weekday() {
        let mut d = base(); // Tuesday
        d.recurrence = Some(Recurrence {
            pattern: Pattern::Weekly { interval: 1, days: vec![Weekday::Mon, Weekday::Wed] },
            range: Range::NoEnd,
        });
        assert_eq!(d.validate(), Err(DraftError::WeekdayNotInPattern(Weekday::Tue)));
    }

    #[test]
    fn weekdays_preset_is_mon_to_fri() {
        let mut d = base(); // Tuesday, inside Mon-Fri
        d.recurrence = Some(Recurrence {
            pattern: Pattern::Weekly { interval: 1, days: weekdays() },
            range: Range::Numbered(10),
        });
        let j = d.to_graph_json().unwrap();
        assert_eq!(
            j["recurrence"]["pattern"]["daysOfWeek"],
            json!(["monday", "tuesday", "wednesday", "thursday", "friday"])
        );
        assert_eq!(j["recurrence"]["range"]["numberOfOccurrences"], 10);
    }

    #[test]
    fn relative_monthly_second_tuesday() {
        let mut d = base();
        d.recurrence = Some(Recurrence {
            pattern: Pattern::RelativeMonthly { interval: 1, day: Weekday::Tue, index: "second".into() },
            range: Range::NoEnd,
        });
        let j = d.to_graph_json().unwrap();
        assert_eq!(j["recurrence"]["pattern"]["type"], "relativeMonthly");
        assert_eq!(j["recurrence"]["pattern"]["index"], "second");
        assert_eq!(j["recurrence"]["pattern"]["daysOfWeek"], json!(["tuesday"]));
        assert_eq!(j["recurrence"]["range"]["type"], "noEnd");
    }

    #[test]
    fn absolute_monthly_and_daily_patterns() {
        let mut d = base();
        d.recurrence = Some(Recurrence {
            pattern: Pattern::AbsoluteMonthly { interval: 2, day_of_month: 15 },
            range: Range::NoEnd,
        });
        let j = d.to_graph_json().unwrap();
        assert_eq!(j["recurrence"]["pattern"], json!({"type":"absoluteMonthly","interval":2,"dayOfMonth":15}));

        d.recurrence = Some(Recurrence { pattern: Pattern::Daily { interval: 1 }, range: Range::NoEnd });
        assert_eq!(d.to_graph_json().unwrap()["recurrence"]["pattern"]["type"], "daily");
    }

    #[test]
    fn rejects_bad_recurrence_inputs() {
        let mut d = base();
        let cases: Vec<(Recurrence, DraftError)> = vec![
            (Recurrence { pattern: Pattern::Daily { interval: 0 }, range: Range::NoEnd }, DraftError::BadInterval),
            (Recurrence { pattern: Pattern::Weekly { interval: 1, days: vec![] }, range: Range::NoEnd }, DraftError::EmptyWeeklyDays),
            (Recurrence { pattern: Pattern::AbsoluteMonthly { interval: 1, day_of_month: 32 }, range: Range::NoEnd }, DraftError::BadDayOfMonth(32)),
            (Recurrence { pattern: Pattern::Daily { interval: 1 }, range: Range::Numbered(0) }, DraftError::BadNumberOfOccurrences),
            (Recurrence { pattern: Pattern::Daily { interval: 1 }, range: Range::EndDate(NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()) }, DraftError::EndBeforeStart),
            (Recurrence { pattern: Pattern::RelativeMonthly { interval: 1, day: Weekday::Tue, index: "eighth".into() }, range: Range::NoEnd }, DraftError::BadRelativeIndex("eighth".into())),
        ];
        for (rec, want) in cases {
            d.recurrence = Some(rec);
            assert_eq!(d.validate(), Err(want));
        }
    }

    #[test]
    fn rejects_end_before_start_and_bad_addresses() {
        let mut d = base();
        d.end = d.start;
        assert_eq!(d.validate(), Err(DraftError::EndNotAfterStart));

        let mut d = base();
        d.attendees = vec![Attendee { name: "".into(), email: "not an address".into(), role: "required".into() }];
        assert_eq!(d.validate(), Err(DraftError::BadAttendeeAddress("not an address".into())));
    }

    #[test]
    fn transaction_id_must_be_present_and_bounded() {
        let mut d = base();
        d.transaction_id = String::new();
        assert_eq!(d.validate(), Err(DraftError::EmptyTransactionId));
        d.transaction_id = "x".repeat(129);
        assert_eq!(d.validate(), Err(DraftError::TransactionIdTooLong));
        d.transaction_id = "x".repeat(128);
        assert!(d.validate().is_ok());
    }

    #[test]
    fn address_check() {
        assert!(looks_like_address("a@b.org"));
        assert!(!looks_like_address("a@b"));
        assert!(!looks_like_address("a@@b.org"));
        assert!(!looks_like_address("@b.org"));
        assert!(!looks_like_address("a b@c.org"));
        assert!(!looks_like_address("a@.org"));
    }

    /// Manual live-test helper: `OMALOOK_DUMP_DIR=<dir> cargo test --lib dump_live_test_bodies -- --ignored`
    /// writes the exact Graph bodies the live check posts (never run in the normal suite).
    #[test]
    #[ignore]
    fn dump_live_test_bodies() {
        let dir = match std::env::var("OMALOOK_DUMP_DIR") { Ok(d) => d, Err(_) => return };
        let mut single = base();
        single.subject = "[omalook test] single".into();
        single.start = dt(2026, 11, 2, 10, 0);
        single.end = dt(2026, 11, 2, 10, 30);
        single.transaction_id = "omalook-live-single-1".into();
        let mut weekly = base();
        weekly.subject = "[omalook test] weekly".into();
        weekly.start = dt(2026, 11, 3, 14, 0);
        weekly.end = dt(2026, 11, 3, 14, 45);
        weekly.transaction_id = "omalook-live-weekly-1".into();
        weekly.recurrence = Some(Recurrence {
            pattern: Pattern::Weekly { interval: 1, days: vec![Weekday::Tue] },
            range: Range::EndDate(NaiveDate::from_ymd_opt(2026, 11, 17).unwrap()),
        });
        std::fs::write(format!("{dir}/single.json"), single.to_graph_json().unwrap().to_string()).unwrap();
        std::fs::write(format!("{dir}/weekly.json"), weekly.to_graph_json().unwrap().to_string()).unwrap();
    }

    #[test]
    fn request_json_becomes_a_draft_and_a_graph_body() {
        let req = json!({
            "subject": "Quarterly planning", "body": "agenda", "start": "2026-10-27T10:00", "end": "2026-10-27T10:30:00",
            "time_zone": "America/New_York", "all_day": false, "location": "Room 4B",
            "attendees": [{"name": "Gary", "email": "gary@x.org", "role": "optional"}, {"email": "j@x.org"}],
            "reminder_minutes": 15, "show_as": "busy", "online_provider": "teamsForBusiness",
            "recurrence": {"pattern": {"type": "weekly", "interval": 1, "daysOfWeek": ["tuesday"]},
                           "range": {"type": "endDate", "endDate": "2026-12-29"}}
        });
        let d = EventDraft::from_request(&req, "tx-9").unwrap();
        assert_eq!(d.attendees[0].role, "optional");
        assert_eq!(d.attendees[1].role, "required");
        let j = d.to_graph_json().unwrap();
        assert_eq!(j["recurrence"]["pattern"]["daysOfWeek"][0], "tuesday");
        assert_eq!(j["recurrence"]["range"]["endDate"], "2026-12-29");
        assert_eq!(j["isOnlineMeeting"], true);
        assert_eq!(j["start"]["dateTime"], "2026-10-27T10:00:00");
    }

    #[test]
    fn request_json_errors_are_refused_not_guessed() {
        assert!(EventDraft::from_request(&json!({"start": "tomorrow", "end": "x"}), "t").is_err());
        let wrong_day = json!({"start": "2026-10-27T10:00", "end": "2026-10-27T11:00",
            "recurrence": {"pattern": {"type": "weekly", "daysOfWeek": ["thursday"]}, "range": {"type": "noEnd"}}});
        let d = EventDraft::from_request(&wrong_day, "t").unwrap();
        assert!(matches!(d.validate(), Err(DraftError::WeekdayNotInPattern(_))));
        let bad_type = json!({"start": "2026-10-27T10:00", "end": "2026-10-27T11:00",
            "recurrence": {"pattern": {"type": "yearly"}, "range": {"type": "noEnd"}}});
        assert!(EventDraft::from_request(&bad_type, "t").is_err());
    }

    #[test]
    fn duplicate_transaction_answer_names_the_existing_event() {
        let body = r#"{"error":{"code":"ErrorDuplicateTransactionId","message":"x","@event.existingEventId":"AAMk123="}}"#;
        assert_eq!(existing_event_id(body).as_deref(), Some("AAMk123="));
        assert_eq!(existing_event_id(r#"{"error":{"code":"ErrorInvalidRequest"}}"#), None);
        assert_eq!(existing_event_id(r#"{"error":{"code":"ErrorDuplicateTransactionId"}}"#), None);
    }
}
