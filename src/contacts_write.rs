//! Writing contacts (docs/PLAN-contacts.md phase 2): what the editor may send, how a save changes the
//! cache at once, and what is queued for the server. Saves are instant (user decision 2026-11-11): the
//! cache changes before anything is sent, the outbox sends it a moment later, and Ctrl-Z writes the old
//! values back. A save carries only the fields that changed; Graph replaces arrays whole and clears a
//! field when it is sent empty (checked live, plan section 2.1).

use crate::db::Database;
use serde_json::{json, Map, Value};

pub const MAX_EMAILS: usize = 3;
pub const MAX_BUSINESS_PHONES: usize = 2;
pub const MAX_HOME_PHONES: usize = 2;

const TEXT: &[&str] = &[
    "givenName", "middleName", "surname", "title", "generation", "nickName", "fileAs", "displayName",
    "yomiGivenName", "yomiSurname", "yomiCompanyName", "companyName", "department", "jobTitle", "officeLocation",
    "profession", "manager", "assistantName", "spouseName", "personalNotes", "businessHomePage", "mobilePhone",
];
const LISTS: &[&str] = &["imAddresses", "children", "categories", "businessPhones", "homePhones"];
const ADDRESSES: &[&str] = &["homeAddress", "businessAddress", "otherAddress"];
const ADDRESS_PARTS: &[&str] = &["street", "city", "state", "countryOrRegion", "postalCode"];

fn empty_of(key: &str) -> Value {
    if key == "birthday" { Value::Null }
    else if key == "emailAddresses" || LISTS.contains(&key) { json!([]) }
    else if ADDRESSES.contains(&key) { json!({}) }
    else { json!("") }
}

/// Nothing there: missing, null, "", [] or {}.
pub fn is_blank(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.values().all(is_blank),
        _ => false,
    }
}

fn same(key: &str, a: &Value, b: &Value) -> bool {
    if is_blank(a) && is_blank(b) { return true; }
    if key == "birthday" {
        let d = |v: &Value| v.as_str().map(|s| s.chars().take(10).collect::<String>()).unwrap_or_default();
        return d(a) == d(b);
    }
    if key == "emailAddresses" {
        let f = |v: &Value| v.as_array().map(|l| l.iter().map(|e| (
            e["address"].as_str().unwrap_or("").to_lowercase(),
            e["name"].as_str().unwrap_or("").to_string(),
        )).filter(|(a, n)| !(a.is_empty() && n.is_empty())).collect::<Vec<_>>()).unwrap_or_default();
        let (x, y) = (f(a), f(b));
        // a name equal to the address is how Graph stores "no name"
        let norm = |l: Vec<(String, String)>| l.into_iter().map(|(a, n)| { let n = if n.to_lowercase() == a { String::new() } else { n }; (a, n) }).collect::<Vec<_>>();
        return norm(x) == norm(y);
    }
    a == b
}

fn clean_text(key: &str, v: &Value) -> Result<Value, String> {
    match v {
        Value::Null => Ok(json!("")),
        Value::String(s) => {
            if s.len() > 200_000 { return Err(format!("{key} is too long")); }
            Ok(json!(s.trim_end_matches(['\r', '\n', ' ']).trim_start_matches([' ']).to_string()))
        }
        _ => Err(format!("{key} must be text")),
    }
}

fn clean_birthday(v: &Value) -> Result<Value, String> {
    match v {
        Value::Null => Ok(Value::Null),
        Value::String(s) if s.trim().is_empty() => Ok(Value::Null),
        Value::String(s) => {
            let d: String = s.trim().chars().take(10).collect();
            chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d").map_err(|_| format!("birthday '{s}' is not a date (use YYYY-MM-DD)"))?;
            Ok(json!(d))
        }
        _ => Err("birthday must be a date".into()),
    }
}

fn clean_list(key: &str, v: &Value, max: usize) -> Result<Value, String> {
    let items: Vec<String> = match v {
        Value::Null => vec![],
        Value::Array(a) => a.iter().map(|x| x.as_str().map(|s| s.trim().to_string()).ok_or(format!("{key} must be a list of text"))).collect::<Result<Vec<_>, _>>()?,
        _ => return Err(format!("{key} must be a list")),
    };
    let items: Vec<String> = items.into_iter().filter(|s| !s.is_empty()).collect();
    if items.len() > max { return Err(format!("{key}: the server keeps at most {max}")); }
    Ok(json!(items))
}

fn clean_emails(v: &Value) -> Result<Value, String> {
    let list = match v { Value::Null => vec![], Value::Array(a) => a.clone(), _ => return Err("emailAddresses must be a list".into()) };
    let mut out = vec![];
    for e in list {
        let address = e["address"].as_str().unwrap_or("").trim().to_string();
        let name = e["name"].as_str().unwrap_or("").trim().to_string();
        if address.is_empty() && name.is_empty() { continue; }
        if address.is_empty() || !address.contains('@') || address.chars().any(char::is_whitespace) || address.starts_with('@') || address.ends_with('@') {
            return Err(format!("'{address}' is not an email address"));
        }
        out.push(if name.is_empty() { json!({ "address": address }) } else { json!({ "name": name, "address": address }) });
    }
    if out.len() > MAX_EMAILS { return Err(format!("the server keeps at most {MAX_EMAILS} email addresses per contact")); }
    Ok(json!(out))
}

