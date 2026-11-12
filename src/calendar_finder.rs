//! Free-time finder, backend (docs/PLAN-calendar-events.md section 5.6, track A).
//!
//! Three parts: the pure slot algorithm (no I/O, unit-tested here), Graph's availability digits and
//! working hours parsed into it, and `find_free_time`, which asks the providers and answers the
//! `GET /calendar/find` route. No UI lives here.
//!
//! Rules (decided for this build, deviations from the plan are noted where they happen):
//! - Times are the machine's local wall clock, the same stamps the cache uses.
//! - A slot must sit inside the working hours of you and of every required attendee. An attendee's
//!   own hours are used when the provider reports them, else the default (`WorkingHours::default_user`).
//! - Any busy, tentative, out-of-office or working-elsewhere time blocks a required person.
//!   Optional people never exclude a slot; they are counted and rank lower.
//! - A required person with no free/busy data is **not assumed free**: the slot is listed with that
//!   person in `unknown` (so `confirmed` is false) and ranks below every confirmed slot.
//! - Grid: 15-minute starts. A start or end that does not exist on the clock (DST gap) is skipped.

use crate::auth::AuthManager;
use crate::db::Database;
use crate::providers::calendar::{local_time_zone, GraphCalendarProvider, ScheduleAnswer};
use crate::providers::CalendarProvider;
use chrono::{Datelike, Duration, Local, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

/// Start times are on this grid (minutes).
pub const GRID_MINUTES: i64 = 15;
/// Largest window one request may ask for (days, end exclusive).
pub const MAX_DAYS: i64 = 14;
/// A slot that starts or ends within this many minutes of a busy block ranks lower.
const NEXT_TO_BUSY_MINUTES: i64 = 15;
/// Lunch window used for ranking when the caller does not give one (plan section 9).
pub fn default_lunch() -> (NaiveTime, NaiveTime) {
    (hm(12, 0), hm(13, 0))
}

fn hm(h: u32, m: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, 0).expect("valid clock time")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusyKind {
    Tentative,
    Busy,
    OutOfOffice,
    Elsewhere,
}

impl BusyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BusyKind::Tentative => "tentative",
            BusyKind::Busy => "busy",
            BusyKind::OutOfOffice => "oof",
            BusyKind::Elsewhere => "elsewhere",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy {
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub kind: BusyKind,
}

/// Working hours on the days marked in `days` (index 0 = Monday).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingHours {
    pub days: [bool; 7],
    pub start: NaiveTime,
    pub end: NaiveTime,
}

impl WorkingHours {
    /// Mon-Fri 09:00-17:30, the plan's default (section 9).
    pub fn default_user() -> Self {
        WorkingHours { days: [true, true, true, true, true, false, false], start: hm(9, 0), end: hm(17, 30) }
    }

    pub fn allows(&self, date: NaiveDate) -> bool {
        self.days[date.weekday().num_days_from_monday() as usize]
    }
}

/// One person a slot must suit. `busy: None` means no free/busy data (never treated as free).
#[derive(Debug, Clone, PartialEq)]
pub struct Person {
    pub label: String,
    pub required: bool,
    pub busy: Option<Vec<Busy>>,
    /// The person's own working hours when reported; None = the request's default hours.
    pub hours: Option<WorkingHours>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// First day (inclusive) and the day after the last (exclusive).
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub duration_min: i64,
    /// Default hours (you, and anyone whose hours are not reported).
    pub hours: WorkingHours,
    pub lunch: Option<(NaiveTime, NaiveTime)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    /// Required people with no free/busy data. The slot is NOT known to be free for them.
    pub unknown: Vec<String>,
    /// Optional people who are busy in this slot (the slot is still offered).
    pub optional_busy: Vec<String>,
    pub lunch: bool,
    pub next_to_busy: bool,
}

impl Slot {
    /// True when every required person is known to be free.
    pub fn confirmed(&self) -> bool {
        self.unknown.is_empty()
    }
}

/// Round `t` up to the next grid point (a no-op on the grid).
fn round_up_to_grid(t: NaiveDateTime) -> NaiveDateTime {
    let step = GRID_MINUTES * 60;
    let secs = (t.hour() * 3600 + t.minute() * 60 + t.second()) as i64;
    let up = (secs + step - 1) / step * step;
    t.date().and_hms_opt(0, 0, 0).expect("midnight") + Duration::seconds(up)
}

