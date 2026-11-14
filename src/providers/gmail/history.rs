//! Gmail incremental sync through `users.history.list`: the changes since a stored historyId,
//! instead of listing every label and comparing ids.
//!
//! Pure parts (page → changed/deleted ids, label → folder) are tested without the network.
//! Every call goes through `GoogleApi::send`, so 401/429/5xx retry and refresh like the rest.
//! Scopes and hosts are the ones the provider already uses (gmail.readonly / gmail.modify).

use super::*;
use crate::errors::OmarchyError;
use crate::providers::HistoryFetch;

/// Label → folder priority for a message carrying several labels. Trash and spam win, so a
/// trashed message is not shown in Inbox; a message with only user labels goes under the first.
const FOLDER_PRIORITY: &[&str] = &["TRASH", "SPAM", "DRAFT", "INBOX", "SENT"];

/// The folder (raw label id) a message belongs to, or None when it is in no shown folder
/// (archived, or only in All Mail/starred): such a message leaves the cache.
pub(crate) fn folder_label(label_ids: &[&str], user_labels: &HashMap<String, String>) -> Option<String> {
    FOLDER_PRIORITY
        .iter()
        .find(|p| label_ids.contains(p))
        .map(|p| p.to_string())
        .or_else(|| label_ids.iter().find(|id| user_labels.contains_key(**id)).map(|id| id.to_string()))
}

/// Message changes of one history walk, keyed by raw message id. The last event for an id wins,
/// so a message added and later deleted is only deleted.
#[derive(Default)]
pub(crate) struct HistoryChanges {
    /// true = added or label-changed, false = deleted.
    state: HashMap<String, bool>,
}

impl HistoryChanges {
    /// Fold one `history.list` page in. Within a record, deletions are applied after additions.
    pub(crate) fn absorb_page(&mut self, page: &serde_json::Value) {
        for rec in page["history"].as_array().into_iter().flatten() {
            for key in ["messagesAdded", "labelsAdded", "labelsRemoved"] {
                for item in rec[key].as_array().into_iter().flatten() {
                    if let Some(id) = item["message"]["id"].as_str() {
                        self.state.insert(id.to_string(), true);
                    }
                }
            }
            for item in rec["messagesDeleted"].as_array().into_iter().flatten() {
                if let Some(id) = item["message"]["id"].as_str() {
                    self.state.insert(id.to_string(), false);
                }
            }
        }
    }

    pub(crate) fn touched(&self) -> Vec<String> {
        let mut v: Vec<String> = self.state.iter().filter(|(_, t)| **t).map(|(id, _)| id.clone()).collect();
        v.sort();
        v
    }

    pub(crate) fn deleted(&self) -> Vec<String> {
        let mut v: Vec<String> = self.state.iter().filter(|(_, t)| !**t).map(|(id, _)| id.clone()).collect();
        v.sort();
        v
    }
}

/// `historyId` is a JSON string on the wire (a uint64); accept a number too.
fn history_id_of(v: &serde_json::Value) -> Option<String> {
    v["historyId"]
        .as_str()
        .map(str::to_string)
        .or_else(|| v["historyId"].as_u64().map(|n| n.to_string()))
}

fn is_not_found(e: &OmarchyError) -> bool {
    e.to_string().contains("Google API error: 404")
}

impl GmailProvider {
    /// The mailbox's current historyId (`users.getProfile`).
    pub(super) async fn history_id_now(&self) -> Result<String> {
        let profile = self.api.get(&format!("{}/profile", BASE)).await?;
        history_id_of(&profile).ok_or_else(|| OmarchyError::HttpError("Gmail profile has no historyId".into()))
    }

