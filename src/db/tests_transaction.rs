//! Unit tests (moved from db.rs; module body dedented by 4 spaces, otherwise verbatim).

use super::*;
use crate::models::ContactDeltaPage;

fn contact(id: &str, folder: &str) -> Contact {
    Contact { id: id.into(), display_name: format!("N-{}", id), folder_id: Some(folder.into()), ..Default::default() }
}
fn count(db: &Database) -> i64 {
    db.conn.query_row("SELECT COUNT(*) FROM contacts", [], |r| r.get(0)).unwrap()
}
fn has(db: &Database, id: &str) -> bool {
    db.conn.query_row("SELECT COUNT(*) FROM contacts WHERE id = ?1", params![id], |r| r.get::<_, i64>(0)).unwrap() == 1
}
/// Test-only: make inserting contact `id` fail with a constraint error (no schema change).
fn fail_inserts_of(db: &Database, id: &str) {
    db.conn.execute_batch(&format!(
        "CREATE TEMP TRIGGER fail_insert_{id} BEFORE INSERT ON contacts WHEN NEW.id = '{id}'
         BEGIN SELECT RAISE(ABORT, 'forced failure'); END"
    )).unwrap();
}
fn fail_deletes_of(db: &Database, id: &str) {
    db.conn.execute_batch(&format!(
        "CREATE TEMP TRIGGER fail_delete_{id} BEFORE DELETE ON contacts WHEN OLD.id = '{id}'
         BEGIN SELECT RAISE(ABORT, 'forced failure'); END"
    )).unwrap();
}

#[test]
fn in_transaction_commits_when_the_closure_is_ok() {
    let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
    let n = db.in_transaction(|| {
        db.upsert_contact(&contact("a", "f1"))?;
        db.upsert_contact(&contact("b", "f1"))?;
        Ok(2usize)
    }).unwrap();
    assert_eq!(n, 2);
    assert_eq!(count(&db), 2);
}

#[test]
fn in_transaction_rolls_back_when_the_closure_fails_and_leaves_the_connection_usable() {
    let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
    let r: Result<()> = db.in_transaction(|| {
        db.upsert_contact(&contact("a", "f1"))?;
        Err(crate::errors::OmarchyError::HttpError("boom".into()))
    });
    assert!(r.is_err());
    assert_eq!(count(&db), 0, "the write before the error must be rolled back");
    // No transaction left open: a plain write and a new transaction both work.
    db.upsert_contact(&contact("z", "f1")).unwrap();
    db.in_transaction(|| db.upsert_contact(&contact("y", "f1"))).unwrap();
    assert_eq!(count(&db), 2);
}

#[test]
fn per_row_skip_inside_one_transaction_commits_the_rest_of_the_batch() {
    let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
    fail_inserts_of(&db, "bad");
    let batch = [contact("a", "f1"), contact("bad", "f1"), contact("c", "f1")];
    let stored = db.in_transaction(|| {
        let mut ok = 0usize;
        for c in &batch {
            if let Ok(()) = db.upsert_contact(c) { ok += 1 }
        }
        Ok(ok)
    }).unwrap();
    assert_eq!(stored, 2);
    assert!(has(&db, "a") && has(&db, "c") && !has(&db, "bad"));
}

#[test]
fn a_failing_contact_in_a_delta_page_stores_nothing_from_that_page() {
    let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
    db.upsert_contact(&contact("old", "f1")).unwrap();
    fail_inserts_of(&db, "bad");
    let page = ContactDeltaPage {
        upserts: vec![contact("a", "f1"), contact("bad", "f1"), contact("c", "f1")],
        removed: vec!["old".into()],
    };
    assert!(db.apply_contact_delta_page(&page).is_err());
    assert!(!has(&db, "a"), "the first contact of a failed page must not be stored");
    assert!(!has(&db, "c"));
    assert!(has(&db, "old"), "a failed page must not remove anything either");
    // The next page applies normally.
    let ok = ContactDeltaPage { upserts: vec![contact("a", "f1")], removed: vec!["old".into()] };
    assert_eq!(db.apply_contact_delta_page(&ok).unwrap(), (1, 1));
    assert!(has(&db, "a") && !has(&db, "old"));
}

#[test]
fn purge_contacts_unseen_deletes_exactly_the_unseen_ok_rows_of_that_folder() {
    let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
    for (id, f) in [("a", "f1"), ("b", "f1"), ("c", "f1"), ("e", "f1"), ("d", "f2")] {
        db.upsert_contact(&contact(id, f)).unwrap();
    }
    // "e" has a write of ours in flight: a purge must leave it alone.
    db.conn.execute("UPDATE contacts SET sync_state = 'pending' WHERE id = 'e'", []).unwrap();
    let seen: std::collections::HashSet<String> = ["a".to_string()].into_iter().collect();
    assert_eq!(db.purge_contacts_unseen("f1", &seen).unwrap(), 2);
    assert!(has(&db, "a") && has(&db, "e") && has(&db, "d"));
    assert!(!has(&db, "b") && !has(&db, "c"));
    assert_eq!(count(&db), 3);
}

#[test]
fn a_failing_purge_delete_rolls_back_the_earlier_deletes() {
    let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
    for id in ["a", "b", "c"] {
        db.upsert_contact(&contact(id, "f1")).unwrap();
    }
    fail_deletes_of(&db, "c");
    let seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    assert!(db.purge_contacts_unseen("f1", &seen).is_err());
    assert_eq!(count(&db), 3, "deletes made before the failure must be rolled back");
}