/// The window a slot may sit in on `day`: the intersection of your hours and every required
/// person's hours. None when anyone required does not work that day, or the hours are empty.
fn day_window(req: &Request, people: &[Person], day: NaiveDate) -> Option<(NaiveDateTime, NaiveDateTime)> {
    if !req.hours.allows(day) {
        return None;
    }
    let mut lo = day.and_time(req.hours.start);
    let mut hi = day.and_time(req.hours.end);
    for p in people.iter().filter(|p| p.required) {
        let h = p.hours.as_ref().unwrap_or(&req.hours);
        if !h.allows(day) {
            return None;
        }
        lo = lo.max(day.and_time(h.start));
        hi = hi.min(day.and_time(h.end));
    }
    let lo = round_up_to_grid(lo);
    if hi <= lo {
        None
    } else {
        Some((lo, hi))
    }
}

fn candidate(
    req: &Request,
    people: &[Person],
    start: NaiveDateTime,
    is_real: &dyn Fn(NaiveDateTime) -> bool,
) -> Option<Slot> {
    let dur = Duration::minutes(req.duration_min);
    let end = start + dur;
    // Every grid point inside the slot must exist on the clock, not only the two ends.
    let step = Duration::minutes(GRID_MINUTES);
    let mut p = start;
    while p <= end {
        if !is_real(p) {
            return None;
        }
        p += step;
    }
    if !is_real(end) {
        return None;
    }

    let mut unknown = Vec::new();
    let mut optional_busy = Vec::new();
    let mut next_to_busy = false;
    let near_before = start - Duration::minutes(NEXT_TO_BUSY_MINUTES);
    let near_after = end + Duration::minutes(NEXT_TO_BUSY_MINUTES);
    for person in people {
        let Some(busy) = &person.busy else {
            if person.required {
                unknown.push(person.label.clone());
            }
            continue;
        };
        let overlaps = busy.iter().any(|b| b.start < end && b.end > start);
        if overlaps {
            if person.required {
                return None;
            }
            optional_busy.push(person.label.clone());
        }
        if busy.iter().any(|b| b.end > near_before && b.start < near_after) {
            next_to_busy = true;
        }
    }
    let lunch = req
        .lunch
        .map(|(a, b)| start.date().and_time(a) < end && start.date().and_time(b) > start)
        .unwrap_or(false);
    Some(Slot { start, end, unknown, optional_busy, lunch, next_to_busy })
}

/// Ranked free slots. Order: confirmed first, then optional attendees free, then outside lunch, then
/// not next to a busy block, then earlier. The plan lists the keys without an order; this one is
/// the decision for this build. `is_real` says whether a local wall-clock time exists (DST gap = false).
pub fn find_slots(req: &Request, people: &[Person], is_real: &dyn Fn(NaiveDateTime) -> bool, limit: usize) -> Vec<Slot> {
    let dur = Duration::minutes(req.duration_min);
    let step = Duration::minutes(GRID_MINUTES);
    let mut slots = Vec::new();
    if req.duration_min <= 0 {
        return slots;
    }
    let mut day = req.from;
    while day < req.to {
        if let Some((lo, hi)) = day_window(req, people, day) {
            let mut t = lo;
            while t + dur <= hi {
                if let Some(s) = candidate(req, people, t, is_real) {
                    slots.push(s);
                }
                t += step;
            }
        }
        match day.succ_opt() {
            Some(d) => day = d,
            None => break,
        }
    }
    slots.sort_by_key(|s| (!s.confirmed(), s.optional_busy.len(), s.lunch, s.next_to_busy, s.start));
    slots.truncate(limit);
    slots
}