fn clean_address(key: &str, v: &Value) -> Result<Value, String> {
    let o = match v { Value::Null => return Ok(json!({})), Value::Object(o) => o, _ => return Err(format!("{key} must be an address")) };
    let mut out = Map::new();
    for (k, x) in o {
        if !ADDRESS_PARTS.contains(&k.as_str()) { return Err(format!("{key}: unknown part {k}")); }
        let s = x.as_str().unwrap_or("").trim().to_string();
        out.insert(k.clone(), json!(s));
    }
    if out.values().all(is_blank) { return Ok(json!({})); }
    for p in ADDRESS_PARTS { out.entry(p.to_string()).or_insert(json!("")); }
    Ok(Value::Object(out))
}

/// The fields of a save, checked and put in the shape Graph takes. Unknown fields are refused, not dropped.
pub fn clean_fields(v: &Value) -> Result<Map<String, Value>, String> {
    let o = v.as_object().ok_or("the fields must be an object")?;
    let mut out = Map::new();
    for (k, x) in o {
        let key = k.as_str();
        let cleaned = if key == "birthday" { clean_birthday(x)? }
            else if key == "emailAddresses" { clean_emails(x)? }
            else if key == "businessPhones" { clean_list(key, x, MAX_BUSINESS_PHONES)? }
            else if key == "homePhones" { clean_list(key, x, MAX_HOME_PHONES)? }
            else if LISTS.contains(&key) { clean_list(key, x, 500)? }
            else if ADDRESSES.contains(&key) { clean_address(key, x)? }
            else if TEXT.contains(&key) { clean_text(key, x)? }
            else { return Err(format!("unknown field {key}")) };
        out.insert(k.clone(), cleaned);
    }
    Ok(out)
}

