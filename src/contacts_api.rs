//! The People view's backend answers (docs/PLAN-contacts.md): pure functions over the database, so
//! the raw-TCP routes in `main.rs` stay thin and these can be tested without a server.

use crate::db::Database;
use serde_json::{json, Value};

/// `GET /contacts/get?id=`: the whole contact as the provider last said it, plus where it lives and
/// whether a write of ours is waiting, failed or conflicts.
pub fn detail(db: &Database, id: &str) -> Value {
    let rec = match db.get_contact_record(id) {
        Ok(Some(r)) => r,
        Ok(None) => return json!({ "ok": false, "error": "no such contact" }),
        Err(e) => return json!({ "ok": false, "error": format!("database: {}", e) }),
    };
    let (account_email, folder_name) = db.contact_context(id).unwrap_or_default();
    let last = db.contact_last_error(id).ok().flatten();
    json!({
        "ok": true,
        "id": rec.id,
        "etag": rec.etag,
        "account_id": rec.account_id,
        "account_email": account_email,
        "folder_id": rec.folder_id,
        "folder_name": folder_name,
        "is_favorite": rec.is_favorite,
        "sync_state": rec.sync_state,
        "problem": last.map(|(op, state, error)| json!({ "op": op, "state": state, "error": error })),
        "contact": rec.raw,
    })
}

/// `GET /contacts/folders?account=`
pub fn folders(db: &Database, account: Option<&str>) -> Value {
    match db.list_contact_folders_ext(account) {
        Ok(f) => json!({ "ok": true, "folders": f }),
        Err(e) => json!({ "ok": false, "error": format!("database: {}", e) }),
    }
}

/// `GET /contacts?view=duplicates`: the rows of groups that look like one person, each group together and
/// marked with `dup_group` (1, 2, ...). `rows` is the ordinary list for the same filters.
pub fn duplicates_view(db: &Database, rows: Vec<crate::models::ContactRow>) -> Vec<Value> {
    let pairs: Vec<(String, Value)> = rows.iter().map(|r| {
        let raw = db.get_contact_record(&r.contact.id).ok().flatten().map(|c| c.raw).unwrap_or(Value::Null);
        // a cache row without the full data still has a name and addresses
        let raw = if raw.is_null() { json!({ "givenName": r.contact.given_name, "surname": r.contact.surname, "emailAddresses": r.contact.emails.iter().map(|e| json!({"address": e})).collect::<Vec<_>>() }) } else { raw };
        (r.contact.id.clone(), raw)
    }).collect();
    let groups = crate::contacts_io::duplicate_groups(&pairs);
    let mut out = vec![];
    for (n, g) in groups.iter().enumerate() {
        for id in g {
            if let Some(r) = rows.iter().find(|r| &r.contact.id == id) {
                let mut v = serde_json::to_value(r).unwrap_or(Value::Null);
                v["dup_group"] = json!(n + 1);
                out.push(v);
            }
        }
    }
    out
}

/// `POST /contacts/export`: the contacts to a file in the downloads folder. `format`: vcf | csv.
pub fn export(db: &Database, ids: &[String], format: &str, dir: &std::path::Path) -> Value {
    let raws: Vec<Value> = ids.iter().filter_map(|i| db.get_contact_record(i).ok().flatten()).filter(|r| r.sync_state != "deleted").map(|r| r.raw).filter(|r| !r.is_null()).collect();
    if raws.is_empty() { return json!({ "ok": false, "error": "nothing to export" }); }
    let (ext, body) = match format {
        "csv" => ("csv", crate::contacts_io::to_csv(&raws)),
        _ => ("vcf", raws.iter().map(crate::contacts_io::to_vcard).collect::<String>()),
    };
    if std::fs::create_dir_all(dir).is_err() { return json!({ "ok": false, "error": "cannot create the downloads folder" }); }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut path = dir.join(format!("omalook-contacts-{stamp}.{ext}"));
    let mut n = 1;
    while path.exists() { path = dir.join(format!("omalook-contacts-{stamp}-{n}.{ext}")); n += 1; }
    match std::fs::write(&path, body) {
        Ok(_) => json!({ "ok": true, "path": path.to_string_lossy(), "count": raws.len() }),
        Err(e) => json!({ "ok": false, "error": format!("could not write the file: {e}") }),
    }
}