/// Graph `availabilityView`: digit i covers `[start + i*interval, start + (i+1)*interval)`
/// (0 free, 1 tentative, 2 busy, 3 out of office, 4 working elsewhere). Consecutive equal non-free
/// digits become one interval, clipped to `end`. None when the view is empty, too short for the
/// window, or holds a character that is not a digit in 0..=4: then the answer is unknown, not free.
pub fn busy_from_view(view: &str, start: NaiveDateTime, interval_min: i64, end: NaiveDateTime) -> Option<Vec<Busy>> {
    let span = (end - start).num_minutes();
    if interval_min <= 0 || span <= 0 || view.is_empty() {
        return None;
    }
    let needed = ((span + interval_min - 1) / interval_min) as usize;
    let digits = view.as_bytes();
    if digits.len() < needed {
        return None;
    }
    let at = |i: usize| start + Duration::minutes(i as i64 * interval_min);
    let mut out = Vec::new();
    let mut run: Option<(BusyKind, usize)> = None;
    let close = |run: &mut Option<(BusyKind, usize)>, upto: usize, out: &mut Vec<Busy>| {
        if let Some((kind, first)) = run.take() {
            out.push(Busy { start: at(first), end: at(upto).min(end), kind });
        }
    };
    for (i, &c) in digits.iter().take(needed).enumerate() {
        let kind = match c {
            b'0' => None,
            b'1' => Some(BusyKind::Tentative),
            b'2' => Some(BusyKind::Busy),
            b'3' => Some(BusyKind::OutOfOffice),
            b'4' => Some(BusyKind::Elsewhere),
            _ => return None,
        };
        if let Some((k, _)) = run {
            if Some(k) != kind {
                close(&mut run, i, &mut out);
            }
        }
        if run.is_none() {
            if let Some(k) = kind {
                run = Some((k, i));
            }
        }
    }
    close(&mut run, needed, &mut out);
    Some(out)
}

/// Graph `workingHours` (`{daysOfWeek, startTime, endTime}`) -> WorkingHours. None if malformed.
pub fn parse_working_hours(v: &Value) -> Option<WorkingHours> {
    let mut days = [false; 7];
    for n in v["daysOfWeek"].as_array()? {
        let idx = match n.as_str()?.to_ascii_lowercase().as_str() {
            "monday" => 0,
            "tuesday" => 1,
            "wednesday" => 2,
            "thursday" => 3,
            "friday" => 4,
            "saturday" => 5,
            "sunday" => 6,
            _ => return None,
        };
        days[idx] = true;
    }
    Some(WorkingHours { days, start: graph_time(v["startTime"].as_str()?)?, end: graph_time(v["endTime"].as_str()?)? })
}

/// "08:00:00.0000000" or "08:00:00" or "08:00" -> time of day.
fn graph_time(s: &str) -> Option<NaiveTime> {
    let s = s.trim();
    let head = s.get(..s.len().min(8))?;
    let short = s.get(..s.len().min(5))?;
    NaiveTime::parse_from_str(head, "%H:%M:%S")
        .or_else(|_| NaiveTime::parse_from_str(short, "%H:%M"))
        .ok()
}