/// The patch with whatever already holds (against `base`) taken out.
pub fn only_changes(base: &Value, patch: &Map<String, Value>) -> Map<String, Value> {
    patch.iter().filter(|(k, v)| !same(k, &base[k.as_str()], v)).map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// `base` with the patch applied (a blank birthday removes the key), for the optimistic cache row.
pub fn apply_patch(base: &Value, patch: &Map<String, Value>) -> Value {
    let mut o = base.as_object().cloned().unwrap_or_default();
    for (k, v) in patch {
        if is_blank(v) && (k == "birthday") { o.remove(k); } else { o.insert(k.clone(), v.clone()); }
    }
    // the name shown follows the parts unless it was set: the server will do the same; show it now
    if patch.keys().any(|k| ["givenName", "surname", "middleName", "title", "generation"].contains(&k.as_str())) && !patch.contains_key("displayName") {
        let part = |k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let name = [part("title"), part("givenName"), part("middleName"), part("surname"), part("generation")].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
        if !name.is_empty() { o.insert("displayName".into(), json!(name)); }
    }
    Value::Object(o)
}

/// The patch that puts back what `before` held for every field `patch` touched.
pub fn inverse_patch(before: &Value, patch: &Map<String, Value>) -> Map<String, Value> {
    let mut want = Map::new();
    for k in patch.keys() {
        let b = &before[k.as_str()];
        want.insert(k.clone(), if b.is_null() || (b.is_string() && b.as_str() == Some("") ) { empty_of(k) } else { b.clone() });
    }
    clean_fields(&Value::Object(want.clone())).unwrap_or(want)
}

fn new_id(prefix: &str, now: i64, n: u32) -> String { format!("{prefix}-{now:x}-{n:03x}") }

/// `POST /contacts/save` on the account's database. `req`: `{ id?, account_id?, folder_id?, fields }`.
/// An existing contact (`id`) takes the changed fields; otherwise a new one is created in `folder_id`
/// (the account's default folder when absent).
pub fn save(db: &Database, req: &Value, seq: u32) -> Value {
    let err = |m: String| json!({ "ok": false, "error": m });
    let fields = match clean_fields(&req["fields"]) { Ok(f) => f, Err(e) => return err(e) };
    let now = Database::now_ms();
    let id = req["id"].as_str().unwrap_or("");

    if id.is_empty() {
        // ---- create
        if fields.values().all(is_blank) { return err("a contact needs at least one field".into()); }
        let folder = match req["folder_id"].as_str().filter(|f| !f.is_empty()) {
            Some(f) => f.to_string(),
            None => match db.default_contact_folder() { Ok(Some(f)) => f, _ => return err("this account's contact folder is not known yet; try again after the next sync".into()) },
        };
        let local = new_id("local", now, seq);
        let mut raw = apply_patch(&json!({ "id": local }), &fields);
        raw["id"] = json!(local);
        if let Err(e) = db.put_contact_json(&raw, Some(&folder), "pending") { return err(format!("database: {e}")); }
        let payload = json!({ "patch": Value::Object(fields), "folder_id": folder }).to_string();
        let op = new_id("cop", now, seq);
        if let Err(e) = db.contact_outbox_enqueue(&op, "create", &local, &payload, "", now) { return err(format!("database: {e}")); }
        return json!({ "ok": true, "id": local, "op": op, "kind": "create" });
    }

    // ---- update
    let rec = match db.get_contact_record(id) { Ok(Some(r)) => r, Ok(None) => return err("no such contact".into()), Err(e) => return err(format!("database: {e}")) };
    if rec.sync_state == "deleted" { return err("that contact was deleted".into()); }
    let changes = only_changes(&rec.raw, &fields);
    if changes.is_empty() { return json!({ "ok": true, "id": id, "unchanged": true }); }
    let folder = rec.folder_id.clone().unwrap_or_default();

    // an edit of a contact whose create has not gone out yet joins that create
    if id.starts_with("local-") {
        match db.contact_outbox_pending_create(id) {
            Ok(Some((op, payload))) => {
                let mut p: Value = serde_json::from_str(&payload).unwrap_or(json!({}));
                let mut patch = p["patch"].as_object().cloned().unwrap_or_default();
                for (k, v) in &changes { patch.insert(k.clone(), v.clone()); }
                patch.retain(|_, v| !is_blank(v));
                p["patch"] = Value::Object(patch.clone());
                let raw = apply_patch(&json!({ "id": id }), &patch);
                if db.contact_outbox_set_payload(&op, &p.to_string()).is_err() || db.put_contact_json(&raw, Some(&folder), "pending").is_err() { return err("database error".into()); }
                return json!({ "ok": true, "id": id, "op": op, "kind": "create" });
            }
            _ => return err("this contact is being saved right now; give it a moment and edit it again".into()),
        }
    }

    let merged = apply_patch(&rec.raw, &changes);
    if let Err(e) = db.put_contact_json(&merged, Some(&folder), "pending") { return err(format!("database: {e}")); }
    let payload = json!({ "patch": Value::Object(changes), "before": rec.raw, "folder_id": folder }).to_string();
    let op = new_id("cop", now, seq);
    if let Err(e) = db.contact_outbox_enqueue(&op, "update", id, &payload, &rec.etag, now) { return err(format!("database: {e}")); }
    json!({ "ok": true, "id": id, "op": op, "kind": "update" })
}

/// `POST /contacts/delete`: each contact is hidden at once and deleted on the server (to Deleted Items).
pub fn delete(db: &Database, ids: &[String], seq: u32) -> Value { delete_ext(db, ids, seq, false) }

/// As `delete`; `permanent` skips Deleted Items (cannot be undone: the undo stack leaves it out).
pub fn delete_ext(db: &Database, ids: &[String], seq: u32, permanent: bool) -> Value {
    let now = Database::now_ms();
    let mut done = 0;
    for (n, id) in ids.iter().enumerate() {
        let rec = match db.get_contact_record(id) { Ok(Some(r)) => r, _ => continue };
        if id.starts_with("local-") {
            // never reached the server: take the create back
            if let Ok(Some((op, _))) = db.contact_outbox_pending_create(id) {
                let _ = db.contact_outbox_mark(&op, "cancelled", None, None);
                let _ = db.delete_contact_row(id);
                done += 1;
            }
            continue;
        }
        let mut payload = json!({ "before": rec.raw, "folder_id": rec.folder_id.clone().unwrap_or_default() });
        if permanent { payload["permanent"] = json!(true); payload["undo_of"] = json!("permanent"); }
        let payload = payload.to_string();
        let op = new_id("cop", now, seq + n as u32);
        if db.set_contact_state(id, "deleted").is_ok() && db.contact_outbox_enqueue(&op, "delete", id, &payload, &rec.etag, now).is_ok() { done += 1; }
    }
    json!({ "ok": true, "deleted": done })
}

/// Add and remove categories on contacts (the tag picker, list membership). Only contacts that change get a write.
pub fn set_categories(db: &Database, ids: &[String], add: &[String], remove: &[String], seq: u32) -> Value {
    let mut changed = 0;
    let mut last_err = String::new();
    for (n, id) in ids.iter().enumerate() {
        let rec = match db.get_contact_record(id) { Ok(Some(r)) => r, _ => continue };
        let mut cats: Vec<String> = rec.raw["categories"].as_array().map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect()).unwrap_or_default();
        let before = cats.clone();
        cats.retain(|c| !remove.iter().any(|r| r.eq_ignore_ascii_case(c)));
        for a in add { if !cats.iter().any(|c| c.eq_ignore_ascii_case(a)) { cats.push(a.clone()); } }
        if cats == before { continue; }
        let r = save(db, &json!({ "id": id, "fields": { "categories": cats } }), seq + n as u32);
        if r["ok"] == true { changed += 1; } else { last_err = r["error"].as_str().unwrap_or("").to_string(); }
    }
    if changed == 0 && !last_err.is_empty() { return json!({ "ok": false, "error": last_err }); }
    json!({ "ok": true, "changed": changed })
}

