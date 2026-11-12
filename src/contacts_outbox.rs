//! Sends the contact writes the People view queued (docs/PLAN-contacts.md phase 2). One worker per
//! account, woken the moment a write is queued. Writes are instant: there is no undo delay, so a due
//! write goes out at once; Ctrl-Z is a reverse write (`contacts_write::undo`). A version mismatch (412)
//! stops the write and marks the contact `conflict`; nothing is overwritten.

use crate::compose::{is_transient, retry_delay_secs, MAX_ATTEMPTS};
use crate::db::Database;
use crate::providers::contacts::error_status;
use crate::providers::ContactsProvider;
use crate::sync_state;
use log::{error, info, warn};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

const IDLE: Duration = Duration::from_secs(300);
const KEEP_MS: i64 = 24 * 3600 * 1000;

pub struct ContactsOutboxWorker {
    account_id: String,
    db: Arc<Database>,
    provider: Arc<dyn ContactsProvider>,
}

/// The human part of a provider error: Graph wraps it in JSON.
pub fn short_error(e: &crate::errors::OmarchyError) -> String {
    let full = e.to_string();
    if error_status(e) == Some(403) {
        return "not allowed: sign in again to allow editing contacts".to_string();
    }
    let msg = regex::Regex::new(r#""message"\s*:\s*"((?:[^"\\]|\\.)*)""#)
        .unwrap()
        .captures(&full)
        .map(|c| c[1].replace("\\n", " ").replace("\\\"", "\""))
        .unwrap_or(full);
    msg.chars().take(300).collect()
}

impl ContactsOutboxWorker {
    pub fn new(account_id: &str, db: Arc<Database>, provider: Arc<dyn ContactsProvider>) -> Self {
        Self { account_id: account_id.to_string(), db, provider }
    }

    pub async fn run(&self) {
        match self.db.contact_outbox_fail_interrupted() {
            Ok(r) if r.is_empty() => {}
            Ok(r) => warn!("Contacts outbox {}: {} write(s) were interrupted; marked failed, not repeated", self.account_id, r.len()),
            Err(e) => error!("Contacts outbox {}: could not check for interrupted writes: {}", self.account_id, e),
        }
        let _ = self.db.contact_outbox_prune(KEEP_MS);
        info!("Contacts outbox worker started for {}", self.account_id);
        loop {
            let wake = sync_state::contact_outbox_requested();
            tokio::pin!(wake);
            let _ = wake.as_mut().enable();
            self.drain().await;
            let wait = match self.db.contact_outbox_next_due() {
                Ok(Some(t)) => Duration::from_millis((t - Database::now_ms()).max(0) as u64),
                Ok(None) => IDLE,
                Err(_) => Duration::from_secs(5),
            };
            tokio::select! {
                _ = tokio::time::sleep(wait.min(IDLE)) => {}
                _ = &mut wake => {}
            }
        }
    }

    /// Send everything that is due. Returns how many operations were attempted.
    pub async fn drain(&self) -> usize {
        let due = match self.db.contact_outbox_claim_due(Database::now_ms()) {
            Ok(d) => d,
            Err(e) => {
                error!("Contacts outbox {}: cannot claim due writes: {}", self.account_id, e);
                return 0;
            }
        };
        let n = due.len();
        // versions learned during this batch: a second edit of a contact guards against the first one's result
        let mut fresh: std::collections::HashMap<String, String> = Default::default();
        for (id, op, contact_id, payload, base_etag, attempt) in due {
            let base_etag = fresh.get(&contact_id).cloned().unwrap_or(base_etag);
            let p: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
            let folder = p["folder_id"].as_str().unwrap_or("").to_string();
            // a delete that belongs to a move waits for its copy, and is dropped if the copy failed
            if let Some(dep) = p["after_create"].as_str() {
                match self.db.contact_outbox_get(dep).ok().flatten().map(|o| o.state) {
                    Some(st) if st == "sent" => {}
                    Some(st) if st == "pending" || st == "sending" => {
                        let _ = self.db.contact_outbox_mark_retry(&id, "waiting for the copy", Database::now_ms() + 3000);
                        continue;
                    }
                    _ => {
                        let _ = self.db.contact_outbox_mark(&id, "failed", Some("not deleted: the copy in the other folder was not made"), None);
                        let _ = self.db.set_contact_state(&contact_id, "ok");
                        continue;
                    }
                }
            }
            let result = match op.as_str() {
                "create" => self.provider.create_contact(&folder, &p["patch"]).await.map(Some),
                "update" => self.provider.update_contact(&contact_id, &p["patch"], if base_etag.is_empty() { None } else { Some(&base_etag) }).await.map(Some),
                "delete" => self.provider.delete_contact(&contact_id, p["permanent"].as_bool().unwrap_or(false)).await.map(|_| None),
                other => {
                    let _ = self.db.contact_outbox_mark(&id, "failed", Some(&format!("unknown operation: {other}")), None);
                    continue;
                }
            };
            match result {
                Ok(raw) => {
                    if let Some(etag) = raw.as_ref().and_then(|r| r["@odata.etag"].as_str()) { fresh.insert(contact_id.clone(), etag.to_string()); }
                    self.succeeded(&id, &op, &contact_id, &folder, raw, p["favorite_from"].as_str())
                }
                Err(e) if is_transient(&e) && attempt < MAX_ATTEMPTS => {
                    let wait = retry_delay_secs(attempt);
                    warn!("Contacts outbox {}: {} not sent ({}); retrying in {}s", self.account_id, id, e, wait);
                    let _ = self.db.contact_outbox_mark_retry(&id, &short_error(&e), Database::now_ms() + wait * 1000);
                }
                Err(e) if error_status(&e) == Some(412) => {
                    let _ = self.db.contact_outbox_mark(&id, "conflict", Some("changed on the server since you started; nothing was written"), None);
                    let _ = self.db.set_contact_state(&contact_id, "conflict");
                }
                Err(e) => {
                    error!("Contacts outbox {}: {} failed: {}", self.account_id, id, e);
                    let msg = if error_status(&e) == Some(404) && op == "update" { "this contact no longer exists on the server".to_string() } else { short_error(&e) };
                    let _ = self.db.contact_outbox_mark(&id, "failed", Some(&msg), None);
                    let _ = self.db.set_contact_state(&contact_id, "failed");
                }
            }
        }
        n
    }

    fn succeeded(&self, id: &str, op: &str, contact_id: &str, folder: &str, raw: Option<Value>, favorite_from: Option<&str>) {
        info!("Contacts outbox {}: {} {} done", self.account_id, op, contact_id);
        let real = raw.as_ref().and_then(|r| r["id"].as_str()).unwrap_or(contact_id).to_string();
        // the operation is done before the row is judged: "is another write still waiting?"
        if let Err(e) = self.db.contact_outbox_mark(id, "sent", None, Some(&real)) {
            error!("Contacts outbox {}: sent {} but could not record it: {}", self.account_id, id, e);
        }
        match (op, raw) {
            ("delete", _) => { let _ = self.db.delete_contact_row(contact_id); }
            (_, Some(raw)) => {
                if op == "create" && real != contact_id {
                    let _ = self.db.contact_outbox_retarget(contact_id, &real);
                    let _ = self.db.move_contact_favorite(contact_id, &real);
                    let _ = self.db.delete_contact_row(contact_id);
                }
                let still = self.db.contact_outbox_open_for(&real).unwrap_or(0) > 0;
                let _ = self.db.put_contact_json(&raw, Some(folder), if still { "pending" } else { "ok" });
                if let Some(from) = favorite_from { let _ = self.db.move_contact_favorite(from, &real); }
                if let Some(etag) = raw["@odata.etag"].as_str() { let _ = self.db.contact_outbox_rebase(&real, etag); }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::{OmarchyError, Result};
    use crate::models::{Contact, ContactFolder};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

    struct Fake {
        script: Mutex<Vec<Result<Value>>>,
        calls: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl ContactsProvider for Fake {
        async fn fetch_folders(&self) -> Result<Vec<ContactFolder>> { Ok(vec![]) }
        async fn fetch_folder_contacts(&self, _f: &str, _l: usize) -> Result<Vec<Contact>> { Ok(vec![]) }
        async fn fetch_all_folder_contacts(&self, _f: &str, _tx: tokio::sync::mpsc::Sender<Vec<Contact>>) -> Result<usize> { Ok(0) }
        async fn is_token_valid(&self) -> Result<bool> { Ok(true) }
        async fn create_contact(&self, folder_id: &str, body: &Value) -> Result<Value> {
            self.calls.lock().unwrap().push(format!("create {folder_id} {body}"));
            self.script.lock().unwrap().remove(0)
        }
        async fn update_contact(&self, id: &str, body: &Value, if_match: Option<&str>) -> Result<Value> {
            self.calls.lock().unwrap().push(format!("update {id} {body} {}", if_match.unwrap_or("-")));
            self.script.lock().unwrap().remove(0)
        }
        async fn delete_contact(&self, id: &str, _permanent: bool) -> Result<()> {
            self.calls.lock().unwrap().push(format!("delete {id}"));
            self.script.lock().unwrap().remove(0).map(|_| ())
        }
    }

    fn setup(script: Vec<Result<Value>>) -> (ContactsOutboxWorker, Arc<Fake>, Arc<Database>) {
        let db = Arc::new(Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap());
        db.upsert_contact_folder(&ContactFolder { id: "f1".into(), display_name: "Contacts".into(), parent_folder_id: None }).unwrap();
        let fake = Arc::new(Fake { script: Mutex::new(script), calls: Mutex::new(vec![]) });
        (ContactsOutboxWorker::new("exchange-aaaaaa", db.clone(), fake.clone()), fake, db)
    }
    fn http(code: u16) -> Result<Value> { Err(OmarchyError::HttpError(format!("Graph API error: {code} {{\"error\":{{\"message\":\"boom\"}}}}"))) }

    #[tokio::test]
    async fn a_create_is_sent_at_once_and_the_placeholder_becomes_the_real_contact() {
        let (w, fake, db) = setup(vec![Ok(json!({"id":"REAL","@odata.etag":"W/\"1\"","givenName":"Ada","displayName":"Ada"}))]);
        let r = crate::contacts_write::save(&db, &json!({"fields":{"givenName":"Ada"},"folder_id":"f1"}), 1);
        let local = r["id"].as_str().unwrap().to_string();
        assert_eq!(w.drain().await, 1);
        assert!(fake.calls.lock().unwrap()[0].starts_with("create f1 {\"givenName\":\"Ada\"}"));
        assert!(db.get_contact_record(&local).unwrap().is_none(), "the placeholder is gone");
        let rec = db.get_contact_record("REAL").unwrap().unwrap();
        assert_eq!(rec.sync_state, "ok");
        assert_eq!(rec.etag, "W/\"1\"");
    }

    #[tokio::test]
    async fn an_update_sends_only_the_change_with_the_version_and_stores_the_answer() {
        let (w, fake, db) = setup(vec![Ok(json!({"id":"c1","@odata.etag":"W/\"2\"","givenName":"Bob","displayName":"Bob"}))]);
        db.put_contact_json(&json!({"id":"c1","@odata.etag":"W/\"1\"","givenName":"Ada"}), Some("f1"), "ok").unwrap();
        crate::contacts_write::save(&db, &json!({"id":"c1","fields":{"givenName":"Bob"}}), 1);
        w.drain().await;
        assert_eq!(fake.calls.lock().unwrap()[0], "update c1 {\"givenName\":\"Bob\"} W/\"1\"");
        let rec = db.get_contact_record("c1").unwrap().unwrap();
        assert_eq!((rec.sync_state.as_str(), rec.etag.as_str()), ("ok", "W/\"2\""));
    }

    #[tokio::test]
    async fn two_quick_edits_both_go_out_the_second_against_the_new_version() {
        let (w, fake, db) = setup(vec![
            Ok(json!({"id":"c1","@odata.etag":"W/\"2\"","givenName":"Bob"})),
            Ok(json!({"id":"c1","@odata.etag":"W/\"3\"","givenName":"Bob","surname":"Z"})),
        ]);
        db.put_contact_json(&json!({"id":"c1","@odata.etag":"W/\"1\"","givenName":"Ada"}), Some("f1"), "ok").unwrap();
        crate::contacts_write::save(&db, &json!({"id":"c1","fields":{"givenName":"Bob"}}), 1);
        crate::contacts_write::save(&db, &json!({"id":"c1","fields":{"surname":"Z"}}), 2);
        w.drain().await;
        let calls = fake.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 2);
        assert!(calls[1].ends_with("W/\"2\""), "{}", calls[1]);
        assert_eq!(db.get_contact_record("c1").unwrap().unwrap().sync_state, "ok");
    }

    #[tokio::test]
    async fn a_stale_version_is_a_conflict_and_is_not_retried() {
        let (w, _f, db) = setup(vec![http(412)]);
        db.put_contact_json(&json!({"id":"c1","@odata.etag":"W/\"1\"","givenName":"Ada"}), Some("f1"), "ok").unwrap();
        crate::contacts_write::save(&db, &json!({"id":"c1","fields":{"givenName":"Bob"}}), 1);
        w.drain().await;
        assert_eq!(db.get_contact_record("c1").unwrap().unwrap().sync_state, "conflict");
        assert_eq!(db.contact_last_error("c1").unwrap().unwrap().1, "conflict");
        assert_eq!(w.drain().await, 0);
    }

    #[tokio::test]
    async fn a_refusal_fails_with_the_providers_words_and_a_throttle_retries() {
        let (w, _f, db) = setup(vec![http(400), http(429), Ok(json!({"id":"c2","@odata.etag":"x"}))]);
        db.put_contact_json(&json!({"id":"c1","givenName":"Ada"}), Some("f1"), "ok").unwrap();
        crate::contacts_write::save(&db, &json!({"id":"c1","fields":{"givenName":"Bob"}}), 1);
        w.drain().await;
        let e = db.contact_last_error("c1").unwrap().unwrap();
        assert_eq!((e.1.as_str(), e.2.as_str()), ("failed", "boom"));
        assert_eq!(db.get_contact_record("c1").unwrap().unwrap().sync_state, "failed");
        crate::contacts_write::save(&db, &json!({"fields":{"givenName":"New"},"folder_id":"f1"}), 2);
        w.drain().await;
        assert_eq!(w.drain().await, 0, "the throttled write waits before its next attempt");
        assert_eq!(db.contact_outbox_list().unwrap().iter().filter(|o| o.state == "pending").count(), 1);
    }

    #[tokio::test]
    async fn a_move_deletes_the_original_only_after_the_copy_exists() {
        let (w, fake, db) = setup(vec![Ok(json!({"id":"NEW","@odata.etag":"W/\"1\"","givenName":"Ada"})), Ok(json!(null))]);
        db.upsert_contact_folder(&ContactFolder { id: "f2".into(), display_name: "Work".into(), parent_folder_id: Some("f1".into()) }).unwrap();
        db.put_contact_json(&json!({"id":"c1","givenName":"Ada"}), Some("f1"), "ok").unwrap();
        db.set_contact_favorite("c1", true).unwrap();
        crate::contacts_write::move_contacts(&db, &["c1".into()], "f2", 1);
        w.drain().await;
        let calls = fake.calls.lock().unwrap().clone();
        assert!(calls[0].starts_with("create f2"), "{calls:?}");
        assert_eq!(calls[1], "delete c1");
        assert!(db.get_contact_record("c1").unwrap().is_none());
        let rec = db.get_contact_record("NEW").unwrap().unwrap();
        assert_eq!((rec.folder_id.as_deref(), rec.is_favorite), (Some("f2"), true), "the favourite follows");
    }

    #[tokio::test]
    async fn a_failed_copy_keeps_the_original() {
        let (w, fake, db) = setup(vec![http(400)]);
        db.put_contact_json(&json!({"id":"c1","givenName":"Ada"}), Some("f1"), "ok").unwrap();
        crate::contacts_write::move_contacts(&db, &["c1".into()], "f2", 1);
        w.drain().await;
        assert_eq!(fake.calls.lock().unwrap().len(), 1, "the delete was never sent");
        assert_eq!(db.get_contact_record("c1").unwrap().unwrap().sync_state, "ok", "the original is back");
    }

    #[tokio::test]
    async fn a_403_says_to_sign_in_again() {
        let (w, _f, db) = setup(vec![http(403)]);
        db.put_contact_json(&json!({"id":"c1","givenName":"Ada"}), Some("f1"), "ok").unwrap();
        crate::contacts_write::save(&db, &json!({"id":"c1","fields":{"givenName":"Bob"}}), 1);
        w.drain().await;
        assert!(db.contact_last_error("c1").unwrap().unwrap().2.contains("sign in again"));
    }

    #[tokio::test]
    async fn a_delete_removes_the_row_and_a_failed_one_shows_again() {
        let (w, _f, db) = setup(vec![Ok(json!(null)), http(400)]);
        db.put_contact_json(&json!({"id":"c1","givenName":"Ada"}), Some("f1"), "ok").unwrap();
        db.put_contact_json(&json!({"id":"c2","givenName":"Bob"}), Some("f1"), "ok").unwrap();
        crate::contacts_write::delete(&db, &["c1".to_string(), "c2".to_string()], 1);
        w.drain().await;
        assert!(db.get_contact_record("c1").unwrap().is_none());
        assert_eq!(db.get_contact_record("c2").unwrap().unwrap().sync_state, "failed");
    }

    #[tokio::test]
    async fn a_write_interrupted_by_a_crash_is_reported_not_repeated() {
        let (w, fake, db) = setup(vec![]);
        db.put_contact_json(&json!({"id":"c1","givenName":"Ada"}), Some("f1"), "ok").unwrap();
        crate::contacts_write::save(&db, &json!({"id":"c1","fields":{"givenName":"Bob"}}), 1);
        db.contact_outbox_claim_due(Database::now_ms() + 1).unwrap(); // claimed, then the app died
        assert_eq!(db.contact_outbox_fail_interrupted().unwrap().len(), 1);
        assert_eq!(w.drain().await, 0);
        assert!(fake.calls.lock().unwrap().is_empty());
        assert_eq!(db.get_contact_record("c1").unwrap().unwrap().sync_state, "failed");
    }
}