    /// Everything that changed since `start`, across all pages. Nothing is stored here.
    /// Any failed request fails the whole walk (the caller keeps its position and retries).
    pub(super) async fn fetch_history_changes(&self, start: &str) -> Result<HistoryFetch> {
        let mut changes = HistoryChanges::default();
        let mut position: Option<String> = None;
        let mut token: Option<String> = None;
        loop {
            let mut url = format!(
                "{}/history?startHistoryId={}&historyTypes=messageAdded&historyTypes=messageDeleted&historyTypes=labelAdded&historyTypes=labelRemoved",
                BASE,
                urlencoding::encode(start)
            );
            if let Some(t) = &token {
                url.push_str(&format!("&pageToken={}", urlencoding::encode(t)));
            }
            let page = match self.api.get(&url).await {
                Ok(p) => p,
                // The start id is older than the server keeps: the caller walks the mailbox again.
                Err(e) if is_not_found(&e) => return Ok(HistoryFetch::Expired),
                Err(e) => return Err(e),
            };
            changes.absorb_page(&page);
            if let Some(h) = history_id_of(&page) {
                position = Some(h);
            }
            match page["nextPageToken"].as_str() {
                Some(t) => token = Some(t.to_string()),
                None => break,
            }
        }

        let mut removed: Vec<String> = changes.deleted().iter().map(|r| scoped(&self.account_id, r)).collect();
        let mut upserts = Vec::new();
        let touched = changes.touched();
        if !touched.is_empty() {
            // Folders come from label names: refresh them first, so a label created since the
            // last poll is known when the message is placed.
            let labels = self.api.get(&format!("{}/labels", BASE)).await?;
            self.remember_labels(&labels);
            let user_labels = self.labels.lock().unwrap_or_else(|p| p.into_inner()).clone();
            for (raw, metadata) in self.fetch_changed_metadata(touched).await? {
                let Some(msg) = metadata else {
                    // Gone from the mailbox since the change.
                    removed.push(scoped(&self.account_id, &raw));
                    continue;
                };
                let ids: Vec<&str> = msg["labelIds"].as_array().into_iter().flatten().filter_map(|l| l.as_str()).collect();
                match folder_label(&ids, &user_labels) {
                    Some(label) => {
                        let folder = scoped(&self.account_id, &label);
                        if let Some(m) = parse_message(&self.account_id, &folder, &msg, &user_labels) {
                            upserts.push(m);
                        }
                    }
                    None => removed.push(scoped(&self.account_id, &raw)),
                }
            }
        }
        Ok(HistoryFetch::Changes { upserts, removed, position })
    }