/// Busy intervals from calendar events (another account's own calendar). `showAs` free = not busy.
pub fn busy_from_events(events: &[crate::models::CalendarEvent]) -> Vec<Busy> {
    let stamp = |s: &str| NaiveDateTime::parse_from_str(s.get(..s.len().min(19)).unwrap_or(""), "%Y-%m-%dT%H:%M:%S").ok();
    events
        .iter()
        .filter(|e| e.show_as != "free")
        .filter_map(|e| {
            let (start, end) = (stamp(&e.start)?, stamp(&e.end)?);
            if end <= start {
                return None;
            }
            let kind = match e.show_as.as_str() {
                "tentative" => BusyKind::Tentative,
                "oof" => BusyKind::OutOfOffice,
                "workingElsewhere" => BusyKind::Elsewhere,
                _ => BusyKind::Busy,
            };
            Some(Busy { start, end, kind })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }
    fn dt(y: i32, m: u32, day: u32, h: u32, mi: u32) -> NaiveDateTime {
        d(y, m, day).and_hms_opt(h, mi, 0).unwrap()
    }
    fn real(_: NaiveDateTime) -> bool {
        true
    }
    fn busy(s: NaiveDateTime, e: NaiveDateTime) -> Busy {
        Busy { start: s, end: e, kind: BusyKind::Busy }
    }
    fn me(busy: Vec<Busy>) -> Person {
        Person { label: "me".into(), required: true, busy: Some(busy), hours: None }
    }
    fn req(day: NaiveDate, dur: i64) -> Request {
        Request { from: day, to: day.succ_opt().unwrap(), duration_min: dur, hours: WorkingHours::default_user(), lunch: Some(default_lunch()) }
    }
    fn starts(slots: &[Slot]) -> Vec<String> {
        slots.iter().map(|s| s.start.format("%H:%M").to_string()).collect()
    }

    // 2026-11-16 is a Monday.
    #[test]
    fn free_day_offers_slots_only_inside_working_hours() {
        let r = req(d(2026, 11, 16), 60);
        let slots = find_slots(&r, &[me(vec![])], &real, 1000);
        assert_eq!(slots.first().unwrap().start, dt(2026, 11, 16, 9, 0));
        // last 60-min start that ends by 17:30
        assert_eq!(slots.len(), 31, "9:00 to 16:30 in 15-minute steps");
        assert_eq!(slots.iter().map(|s| s.start).max(), Some(dt(2026, 11, 16, 16, 30)));
        assert!(slots.iter().all(|s| s.confirmed() && s.end <= dt(2026, 11, 16, 17, 30)));
    }

    #[test]
    fn required_busy_excludes_every_overlapping_start_and_touching_is_fine() {
        let r = req(d(2026, 11, 16), 30);
        let slots = find_slots(&r, &[me(vec![busy(dt(2026, 11, 16, 10, 0), dt(2026, 11, 16, 11, 0))])], &real, 1000);
        let s = starts(&slots);
        assert!(s.contains(&"09:30".to_string()), "09:30-10:00 touches the busy block and must stay");
        assert!(!s.contains(&"10:00".to_string()));
        assert!(!s.contains(&"10:45".to_string()), "10:45-11:15 overlaps 10:00-11:00");
        assert!(s.contains(&"11:00".to_string()));
    }

    #[test]
    fn optional_busy_is_listed_ranked_lower_never_excluded() {
        let r = req(d(2026, 11, 16), 30);
        let optional = Person { label: "opt@x".into(), required: false, busy: Some(vec![busy(dt(2026, 11, 16, 9, 0), dt(2026, 11, 16, 12, 0))]), hours: None };
        let slots = find_slots(&r, &[me(vec![]), optional], &real, 1000);
        let first = slots.first().unwrap();
        assert!(first.start >= dt(2026, 11, 16, 12, 0), "a slot with the optional person free ranks first: {:?}", first.start);
        let early = slots.iter().find(|s| s.start == dt(2026, 11, 16, 9, 0)).unwrap();
        assert_eq!(early.optional_busy, vec!["opt@x".to_string()]);
    }

    #[test]
    fn unknown_required_person_is_flagged_not_assumed_free_and_ranks_last() {
        let r = req(d(2026, 11, 16), 30);
        let ext = Person { label: "ext@other.org".into(), required: true, busy: None, hours: None };
        let slots = find_slots(&r, &[me(vec![busy(dt(2026, 11, 16, 9, 0), dt(2026, 11, 16, 10, 0))]), ext], &real, 1000);
        assert!(!slots.is_empty());
        assert!(slots.iter().all(|s| !s.confirmed() && s.unknown == vec!["ext@other.org".to_string()]));
        // Nothing is ever reported as confirmed for a person we cannot read.
        let mixed = vec![
            Person { label: "ext@other.org".into(), required: true, busy: None, hours: None },
            Person { label: "ok@x".into(), required: true, busy: Some(vec![busy(dt(2026, 11, 16, 9, 0), dt(2026, 11, 16, 13, 0))]), hours: None },
        ];
        let slots = find_slots(&req(d(2026, 11, 16), 30), &mixed, &real, 1000);
        let first_unconfirmed = slots.iter().position(|s| !s.confirmed()).unwrap_or(slots.len());
        let last_confirmed = slots.iter().rposition(|s| s.confirmed());
        assert!(last_confirmed.is_none() || last_confirmed.unwrap() < first_unconfirmed, "confirmed slots come first");
        assert!(slots.iter().all(|s| s.confirmed() == false), "the unknown person is never confirmed free");
    }

    #[test]
    fn a_known_free_person_next_to_an_unknown_one_still_gets_flagged_only_for_the_unknown() {
        let mixed = vec![
            me(vec![]),
            Person { label: "ext@other.org".into(), required: true, busy: None, hours: None },
            Person { label: "ok@x".into(), required: true, busy: Some(vec![]), hours: None },
        ];
        let slots = find_slots(&req(d(2026, 11, 16), 30), &mixed, &real, 1);
        assert_eq!(slots[0].unknown, vec!["ext@other.org".to_string()]);
    }

    #[test]
    fn a_person_hours_narrow_the_slots_and_a_day_off_removes_the_day() {
        let narrow = Person {
            label: "p@x".into(),
            required: true,
            busy: Some(vec![]),
            hours: Some(WorkingHours { days: [true; 7], start: hm(10, 0), end: hm(12, 0) }),
        };
        let slots = find_slots(&req(d(2026, 11, 16), 30), &[me(vec![]), narrow], &real, 1000);
        assert_eq!(slots.first().unwrap().start, dt(2026, 11, 16, 10, 0));
        assert_eq!(slots.last().unwrap().end, dt(2026, 11, 16, 12, 0));
        let off_monday = Person {
            label: "p@x".into(),
            required: true,
            busy: Some(vec![]),
            hours: Some(WorkingHours { days: [false, true, true, true, true, false, false], start: hm(9, 0), end: hm(17, 0) }),
        };
        assert!(find_slots(&req(d(2026, 11, 16), 30), &[me(vec![]), off_monday], &real, 1000).is_empty(), "Monday off");
    }

    #[test]
    fn weekend_is_outside_default_working_days() {
        // 2026-11-14 is a Saturday
        assert!(find_slots(&req(d(2026, 11, 14), 30), &[me(vec![])], &real, 10).is_empty());
    }

    #[test]
    fn lunch_and_next_to_busy_rank_after_clean_slots_and_earlier_wins_among_equals() {
        // Busy 09:00-10:00. 10:00 touches it (penalised); 10:15 is clean. 12:00 is lunch.
        let r = Request { hours: WorkingHours { days: [true; 7], start: hm(9, 0), end: hm(11, 0) }, ..req(d(2026, 11, 16), 15) };
        let slots = find_slots(&r, &[me(vec![busy(dt(2026, 11, 16, 9, 0), dt(2026, 11, 16, 10, 0))])], &real, 1000);
        assert_eq!(slots.first().unwrap().start, dt(2026, 11, 16, 10, 15));
        assert!(slots.iter().any(|s| s.start == dt(2026, 11, 16, 10, 0) && s.next_to_busy));
        assert!(!slots.first().unwrap().next_to_busy && !slots.first().unwrap().lunch);

        let lunchy = Request { hours: WorkingHours { days: [true; 7], start: hm(11, 30), end: hm(13, 0) }, ..req(d(2026, 11, 16), 30) };
        let slots = find_slots(&lunchy, &[me(vec![])], &real, 1000);
        let first = slots.first().unwrap();
        assert!(first.start == dt(2026, 11, 16, 11, 30) && !first.lunch, "11:30-12:00 is clean and before lunch");
        assert!(slots.iter().find(|s| s.start == dt(2026, 11, 16, 12, 0)).unwrap().lunch);
        let first_lunch = slots.iter().position(|s| s.lunch).unwrap();
        assert_eq!(first_lunch, 1, "11:30 is the only clean start; every lunch-overlapping start follows it");
        assert!(slots[..first_lunch].iter().all(|s| !s.lunch));
    }

    #[test]
    fn limit_truncates_after_ranking() {
        let slots = find_slots(&req(d(2026, 11, 16), 30), &[me(vec![])], &real, 3);
        assert_eq!(slots.len(), 3);
        assert_eq!(starts(&slots), vec!["09:00", "09:15", "09:30"]);
    }

    #[test]
    fn range_is_from_inclusive_to_exclusive_and_spans_days() {
        let r = Request { from: d(2026, 11, 16), to: d(2026, 11, 18), ..req(d(2026, 11, 16), 60) };
        let slots = find_slots(&r, &[me(vec![])], &real, 100_000);
        assert!(slots.iter().all(|s| s.start.date() != d(2026, 11, 18)));
        assert!(slots.iter().any(|s| s.start.date() == d(2026, 11, 17)));
        let empty = Request { from: d(2026, 11, 16), to: d(2026, 11, 16), ..req(d(2026, 11, 16), 60) };
        assert!(find_slots(&empty, &[me(vec![])], &real, 10).is_empty());
    }

    #[test]
    fn dst_spring_forward_gap_is_never_offered_and_a_slot_across_it_is_dropped() {
        // US spring forward 2026-03-08: local 02:00-03:00 does not exist.
        let day = d(2026, 3, 8);
        let gap = |t: NaiveDateTime| !(t.date() == day && t.time() >= hm(2, 0) && t.time() < hm(3, 0));
        let r = Request { hours: WorkingHours { days: [true; 7], start: hm(1, 0), end: hm(5, 0) }, lunch: None, ..req(day, 30) };
        let s = starts(&find_slots(&r, &[me(vec![])], &gap, 1000));
        assert!(s.contains(&"01:00".to_string()) && s.contains(&"01:15".to_string()));
        assert!(!s.contains(&"01:30".to_string()), "01:30-02:00 ends in the gap");
        assert!(!s.iter().any(|x| x == "02:00" || x == "02:15" || x == "02:30" || x == "02:45"));
        assert!(s.contains(&"03:00".to_string()) && s.contains(&"04:30".to_string()));

        let long = Request { duration_min: 120, ..r.clone() };
        let s = starts(&find_slots(&long, &[me(vec![])], &gap, 1000));
        assert!(!s.contains(&"01:00".to_string()), "01:00-03:00 crosses the gap, so it is 60 real minutes, not 120");
        assert!(s.contains(&"03:00".to_string()));
    }

    #[test]
    fn availability_digits_become_coalesced_intervals() {
        let start = dt(2026, 11, 16, 9, 0);
        let end = dt(2026, 11, 16, 11, 0);
        // 8 digits of 15 min from 09:00: 09:00 free, 09:15 free, 09:30/09:45/10:00 busy, 10:15 tentative, 10:30/10:45 free
        let v = busy_from_view("00222100", start, 15, end).unwrap();
        assert_eq!(v, vec![
            Busy { start: dt(2026, 11, 16, 9, 30), end: dt(2026, 11, 16, 10, 15), kind: BusyKind::Busy },
            Busy { start: dt(2026, 11, 16, 10, 15), end: dt(2026, 11, 16, 10, 30), kind: BusyKind::Tentative },
        ]);
        assert!(busy_from_view("00000000", start, 15, end).unwrap().is_empty());
        // out-of-office and working-elsewhere keep their kind
        let v = busy_from_view("3344", start, 15, dt(2026, 11, 16, 10, 0)).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].kind, v[0].start, v[0].end), (BusyKind::OutOfOffice, start, dt(2026, 11, 16, 9, 30)));
        assert_eq!((v[1].kind, v[1].end), (BusyKind::Elsewhere, dt(2026, 11, 16, 10, 0)));
    }

    #[test]
    fn unreadable_availability_is_unknown_never_free() {
        let start = dt(2026, 11, 16, 9, 0);
        let end = dt(2026, 11, 16, 11, 0);
        assert!(busy_from_view("", start, 15, end).is_none(), "empty view = no data");
        assert!(busy_from_view("0000", start, 15, end).is_none(), "8 slots needed, 4 given");
        assert!(busy_from_view("00x00000", start, 15, end).is_none(), "not a digit");
        assert!(busy_from_view("00000000", start, 0, end).is_none(), "zero interval");
    }

    #[test]
    fn last_partial_interval_is_clipped_to_the_window_end() {
        let start = dt(2026, 11, 16, 9, 0);
        let end = dt(2026, 11, 16, 9, 20);
        let v = busy_from_view("02", start, 15, end).unwrap();
        assert_eq!(v[0].end, end, "a 20-minute window never reports busy past 09:20");
    }

    #[test]
    fn graph_working_hours_parse() {
        let v = json!({"daysOfWeek": ["monday", "tuesday", "friday"], "startTime": "08:00:00.0000000", "endTime": "16:30:00.0000000", "timeZone": {"name": "Eastern Standard Time"}});
        let h = parse_working_hours(&v).unwrap();
        assert_eq!(h.days, [true, true, false, false, true, false, false]);
        assert_eq!((h.start, h.end), (hm(8, 0), hm(16, 30)));
        assert!(parse_working_hours(&json!({"daysOfWeek": ["funday"], "startTime": "08:00", "endTime": "09:00"})).is_none());
        assert!(parse_working_hours(&json!({"daysOfWeek": ["monday"]})).is_none());
    }

    #[test]
    fn events_busy_skips_free_and_unparseable() {
        let mut busy_ev = crate::models::CalendarEvent::default();
        busy_ev.start = "2026-11-16T09:00:00".into();
        busy_ev.end = "2026-11-16T10:00:00".into();
        busy_ev.show_as = "busy".into();
        let mut free_ev = busy_ev.clone();
        free_ev.show_as = "free".into();
        let mut bad = busy_ev.clone();
        bad.end = String::new();
        let mut oof = busy_ev.clone();
        oof.show_as = "oof".into();
        let v = busy_from_events(&[busy_ev, free_ev, bad, oof]);
        assert_eq!(v.len(), 2);
        assert_eq!(v[1].kind, BusyKind::OutOfOffice);
    }
}

