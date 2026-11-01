//! Outbox worker: sends the messages the user queued with Send.
//!
//! One worker per account, on its own thread (see `scheduler`), independent of the mail sync
//! loop: a long first-run crawl or a slow poll must never hold a message back. The worker
//! sleeps until the earliest queued message is due, and is woken at once by
//! `sync_state::request_outbox_run()` (called when a message is queued), so with a delay of
//! 0 seconds the message goes out the moment the button is pressed.
//!
//! Guarantees:
//! * a message is claimed (`pending` to `sending`) before the network call, so it is sent by
//!   one worker at most once, and an undo that arrives after the claim is refused;
//! * only failures that cannot have delivered the message are retried (`compose::is_transient`);
//! * after a crash, a message left `sending` is reported as failed, not sent again.

use crate::compose::{is_transient, retry_delay_secs, OutgoingMessage, MAX_ATTEMPTS};
use crate::db::Database;
use crate::providers::EmailProvider;
use crate::sync_state;
use log::{error, info, warn};
use regex::Regex;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// How long a worker with nothing queued sleeps before looking again (it is woken early by a send).
const IDLE: Duration = Duration::from_secs(300);
/// Finished messages are kept this long, so the UI can still ask how a send went.
const KEEP_MS: i64 = 24 * 3600 * 1000;

pub struct OutboxWorker {
    account_id: String,
    db: Arc<Database>,
    provider: Arc<dyn EmailProvider>,
    /// Where the attachment copies of queued messages live (`attach_spool`).
    spool_root: PathBuf,
}

