//! Unit tests (moved from db.rs; module body dedented by 4 spaces, otherwise verbatim).

use super::*;

fn occ(id: &str, start: &str, kind: &str) -> CalendarEvent {
    CalendarEvent {
        id: id.into(), subject: "Standup".into(), start: start.into(),
        end: format!("{}:30:00", &start[..start.len() - 6]),
        time_zone: "UTC".into(), event_type: kind.into(), series_master_id: Some("M".into()), ..Default::default()
    }
}

#[test]
fn series_masters_are_stored_but_never_displayed() {
    let db = Database::open(":memory:").unwrap();
    db.upsert_event(&occ("M", "2026-10-01T09:00:00", "seriesMaster")).unwrap();
    db.upsert_event(&occ("o1", "2026-10-02T09:00:00", "occurrence")).unwrap();
    let shown = db.get_events_for_month("2026-10").unwrap();
    assert_eq!(shown.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), vec!["o1"]);
}

#[test]
fn resyncing_a_window_removes_cancelled_occurrences_and_keeps_other_windows() {
    let db = Database::open(":memory:").unwrap();
    let first = vec![occ("o1", "2026-10-05T09:00:00", "occurrence"), occ("o2", "2026-10-12T09:00:00", "occurrence"),
                     occ("o3", "2026-10-19T09:00:00", "occurrence")];
    db.replace_occurrences("2026-10-01T00:00:00", "2026-11-01T00:00:00", &first).unwrap();
    db.upsert_event(&occ("far", "2026-12-07T09:00:00", "occurrence")).unwrap();     // outside the window
    let single = CalendarEvent { id: "s".into(), subject: "One-off".into(), start: "2026-10-06T10:00:00".into(),
        end: "2026-10-06T11:00:00".into(), ..Default::default() };
    db.upsert_event(&single).unwrap();                                              // not an occurrence

    // o2 was cancelled at the server, o3 moved
    let second = vec![occ("o1", "2026-10-05T09:00:00", "occurrence"), occ("o3", "2026-10-20T09:00:00", "exception")];
    db.replace_occurrences("2026-10-01T00:00:00", "2026-11-01T00:00:00", &second).unwrap();

    let oct: Vec<_> = db.get_events_for_month("2026-10").unwrap().into_iter().map(|e| (e.id, e.start[..10].to_string())).collect();
    assert_eq!(oct, vec![("o1".into(), "2026-10-05".into()), ("s".into(), "2026-10-06".into()), ("o3".into(), "2026-10-20".into())]);
    assert_eq!(db.get_events_for_month("2026-12").unwrap().len(), 1, "other windows untouched");
}

#[test]
fn event_type_columns_are_added_to_an_existing_calendar_table() {
    let path = std::env::temp_dir().join(format!("omalook-evtype-{}", std::process::id())).to_string_lossy().to_string();
    let _ = std::fs::remove_file(&path);
    {
        let c = Connection::open(&path).unwrap();
        c.execute_batch("CREATE TABLE calendar_events (id TEXT PRIMARY KEY, subject TEXT NOT NULL, body TEXT NOT NULL DEFAULT '',
            start_at TEXT NOT NULL, end_at TEXT NOT NULL, is_all_day BOOLEAN DEFAULT 0, time_zone TEXT, cached_at DATETIME DEFAULT CURRENT_TIMESTAMP);
            INSERT INTO calendar_events (id, subject, start_at, end_at) VALUES ('e','old','2026-10-01T09:00:00','2026-10-01T10:00:00');").unwrap();
    }
    let db = Database::open(&path).unwrap();
    let e = &db.get_events_for_month("2026-10").unwrap()[0];
    assert_eq!(e.event_type, "singleInstance");
    let _ = std::fs::remove_file(&path);
}