/// Move contacts to another folder of the same account. Graph has no move for contacts (checked live), so
/// each one is copied into the target and the original deleted once the copy exists (the delete waits for
/// the copy, and is dropped if the copy fails). The server gives the copy a new id; favourites follow it.
pub fn move_contacts(db: &Database, ids: &[String], folder_id: &str, seq: u32) -> Value {
    let now = Database::now_ms();
    let mut moved = 0;
    for (n, id) in ids.iter().enumerate() {
        let rec = match db.get_contact_record(id) { Ok(Some(r)) => r, _ => continue };
        if rec.folder_id.as_deref() == Some(folder_id) || id.starts_with("local-") || rec.sync_state != "ok" { continue; }
        let mut patch = Map::new();
        for k in TEXT.iter().chain(LISTS.iter()).chain(ADDRESSES.iter()).chain(["emailAddresses", "birthday"].iter()) {
            let v = &rec.raw[*k];
            if !is_blank(v) { patch.insert((*k).to_string(), v.clone()); }
        }
        let patch = clean_fields(&Value::Object(patch)).unwrap_or_default();
        let local = new_id("local", now, seq + n as u32);
        let mut raw = apply_patch(&json!({ "id": local }), &patch);
        raw["id"] = json!(local);
        let create_op = new_id("cop", now, seq + n as u32);
        let delete_op = new_id("cop", now + 1, seq + n as u32);
        if db.put_contact_json(&raw, Some(folder_id), "pending").is_err() { continue; }
        let cp = json!({ "patch": Value::Object(patch), "folder_id": folder_id, "undo_of": "move", "favorite_from": id }).to_string();
        let dp = json!({ "before": rec.raw, "folder_id": rec.folder_id.clone().unwrap_or_default(), "undo_of": "move", "after_create": create_op }).to_string();
        if db.contact_outbox_enqueue(&create_op, "create", &local, &cp, "", now).is_ok()
            && db.set_contact_state(id, "deleted").is_ok()
            && db.contact_outbox_enqueue(&delete_op, "delete", id, &dp, &rec.etag, now + 1).is_ok() { moved += 1; }
    }
    json!({ "ok": true, "moved": moved })
}

/// Merge duplicates: the first id keeps its place and gains what the others have that it lacks (nothing it
/// already has is overwritten); the others are deleted. Undo reverses the last step only (the deletion).
pub fn merge(db: &Database, ids: &[String], seq: u32) -> Value {
    if ids.len() < 2 { return json!({ "ok": false, "error": "mark at least two contacts to merge" }); }
    let mut recs = vec![];
    for id in ids {
        match db.get_contact_record(id) {
            Ok(Some(r)) if r.sync_state == "ok" && !id.starts_with("local-") => recs.push(r),
            Ok(Some(_)) => return json!({ "ok": false, "error": "wait for pending changes to finish before merging" }),
            _ => return json!({ "ok": false, "error": "a marked contact no longer exists" }),
        }
    }
    if recs.iter().any(|r| r.account_id != recs[0].account_id) { return json!({ "ok": false, "error": "contacts of different accounts cannot be merged" }); }
    let others: Vec<Value> = recs[1..].iter().map(|r| r.raw.clone()).collect();
    let fields = crate::contacts_io::merge_fields(&recs[0].raw, &others);
    if !fields.is_empty() {
        let r = save(db, &json!({ "id": recs[0].id, "fields": Value::Object(fields) }), seq);
        if r["ok"] != true { return r; }
    }
    let rest: Vec<String> = recs[1..].iter().map(|r| r.id.clone()).collect();
    delete(db, &rest, seq + 1);
    json!({ "ok": true, "kept": recs[0].id, "removed": rest.len() })
}

