//! Draft sync (docs/PLAN-compose.md Phase B2): pushes local drafts to the provider's Drafts folder,
//! and sends a draft from its provider copy.
//!
//! Rules (see also `db/drafts.rs`):
//! * The local row is never removed before the provider has confirmed a delete or send.
//! * Pushes are debounced: an autosaved draft waits `DEBOUNCE_MS` after its last change; a draft
//!   the user saved, closed or quit with goes at once.
//! * Network / 5xx / 429 / auth failures leave the draft dirty and retry with backoff. Other 4xx
//!   answers and local problems park the draft (it stays local and dirty) until the next save.
//! * A conflict (the provider copy changed since our last push, or was deleted elsewhere) keeps
//!   both: our version is created as a new draft and the draft row is told to say so.
//! * A draft is sent from its provider copy, so neither Drafts nor Sent gets a duplicate. A
//!   draft that is gone at send time is reported, not sent again.
//! * Attachments are copied into `attach_spool::snapshot_draft` (keyed by draft id); a copy that is
//!   missing blocks the push and the send, and shows as a '!' chip on resume.

use crate::compose::{is_transient, OutgoingAttachment, OutgoingMessage};
use crate::db::drafts::{DraftRow, PARKED};
use crate::db::Database;
use crate::errors::{OmarchyError, Result};
use crate::providers::EmailProvider;
use log::{info, warn};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// An autosaved draft is pushed this long after its last change.
pub const DEBOUNCE_MS: i64 = 30_000;

/// Gmail: above this many bytes of attachments a draft is pushed only on save / close / send.
pub const LARGE_PUSH_BYTES: u64 = 10 * 1024 * 1024;

/// `<cache>/omalook/drafts`: the spool of draft attachments, one directory per draft id.
pub fn default_root() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join("omalook").join("drafts")
}

/// Drafts whose provider copy is being sent right now (the push worker keeps off them).
static SENDING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn claim(id: &str) -> bool {
    let mut g = SENDING.lock().unwrap_or_else(|p| p.into_inner());
    g.get_or_insert_with(HashSet::new).insert(id.to_string())
}

fn release(id: &str) {
    if let Some(s) = SENDING.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
        s.remove(id);
    }
}

fn is_sending(id: &str) -> bool {
    SENDING.lock().unwrap_or_else(|p| p.into_inner()).as_ref().map(|s| s.contains(id)).unwrap_or(false)
}

/// The draft push loop of one account: pushes what is due, then sleeps until the next draft is due
/// (at most `IDLE`, so a change made from another thread is seen within that time).
pub struct DraftWorker {
    account_id: String,
    db: std::sync::Arc<Database>,
    provider: std::sync::Arc<dyn EmailProvider>,
    push_limit: Option<u64>,
}

const IDLE: std::time::Duration = std::time::Duration::from_secs(5);

impl DraftWorker {
    pub fn new(account_id: &str, db: std::sync::Arc<Database>, provider: std::sync::Arc<dyn EmailProvider>, push_limit: Option<u64>) -> Self {
        Self { account_id: account_id.to_string(), db, provider, push_limit }
    }

    pub async fn run(&self) {
        info!("Drafts worker started for {}", self.account_id);
        // Once per start: spool directories a crash left behind (no live draft owns them).
        let swept = sweep_spool(&self.db, &default_root());
        if swept > 0 {
            info!("Drafts {}: removed {} orphaned draft spool dir(s)", self.account_id, swept);
        }
        loop {
            self.drain_now().await;
            let wait = match self.db.draft_next_due(&self.account_id, DEBOUNCE_MS) {
                Ok(Some(t)) => std::time::Duration::from_millis((t - Database::now_ms()).max(0) as u64),
                Ok(None) => IDLE,
                Err(e) => {
                    warn!("Drafts {}: cannot read the queue: {}", self.account_id, e);
                    IDLE
                }
            };
            tokio::time::sleep(wait.min(IDLE)).await;
        }
    }

    /// One pass over the due drafts.
    pub async fn drain_now(&self) -> usize {
        drain(&self.db, self.provider.as_ref(), &self.account_id, self.push_limit, Database::now_ms()).await
    }
}

