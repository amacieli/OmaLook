use super::*;

#[test]
fn review_sec012_delete_account_leaves_rows_behind() {
    let db = Database::open(":memory:").unwrap();
    db.insert_account("exchange-gone1", "exchange", Some("gone@x.com"), None, "{}").unwrap();
    let tables: Vec<String> = db.conn.prepare("SELECT name FROM sqlite_master WHERE type='table'").unwrap()
        .query_map([], |r| r.get::<_, String>(0)).unwrap().map(|r| r.unwrap()).collect();
    let mut seeded = Vec::new();
    for t in &tables {
        let has = db.conn.query_row(&format!("SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name='account_id'", t), [], |r| r.get::<_, i64>(0)).unwrap() > 0;
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
        match db.conn.execute(&sql, []) { Ok(_) => seeded.push(t.clone()), Err(e) => eprintln!("could not seed {}: {}", t, e) }
    }
    db.delete_account("exchange-gone1").unwrap();
    let mut left = Vec::new();
    for t in &seeded {
        let n: i64 = db.conn.query_row(&format!("SELECT COUNT(*) FROM {} WHERE account_id='exchange-gone1'", t), [], |r| r.get(0)).unwrap();
        if n > 0 { left.push(t.clone()); }
    }
    eprintln!("REVIEW seeded {} tables; rows left after delete_account in {}: {:?}", seeded.len(), left.len(), left);
    assert!(left.is_empty(), "delete_account left rows in {:?}", left);
}