/// `POST /contacts/import`: a vCard or CSV file into one folder. Contacts whose first address is already in
/// the account are skipped, not duplicated; what does not fit the server's limits is counted.
pub fn import(db: &Database, path: &std::path::Path, folder_id: &str, seq: u32) -> Value {
    let err = |m: String| json!({ "ok": false, "error": m });
    let meta = match std::fs::metadata(path) { Ok(m) if m.is_file() => m, _ => return err(format!("no such file: {}", path.display())) };
    if meta.len() > 20 * 1024 * 1024 { return err("that file is larger than 20 MB".into()); }
    let bytes = match std::fs::read(path) { Ok(b) => b, Err(e) => return err(format!("could not read it: {e}")) };
    let text = String::from_utf8_lossy(&bytes).to_string();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let items = match ext.as_str() {
        "vcf" | "vcard" => crate::contacts_io::parse_vcards(&text),
        "csv" => crate::contacts_io::parse_csv(&text),
        _ => return err("only .vcf and .csv files can be imported".into()),
    };
    if items.is_empty() { return err("no contacts found in that file".into()); }
    if items.len() > 3000 { return err(format!("{} contacts in one file is too many; split it (3000 at a time)", items.len())); }
    let (mut created, mut skipped, mut failed, mut dropped) = (0, 0, 0, 0);
    let mut first_error = String::new();
    for (n, it) in items.into_iter().enumerate() {
        dropped += it.dropped.len();
        let first = it.fields.get("emailAddresses").and_then(|e| e[0]["address"].as_str()).map(String::from);
        if let Some(a) = &first {
            if db.contacts_by_first_address(a).map(|v| !v.is_empty()).unwrap_or(false) { skipped += 1; continue; }
        }
        let r = crate::contacts_write::save(db, &json!({ "fields": Value::Object(it.fields), "folder_id": folder_id }), seq + n as u32);
        if r["ok"] == true { created += 1; } else { failed += 1; if first_error.is_empty() { first_error = r["error"].as_str().unwrap_or("").to_string(); } }
    }
    json!({ "ok": true, "created": created, "skipped_existing": skipped, "failed": failed, "dropped_fields": dropped, "first_error": first_error })
}