/// Remove draft spool directories no live draft (of any account) owns: left by a crash between
/// copying and saving. Directories younger than `SWEEP_MIN_AGE` are kept.
pub fn sweep_spool(db: &Database, root: &Path) -> usize {
    let live: HashSet<String> = match db.draft_all_live_ids() {
        Ok(ids) => ids.into_iter().collect(),
        Err(_) => return 0,
    };
    crate::attach_spool::sweep(root, &live, crate::attach_spool::SWEEP_MIN_AGE)
}

/// Total bytes of the attached files (their sizes as copied).
fn attachments_size(files: &[OutgoingAttachment]) -> u64 {
    files.iter().map(|a| a.size).sum()
}

/// What decides whether the provider copy needs its attachments re-sent: the spool copies, their
/// sizes and names, and the forward originals kept.
pub fn push_signature(msg: &OutgoingMessage) -> String {
    let keep = msg.forwarded_keep.as_ref().map(|k| k.join(",")).unwrap_or_else(|| "-".into());
    format!("{}|keep:{}", crate::db::drafts::attachments_sig(&msg.attachments), keep)
}

/// Seconds to wait before the next try of a draft that failed `attempts` times (5 s, doubling, 5 min).
pub fn backoff_ms(attempts: i64) -> i64 {
    let secs = 5i64.saturating_mul(1i64 << attempts.clamp(0, 6) as u32).min(300);
    secs * 1000
}

/// Whether a failed push is worth retrying: connection, auth, 429 and any 5xx. Other provider
/// answers (400, 403, 404, ...) will not change by waiting.
pub fn retryable(e: &OmarchyError) -> bool {
    if is_transient(e) {
        return true;
    }
    match e {
        OmarchyError::HttpError(m) => {
            let re = regex::Regex::new(r"(?:Graph|Google) API error: 5\d\d\b").unwrap();
            re.is_match(m)
        }
        _ => false,
    }
}

/// Outcome of one push attempt, for logs and tests.
#[derive(Debug, PartialEq)]
pub enum Pushed {
    /// Provider copy saved. `conflict` = our version was split off as a new draft.
    Saved { conflict: bool },
    /// Not pushed now; it is retried at `retry_at` (`PARKED` = waits for the next save).
    Deferred { retry_at: i64 },
    /// Skipped: not due, or being sent.
    Skipped,
}

/// Push one draft to the provider. `push_limit` is `Some(LARGE_PUSH_BYTES)` for Gmail (its drafts
/// are rewritten whole) and `None` where attachments are sent as they are.
pub async fn push_one(db: &Database, provider: &dyn EmailProvider, row: &DraftRow, from: &str, push_limit: Option<u64>, now: i64) -> Pushed {
    if is_sending(&row.local_id) {
        return Pushed::Skipped;
    }
    let fail = |db: &Database, why: &str, retryable_flag: bool, attempts: i64| -> Pushed {
        let retry_at = if retryable_flag { now + backoff_ms(attempts) } else { PARKED };
        if let Err(e) = db.draft_mark_failed(&row.local_id, why, retry_at) {
            warn!("Drafts: could not record failure of {}: {}", row.local_id, e);
        }
        Pushed::Deferred { retry_at }
    };
    let msg: OutgoingMessage = match serde_json::from_str(&row.payload_json) {
        Ok(m) => m,
        Err(_) => return fail(db, "the saved draft could not be read; it is kept on this computer", false, row.attempts),
    };
    let missing = crate::attach_spool::missing_copies(&msg.attachments);
    if !missing.is_empty() {
        let why = format!("attachment missing from the saved draft: {}. Attach it again.", missing.join(", "));
        return fail(db, &why, false, row.attempts);
    }
    if let Some(limit) = push_limit {
        if row.state != "saved" && attachments_size(&msg.attachments) > limit {
            return fail(db, "large attachments: this draft goes to Drafts when you close or send it", false, row.attempts);
        }
    }
    let sig = push_signature(&msg);
    let mut remote_id = row.remote_id.clone();
    let mut conflict_note: Option<String> = None;
    if let Some(rid) = remote_id.clone() {
        match provider.draft_version(&rid).await {
            Ok(None) => {
                conflict_note = Some("The draft was deleted in another client. Your version was saved again as a new draft.".into());
                remote_id = None;
            }
            Ok(Some(v)) if row.remote_version.as_deref().map(|known| known != v).unwrap_or(false) => {
                conflict_note = Some("Edited in another client since your last save. Your version is in Drafts as a new draft; the other one is unchanged.".into());
                remote_id = None;
            }
            Ok(Some(_)) => {}
            Err(e) => return fail(db, &short(&e), retryable(&e), row.attempts),
        }
    }
    let conflict = conflict_note.is_some();
    let attachments_changed = remote_id.is_none() || sig != row.attachments_sig;
    match provider.save_draft(&msg, remote_id.as_deref(), from, attachments_changed).await {
        Ok(rd) => {
            match db.draft_mark_pushed(&row.local_id, row.updated_at, &rd.id, rd.version.as_deref(), &sig) {
                Ok(clean) => info!("Drafts: {} saved to Drafts (clean: {})", row.local_id, clean),
                Err(e) => warn!("Drafts: pushed {} but could not record it: {}", row.local_id, e),
            }
            if let Some(note) = conflict_note {
                let _ = db.draft_set_notice(&row.local_id, Some(&note));
            }
            Pushed::Saved { conflict }
        }
        Err(e) => fail(db, &short(&e), retryable(&e), row.attempts),
    }
}

