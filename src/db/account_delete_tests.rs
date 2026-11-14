use super::*;

fn all_tables(db: &Database) -> Vec<String> {
    db.conn.prepare("SELECT name FROM sqlite_master WHERE type='table'").unwrap()
        .query_map([], |r| r.get::<_, String>(0)).unwrap().map(|r| r.unwrap()).collect()
}

fn has_column(db: &Database, table: &str, col: &str) -> bool {
    db.conn.query_row(&format!("SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name='{}'", table, col), [], |r| r.get::<_, i64>(0)).unwrap() > 0
}

/// SEC-012 regression (from the review's p3 patch): one row per account table, delete, nothing may remain.
#[test]
fn delete_account_leaves_no_rows_behind() {
    let db = Database::open(":memory:").unwrap();
    db.insert_account("exchange-gone1", "exchange", Some("gone@x.com"), None, "{}").unwrap();
    // dl_sync_base.list_id references dl_lists (a global table, kept on purpose); the seeder uses id 1
    db.conn.execute("INSERT INTO dl_lists (id, name, name_key, created_at, updated_at) VALUES (1, 'L', 'l', 0, 0)", []).unwrap();
    let tables = all_tables(&db);
    let mut seeded = Vec::new();
    for t in &tables {
        let has = has_column(&db, t, "account_id");
        if !has || t == "accounts" { continue; }
        let cols: Vec<(String, String, bool, bool)> = db.conn.prepare(&format!("SELECT name, type, \"notnull\", dflt_value IS NOT NULL FROM pragma_table_info('{}')", t)).unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? == 1, r.get(3)?))).unwrap().map(|r| r.unwrap()).collect();
        let mut names = vec!["account_id".to_string()];
        let mut vals = vec!["'exchange-gone1'".to_string()];
        for (n, ty, nn, has_def) in &cols {
            if n == "account_id" || !*nn || *has_def { continue; }
            names.push(n.clone());
            let u = ty.to_uppercase();
            vals.push(if u.contains("INT") || u.contains("REAL") { "1".into() } else { format!("'x-{}'", n) });
        }
        let sql = format!("INSERT INTO {} ({}) VALUES ({})", t, names.join(","), vals.join(","));
        match db.conn.execute(&sql, []) { Ok(_) => seeded.push(t.clone()), Err(e) => panic!("could not seed {}: {}", t, e) }
    }
    assert_eq!(seeded.len(), ACCOUNT_TABLES.len(), "seeded {:?}", seeded);
    db.delete_account("exchange-gone1").unwrap();
    let mut left = Vec::new();
    for t in &seeded {
        let n: i64 = db.conn.query_row(&format!("SELECT COUNT(*) FROM {} WHERE account_id='exchange-gone1'", t), [], |r| r.get(0)).unwrap();
        if n > 0 { left.push(t.clone()); }
    }
    assert!(left.is_empty(), "delete_account left rows in {:?}", left);
}

/// A new table with `account_id` (or `message_id`) that nobody registered must fail here.
#[test]
fn account_tables_cover_every_account_id_table() {
    let db = Database::open(":memory:").unwrap();
    let tables = all_tables(&db);
    let mut missing = Vec::new();
    for t in &tables {
        if t == "accounts" { continue; }
        if has_column(&db, t, "account_id") && !ACCOUNT_TABLES.contains(&t.as_str()) { missing.push(t.clone()); }
        if has_column(&db, t, "message_id") && !MESSAGE_KEYED_TABLES.iter().any(|(n, _)| n == t) { missing.push(t.clone()); }
    }
    assert!(missing.is_empty(), "register in ACCOUNT_TABLES / MESSAGE_KEYED_TABLES: {:?}", missing);
    for t in ACCOUNT_TABLES { assert!(tables.contains(&t.to_string()) && has_column(&db, t, "account_id"), "stale entry {t}"); }
    for (t, c) in MESSAGE_KEYED_TABLES { assert!(has_column(&db, t, c), "stale entry {t}"); }
}

#[test]
fn delete_account_keeps_other_accounts_and_clears_fts() {
    let db = Database::open(":memory:").unwrap();
    db.insert_account("exchange-gone1", "exchange", Some("g@x.com"), None, "{}").unwrap();
    db.insert_account("exchange-keep1", "exchange", Some("k@x.com"), None, "{}").unwrap();
    for (id, acct) in [("m-gone", "exchange-gone1"), ("m-keep", "exchange-keep1")] {
        db.conn.execute("INSERT INTO messages (id, account_id, subject, from_email, body, received_at) VALUES (?1, ?2, 'zebra', 'a@b.c', 'zebra body', '2026-01-01T00:00:00Z')", params![id, acct]).unwrap();
        db.conn.execute("INSERT INTO attachment_cache (message_id, json, has_real, fetched_at) VALUES (?1, '[]', 0, 0)", params![id]).unwrap();
    }
    let keys = db.account_cache_keys("exchange-gone1").unwrap();
    assert_eq!(keys.message_ids, vec!["m-gone".to_string()]);
    db.delete_account("exchange-gone1").unwrap();
    let n = |sql: &str| db.conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(n("SELECT COUNT(*) FROM messages"), 1);
    assert_eq!(n("SELECT COUNT(*) FROM messages_fts"), 1);
    assert_eq!(n("SELECT COUNT(*) FROM messages_fts WHERE id='m-gone'"), 0);
    assert_eq!(n("SELECT COUNT(*) FROM attachment_cache"), 1);
    assert_eq!(n("SELECT COUNT(*) FROM accounts WHERE id='exchange-gone1'"), 0);
    // the delete trigger is back
    db.conn.execute("DELETE FROM messages WHERE id='m-keep'", []).unwrap();
    assert_eq!(n("SELECT COUNT(*) FROM messages_fts"), 0);
}
