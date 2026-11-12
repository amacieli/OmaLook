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
}
