//! Unit tests (moved from db.rs; module body dedented by 4 spaces, otherwise verbatim).

use super::*;
use crate::models::EmailMessage;

fn msg(id: &str, folder: Option<&str>) -> EmailMessage {
    EmailMessage { id: id.into(), from: "a@b.c".into(), subject: format!("s {id}"), received: "2026-10-06T12:00:00Z".into(),
                   body: "p".into(), folder_id: folder.map(String::from), is_read: false, ..Default::default() }
}

#[test]
fn batch_counts_only_new_rows_and_is_idempotent() {
    let db = Database::open(":memory:").unwrap();
    let first = vec![msg("1", Some("f")), msg("2", Some("f")), msg("3", Some("f"))];
    assert_eq!(db.insert_emails_batch(&first).unwrap(), 3);
    let again = vec![msg("2", Some("f")), msg("3", Some("f")), msg("4", Some("f"))];
    assert_eq!(db.insert_emails_batch(&again).unwrap(), 1, "only id 4 is new");
    assert_eq!(db.insert_emails_batch(&[]).unwrap(), 0);
    assert!(db.email_exists("4").unwrap());
}

#[test]
fn batch_backfills_missing_folder_id_on_existing_rows() {
    let db = Database::open(":memory:").unwrap();
    db.insert_emails_batch(&[msg("1", None)]).unwrap();
    db.insert_emails_batch(&[msg("1", Some("inbox-id"))]).unwrap();
    let n: i64 = db.conn.query_row("SELECT COUNT(*) FROM messages WHERE id='1' AND folder_id='inbox-id'", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}