/// `POST /contacts/undo`: put back what the newest write of this account changed. A write still
/// waiting or failed is taken back as it stands; one already sent is reversed by a new write.
pub fn undo(db: &Database, seq: u32) -> Value {
    let err = |m: &str| json!({ "ok": false, "error": m });
    let now = Database::now_ms();
    let op = match db.contact_outbox_newest_undoable(now - 15 * 60_000) { Ok(Some(o)) => o, Ok(None) => return err("nothing to undo"), Err(e) => return err(&format!("database: {e}")) };
    let payload: Value = serde_json::from_str(&op.payload).unwrap_or(json!({}));
    let folder = payload["folder_id"].as_str().unwrap_or("").to_string();
    let cid = op.contact_id.clone();
    match op.state.as_str() {
        "pending" | "failed" | "conflict" => {
            let _ = db.contact_outbox_mark(&op.id, "undone", None, None);
            match op.op.as_str() {
                "create" => { let _ = db.delete_contact_row(&cid); }
                "update" | "delete" => {
                    let before = payload["before"].clone();
                    let _ = db.put_contact_json(&before, Some(&folder), "ok");
                    if db.contact_outbox_open_for(&cid).unwrap_or(0) > 0 { let _ = db.set_contact_state(&cid, "pending"); }
                }
                _ => {}
            }
            json!({ "ok": true, "undone": op.op, "id": cid })
        }
        "sent" => {
            let _ = db.contact_outbox_mark(&op.id, "undone", None, None);
            let new_op = new_id("cop", now, seq);
            match op.op.as_str() {
                "update" => {
                    let patch = payload["patch"].as_object().cloned().unwrap_or_default();
                    let inv = inverse_patch(&payload["before"], &patch);
                    let rec = match db.get_contact_record(&cid) { Ok(Some(r)) => r, _ => return err("the contact is gone") };
                    let merged = apply_patch(&rec.raw, &inv);
                    let _ = db.put_contact_json(&merged, Some(&folder), "pending");
                    let p = json!({ "patch": Value::Object(inv), "before": rec.raw, "folder_id": folder, "undo_of": op.id }).to_string();
                    let _ = db.contact_outbox_enqueue(&new_op, "update", &cid, &p, &rec.etag, now);
                }
                "create" => {
                    // the contact exists on the server now: delete it
                    let real = op.result_id.clone().unwrap_or_default();
                    let rec = match db.get_contact_record(&real) { Ok(Some(r)) => r, _ => return err("the created contact is gone") };
                    let p = json!({ "before": rec.raw, "folder_id": folder, "undo_of": op.id }).to_string();
                    let _ = db.set_contact_state(&real, "deleted");
                    let _ = db.contact_outbox_enqueue(&new_op, "delete", &real, &p, &rec.etag, now);
                }
                "delete" => {
                    // recreate it (the server gives it a new id)
                    let before = payload["before"].clone();
                    let mut patch = Map::new();
                    for k in TEXT.iter().chain(LISTS.iter()).chain(ADDRESSES.iter()).chain(["emailAddresses", "birthday"].iter()) {
                        let v = &before[*k];
                        if !is_blank(v) { patch.insert((*k).to_string(), v.clone()); }
                    }
                    let patch = clean_fields(&Value::Object(patch)).unwrap_or_default();
                    let local = new_id("local", now, seq);
                    let mut raw = apply_patch(&json!({ "id": local }), &patch);
                    raw["id"] = json!(local);
                    let _ = db.put_contact_json(&raw, Some(&folder), "pending");
                    let p = json!({ "patch": Value::Object(patch), "folder_id": folder, "undo_of": op.id }).to_string();
                    let _ = db.contact_outbox_enqueue(&new_op, "create", &local, &p, "", now);
                }
                _ => {}
            }
            json!({ "ok": true, "undone": op.op, "id": cid })
        }
        _ => err("nothing to undo"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(v: Value) -> Map<String, Value> { v.as_object().unwrap().clone() }

    #[test]
    fn unknown_fields_and_wrong_shapes_are_refused() {
        assert!(clean_fields(&json!({"bogus": "x"})).unwrap_err().contains("unknown field bogus"));
        assert!(clean_fields(&json!({"givenName": 5})).is_err());
        assert!(clean_fields(&json!("x")).is_err());
        assert!(clean_fields(&json!({"homeAddress": {"planet": "mars"}})).is_err());
    }

    #[test]
    fn graph_limits_are_enforced_before_anything_is_sent() {
        let four = json!({"emailAddresses": [{"address":"a@x.org"},{"address":"b@x.org"},{"address":"c@x.org"},{"address":"d@x.org"}]});
        assert!(clean_fields(&four).unwrap_err().contains("at most 3"));
        assert!(clean_fields(&json!({"businessPhones": ["1","2","3"]})).is_err());
        assert!(clean_fields(&json!({"homePhones": ["1","2","3"]})).is_err());
        assert!(clean_fields(&json!({"businessPhones": ["1","2"], "homePhones": ["3","4"]})).is_ok());
    }

    #[test]
    fn email_addresses_are_checked_and_blank_rows_dropped() {
        assert!(clean_fields(&json!({"emailAddresses": [{"address":"nope"}]})).is_err());
        assert!(clean_fields(&json!({"emailAddresses": [{"address":"a b@x.org"}]})).is_err());
        assert!(clean_fields(&json!({"emailAddresses": [{"name":"Ada"}]})).is_err());
        let f = clean_fields(&json!({"emailAddresses": [{"address":" a@x.org ","name":"Ada"},{"address":"","name":""}]})).unwrap();
        assert_eq!(f["emailAddresses"], json!([{"name":"Ada","address":"a@x.org"}]));
    }

    #[test]
    fn birthdays_are_dates_and_blank_clears() {
        assert_eq!(clean_fields(&json!({"birthday":"1980-03-03T11:59:00Z"})).unwrap()["birthday"], json!("1980-03-03"));
        assert_eq!(clean_fields(&json!({"birthday":"1604-03-03"})).unwrap()["birthday"], json!("1604-03-03"));
        assert_eq!(clean_fields(&json!({"birthday":""})).unwrap()["birthday"], Value::Null);
        assert!(clean_fields(&json!({"birthday":"1980-13-40"})).is_err());
        assert!(clean_fields(&json!({"birthday":"soon"})).is_err());
    }

    #[test]
    fn blank_addresses_and_lists_clear_and_text_is_trimmed() {
        let f = clean_fields(&json!({"homeAddress": {"street":" ","city":""}, "children": ["", "K"], "nickName": null, "personalNotes": "hi\n\n"})).unwrap();
        assert_eq!(f["homeAddress"], json!({}));
        assert_eq!(f["children"], json!(["K"]));
        assert_eq!(f["nickName"], json!(""));
        assert_eq!(f["personalNotes"], json!("hi"));
        let a = clean_fields(&json!({"businessAddress": {"city": "Paris"}})).unwrap();
        assert_eq!(a["businessAddress"]["street"], json!(""), "every part is sent so a cleared part is cleared");
    }

    #[test]
    fn only_real_changes_are_kept() {
        let base = json!({"givenName":"Ada","nickName":null,"birthday":"1980-03-03T11:59:00Z","emailAddresses":[{"name":"a@x.org","address":"a@x.org"}],"homeAddress":{}});
        let patch = obj(json!({"givenName":"Ada","nickName":"","birthday":"1980-03-03","emailAddresses":[{"address":"A@x.org"}],"homeAddress":{},"surname":"L"}));
        let c = only_changes(&base, &patch);
        assert_eq!(c.keys().cloned().collect::<Vec<_>>(), vec!["surname".to_string()]);
    }

    #[test]
    fn applying_a_patch_updates_the_shown_name_unless_it_was_set() {
        let base = json!({"id":"1","givenName":"Ada","surname":"L","displayName":"Ada L"});
        let v = apply_patch(&base, &obj(json!({"surname":"Lovelace"})));
        assert_eq!(v["displayName"], json!("Ada Lovelace"));
        let v = apply_patch(&base, &obj(json!({"surname":"Lovelace","displayName":"The Countess"})));
        assert_eq!(v["displayName"], json!("The Countess"));
        let v = apply_patch(&json!({"birthday":"1980-01-01"}), &obj(json!({"birthday":null})));
        assert!(v.get("birthday").is_none());
    }

    #[test]
    fn the_inverse_patch_restores_old_values_and_clears_new_ones() {
        let before = json!({"givenName":"Ada","nickName":null,"birthday":null,"businessPhones":["1"],"homeAddress":{"city":"Paris","street":"","state":"","postalCode":"","countryOrRegion":""}});
        let patch = obj(json!({"givenName":"Bob","nickName":"Bo","birthday":"1980-01-01","businessPhones":[],"homeAddress":{"city":"Rome"}}));
        let inv = inverse_patch(&before, &patch);
        assert_eq!(inv["givenName"], json!("Ada"));
        assert_eq!(inv["nickName"], json!(""));
        assert_eq!(inv["birthday"], Value::Null);
        assert_eq!(inv["businessPhones"], json!(["1"]));
        assert_eq!(inv["homeAddress"]["city"], json!("Paris"));
    }

    fn db() -> Database {
        let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
        db.upsert_contact_folder(&crate::models::ContactFolder { id: "f1".into(), display_name: "Contacts".into(), parent_folder_id: None }).unwrap();
        db
    }

    #[test]
    fn a_new_contact_appears_at_once_and_is_queued() {
        let db = db();
        let r = save(&db, &json!({"fields": {"givenName":"Ada","emailAddresses":[{"address":"a@x.org"}]}, "folder_id": "f1"}), 1);
        assert_eq!(r["ok"], true, "{r}");
        let id = r["id"].as_str().unwrap();
        assert!(id.starts_with("local-"));
        let rec = db.get_contact_record(id).unwrap().unwrap();
        assert_eq!(rec.sync_state, "pending");
        assert_eq!(rec.raw["displayName"], json!("Ada"));
        let due = db.contact_outbox_claim_due(Database::now_ms() + 1).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].1, "create");
    }

    #[test]
    fn an_empty_new_contact_is_refused() {
        assert_eq!(save(&db(), &json!({"fields": {"givenName": "  "}, "folder_id": "f1"}), 1)["ok"], false);
    }

    #[test]
    fn an_edit_changes_the_row_at_once_and_keeps_the_old_values_for_undo() {
        let db = db();
        db.put_contact_json(&json!({"id":"c1","@odata.etag":"W/\"1\"","givenName":"Ada","surname":"L","displayName":"Ada L"}), Some("f1"), "ok").unwrap();
        let r = save(&db, &json!({"id":"c1","fields":{"givenName":"Ada","surname":"Lovelace"}}), 2);
        assert_eq!(r["ok"], true);
        let rec = db.get_contact_record("c1").unwrap().unwrap();
        assert_eq!(rec.sync_state, "pending");
        assert_eq!(rec.raw["surname"], json!("Lovelace"));
        let due = db.contact_outbox_claim_due(Database::now_ms() + 1).unwrap();
        let p: Value = serde_json::from_str(&due[0].3).unwrap();
        assert_eq!(p["patch"], json!({"surname":"Lovelace"}), "only the changed field is sent");
        assert_eq!(p["before"]["surname"], json!("L"));
        assert_eq!(due[0].4, "W/\"1\"", "the version the user saw guards the write");
    }

    #[test]
    fn saving_without_a_change_sends_nothing() {
        let db = db();
        db.put_contact_json(&json!({"id":"c1","givenName":"Ada"}), Some("f1"), "ok").unwrap();
        let r = save(&db, &json!({"id":"c1","fields":{"givenName":"Ada"}}), 2);
        assert_eq!(r["unchanged"], true);
        assert!(db.contact_outbox_claim_due(Database::now_ms() + 1).unwrap().is_empty());
    }

    #[test]
    fn editing_a_contact_not_yet_created_joins_its_create() {
        let db = db();
        let r = save(&db, &json!({"fields": {"givenName":"Ada"}, "folder_id": "f1"}), 1);
        let id = r["id"].as_str().unwrap().to_string();
        let r2 = save(&db, &json!({"id": id, "fields": {"surname":"L"}}), 2);
        assert_eq!(r2["kind"], "create");
        let due = db.contact_outbox_claim_due(Database::now_ms() + 1).unwrap();
        assert_eq!(due.len(), 1, "still one write");
        let p: Value = serde_json::from_str(&due[0].3).unwrap();
        assert_eq!(p["patch"]["givenName"], json!("Ada"));
        assert_eq!(p["patch"]["surname"], json!("L"));
    }

    #[test]
    fn delete_hides_at_once_and_a_local_one_is_simply_dropped() {
        let db = db();
        db.put_contact_json(&json!({"id":"c1","givenName":"Ada"}), Some("f1"), "ok").unwrap();
        let r = delete(&db, &["c1".to_string()], 1);
        assert_eq!(r["deleted"], 1);
        assert_eq!(db.get_contact_record("c1").unwrap().unwrap().sync_state, "deleted");
        let local = save(&db, &json!({"fields": {"givenName":"Bob"}, "folder_id": "f1"}), 5)["id"].as_str().unwrap().to_string();
        assert_eq!(delete(&db, &[local.clone()], 6)["deleted"], 1);
        assert!(db.get_contact_record(&local).unwrap().is_none());
    }

    #[test]
    fn categories_are_added_and_removed_case_blind_and_only_changed_contacts_are_written() {
        let db = db();
        db.put_contact_json(&json!({"id":"c1","categories":["Blue","red"]}), Some("f1"), "ok").unwrap();
        db.put_contact_json(&json!({"id":"c2","categories":["Blue"]}), Some("f1"), "ok").unwrap();
        let r = set_categories(&db, &["c1".into(), "c2".into()], &["blue".into(), "Green".into()], &["RED".into()], 1);
        assert_eq!(r["changed"], 2);
        let due = db.contact_outbox_claim_due(Database::now_ms() + 1).unwrap();
        let p: Vec<Value> = due.iter().map(|d| serde_json::from_str::<Value>(&d.3).unwrap()["patch"]["categories"].clone()).collect();
        assert_eq!(p[0], json!(["Blue", "Green"]));
        assert_eq!(p[1], json!(["Blue", "Green"]));
        let r = set_categories(&db, &["c1".into()], &["Blue".into()], &[], 9);
        assert_eq!(r["changed"], 0, "{r}");
    }

    #[test]
    fn a_move_copies_then_deletes_and_the_delete_waits_for_the_copy() {
        let db = db();
        db.upsert_contact_folder(&crate::models::ContactFolder { id: "f2".into(), display_name: "Work".into(), parent_folder_id: Some("f1".into()) }).unwrap();
        db.put_contact_json(&json!({"id":"c1","givenName":"Ada","emailAddresses":[{"address":"a@x.org"}],"displayName":"Ada"}), Some("f1"), "ok").unwrap();
        let r = move_contacts(&db, &["c1".into()], "f2", 1);
        assert_eq!(r["moved"], 1);
        assert_eq!(db.get_contact_record("c1").unwrap().unwrap().sync_state, "deleted");
        let due = db.contact_outbox_claim_due(Database::now_ms() + 5).unwrap();
        assert_eq!(due.len(), 2);
        assert_eq!(due[0].1, "create");
        let cp: Value = serde_json::from_str(&due[0].3).unwrap();
        assert_eq!(cp["folder_id"], "f2");
        assert_eq!(cp["patch"]["emailAddresses"][0]["address"], "a@x.org");
        let dp: Value = serde_json::from_str(&due[1].3).unwrap();
        assert_eq!(dp["after_create"], json!(due[0].0));
        // a move is not on the undo stack
        assert_eq!(undo(&db, 3)["ok"], false);
        // moving into the folder it is already in does nothing
        db.put_contact_json(&json!({"id":"c2","givenName":"B"}), Some("f2"), "ok").unwrap();
        assert_eq!(move_contacts(&db, &["c2".into()], "f2", 4)["moved"], 0);
    }

    #[test]
    fn a_permanent_delete_is_marked_and_cannot_be_undone() {
        let db = db();
        db.put_contact_json(&json!({"id":"c1","givenName":"Ada"}), Some("f1"), "ok").unwrap();
        delete_ext(&db, &["c1".into()], 1, true);
        let due = db.contact_outbox_claim_due(Database::now_ms() + 1).unwrap();
        let p: Value = serde_json::from_str(&due[0].3).unwrap();
        assert_eq!(p["permanent"], true);
        assert_eq!(undo(&db, 2)["ok"], false);
    }

    #[test]
    fn merging_fills_the_keeper_and_deletes_the_rest() {
        let db = db();
        db.put_contact_json(&json!({"id":"c1","@odata.etag":"W/\"1\"","givenName":"Ada","emailAddresses":[{"address":"a@x.org"}]}), Some("f1"), "ok").unwrap();
        db.put_contact_json(&json!({"id":"c2","givenName":"Adeline","surname":"L","mobilePhone":"5","emailAddresses":[{"address":"a@x.org"},{"address":"b@x.org"}]}), Some("f1"), "ok").unwrap();
        assert_eq!(merge(&db, &["c1".into()], 1)["ok"], false);
        let r = merge(&db, &["c1".into(), "c2".into()], 2);
        assert_eq!(r["ok"], true, "{r}");
        let k = db.get_contact_record("c1").unwrap().unwrap();
        assert_eq!(k.raw["givenName"], "Ada");
        assert_eq!(k.raw["surname"], "L");
        assert_eq!(k.raw["mobilePhone"], "5");
        assert_eq!(k.raw["emailAddresses"].as_array().unwrap().len(), 2);
        assert_eq!(db.get_contact_record("c2").unwrap().unwrap().sync_state, "deleted");
    }

    #[test]
    fn undo_of_a_waiting_edit_restores_the_row() {
        let db = db();
        db.put_contact_json(&json!({"id":"c1","givenName":"Ada","displayName":"Ada"}), Some("f1"), "ok").unwrap();
        save(&db, &json!({"id":"c1","fields":{"givenName":"Bob"}}), 1);
        let r = undo(&db, 2);
        assert_eq!(r["ok"], true, "{r}");
        let rec = db.get_contact_record("c1").unwrap().unwrap();
        assert_eq!(rec.raw["givenName"], json!("Ada"));
        assert_eq!(rec.sync_state, "ok");
        assert!(db.contact_outbox_claim_due(Database::now_ms() + 1).unwrap().is_empty());
        assert_eq!(undo(&db, 3)["ok"], false, "nothing is left to undo");
    }

    #[test]
    fn undo_of_a_sent_edit_queues_the_reverse_write() {
        let db = db();
        db.put_contact_json(&json!({"id":"c1","givenName":"Ada","displayName":"Ada"}), Some("f1"), "ok").unwrap();
        let op = save(&db, &json!({"id":"c1","fields":{"givenName":"Bob"}}), 1)["op"].as_str().unwrap().to_string();
        db.contact_outbox_claim_due(Database::now_ms() + 1).unwrap();
        db.contact_outbox_mark(&op, "sent", None, Some("c1")).unwrap();
        db.put_contact_json(&json!({"id":"c1","givenName":"Bob","displayName":"Bob"}), Some("f1"), "ok").unwrap();
        let r = undo(&db, 2);
        assert_eq!(r["ok"], true, "{r}");
        let due = db.contact_outbox_claim_due(Database::now_ms() + 1).unwrap();
        assert_eq!(due.len(), 1);
        let p: Value = serde_json::from_str(&due[0].3).unwrap();
        assert_eq!(p["patch"]["givenName"], json!("Ada"));
        assert_eq!(db.get_contact_record("c1").unwrap().unwrap().raw["givenName"], json!("Ada"));
    }

    #[test]
    fn undo_of_a_sent_delete_recreates_the_contact() {
        let db = db();
        db.put_contact_json(&json!({"id":"c1","givenName":"Ada","emailAddresses":[{"address":"a@x.org"}],"displayName":"Ada"}), Some("f1"), "ok").unwrap();
        delete(&db, &["c1".to_string()], 1);
        let due = db.contact_outbox_claim_due(Database::now_ms() + 1).unwrap();
        db.contact_outbox_mark(&due[0].0, "sent", None, None).unwrap();
        db.delete_contact_row("c1").unwrap();
        let r = undo(&db, 2);
        assert_eq!(r["ok"], true, "{r}");
        let due = db.contact_outbox_claim_due(Database::now_ms() + 1).unwrap();
        assert_eq!(due[0].1, "create");
        let p: Value = serde_json::from_str(&due[0].3).unwrap();
        assert_eq!(p["patch"]["givenName"], json!("Ada"));
        assert_eq!(p["patch"]["emailAddresses"][0]["address"], json!("a@x.org"));
    }
}