/// Push every draft of the account that is due. Returns the number of attempts made.
pub async fn drain(db: &Database, provider: &dyn EmailProvider, account_id: &str, push_limit: Option<u64>, now: i64) -> usize {
    let due = match db.draft_due(account_id, now, DEBOUNCE_MS) {
        Ok(d) => d,
        Err(e) => {
            warn!("Drafts {}: cannot list due drafts: {}", account_id, e);
            return 0;
        }
    };
    let from = db.account_email(account_id).ok().flatten().unwrap_or_default();
    let mut n = 0;
    for row in &due {
        if matches!(push_one(db, provider, row, &from, push_limit, now).await, Pushed::Saved { .. } | Pushed::Deferred { .. }) {
            n += 1;
        }
    }
    n
}

/// Send a queued message that comes from a draft (`msg.draft_id` set). Called by the outbox in
/// place of `send_message`. Errors are classified like a send's: retryable ones are retried by
/// the outbox, the rest end the outbox row with the message.
pub async fn send_from_draft(db: &Database, provider: &dyn EmailProvider, spool_root: &Path, msg: &OutgoingMessage, from: &str) -> Result<()> {
    let id = msg.draft_id.as_str();
    let row = match db.draft_get(id)? {
        Some(r) => r,
        None => return Err(OmarchyError::HttpError("the draft no longer exists on this computer; nothing was sent".into())),
    };
    if row.state == "sent" {
        return Ok(());
    }
    if !claim(id) {
        return Err(OmarchyError::HttpError("this draft is already being sent".into()));
    }
    let result = send_claimed(db, provider, spool_root, &row, msg, from).await;
    release(id);
    result
}

async fn send_claimed(db: &Database, provider: &dyn EmailProvider, spool_root: &Path, row: &DraftRow, msg: &OutgoingMessage, from: &str) -> Result<()> {
    let missing = crate::attach_spool::missing_copies(&msg.attachments);
    if !missing.is_empty() {
        return Err(OmarchyError::HttpError(format!("attachment missing from the saved draft: {}. Nothing was sent.", missing.join(", "))));
    }
    match row.remote_id.clone() {
        // Never pushed: a plain send creates no copy anywhere but Sent.
        None => provider.send_message(msg, from).await?,
        Some(rid) => {
            let v = provider.draft_version(&rid).await?;
            match v {
                None => {
                    return Err(OmarchyError::HttpError(
                        "the draft is no longer in Drafts (deleted or already sent elsewhere). Nothing was sent; open it again to check.".into(),
                    ))
                }
                Some(v) if row.remote_version.as_deref().map(|known| known != v).unwrap_or(false) => {
                    // Edited elsewhere: send what the user sees as a new message and leave the other draft alone.
                    warn!("Drafts: {} changed in another client; sending this version as a new message", row.local_id);
                    provider.send_message(msg, from).await?;
                    let note = "Sent. The draft was also edited in another client, which is still in Drafts.";
                    let _ = db.draft_set_notice(&row.local_id, Some(note));
                }
                Some(_) => provider.send_draft(msg, &rid, from).await?,
            }
        }
    }
    db.draft_mark_sent(&row.local_id)?;
    db.draft_prune_sent()?;
    crate::attach_spool::remove(spool_root, &row.local_id);
    Ok(())
}

