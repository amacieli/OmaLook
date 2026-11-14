//! Unit tests (moved from db.rs; module body dedented by 4 spaces, otherwise verbatim).

use super::*;

#[test]
fn an_answer_without_a_version_keeps_the_known_one() {
    let dir = std::env::temp_dir().join(format!("omalook-etag-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Database::open_for_account(dir.join("m.db").to_str().unwrap(), "acct").unwrap();
    let mut ev = CalendarEvent { id: "e".into(), subject: "s".into(), start: "2026-10-05T09:00:00".into(), end: "2026-10-05T10:00:00".into(), etag: "v1".into(), ..Default::default() };
    db.upsert_event(&ev).unwrap();
    assert_eq!(db.event_etag("e"), "v1");
    ev.etag = String::new();
    db.upsert_event(&ev).unwrap();
    assert_eq!(db.event_etag("e"), "v1");
    ev.etag = "v2".into();
    db.upsert_event(&ev).unwrap();
    assert_eq!(db.event_etag("e"), "v2");
    db.set_event_etag("e", "").unwrap();
    assert_eq!(db.event_etag("e"), "v2", "an empty version is never stored over a known one");
    assert_eq!(db.event_etag("missing"), "");
    // the queued change records it
    db.cal_queue_event_change("q", "delete", "e", &serde_json::json!({}), 0, 0).unwrap();
    let payload: String = db.conn.query_row("SELECT payload_json FROM calendar_outbox WHERE id = 'q'", [], |r| r.get(0)).unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&payload).unwrap()["etag"], "v2");
    let _ = std::fs::remove_dir_all(&dir);
}