// ---------------------------------------------------------------------------------------------
// Request handling: asks Graph (getSchedule for your account and attendees, calendarView for
// other accounts and for the own-calendar-only fallback), then runs the algorithm above.
// ---------------------------------------------------------------------------------------------

/// What `GET /calendar/find` asks for (parsed and checked by the route).
#[derive(Debug, Clone, PartialEq)]
pub struct FindRequest {
    /// The account the answer is for (the one chosen in the editor). Must be an Exchange account.
    pub account_id: String,
    /// `(email, required)`; the account's own address is ignored if listed.
    pub attendees: Vec<(String, bool)>,
    /// Other signed-in accounts whose calendars count as your busy time (opt-in, plan 5.6 `a`).
    pub other_accounts: Vec<String>,
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub duration_min: i64,
    pub limit: usize,
}

impl FindRequest {
    /// Checks the route's numbers and addresses; the error is shown to the user.
    pub fn validate(&self) -> Result<(), String> {
        if !(5..=1440).contains(&self.duration_min) {
            return Err("duration must be 5 to 1440 minutes".into());
        }
        if self.to <= self.from {
            return Err("to must be after from".into());
        }
        if (self.to - self.from).num_days() > MAX_DAYS {
            return Err(format!("the window is at most {} days", MAX_DAYS));
        }
        if !(1..=20).contains(&self.limit) {
            return Err("limit must be 1 to 20".into());
        }
        if self.attendees.len() > 50 {
            return Err("at most 50 attendees".into());
        }
        if let Some((bad, _)) = self.attendees.iter().find(|(a, _)| !crate::calendar_write::looks_like_address(a)) {
            return Err(format!("\"{}\" is not an email address", bad));
        }
        Ok(())
    }
}

