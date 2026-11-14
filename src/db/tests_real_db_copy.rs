//! Unit tests (moved from db.rs; module body dedented by 4 spaces, otherwise verbatim).

use super::*;

/// Opens a COPY of a real database (path in OMARCHY_DB_COPY) and checks the account backfill.
/// Run with: OMARCHY_DB_COPY=/path/copy.db cargo test real_db_copy -- --ignored --nocapture
#[test]
#[ignore]
fn backfill_on_copy_of_real_db() {
    let path = std::env::var("OMARCHY_DB_COPY").expect("set OMARCHY_DB_COPY");
    let before = Connection::open(&path).unwrap();
    let counts = |c: &Connection| -> Vec<i64> {
        ["messages", "folders", "calendar_events"].iter()
            .map(|t| c.query_row(&format!("SELECT COUNT(*) FROM {}", t), [], |r| r.get(0)).unwrap()).collect()
    };
    let n_before = counts(&before);
    drop(before);

    let t = std::time::Instant::now();
    let db = Database::open(&path).unwrap();
    println!("open+backfill took {:?}", t.elapsed());

    assert_eq!(counts(&db.conn), n_before, "row counts must not change");
    for table in ["messages", "folders", "calendar_events"] {
        let nulls: i64 = db.conn.query_row(&format!("SELECT COUNT(*) FROM {} WHERE account_id IS NULL", table), [], |r| r.get(0)).unwrap();
        assert_eq!(nulls, 0, "{}", table);
    }
    println!("rows before/after: {:?}", n_before);
    println!("accounts: {:?}", db.list_accounts().unwrap());
    let fts: i64 = db.conn.query_row("SELECT COUNT(*) FROM messages_fts", [], |r| r.get(0)).unwrap();
    println!("messages_fts rows: {}", fts);
}