/// Remove a draft: from the provider (when it has a copy), then locally. The local row is removed
/// only after the provider's delete succeeded or there was nothing to delete.
pub async fn discard(db: &Database, provider: &dyn EmailProvider, spool_root: &Path, local_id: &str) -> Result<()> {
    // Claimed like a send: the push worker and a save of the same draft keep off it meanwhile, so
    // a push cannot re-create the provider copy between the remote delete and the local one.
    if !claim(local_id) {
        return Err(OmarchyError::HttpError("this draft is being sent or saved right now".into()));
    }
    let result = discard_claimed(db, provider, spool_root, local_id).await;
    release(local_id);
    result
}

async fn discard_claimed(db: &Database, provider: &dyn EmailProvider, spool_root: &Path, local_id: &str) -> Result<()> {
    let Some(row) = db.draft_get(local_id)? else { return Ok(()) };
    if let Some(rid) = row.remote_id.as_deref() {
        provider.delete_draft(rid).await?;
    }
    db.draft_delete(local_id)?;
    crate::attach_spool::remove(spool_root, local_id);
    Ok(())
}

/// A new local draft id (`dr-<time>-<counter>`), unique within the process and across restarts.
pub fn new_draft_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("dr-{:x}-{:04x}", Database::now_ms(), COUNTER.fetch_add(1, Ordering::Relaxed) & 0xffff)
}

/// Save what the compose pane holds as the local copy of a draft (`POST /drafts/save`). The new
/// attachments are copied into the spool under the draft's id (`attach_spool::snapshot_draft`); an
/// attachment that was already spooled and whose copy has gone stays in the list, so the UI shows
/// it with a '!' chip instead of dropping it. `msg.draft_id` is set to the id used. `state` is
/// `editing` (autosave) or `saved` (explicit save, close, quit). Returns the draft id.
pub fn save_local(db: &Database, spool_root: &Path, msg: &mut OutgoingMessage, state: &str) -> std::result::Result<String, String> {
    if state != "editing" && state != "saved" {
        return Err(format!("unknown draft state '{}'", state));
    }
    let previous: Vec<OutgoingAttachment> = if msg.draft_id.is_empty() {
        Vec::new()
    } else {
        if is_sending(&msg.draft_id) {
            return Err("this draft is being sent right now".into());
        }
        match db.draft_get(&msg.draft_id).map_err(|e| e.to_string())? {
            Some(row) if row.state == "sent" => return Err("this draft was already sent".into()),
            Some(row) => serde_json::from_str::<OutgoingMessage>(&row.payload_json).map(|m| m.attachments).unwrap_or_default(),
            None => Vec::new(),
        }
    };
    let id = if msg.draft_id.is_empty() { new_draft_id() } else { msg.draft_id.clone() };
    // Attachments that were spooled before and whose copy is gone: kept as they are (for the '!' chip).
    let gone = |a: &OutgoingAttachment| previous.iter().any(|p| p.path == a.path) && !Path::new(&a.path).is_file();
    let to_copy: Vec<OutgoingAttachment> = msg.attachments.iter().filter(|a| !gone(a)).cloned().collect();
    let copied = crate::attach_spool::snapshot_draft(spool_root, &id, &to_copy, &previous)?;
    let mut copied = copied.into_iter();
    let mut merged = Vec::with_capacity(msg.attachments.len());
    for a in &msg.attachments {
        if gone(a) {
            merged.push(a.clone());
        } else if let Some(c) = copied.next() {
            merged.push(c);
        }
    }
    msg.attachments = merged;
    msg.draft_id = id.clone();
    let payload = serde_json::to_string(&msg).map_err(|e| e.to_string())?;
    db.draft_save(&id, &msg.kind, &msg.in_reply_to, &payload, &msg.mode, state).map_err(|e| e.to_string())?;
    Ok(id)
}

/// One draft as JSON for the UI (`GET /drafts`, `GET /drafts/get`): the row, the saved message
/// under `payload`, `recover` (an autosaved draft a crash left unsaved: offered at startup) and
/// `missing` (names of spooled attachments whose copy is gone).
pub fn draft_json(row: &DraftRow) -> serde_json::Value {
    let payload: serde_json::Value = serde_json::from_str(&row.payload_json).unwrap_or(serde_json::Value::Null);
    let missing: Vec<String> = match serde_json::from_str::<OutgoingMessage>(&row.payload_json) {
        Ok(m) => crate::attach_spool::missing_copies(&m.attachments),
        Err(_) => Vec::new(),
    };
    serde_json::json!({
        "local_id": row.local_id,
        "account_id": row.account_id,
        "remote_id": row.remote_id,
        "kind": row.kind,
        "in_reply_to": row.in_reply_to,
        "mode": row.mode,
        "state": row.state,
        "dirty": row.dirty,
        "recover": row.state == "editing" && row.dirty,
        "last_error": row.last_error,
        "notice": row.notice,
        "updated_at": row.updated_at,
        "payload": payload,
        "missing": missing,
    })
}