/// The human part of a provider error: Graph and Google wrap it in JSON.
fn short_error(e: &crate::errors::OmarchyError) -> String {
    let full = e.to_string();
    let msg = Regex::new(r#""message"\s*:\s*"((?:[^"\\]|\\.)*)""#)
        .unwrap()
        .captures(&full)
        .map(|c| c[1].replace("\\n", " ").replace("\\\"", "\""))
        .unwrap_or(full);
    msg.chars().take(300).collect()
}

impl OutboxWorker {
    pub fn new(account_id: &str, db: Arc<Database>, provider: Arc<dyn EmailProvider>) -> Self {
        Self { account_id: account_id.to_string(), db, provider, spool_root: crate::attach_spool::default_root() }
    }

    pub fn with_spool_root(mut self, root: PathBuf) -> Self {
        self.spool_root = root;
        self
    }

    /// Remove spool directories nobody owns any more (a crash between copying and queuing, or
    /// between sending and cleaning up).
    fn sweep_spool(&self) {
        let live: HashSet<String> = match self.db.outbox_live_ids() {
            Ok(ids) => ids.into_iter().collect(),
            Err(e) => {
                error!("Outbox {}: cannot list live messages, not sweeping the attachment spool: {}", self.account_id, e);
                return;
            }
        };
        let n = crate::attach_spool::sweep(&self.spool_root, &live, crate::attach_spool::SWEEP_MIN_AGE);
        if n > 0 {
            info!("Outbox {}: removed {} orphaned attachment spool(s)", self.account_id, n);
        }
    }

    /// Run until cancelled.
    pub async fn run(&self) {
        match self.db.outbox_fail_interrupted(&self.account_id) {
            Ok(0) => {}
            Ok(n) => warn!("Outbox {}: {} message(s) were interrupted mid-send; marked failed, not resent", self.account_id, n),
            Err(e) => error!("Outbox {}: could not check for interrupted sends: {}", self.account_id, e),
        }
        let _ = self.db.outbox_prune(KEEP_MS);
        self.sweep_spool();
        info!("Outbox worker started for {}", self.account_id);

        loop {
            // Registered before looking at the queue, so a message queued while we work wakes us.
            let wake = sync_state::outbox_requested();
            tokio::pin!(wake);
            let _ = wake.as_mut().enable();

            self.drain().await;

            let wait = match self.db.outbox_next_due(&self.account_id) {
                Ok(Some(t)) => Duration::from_millis((t - Database::now_ms()).max(0) as u64),
                Ok(None) => IDLE,
                Err(e) => {
                    error!("Outbox {}: cannot read the queue: {}", self.account_id, e);
                    Duration::from_secs(5)
                }
            };
            tokio::select! {
                _ = tokio::time::sleep(wait.min(IDLE)) => {}
                _ = &mut wake => {}
            }
        }
    }

    /// Send everything that is due. Returns how many messages were attempted.
    pub async fn drain(&self) -> usize {
        let due = match self.db.outbox_claim_due(&self.account_id, Database::now_ms()) {
            Ok(d) => d,
            Err(e) => {
                error!("Outbox {}: cannot claim due messages: {}", self.account_id, e);
                return 0;
            }
        };
        if due.is_empty() {
            return 0;
        }
        let from = self.db.account_email(&self.account_id).ok().flatten().unwrap_or_default();
        let n = due.len();
        for (id, payload, attempt) in due {
            let msg: OutgoingMessage = match serde_json::from_str(&payload) {
                Ok(m) => m,
                Err(e) => {
                    error!("Outbox {}: unreadable message {}: {}", self.account_id, id, e);
                    let _ = self.db.outbox_mark_failed(&id, "the queued message could not be read");
                    continue;
                }
            };
            // What is about to go out must be what was queued: the spooled copies, intact.
            if let Err(why) = crate::attach_spool::verify(&msg.attachments) {
                error!("Outbox {}: {} not sent: {}", self.account_id, id, why);
                let _ = self.db.outbox_mark_failed(&id, &why);
                crate::attach_spool::remove(&self.spool_root, &id);
                continue;
            }
            match self.provider.send_message(&msg, &from).await {
                Ok(()) => {
                    info!("Outbox {}: sent {} (attempt {})", self.account_id, id, attempt);
                    if let Err(e) = self.db.outbox_mark_sent(&id) {
                        error!("Outbox {}: sent {} but could not record it: {}", self.account_id, id, e);
                    }
                    crate::attach_spool::remove(&self.spool_root, &id);
                    sync_state::bump_mail("message sent");
                }
                Err(e) if is_transient(&e) && attempt < MAX_ATTEMPTS => {
                    let wait = retry_delay_secs(attempt);
                    warn!("Outbox {}: {} not sent ({}); retrying in {}s", self.account_id, id, e, wait);
                    let _ = self.db.outbox_mark_retry(&id, &short_error(&e), Database::now_ms() + wait * 1000);
                }
                Err(e) => {
                    error!("Outbox {}: {} failed: {}", self.account_id, id, e);
                    let _ = self.db.outbox_mark_failed(&id, &short_error(&e));
                    crate::attach_spool::remove(&self.spool_root, &id);
                }
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::{Address, Body};
    use crate::errors::{OmarchyError, Result};
    use crate::models::{EmailMessage, MailFolder};
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// Records what it is asked to send and answers from a script.
    struct Fake {
        sent: Mutex<Vec<(OutgoingMessage, String)>>,
        script: Mutex<Vec<Result<()>>>,
        /// The bytes of each attachment as they were on disk at the moment of sending.
        files: Mutex<Vec<Vec<u8>>>,
    }

    #[async_trait]
    impl EmailProvider for Fake {
        async fn fetch_inbox(&self, _l: usize) -> Result<Vec<EmailMessage>> { Ok(vec![]) }
        async fn fetch_folder_messages(&self, _f: &str, _l: usize) -> Result<Vec<EmailMessage>> { Ok(vec![]) }
        async fn fetch_folders(&self) -> Result<Vec<MailFolder>> { Ok(vec![]) }
        async fn is_token_valid(&self) -> Result<bool> { Ok(true) }
        async fn send_message(&self, msg: &OutgoingMessage, from: &str) -> Result<()> {
            self.sent.lock().unwrap().push((msg.clone(), from.to_string()));
            for a in &msg.attachments {
                self.files.lock().unwrap().push(std::fs::read(&a.path).unwrap_or_default());
            }
            let mut s = self.script.lock().unwrap();
            if s.is_empty() { Ok(()) } else { s.remove(0) }
        }
    }

    fn payload(subject: &str) -> String {
        serde_json::to_string(&OutgoingMessage {
            kind: "new".into(),
            account_id: "a1".into(),
            to: vec![Address { email: "p@example.com".into(), name: String::new() }],
            subject: subject.into(),
            mode: "system".into(),
            body: Body { format: "text".into(), content: "hi".into() },
            ..Default::default()
        })
        .unwrap()
    }

    fn setup(script: Vec<Result<()>>) -> (OutboxWorker, Arc<Fake>, Arc<Database>) {
        let db = Arc::new(Database::open_for_account(":memory:", "a1").unwrap());
        let fake = Arc::new(Fake { sent: Mutex::new(vec![]), script: Mutex::new(script), files: Mutex::new(vec![]) });
        (OutboxWorker::new("a1", db.clone(), fake.clone()), fake, db)
    }

    fn block<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
    }

    #[test]
    fn a_due_message_is_sent_once_and_recorded() {
        let (w, fake, db) = setup(vec![]);
        db.outbox_enqueue("o1", "a1", &payload("one"), Database::now_ms()).unwrap();
        assert_eq!(block(w.drain()), 1);
        assert_eq!(block(w.drain()), 0, "already sent: nothing left to do");
        assert_eq!(fake.sent.lock().unwrap().len(), 1);
        assert_eq!(fake.sent.lock().unwrap()[0].0.subject, "one");
        assert_eq!(db.outbox_status("o1").unwrap().unwrap().0, "sent");
    }

    #[test]
    fn a_message_not_yet_due_waits_and_a_cancelled_one_never_goes() {
        let (w, fake, db) = setup(vec![]);
        db.outbox_enqueue("later", "a1", &payload("later"), Database::now_ms() + 60_000).unwrap();
        db.outbox_enqueue("undone", "a1", &payload("undone"), Database::now_ms()).unwrap();
        assert!(db.outbox_cancel("undone").unwrap());
        assert_eq!(block(w.drain()), 0);
        assert!(fake.sent.lock().unwrap().is_empty());
        assert_eq!(db.outbox_status("later").unwrap().unwrap().0, "pending");
    }

    #[test]
    fn zero_delay_is_sent_by_the_waking_worker_without_waiting_for_any_poll() {
        // The real loop: a worker idle on its long wait must send as soon as a message is queued.
        let (w, fake, db) = setup(vec![]);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async {
            tokio::task::spawn_local(async move { w.run().await });
            tokio::time::sleep(Duration::from_millis(50)).await; // worker is idle: nothing queued
            let queued = std::time::Instant::now();
            db.outbox_enqueue("o1", "a1", &payload("now"), Database::now_ms()).unwrap();
            sync_state::request_outbox_run();
            for _ in 0..100 {
                if !fake.sent.lock().unwrap().is_empty() { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(fake.sent.lock().unwrap().len(), 1, "sent without waiting for the idle timeout");
            assert!(queued.elapsed() < Duration::from_secs(2));
        });
    }

    #[test]
    fn a_delayed_message_goes_out_when_its_time_comes_not_before() {
        let (w, fake, db) = setup(vec![]);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async {
            tokio::task::spawn_local(async move { w.run().await });
            tokio::time::sleep(Duration::from_millis(50)).await;
            db.outbox_enqueue("o1", "a1", &payload("soon"), Database::now_ms() + 400).unwrap();
            sync_state::request_outbox_run();
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert!(fake.sent.lock().unwrap().is_empty(), "still inside the delay");
            tokio::time::sleep(Duration::from_millis(500)).await;
            assert_eq!(fake.sent.lock().unwrap().len(), 1, "sent once the delay ran out");
        });
    }

    #[test]
    fn transient_failures_are_retried_later_and_permanent_ones_are_not() {
        let (w, fake, db) = setup(vec![
            Err(OmarchyError::HttpError("error sending request for url (x)".into())),
            Err(OmarchyError::HttpError("Graph API error: 400 Bad Request — {\"error\":{\"code\":\"ErrorInvalidRecipients\",\"message\":\"One or more recipients are invalid.\"}}".into())),
        ]);
        db.outbox_enqueue("o1", "a1", &payload("net"), 0).unwrap();
        db.outbox_enqueue("o2", "a1", &payload("bad"), 0).unwrap();
        block(w.drain());
        let o1 = db.outbox_status("o1").unwrap().unwrap();
        assert_eq!(o1.0, "pending", "network failure: queued again");
        assert!(db.outbox_next_due("a1").unwrap().unwrap() > Database::now_ms(), "after a backoff");
        let o2 = db.outbox_status("o2").unwrap().unwrap();
        assert_eq!(o2.0, "failed");
        assert_eq!(o2.1.as_deref(), Some("One or more recipients are invalid."), "the provider's own words, not raw JSON");
        assert_eq!(fake.sent.lock().unwrap().len(), 2);
    }

    #[test]
    fn retries_give_up_after_the_attempt_limit() {
        let net = || Err(OmarchyError::HttpError("error sending request".into()));
        let (w, _fake, db) = setup((0..MAX_ATTEMPTS).map(|_| net()).collect());
        db.outbox_enqueue("o1", "a1", &payload("x"), 0).unwrap();
        for _ in 0..MAX_ATTEMPTS {
            // make it due again, as the backoff would have
            db.outbox_mark_retry("o1", "net", 0).ok();
            block(w.drain());
        }
        assert_eq!(db.outbox_status("o1").unwrap().unwrap().0, "failed");
    }

    #[test]
    fn the_from_address_comes_from_the_account() {
        let (w, fake, db) = setup(vec![]);
        db.insert_account("a1", "gmail", Some("me@gmail.example"), None, "{}").unwrap();
        db.outbox_enqueue("o1", "a1", &payload("x"), 0).unwrap();
        block(w.drain());
        assert_eq!(fake.sent.lock().unwrap()[0].1, "me@gmail.example");
    }

    // ---- attachment spool lifecycle (docs/PLAN-compose-attachments.md, phase 1)

    fn tdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("olook-outbox-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Queue a message with one attached file the way `/compose/send` does: spool first, then enqueue.
    fn queue_with_file(db: &Database, root: &std::path::Path, id: &str, file: &std::path::Path) -> OutgoingMessage {
        let mut m: OutgoingMessage = serde_json::from_str(&payload("with file")).unwrap();
        m.attachments = vec![crate::compose::OutgoingAttachment { path: file.to_string_lossy().into(), ..Default::default() }];
        m.attachments = crate::attach_spool::snapshot(root, id, &m.attachments).unwrap();
        db.outbox_enqueue(id, "a1", &serde_json::to_string(&m).unwrap(), Database::now_ms()).unwrap();
        m
    }

    #[test]
    fn a_file_deleted_or_replaced_during_the_undo_window_still_sends_the_original_bytes() {
        let (src, root) = (tdir("a-src"), tdir("a-root"));
        let (w, fake, db) = setup(vec![]);
        let w = w.with_spool_root(root.clone());
        let (gone, replaced) = (src.join("gone.txt"), src.join("replaced.txt"));
        std::fs::write(&gone, b"original one").unwrap();
        std::fs::write(&replaced, b"original two").unwrap();
        queue_with_file(&db, &root, "o1", &gone);
        queue_with_file(&db, &root, "o2", &replaced);
        std::fs::remove_file(&gone).unwrap();
        std::fs::write(&replaced, b"something else entirely").unwrap();
        assert_eq!(block(w.drain()), 2);
        assert_eq!(*fake.files.lock().unwrap(), vec![b"original one".to_vec(), b"original two".to_vec()]);
        assert_eq!(db.outbox_status("o1").unwrap().unwrap().0, "sent");
        assert!(!crate::attach_spool::spool_dir(&root, "o1").exists(), "spool removed once sent");
        assert!(!crate::attach_spool::spool_dir(&root, "o2").exists());
        std::fs::remove_dir_all(&src).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_spool_copy_fails_the_message_without_calling_the_provider() {
        let (src, root) = (tdir("b-src"), tdir("b-root"));
        let (w, fake, db) = setup(vec![]);
        let w = w.with_spool_root(root.clone());
        let f = src.join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        let m = queue_with_file(&db, &root, "o1", &f);
        std::fs::remove_file(&m.attachments[0].path).unwrap();
        block(w.drain());
        assert!(fake.sent.lock().unwrap().is_empty());
        let st = db.outbox_status("o1").unwrap().unwrap();
        assert_eq!(st.0, "failed");
        assert_eq!(st.1.as_deref(), Some("the prepared copy of a.txt is missing"));
        std::fs::remove_dir_all(&src).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_spool_survives_a_retry_and_goes_when_the_send_finally_fails() {
        let (src, root) = (tdir("c-src"), tdir("c-root"));
        let (w, fake, db) = setup(vec![
            Err(OmarchyError::HttpError("error sending request for url (x)".into())),
            Err(OmarchyError::HttpError("Graph API error: 400 Bad Request — {\"error\":{\"message\":\"nope\"}}".into())),
        ]);
        let w = w.with_spool_root(root.clone());
        let f = src.join("a.txt");
        std::fs::write(&f, b"bytes").unwrap();
        queue_with_file(&db, &root, "o1", &f);
        block(w.drain());
        assert_eq!(db.outbox_status("o1").unwrap().unwrap().0, "pending", "network failure: queued again");
        assert!(crate::attach_spool::spool_dir(&root, "o1").exists(), "the retry needs the copy");
        db.outbox_mark_retry("o1", "net", 0).unwrap(); // due again
        block(w.drain());
        assert_eq!(db.outbox_status("o1").unwrap().unwrap().0, "failed");
        assert!(!crate::attach_spool::spool_dir(&root, "o1").exists());
        assert_eq!(*fake.files.lock().unwrap(), vec![b"bytes".to_vec(), b"bytes".to_vec()], "both attempts sent the same bytes");
        std::fs::remove_dir_all(&src).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_startup_sweep_keeps_queued_spools_and_removes_orphans() {
        let (src, root) = (tdir("d-src"), tdir("d-root"));
        let (w, _fake, db) = setup(vec![]);
        let w = w.with_spool_root(root.clone());
        let f = src.join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        db.outbox_enqueue("gone", "a1", "{}", Database::now_ms() + 60_000).unwrap();
        queue_with_file(&db, &root, "queued", &f);
        crate::attach_spool::snapshot(&root, "orphan", &[crate::compose::OutgoingAttachment { path: f.to_string_lossy().into(), ..Default::default() }]).unwrap();
        assert_eq!(db.outbox_live_ids().unwrap().len(), 2);
        // fresh directories are never swept (they may still be copying)...
        w.sweep_spool();
        assert!(crate::attach_spool::spool_dir(&root, "orphan").exists());
        // ...and an old orphan goes while the queued message's copy stays.
        let live: std::collections::HashSet<String> = db.outbox_live_ids().unwrap().into_iter().collect();
        assert_eq!(crate::attach_spool::sweep(&root, &live, Duration::ZERO), 1);
        assert!(crate::attach_spool::spool_dir(&root, "queued").exists());
        assert!(!crate::attach_spool::spool_dir(&root, "orphan").exists());
        std::fs::remove_dir_all(&src).ok();
        std::fs::remove_dir_all(&root).ok();
    }
}
