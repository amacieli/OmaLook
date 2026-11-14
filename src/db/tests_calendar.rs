//! Unit tests (moved from db.rs; module body dedented by 4 spaces, otherwise verbatim).

use super::*;

fn ev(id: &str, start: &str, end: &str, subject: &str) -> CalendarEvent {
    CalendarEvent {
        id: id.into(), subject: subject.into(), body: "body".into(),
        start: start.into(), end: end.into(), is_all_day: false, time_zone: "America/New_York".into(),
        ..Default::default()
    }
}

#[test]
fn calendar_detail_fields_round_trip_and_update() {
    let db = Database::open(":memory:").unwrap();
    let mut e = ev("d1", "2026-10-05T09:00:00", "2026-10-05T10:00:00", "Review");
    e.location = "Room 4".into();
    e.organizer_email = "ann@contoso.com".into();
    e.attendees_json = r#"[{"name":"Bob","email":"bob@contoso.com","role":"required","response":"accepted"}]"#.into();
    e.my_response = "accepted".into();
    e.online_url = "https://teams.microsoft.com/l/meetup-join/q".into();
    e.reminder_minutes = Some(15);
    e.is_organizer = true;
    db.upsert_event(&e).unwrap();
    let got = db.get_events_for_month("2026-10").unwrap().remove(0);
    assert_eq!((got.location.as_str(), got.organizer_email.as_str(), got.my_response.as_str()), ("Room 4", "ann@contoso.com", "accepted"));
    assert_eq!((got.reminder_minutes, got.is_organizer, got.online_url.as_str()), (Some(15), true, "https://teams.microsoft.com/l/meetup-join/q"));
    assert!(got.attendees_json.contains("bob@contoso.com"));
    e.location = "Room 9".into();
    e.reminder_minutes = None;
    db.upsert_event(&e).unwrap();
    let got = db.get_events_for_month("2026-10").unwrap().remove(0);
    assert_eq!((got.location.as_str(), got.reminder_minutes), ("Room 9", None));
}

#[test]
fn adding_a_column_that_a_racing_thread_just_added_is_not_an_error() {
    let db = Database::open(":memory:").unwrap();
    db.add_column("ALTER TABLE calendar_events ADD COLUMN race_probe TEXT").unwrap();
    db.add_column("ALTER TABLE calendar_events ADD COLUMN race_probe TEXT").unwrap(); // second thread: "duplicate column name"
    assert!(db.add_column("ALTER TABLE no_such_table ADD COLUMN x TEXT").is_err(), "other failures still surface");
}

#[test]
fn concurrent_first_opens_of_an_old_database_all_succeed() {
    let dir = std::env::temp_dir().join(format!("omalook-race-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("m.db");
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch("CREATE TABLE calendar_events (id TEXT PRIMARY KEY, subject TEXT NOT NULL, body TEXT NOT NULL DEFAULT '',
            start_at TEXT NOT NULL, end_at TEXT NOT NULL, is_all_day BOOLEAN DEFAULT 0, time_zone TEXT, cached_at DATETIME DEFAULT CURRENT_TIMESTAMP);").unwrap();
    }
    let p = path.to_str().unwrap().to_string();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let handles: Vec<_> = (0..8).map(|_| {
        let (p, b) = (p.clone(), barrier.clone());
        std::thread::spawn(move || { b.wait(); Database::open(&p).map(|_| ()).map_err(|e| e.to_string()) })
    }).collect();
    for h in handles { h.join().unwrap().unwrap(); }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn old_calendar_table_gains_the_detail_columns() {
    // a cache created before the detail fields existed is migrated in place and keeps its rows
    let dir = std::env::temp_dir().join(format!("omalook-migr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("m.db");
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch("CREATE TABLE calendar_events (id TEXT PRIMARY KEY, subject TEXT NOT NULL, body TEXT NOT NULL DEFAULT '',
            start_at TEXT NOT NULL, end_at TEXT NOT NULL, is_all_day BOOLEAN DEFAULT 0, time_zone TEXT, cached_at DATETIME DEFAULT CURRENT_TIMESTAMP);
            INSERT INTO calendar_events (id, subject, start_at, end_at) VALUES ('old','Old','2026-10-05T09:00:00','2026-10-05T10:00:00');").unwrap();
    }
    let db = Database::open(path.to_str().unwrap()).unwrap();
    let got = db.get_events_for_month("2026-10").unwrap();
    assert_eq!((got.len(), got[0].subject.as_str(), got[0].location.as_str(), got[0].reminder_minutes), (1, "Old", "", None));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn attachment_cache_round_trips_and_tracks_real_attachments() {
    use crate::attachments::Attachment;
    let db = Database::open(":memory:").unwrap();
    assert!(db.cached_attachments("m1").unwrap().is_none());
    let a = |id: &str, inline: bool| Attachment { id: id.into(), name: format!("{}.png", id), size: 1, content_type: "image/png".into(), inline, kind: "file".into() };
    db.store_attachments("m1", &[a("logo", true)]).unwrap();
    assert_eq!(db.cached_attachments("m1").unwrap().unwrap().len(), 1);
    let has_real = |db: &Database| db.conn.query_row("SELECT has_real FROM attachment_cache WHERE message_id='m1'", [], |r| r.get::<_, i32>(0)).unwrap();
    assert_eq!(has_real(&db), 0, "an inline logo is not an attachment");
    db.store_attachments("m1", &[a("logo", true), a("scan", false)]).unwrap();
    assert_eq!(has_real(&db), 1);
    assert_eq!(db.cached_attachments("m1").unwrap().unwrap().len(), 2);
}

#[test]
fn upsert_and_month_query() {
    let db = Database::open(":memory:").unwrap();
    db.upsert_event(&ev("a", "2026-10-03T09:00:00", "2026-10-03T10:00:00", "Standup")).unwrap();
    db.upsert_event(&ev("b", "2026-09-30T23:00:00", "2026-10-01T01:00:00", "Spans in")).unwrap();
    db.upsert_event(&ev("c", "2026-11-01T09:00:00", "2026-11-01T10:00:00", "Next month")).unwrap();
    db.upsert_event(&ev("d", "2026-12-31T20:00:00", "2027-01-01T01:00:00", "Spans out")).unwrap();

    let oct = db.get_events_for_month("2026-10").unwrap();
    assert_eq!(oct.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), vec!["b", "a"]);
    assert_eq!(db.get_events_for_month("2027-01").unwrap().len(), 1);
    assert_eq!(db.get_events_for_month("2026-12").unwrap().len(), 1);

    // upsert updates in place (reschedule)
    db.upsert_event(&ev("a", "2026-10-04T09:00:00", "2026-10-04T10:00:00", "Standup moved")).unwrap();
    let oct = db.get_events_for_month("2026-10").unwrap();
    assert_eq!(oct.len(), 2);
    assert!(oct.iter().any(|e| e.subject == "Standup moved" && e.start.starts_with("2026-10-04")));
}