fn short(e: &OmarchyError) -> String {
    let s = e.to_string();
    s.chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::{Body, OutgoingAttachment};
    use crate::providers::RemoteDraft;
    use async_trait::async_trait;
    use std::sync::Mutex as StdMutex;

    /// Scripted provider: records calls, answers from queues.
    #[derive(Default)]
    struct Fake {
        saves: StdMutex<Vec<(Option<String>, bool, String)>>,
        save_script: StdMutex<Vec<std::result::Result<RemoteDraft, String>>>,
        versions: StdMutex<Vec<std::result::Result<Option<String>, String>>>,
        calls: StdMutex<Vec<String>>,
    }

    fn api(m: &str) -> OmarchyError {
        OmarchyError::HttpError(m.into())
    }

    #[async_trait]
    impl EmailProvider for Fake {
        async fn fetch_folders(&self) -> Result<Vec<crate::models::MailFolder>> {
            Ok(vec![])
        }
        async fn fetch_inbox(&self, _limit: usize) -> Result<Vec<crate::models::EmailMessage>> {
            Ok(vec![])
        }
        async fn fetch_folder_messages(&self, _folder_id: &str, _limit: usize) -> Result<Vec<crate::models::EmailMessage>> {
            Ok(vec![])
        }
        async fn is_token_valid(&self) -> Result<bool> {
            Ok(true)
        }
        async fn send_message(&self, _m: &OutgoingMessage, _f: &str) -> Result<()> {
            self.calls.lock().unwrap().push("send_message".into());
            Ok(())
        }
        async fn save_draft(&self, m: &OutgoingMessage, rid: Option<&str>, _f: &str, changed: bool) -> Result<RemoteDraft> {
            self.saves.lock().unwrap().push((rid.map(str::to_string), changed, m.subject.clone()));
            let mut s = self.save_script.lock().unwrap();
            if s.is_empty() {
                return Ok(RemoteDraft { id: rid.unwrap_or("NEW").to_string(), version: Some("v".into()) });
            }
            s.remove(0).map_err(|e| api(&e))
        }
        async fn draft_version(&self, _rid: &str) -> Result<Option<String>> {
            let mut v = self.versions.lock().unwrap();
            if v.is_empty() {
                return Ok(Some("v".into()));
            }
            v.remove(0).map_err(|e| api(&e))
        }
        async fn delete_draft(&self, rid: &str) -> Result<()> {
            self.calls.lock().unwrap().push(format!("delete {}", rid));
            Ok(())
        }
        async fn send_draft(&self, _m: &OutgoingMessage, rid: &str, _f: &str) -> Result<()> {
            self.calls.lock().unwrap().push(format!("send_draft {}", rid));
            Ok(())
        }
    }

    fn db() -> Database {
        Database::open_for_account(":memory:", "a1").unwrap()
    }

    fn msg(subject: &str) -> OutgoingMessage {
        OutgoingMessage { kind: "new".into(), account_id: "a1".into(), subject: subject.into(), body: Body { format: "text".into(), content: "hi".into() }, ..Default::default() }
    }

    fn save(db: &Database, id: &str, m: &OutgoingMessage, state: &str) {
        db.draft_save(id, &m.kind, &m.in_reply_to, &serde_json::to_string(m).unwrap(), &m.mode, state).unwrap();
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff_ms(0), 5_000);
        assert_eq!(backoff_ms(1), 10_000);
        assert_eq!(backoff_ms(2), 20_000);
        assert_eq!(backoff_ms(50), 300_000);
    }

    #[test]
    fn retry_classification() {
        assert!(retryable(&api("Graph API error: 503 — busy")));
        assert!(retryable(&api("Google API error: 500 Internal Server Error — x")));
        assert!(retryable(&api("Google API error: 429 Too Many Requests — x")));
        assert!(retryable(&api("error sending request for url (https://graph...)")));
        assert!(!retryable(&api("Graph API error: 400 Bad Request — x")));
        assert!(!retryable(&api("Graph API error: 404 Not Found — x")));
        assert!(!retryable(&OmarchyError::HttpError("draft is too large".into())));
    }

    #[test]
    fn push_signature_tracks_attachments_and_keep() {
        let mut m = msg("s");
        let a = push_signature(&m);
        m.attachments = vec![OutgoingAttachment { path: "/spool/1/file".into(), name: "a.pdf".into(), size: 3, ..Default::default() }];
        let b = push_signature(&m);
        assert_ne!(a, b);
        m.forwarded_keep = Some(vec!["p1".into()]);
        assert_ne!(b, push_signature(&m));
    }

    #[tokio::test]
    async fn offline_then_reconnect_pushes_once() {
        let db = db();
        let fake = Fake::default();
        let m = msg("hello");
        save(&db, "d1", &m, "editing");
        // Offline: the push fails with a network error and is retried later.
        fake.save_script.lock().unwrap().push(Err("error sending request for url (x)".into()));
        let row = db.draft_get("d1").unwrap().unwrap();
        let now = row.updated_at + DEBOUNCE_MS + 1;
        assert_eq!(drain(&db, &fake, "a1", None, now).await, 1);
        let row = db.draft_get("d1").unwrap().unwrap();
        assert!(row.dirty, "the local copy stays dirty while offline");
        assert!(row.next_attempt_at > now, "retry is scheduled");
        // Reconnected: the retry pushes once and clears the dirty flag.
        assert_eq!(drain(&db, &fake, "a1", None, row.next_attempt_at).await, 1);
        assert_eq!(fake.saves.lock().unwrap().len(), 2);
        let row = db.draft_get("d1").unwrap().unwrap();
        assert!(!row.dirty);
        assert_eq!(row.remote_id.as_deref(), Some("NEW"));
        // Nothing more to do.
        assert_eq!(drain(&db, &fake, "a1", None, i64::MAX - 1).await, 0);
    }

    #[tokio::test]
    async fn autosave_is_debounced_but_a_save_goes_at_once() {
        let db = db();
        let fake = Fake::default();
        let m = msg("typing");
        save(&db, "d1", &m, "editing");
        let row = db.draft_get("d1").unwrap().unwrap();
        assert_eq!(drain(&db, &fake, "a1", None, row.updated_at + DEBOUNCE_MS - 1).await, 0, "still typing: no push");
        save(&db, "d2", &m, "saved");
        assert_eq!(drain(&db, &fake, "a1", None, db.draft_get("d2").unwrap().unwrap().updated_at).await, 1, "saved: pushed now");
        assert_eq!(drain(&db, &fake, "a1", None, row.updated_at + DEBOUNCE_MS).await, 1, "autosave goes after the debounce");
    }

    #[tokio::test]
    async fn four_xx_keeps_local_copy_and_parks() {
        let db = db();
        let fake = Fake::default();
        let m = msg("rejected");
        save(&db, "d1", &m, "saved");
        fake.save_script.lock().unwrap().push(Err("Graph API error: 400 Bad Request — bad recipient".into()));
        drain(&db, &fake, "a1", None, i64::MAX - 1).await;
        let row = db.draft_get("d1").unwrap().unwrap();
        assert!(row.dirty);
        assert_eq!(row.next_attempt_at, PARKED);
        assert!(row.last_error.unwrap().contains("400"));
        assert_eq!(db.draft_next_due("a1", 0).unwrap(), None);
    }

    #[tokio::test]
    async fn missing_spool_copy_blocks_push_and_names_the_file() {
        let db = db();
        let fake = Fake::default();
        let mut m = msg("with file");
        m.attachments = vec![OutgoingAttachment { path: "/nonexistent/omalook-test/x.pdf".into(), name: "x.pdf".into(), size: 1, ..Default::default() }];
        save(&db, "d1", &m, "saved");
        drain(&db, &fake, "a1", None, i64::MAX - 1).await;
        assert!(fake.saves.lock().unwrap().is_empty(), "nothing is pushed without the file");
        let err = db.draft_get("d1").unwrap().unwrap().last_error.unwrap();
        assert!(err.contains("x.pdf"), "{}", err);
    }

    #[tokio::test]
    async fn gmail_large_draft_waits_for_close() {
        let db = db();
        let fake = Fake::default();
        let dir = std::env::temp_dir().join(format!("omalook-drafts-large-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("big.bin");
        std::fs::write(&f, vec![0u8; 64]).unwrap();
        let mut m = msg("big");
        m.attachments = vec![OutgoingAttachment { path: f.to_string_lossy().into(), name: "big.bin".into(), size: 64, ..Default::default() }];
        save(&db, "d1", &m, "editing");
        // Pretend the limit is 10 bytes: the autosave is held back...
        drain(&db, &fake, "a1", Some(10), i64::MAX - 1).await;
        assert!(fake.saves.lock().unwrap().is_empty());
        assert_eq!(db.draft_next_due("a1", 0).unwrap(), None, "parked until the next save");
        // ...and a save on close pushes it.
        save(&db, "d1", &m, "saved");
        drain(&db, &fake, "a1", Some(10), i64::MAX - 1).await;
        assert_eq!(fake.saves.lock().unwrap().len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn conflict_keeps_both_drafts() {
        let db = db();
        let fake = Fake::default();
        let m = msg("mine");
        save(&db, "d1", &m, "saved");
        // First push succeeds with version v1.
        fake.save_script.lock().unwrap().push(Ok(RemoteDraft { id: "R1".into(), version: Some("v1".into()) }));
        drain(&db, &fake, "a1", None, i64::MAX - 1).await;
        // Someone edits the draft elsewhere (version v2); we save again.
        save(&db, "d1", &msg("mine again"), "saved");
        fake.versions.lock().unwrap().push(Ok(Some("v2".into())));
        fake.save_script.lock().unwrap().push(Ok(RemoteDraft { id: "R2".into(), version: Some("v3".into()) }));
        drain(&db, &fake, "a1", None, i64::MAX - 1).await;
        let saves = fake.saves.lock().unwrap();
        assert_eq!(saves[1].0, None, "our version is created as a new draft, the other is not touched");
        let row = db.draft_get("d1").unwrap().unwrap();
        assert_eq!(row.remote_id.as_deref(), Some("R2"));
        assert!(row.notice.unwrap().contains("another client"));
    }

    #[tokio::test]
    async fn send_from_draft_uses_the_remote_copy() {
        let db = db();
        let fake = Fake::default();
        let root = std::env::temp_dir().join(format!("omalook-drafts-send-{}", std::process::id()));
        let m = msg("send me");
        save(&db, "d1", &m, "saved");
        let r = db.draft_get("d1").unwrap().unwrap();
        db.draft_mark_pushed("d1", r.updated_at, "R1", Some("v"), "").unwrap();
        let mut out = m.clone();
        out.draft_id = "d1".into();
        send_from_draft(&db, &fake, &root, &out, "me@x").await.unwrap();
        assert_eq!(fake.calls.lock().unwrap().as_slice(), ["send_draft R1"], "sent as the draft, no second copy");
        assert!(db.draft_get("d1").unwrap().is_none(), "local row pruned after the send");
    }

    #[tokio::test]
    async fn send_from_unsaved_draft_is_a_plain_send() {
        let db = db();
        let fake = Fake::default();
        let root = std::env::temp_dir().join("omalook-drafts-send-plain");
        let mut m = msg("never pushed");
        m.draft_id = "d9".into();
        save(&db, "d9", &m, "saved");
        send_from_draft(&db, &fake, &root, &m, "me@x").await.unwrap();
        assert_eq!(fake.calls.lock().unwrap().as_slice(), ["send_message"]);
        assert!(db.draft_get("d9").unwrap().is_none());
    }

    #[tokio::test]
    async fn send_of_a_deleted_draft_is_reported_not_resent() {
        let db = db();
        let fake = Fake::default();
        let root = std::env::temp_dir().join("omalook-drafts-send-gone");
        let m = msg("gone");
        save(&db, "d1", &m, "saved");
        let r = db.draft_get("d1").unwrap().unwrap();
        db.draft_mark_pushed("d1", r.updated_at, "R1", Some("v"), "").unwrap();
        fake.versions.lock().unwrap().push(Ok(None));
        let mut out = m.clone();
        out.draft_id = "d1".into();
        let err = send_from_draft(&db, &fake, &root, &out, "me@x").await.unwrap_err();
        assert!(err.to_string().contains("no longer in Drafts"));
        assert!(fake.calls.lock().unwrap().is_empty(), "nothing sent");
        assert!(db.draft_get("d1").unwrap().is_some(), "local copy kept");
    }

    #[tokio::test]
    async fn discard_removes_remote_then_local() {
        let db = db();
        let fake = Fake::default();
        let root = std::env::temp_dir().join("omalook-drafts-discard");
        let m = msg("bin");
        save(&db, "d1", &m, "saved");
        let r = db.draft_get("d1").unwrap().unwrap();
        db.draft_mark_pushed("d1", r.updated_at, "R1", Some("v"), "").unwrap();
        discard(&db, &fake, &root, "d1").await.unwrap();
        assert_eq!(fake.calls.lock().unwrap().as_slice(), ["delete R1"]);
        assert!(db.draft_get("d1").unwrap().is_none());
    }

    #[test]
    fn draft_json_flags_recovery_and_missing_copies() {
        let db = db();
        let mut m = msg("unsaved");
        m.attachments = vec![OutgoingAttachment { path: "/gone/omalook/x.pdf".into(), name: "x.pdf".into(), size: 1, ..Default::default() }];
        save(&db, "d1", &m, "editing");
        let row = db.draft_get("d1").unwrap().unwrap();
        let j = draft_json(&row);
        assert_eq!(j["recover"], true, "an autosaved, unsaved draft is offered for recovery");
        assert_eq!(j["missing"], serde_json::json!(["x.pdf"]));
        assert_eq!(j["payload"]["subject"], "unsaved");
        db.draft_save("d1", "new", "", &serde_json::to_string(&m).unwrap(), "system", "saved").unwrap();
        assert_eq!(draft_json(&db.draft_get("d1").unwrap().unwrap())["recover"], false);
    }

    #[test]
    fn save_local_copies_attachments_and_keeps_the_gone_ones() {
        let db = db();
        let root = std::env::temp_dir().join(format!("omalook-save-local-{}", std::process::id()));
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let f = src.join("report.txt");
        std::fs::write(&f, b"hello").unwrap();
        let mut m = msg("with file");
        m.attachments = vec![OutgoingAttachment { path: f.to_string_lossy().into(), name: "report.txt".into(), size: 5, ..Default::default() }];
        let id = save_local(&db, &root.join("spool"), &mut m, "editing").unwrap();
        assert!(id.starts_with("dr-"));
        assert_eq!(m.draft_id, id);
        let spooled = m.attachments[0].path.clone();
        assert!(spooled.starts_with(&root.join("spool").to_string_lossy().to_string()), "copied into the spool: {}", spooled);
        assert!(Path::new(&spooled).is_file());
        // Resume after the spool copy vanished and the source is gone too: the entry stays (a '!' chip),
        // and the save still succeeds for the rest of the draft.
        std::fs::remove_file(&spooled).unwrap();
        std::fs::remove_file(&f).unwrap();
        let mut resumed = serde_json::from_str::<OutgoingMessage>(&db.draft_get(&id).unwrap().unwrap().payload_json).unwrap();
        resumed.subject = "edited".into();
        let again = save_local(&db, &root.join("spool"), &mut resumed, "saved").unwrap();
        assert_eq!(again, id, "a resumed draft keeps its id");
        assert_eq!(resumed.attachments.len(), 1, "the missing attachment is not dropped");
        let j = draft_json(&db.draft_get(&id).unwrap().unwrap());
        assert_eq!(j["missing"], serde_json::json!(["report.txt"]));
        assert_eq!(j["state"], "saved");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn save_local_refuses_a_sent_or_unknown_state() {
        let db = db();
        let root = std::env::temp_dir().join("omalook-save-local-refuse");
        let mut m = msg("x");
        m.draft_id = "d-sent".into();
        save(&db, "d-sent", &m, "saved");
        db.draft_mark_sent("d-sent").unwrap();
        assert!(save_local(&db, &root, &mut m, "saved").unwrap_err().contains("already sent"));
        let mut fresh = msg("y");
        assert!(save_local(&db, &root, &mut fresh, "sending").is_err(), "only editing / saved are valid");
    }

    #[test]
    fn new_draft_ids_are_unique() {
        let a = new_draft_id();
        let b = new_draft_id();
        assert_ne!(a, b);
        assert!(a.starts_with("dr-"));
    }

    #[test]
    fn sending_claim_is_exclusive() {
        assert!(claim("x1"));
        assert!(!claim("x1"));
        assert!(is_sending("x1"));
        release("x1");
        assert!(claim("x1"));
        release("x1");
    }
}