fn is_exchange(account_id: &str) -> bool {
    matches!(crate::accounts::provider_of(account_id), "exchange" | "outlook")
}

/// Answer `GET /calendar/find`. Blocking (runs its own runtime). Returns the JSON answer or the error text.
pub fn find_free_time(config_dir: &Path, req: &FindRequest) -> Result<Value, String> {
    req.validate()?;
    if !is_exchange(&req.account_id) {
        return Err("the free-time finder needs a work or school (Exchange) account for the editor's account".into());
    }
    let db = Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db"))
        .map_err(|e| format!("database: {}", e))?;
    let self_email = db
        .account_email(&req.account_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "that account has no email address yet".to_string())?
        .to_lowercase();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    let tz = local_time_zone();
    let start = req.from.and_hms_opt(0, 0, 0).expect("midnight");
    let end = req.to.and_hms_opt(0, 0, 0).expect("midnight");
    let provider = GraphCalendarProvider::new(AuthManager::for_account(&req.account_id));

    // Who to ask: you first, then each distinct attendee.
    let mut addresses = vec![self_email.clone()];
    let mut attendees: Vec<(String, bool)> = Vec::new();
    for (email, required) in &req.attendees {
        let key = email.to_lowercase();
        if key == self_email || attendees.iter().any(|(a, _)| a.to_lowercase() == key) {
            continue;
        }
        attendees.push((key.clone(), *required));
        addresses.push(key);
    }

    // getSchedule in chunks of 20 mailboxes (Graph's limit per call is conservative here).
    let mut answers: HashMap<String, Result<ScheduleAnswer, String>> = HashMap::new();
    for chunk in addresses.chunks(20) {
        match rt.block_on(provider.get_schedule(chunk, start, end, GRID_MINUTES as u32)) {
            Ok(list) => {
                for a in list {
                    answers.insert(a.schedule_id.to_lowercase(), Ok(a));
                }
            }
            Err(e) => {
                for addr in chunk {
                    answers.insert(addr.clone(), Err(e.to_string()));
                }
            }
        }
    }
    let read = |addr: &str| -> Result<(Vec<Busy>, Option<WorkingHours>), String> {
        match answers.get(addr) {
            None => Err("no answer from free/busy".to_string()),
            Some(Err(e)) => Err(e.clone()),
            Some(Ok(a)) => {
                if let Some(err) = &a.error {
                    return Err(err.clone());
                }
                let busy = busy_from_view(&a.availability_view, start, GRID_MINUTES, end)
                    .ok_or_else(|| "free/busy came back empty or too short".to_string())?;
                Ok((busy, a.working_hours.as_ref().and_then(parse_working_hours)))
            }
        }
    };

    // Your own time: getSchedule, or the calendar itself when free/busy is not available.
    let mut mode = "people";
    let mut reason: Option<String> = None;
    let mut own_busy: Vec<Busy>;
    let mut own_hours: Option<WorkingHours> = None;
    match read(&self_email) {
        Ok((busy, hours)) => {
            own_busy = busy;
            own_hours = hours;
        }
        Err(e) => {
            mode = "own";
            reason = Some(format!("free/busy for your account is not available ({}); showing your calendar only", e));
            let events = rt.block_on(provider.fetch_window(req.from, req.to)).map_err(|e| e.to_string())?;
            own_busy = busy_from_events(&events);
        }
    }
    if attendees.is_empty() && reason.is_none() {
        mode = "own";
        reason = Some("no attendees: your calendar only".into());
    }

    // Other accounts of yours: their calendars count as your busy time.
    let mut other_unread: Vec<Value> = Vec::new();
    let mut other_checked = 0usize;
    for id in &req.other_accounts {
        if id == &req.account_id {
            continue;
        }
        if !is_exchange(id) {
            other_unread.push(json!({"account_id": id, "reason": "free/busy for this account type is not built yet"}));
            continue;
        }
        let p = GraphCalendarProvider::new(AuthManager::for_account(id));
        match rt.block_on(p.fetch_window(req.from, req.to)) {
            Ok(events) => {
                other_checked += 1;
                own_busy.extend(busy_from_events(&events));
            }
            Err(e) => other_unread.push(json!({"account_id": id, "reason": e.to_string()})),
        }
    }

    let mut people = vec![Person { label: self_email.clone(), required: true, busy: Some(own_busy), hours: own_hours }];
    let mut people_out = vec![json!({"email": self_email, "required": true, "status": "you"})];
    let mut unknown_required: Vec<String> = Vec::new();
    for (email, required) in &attendees {
        match read(email) {
            Ok((busy, hours)) => {
                people.push(Person { label: email.clone(), required: *required, busy: Some(busy), hours });
                people_out.push(json!({"email": email, "required": required, "status": "read"}));
            }
            Err(e) => {
                if *required {
                    unknown_required.push(email.clone());
                }
                people.push(Person { label: email.clone(), required: *required, busy: None, hours: None });
                people_out.push(json!({"email": email, "required": required, "status": "unknown", "reason": e}));
            }
        }
    }
    if !unknown_required.is_empty() && reason.is_none() {
        reason = Some(format!("{} required attendee(s) have no free/busy data; their slots are flagged, not assumed free", unknown_required.len()));
    }

    let request = Request {
        from: req.from,
        to: req.to,
        duration_min: req.duration_min,
        hours: WorkingHours::default_user(),
        lunch: Some(default_lunch()),
    };
    // A local time that does not exist (spring-forward gap) is not offered.
    let is_real = |t: NaiveDateTime| !matches!(Local.from_local_datetime(&t), LocalResult::None);
    let slots = find_slots(&request, &people, &is_real, req.limit);

    Ok(json!({
        "ok": true,
        "mode": mode,
        "reason": reason,
        "time_zone": tz,
        "from": req.from.to_string(),
        "to": req.to.to_string(),
        "duration_min": req.duration_min,
        "people": people_out,
        "unknown_required": unknown_required,
        "other_accounts": {"checked": other_checked, "unread": other_unread},
        "slots": slots.iter().map(|s| json!({
            "start": s.start.format("%Y-%m-%dT%H:%M:%S").to_string(),
            "end": s.end.format("%Y-%m-%dT%H:%M:%S").to_string(),
            "confirmed": s.confirmed(),
            "unknown": s.unknown,
            "optional_busy": s.optional_busy,
            "lunch": s.lunch,
            "next_to_busy": s.next_to_busy,
        })).collect::<Vec<_>>(),
    }))
}
