//! Unit tests (moved from db.rs; module body dedented by 4 spaces, otherwise verbatim).

use super::*;

#[test]
fn queue_claim_cancel_and_retry() {
    let db = Database::open_for_account(":memory:", "a1").unwrap();
    db.outbox_enqueue("o1", "a1", "{}", 1_000).unwrap();
    db.outbox_enqueue("o2", "a1", "{}", 5_000).unwrap();
    db.outbox_enqueue("o3", "other", "{}", 0).unwrap();
    assert_eq!(db.outbox_next_due("a1").unwrap(), Some(1_000));

    // nothing is due before its time; another account's rows are never touched
    assert!(db.outbox_claim_due("a1", 999).unwrap().is_empty());
    let got = db.outbox_claim_due("a1", 1_000).unwrap();
    assert_eq!(got.iter().map(|g| g.0.as_str()).collect::<Vec<_>>(), vec!["o1"]);
    assert_eq!(got[0].2, 1, "attempt number");
    assert_eq!(db.outbox_status("o1").unwrap().unwrap().0, "sending");
    assert!(db.outbox_claim_due("a1", 1_000).unwrap().is_empty(), "a claimed message is not claimed twice");
    assert_eq!(db.outbox_next_due("a1").unwrap(), Some(5_000));

    // undo works while pending, not once sending
    assert!(db.outbox_cancel("o2").unwrap());
    assert!(!db.outbox_cancel("o2").unwrap());
    assert!(!db.outbox_cancel("o1").unwrap(), "too late: already sending");
    assert!(db.outbox_claim_due("a1", 99_999).unwrap().is_empty(), "cancelled never goes out");

    // transient failure: pending again, later, with the error kept; attempts keep counting
    db.outbox_mark_retry("o1", "offline", 7_000).unwrap();
    assert_eq!(db.outbox_next_due("a1").unwrap(), Some(7_000));
    let again = db.outbox_claim_due("a1", 7_000).unwrap();
    assert_eq!(again[0].2, 2);
    db.outbox_mark_sent("o1").unwrap();
    let st = db.outbox_status("o1").unwrap().unwrap();
    assert_eq!((st.0.as_str(), st.1), ("sent", None));
    assert!(db.outbox_status("nope").unwrap().is_none());
}

#[test]
fn a_crash_mid_send_is_reported_not_resent() {
    let db = Database::open_for_account(":memory:", "a1").unwrap();
    db.outbox_enqueue("o1", "a1", "{}", 0).unwrap();
    db.outbox_claim_due("a1", 1).unwrap();
    assert_eq!(db.outbox_fail_interrupted("a1").unwrap(), 1);
    let st = db.outbox_status("o1").unwrap().unwrap();
    assert_eq!(st.0, "failed");
    assert!(st.1.unwrap().contains("Sent folder"));
    assert!(db.outbox_claim_due("a1", 99).unwrap().is_empty());
}