/// `POST /contacts/quick`: add a sender (name, address) to the contacts without opening the editor.
pub fn quick_add(db: &Database, name: &str, address: &str, seq: u32) -> Value {
    let address = address.trim();
    if !address.contains('@') { return json!({ "ok": false, "error": "no address to add" }); }
    if let Ok(found) = db.contacts_by_first_address(address) {
        if let Some(c) = found.first() { return json!({ "ok": false, "error": format!("already in your contacts as {}", if c.name.is_empty() { address.to_string() } else { c.name.clone() }) }); }
    }
    let name = name.trim();
    let mut fields = json!({ "emailAddresses": [{ "address": address }] });
    if !name.is_empty() && !name.contains('@') {
        let (first, last) = match name.rsplit_once(' ') { Some((f, l)) => (f.trim().to_string(), l.trim().to_string()), None => (name.to_string(), String::new()) };
        fields["givenName"] = json!(first);
        if !last.is_empty() { fields["surname"] = json!(last); }
    }
    let r = crate::contacts_write::save(db, &json!({ "fields": fields }), seq);
    if r["ok"] == true { json!({ "ok": true, "id": r["id"], "name": name }) } else { r }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_carries_the_whole_contact_and_where_it_lives() {
        let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
        db.put_contact_json(&json!({"id": "c1", "@odata.etag": "W/\"1\"", "displayName": "Ada", "nickName": "Al"}), Some("f1"), "ok").unwrap();
        let d = detail(&db, "c1");
        assert_eq!(d["ok"], true);
        assert_eq!(d["contact"]["nickName"], "Al");
        assert_eq!(d["etag"], "W/\"1\"");
        assert_eq!(d["sync_state"], "ok");
        assert_eq!(detail(&db, "nope")["ok"], false);
    }

    #[test]
    fn detail_reports_a_failed_write() {
        let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
        db.put_contact_json(&json!({"id": "c1", "displayName": "Ada"}), None, "failed").unwrap();
        db.contact_outbox_enqueue("op1", "update", "c1", "{}", "", 0).unwrap();
        db.contact_outbox_mark("op1", "failed", Some("refused"), None).unwrap();
        let d = detail(&db, "c1");
        assert_eq!(d["problem"]["state"], "failed");
        assert_eq!(d["problem"]["error"], "refused");
    }

    #[test]
    fn folders_list_counts() {
        let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
        db.upsert_contact_folder(&crate::models::ContactFolder { id: "f1".into(), display_name: "Contacts".into(), parent_folder_id: None }).unwrap();
        db.put_contact_json(&json!({"id": "c1", "displayName": "Ada"}), Some("f1"), "ok").unwrap();
        let f = folders(&db, None);
        assert_eq!(f["folders"][0]["count"], 1);
    }

    #[test]
    fn export_import_round_trip_through_files_and_skip_what_exists() {
        let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
        db.upsert_contact_folder(&crate::models::ContactFolder { id: "f1".into(), display_name: "Contacts".into(), parent_folder_id: None }).unwrap();
        db.put_contact_json(&json!({"id":"c1","givenName":"Ada","surname":"L","emailAddresses":[{"address":"a@x.org"}],"mobilePhone":"5"}), Some("f1"), "ok").unwrap();
        db.put_contact_json(&json!({"id":"c2","givenName":"Bob","emailAddresses":[{"address":"b@x.org"}]}), Some("f1"), "ok").unwrap();
        let dir = std::env::temp_dir().join(format!("ol-io-{}", std::process::id()));
        for fmt in ["vcf", "csv"] {
            let e = export(&db, &["c1".into(), "c2".into(), "nope".into()], fmt, &dir);
            assert_eq!(e["ok"], true, "{e}");
            assert_eq!(e["count"], 2);
            let path = std::path::PathBuf::from(e["path"].as_str().unwrap());
            // importing into the same account: both exist, so both are skipped
            let i = import(&db, &path, "f1", 1);
            assert_eq!((i["created"].as_i64(), i["skipped_existing"].as_i64()), (Some(0), Some(2)), "{i}");
            db.delete_contact_row("c2").unwrap();
            let i = import(&db, &path, "f1", 10);
            assert_eq!((i["created"].as_i64(), i["skipped_existing"].as_i64()), (Some(1), Some(1)), "{i}");
            // the created one is queued with Bob's data
            let due = db.contact_outbox_claim_due(Database::now_ms() + 1).unwrap();
            let p: Value = serde_json::from_str(&due.last().unwrap().3).unwrap();
            assert_eq!(p["patch"]["givenName"], "Bob");
            assert_eq!(p["folder_id"], "f1");
            db.put_contact_json(&json!({"id":"c2","givenName":"Bob","emailAddresses":[{"address":"b@x.org"}]}), Some("f1"), "ok").unwrap();
            db.delete_contact_row(&due.last().unwrap().2).unwrap();
        }
        assert_eq!(import(&db, std::path::Path::new("/nonexistent.vcf"), "f1", 1)["ok"], false);
        let bad = dir.join("x.txt"); std::fs::write(&bad, "hi").unwrap();
        assert!(import(&db, &bad, "f1", 1)["error"].as_str().unwrap().contains("only .vcf and .csv"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quick_add_splits_the_name_and_refuses_a_known_address() {
        let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
        db.upsert_contact_folder(&crate::models::ContactFolder { id: "f1".into(), display_name: "Contacts".into(), parent_folder_id: None }).unwrap();
        assert_eq!(quick_add(&db, "Mary Ann Smith", "m@x.org", 1)["ok"], true);
        let due = db.contact_outbox_claim_due(Database::now_ms() + 1).unwrap();
        let p: Value = serde_json::from_str(&due[0].3).unwrap();
        assert_eq!((p["patch"]["givenName"].as_str(), p["patch"]["surname"].as_str()), (Some("Mary Ann"), Some("Smith")));
        assert!(quick_add(&db, "Mary", "M@x.org", 2)["error"].as_str().unwrap().contains("already in your contacts"));
        assert_eq!(quick_add(&db, "", "no-at", 3)["ok"], false);
        assert_eq!(quick_add(&db, "a@b.org", "a@b.org", 4)["ok"], true, "a name that is the address is not split");
    }
}