    /// Metadata for changed messages, a few at a time. A 404 comes back as None (deleted); a
    /// transient failure fails the walk; any other failure skips that message (it is not stored
    /// or removed, so the cache keeps its last state for it).
    async fn fetch_changed_metadata(&self, raw_ids: Vec<String>) -> Result<Vec<(String, Option<serde_json::Value>)>> {
        let mut out = Vec::with_capacity(raw_ids.len());
        for chunk in raw_ids.chunks(FETCH_CONCURRENCY) {
            let mut set = tokio::task::JoinSet::new();
            for id in chunk {
                let api = Arc::clone(&self.api);
                let raw_id = id.clone();
                let url = metadata_url(id);
                set.spawn(async move { (raw_id, api.get(&url).await) });
            }
            while let Some(res) = set.join_next().await {
                match res {
                    Ok((raw, Ok(json))) => out.push((raw, Some(json))),
                    Ok((raw, Err(e))) if is_not_found(&e) => out.push((raw, None)),
                    Ok((_, Err(e))) if crate::compose::is_transient(&e) => return Err(e),
                    Ok((raw, Err(e))) => warn!("Gmail: skipping history change for {}: {}", raw, e),
                    Err(e) => return Err(OmarchyError::HttpError(format!("Gmail fetch task failed: {}", e))),
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::test_server::{FakeServer, Reply, Seen};
    use serde_json::json;

    fn provider_for(srv: &FakeServer) -> GmailProvider {
        GmailProvider {
            account_id: "acct".into(),
            api: Arc::new(GoogleApi::with_base_override("acct", &srv.base())),
            db_path: None,
            labels: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn meta(id: &str, labels: &[&str]) -> serde_json::Value {
        json!({
            "id": id, "threadId": "t", "labelIds": labels, "internalDate": "1790000000000", "snippet": "hi",
            "payload": {"mimeType": "text/plain", "headers": [
                {"name": "From", "value": "Ada <ada@example.com>"}, {"name": "Subject", "value": "S"}]}
        })
    }

    fn labels_json() -> String {
        json!({"labels": [
            {"id": "INBOX", "name": "INBOX", "type": "system"},
            {"id": "Label_9", "name": "Work", "type": "user"}
        ]}).to_string()
    }

    fn page(body: serde_json::Value) -> Reply {
        Reply::new(200, &body.to_string())
    }

    fn is_history(req: &Seen) -> bool {
        req.path.contains("/history?")
    }

    fn history_req_token(req: &Seen) -> Option<String> {
        req.path.split('?').nth(1)?.split('&').find_map(|kv| kv.strip_prefix("pageToken=").map(String::from))
    }

    #[test]
    fn folder_follows_priority_then_user_label() {
        let mut user = HashMap::new();
        user.insert("Label_9".to_string(), "Work".to_string());
        assert_eq!(folder_label(&["INBOX", "UNREAD"], &user).as_deref(), Some("INBOX"));
        assert_eq!(folder_label(&["INBOX", "TRASH"], &user).as_deref(), Some("TRASH"));
        assert_eq!(folder_label(&["SENT", "SPAM"], &user).as_deref(), Some("SPAM"));
        assert_eq!(folder_label(&["Label_9"], &user).as_deref(), Some("Label_9"));
        assert_eq!(folder_label(&["STARRED", "UNREAD"], &user), None, "archived: in no shown folder");
        assert_eq!(folder_label(&["Label_404"], &user), None, "unknown label is not a folder");
    }

    #[test]
    fn last_event_per_message_wins_and_deletion_beats_addition_in_one_record() {
        let mut c = HistoryChanges::default();
        c.absorb_page(&json!({"history": [
            {"id": "1", "messagesAdded": [{"message": {"id": "a"}}], "messagesDeleted": [{"message": {"id": "a"}}]},
            {"id": "2", "labelsAdded": [{"message": {"id": "b"}}]},
            {"id": "3", "messagesDeleted": [{"message": {"id": "c"}}]},
            {"id": "4", "labelsRemoved": [{"message": {"id": "c"}}]}
        ]}));
        assert_eq!(c.deleted(), vec!["a".to_string()]);
        assert_eq!(c.touched(), vec!["b".to_string(), "c".to_string()], "c was deleted then relabelled: later wins");
    }

    #[test]
    fn profile_history_id_is_read_as_string_or_number() {
        assert_eq!(history_id_of(&json!({"historyId": "42"})).as_deref(), Some("42"));
        assert_eq!(history_id_of(&json!({"historyId": 42})).as_deref(), Some("42"));
        assert_eq!(history_id_of(&json!({})), None);
    }

    #[tokio::test]
    async fn current_history_id_comes_from_the_profile() {
        let srv = FakeServer::start(vec![page(json!({"emailAddress": "a@b.c", "historyId": "300"}))]);
        assert_eq!(provider_for(&srv).history_id_now().await.unwrap(), "300");
        assert!(srv.requests()[0].path.ends_with("/users/me/profile"));
    }

    #[tokio::test]
    async fn incremental_add_delete_and_label_changes_are_applied_as_changes() {
        let srv = FakeServer::start_routed(|req| {
            if req.path.contains("/labels") {
                Reply::new(200, &labels_json())
            } else if is_history(req) {
                page(json!({"history": [
                    {"id": "101", "messagesAdded": [{"message": {"id": "m1", "labelIds": ["INBOX", "UNREAD"]}}]},
                    {"id": "102", "messagesDeleted": [{"message": {"id": "m2"}}]},
                    {"id": "103", "labelsRemoved": [{"message": {"id": "m3"}, "labelIds": ["UNREAD"]}]},
                    {"id": "104", "labelsAdded": [{"message": {"id": "m5"}, "labelIds": ["Label_9"]}]}
                ], "historyId": "104"}))
            } else if req.path.contains("/messages/m1?") {
                page(meta("m1", &["INBOX", "UNREAD"]))
            } else if req.path.contains("/messages/m3?") {
                page(meta("m3", &["INBOX"]))
            } else if req.path.contains("/messages/m5?") {
                page(meta("m5", &["Label_9"]))
            } else {
                Reply::new(599, "unexpected")
            }
        });
        let got = provider_for(&srv).fetch_history_changes("100").await.unwrap();
        let HistoryFetch::Changes { upserts, removed, position } = got else { panic!("expected changes") };
        assert_eq!(position.as_deref(), Some("104"));
        assert_eq!(removed, vec!["acct:m2".to_string()]);
        let by_id: HashMap<_, _> = upserts.iter().map(|m| (m.id.clone(), m)).collect();
        assert_eq!(by_id.len(), 3);
        let m1 = by_id["acct:m1"];
        assert_eq!(m1.folder_id.as_deref(), Some("acct:INBOX"));
        assert!(!m1.is_read);
        let m3 = by_id["acct:m3"];
        assert!(m3.is_read, "UNREAD removed: now read");
        assert_eq!(m3.folder_id.as_deref(), Some("acct:INBOX"));
        let m5 = by_id["acct:m5"];
        assert_eq!(m5.folder_id.as_deref(), Some("acct:Label_9"));
        assert_eq!(m5.categories.as_deref(), Some(&["Work".to_string()][..]), "user label becomes a tag");
        let first = srv.requests()[0].path.clone();
        assert!(first.contains("startHistoryId=100"), "{first}");
        assert!(first.contains("historyTypes=messageAdded") && first.contains("historyTypes=labelRemoved"), "{first}");
    }

    #[tokio::test]
    async fn pages_are_followed_with_the_page_token_and_merged() {
        let srv = FakeServer::start_routed(|req| {
            if is_history(req) {
                match history_req_token(req).as_deref() {
                    None => page(json!({"history": [{"id": "1", "messagesDeleted": [{"message": {"id": "a"}}]}],
                                        "nextPageToken": "P2", "historyId": "50"})),
                    Some("P2") => page(json!({"history": [{"id": "2", "messagesDeleted": [{"message": {"id": "b"}}]}],
                                              "historyId": "60"})),
                    _ => Reply::new(599, "unexpected token"),
                }
            } else {
                Reply::new(599, "unexpected")
            }
        });
        let got = provider_for(&srv).fetch_history_changes("10").await.unwrap();
        let HistoryFetch::Changes { removed, position, .. } = got else { panic!("expected changes") };
        assert_eq!(removed, vec!["acct:a".to_string(), "acct:b".to_string()]);
        assert_eq!(position.as_deref(), Some("60"), "position is the last page's");
        assert_eq!(srv.requests().len(), 2, "no metadata or labels call when nothing was added");
    }

    #[tokio::test]
    async fn too_old_start_id_is_expired_so_the_caller_walks_again() {
        let srv = FakeServer::start(vec![Reply::new(404, r#"{"error":{"code":404,"message":"Requested entity was not found."}}"#)]);
        assert!(matches!(provider_for(&srv).fetch_history_changes("1").await.unwrap(), HistoryFetch::Expired));
    }

    #[tokio::test]
    async fn empty_history_changes_nothing_and_keeps_the_position_moving() {
        let srv = FakeServer::start(vec![page(json!({"historyId": "200"}))]);
        let HistoryFetch::Changes { upserts, removed, position } =
            provider_for(&srv).fetch_history_changes("150").await.unwrap() else { panic!("expected changes") };
        assert!(upserts.is_empty() && removed.is_empty());
        assert_eq!(position.as_deref(), Some("200"));
        assert_eq!(srv.requests().len(), 1, "no labels or metadata call for an empty page");
    }

    #[tokio::test]
    async fn a_failed_later_page_fails_the_whole_walk_so_nothing_is_applied() {
        let srv = FakeServer::start_routed(|req| {
            if is_history(req) && history_req_token(req).is_none() {
                page(json!({"history": [{"id": "1", "messagesDeleted": [{"message": {"id": "a"}}]}],
                            "nextPageToken": "P2", "historyId": "50"}))
            } else {
                Reply::new(400, r#"{"error":{"code":400,"message":"Bad page"}}"#)
            }
        });
        assert!(provider_for(&srv).fetch_history_changes("10").await.is_err());
    }

    #[tokio::test]
    async fn a_transient_metadata_failure_fails_the_walk() {
        let srv = FakeServer::start_routed(|req| {
            if is_history(req) {
                page(json!({"history": [{"id": "1", "messagesAdded": [{"message": {"id": "m1"}}]}], "historyId": "9"}))
            } else if req.path.contains("/labels") {
                Reply::new(200, &labels_json())
            } else {
                Reply::new(503, "{}").header("Retry-After", "0")
            }
        });
        // m1 keeps failing with 503 (transient): the walk fails, nothing is applied.
        assert!(provider_for(&srv).fetch_history_changes("5").await.is_err());
    }

    #[tokio::test]
    async fn a_message_gone_on_the_server_is_removed_not_failed() {
        let srv = FakeServer::start_routed(|req| {
            if is_history(req) {
                page(json!({"history": [{"id": "1", "messagesAdded": [{"message": {"id": "m4"}}]}], "historyId": "9"}))
            } else if req.path.contains("/labels") {
                Reply::new(200, &labels_json())
            } else {
                Reply::new(404, r#"{"error":{"code":404,"message":"Not Found"}}"#)
            }
        });
        let HistoryFetch::Changes { upserts, removed, position } =
            provider_for(&srv).fetch_history_changes("5").await.unwrap() else { panic!("expected changes") };
        assert!(upserts.is_empty());
        assert_eq!(removed, vec!["acct:m4".to_string()]);
        assert_eq!(position.as_deref(), Some("9"));
    }
}
