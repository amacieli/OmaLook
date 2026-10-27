//! Calendar outbox worker: sends the calendar writes the user queued from the event editor
//! (docs/PLAN-calendar-events.md section 6). Same discipline as the mail outbox (`outbox.rs`):
//!
//! * every write waits `[mail] send_delay_secs` in the table first; that wait is the undo window;
//! * an operation is claimed (`pending` to `sending`) before the network call, so an undo that
//!   arrives after the claim is refused and a write is sent by one worker at most once;
//! * only failures that cannot have created the event are retried (`compose::is_transient`), and the
//!   same `transactionId` rides every retry, so a retry after a lost answer finds the event it made;
//! * after a crash an operation left `sending` is reported, never sent again blindly.
//!
//! Phase 2 knows one operation, `create`. After a create the calendar cache is refreshed at once
//! (the few days around a single event, the whole configured window for a series).

use crate::calendar_daemon::{CalendarDaemon, CalendarDaemonConfig};
use crate::calendar_write::EventDraft;
use crate::compose::{is_transient, retry_delay_secs, MAX_ATTEMPTS};
use crate::db::Database;
use crate::providers::CalendarProvider;
use crate::sync_state;
use log::{error, info, warn};
use regex::Regex;
use std::sync::Arc;
use std::time::Duration;

const IDLE: Duration = Duration::from_secs(300);
const KEEP_MS: i64 = 24 * 3600 * 1000;

pub struct CalendarOutboxWorker {
    account_id: String,
    db: Arc<Database>,
    provider: Arc<dyn CalendarProvider>,
    settings_path: Option<std::path::PathBuf>,
}

/// The human part of a provider error: Graph wraps it in JSON.
fn short_error(e: &crate::errors::OmarchyError) -> String {
    let full = e.to_string();
    if full.contains("403") {
        return "not allowed: sign in again to allow editing this calendar".to_string();
    }
    let msg = Regex::new(r#""message"\s*:\s*"((?:[^"\\]|\\.)*)""#)
        .unwrap()
        .captures(&full)
        .map(|c| c[1].replace("\\n", " ").replace("\\\"", "\""))
        .unwrap_or(full);
    msg.chars().take(300).collect()
}

impl CalendarOutboxWorker {
    pub fn new(account_id: &str, db: Arc<Database>, provider: Arc<dyn CalendarProvider>, settings_path: Option<std::path::PathBuf>) -> Self {
        Self { account_id: account_id.to_string(), db, provider, settings_path }
    }

    pub async fn run(&self) {
        match self.db.cal_outbox_fail_interrupted(&self.account_id) {
            Ok(0) => {}
            Ok(n) => warn!("Calendar outbox {}: {} write(s) were interrupted; marked failed, not repeated", self.account_id, n),
            Err(e) => error!("Calendar outbox {}: could not check for interrupted writes: {}", self.account_id, e),
        }
        let _ = self.db.cal_outbox_prune(KEEP_MS);
        info!("Calendar outbox worker started for {}", self.account_id);
        loop {
            let wake = sync_state::cal_outbox_requested();
            tokio::pin!(wake);
            let _ = wake.as_mut().enable();

            self.drain().await;

            let wait = match self.db.cal_outbox_next_due(&self.account_id) {
                Ok(Some(t)) => Duration::from_millis((t - Database::now_ms()).max(0) as u64),
                Ok(None) => IDLE,
                Err(e) => {
                    error!("Calendar outbox {}: cannot read the queue: {}", self.account_id, e);
                    Duration::from_secs(5)
                }
            };
            tokio::select! {
                _ = tokio::time::sleep(wait.min(IDLE)) => {}
                _ = &mut wake => {}
            }
        }
    }

    /// Send everything that is due. Returns how many operations were attempted.
    pub async fn drain(&self) -> usize {
        let due = match self.db.cal_outbox_claim_due(&self.account_id, Database::now_ms()) {
            Ok(d) => d,
            Err(e) => {
                error!("Calendar outbox {}: cannot claim due writes: {}", self.account_id, e);
                return 0;
            }
        };
        let n = due.len();
        for (id, op, payload, tx, attempt) in due {
            if op != "create" {
                let _ = self.db.cal_outbox_mark_failed(&id, &format!("unknown operation: {op}"));
                continue;
            }
            let draft = match serde_json::from_str::<serde_json::Value>(&payload)
                .map_err(|e| e.to_string())
                .and_then(|v| EventDraft::from_request(&v, &tx))
            {
                Ok(d) => d,
                Err(e) => {
                    error!("Calendar outbox {}: unreadable write {}: {}", self.account_id, id, e);
                    let _ = self.db.cal_outbox_mark_failed(&id, "the queued event could not be read");
                    continue;
                }
            };
            match self.provider.create_event(&draft).await {
                Ok(created) => {
                    info!("Calendar outbox {}: created {} (attempt {})", self.account_id, id, attempt);
                    if let Err(e) = self.db.cal_outbox_mark_sent(&id, &created.id) {
                        error!("Calendar outbox {}: created {} but could not record it: {}", self.account_id, id, e);
                    }
                    self.refresh_cache(&draft, created.is_series || draft.recurrence.is_some()).await;
                }
                Err(e) if is_transient(&e) && attempt < MAX_ATTEMPTS => {
                    let wait = retry_delay_secs(attempt);
                    warn!("Calendar outbox {}: {} not created ({}); retrying in {}s", self.account_id, id, e, wait);
                    let _ = self.db.cal_outbox_mark_retry(&id, &short_error(&e), Database::now_ms() + wait * 1000);
                }
                Err(e) => {
                    error!("Calendar outbox {}: {} failed: {}", self.account_id, id, e);
                    let _ = self.db.cal_outbox_mark_failed(&id, &short_error(&e));
                }
            }
        }
        n
    }

    /// Make the new event visible now instead of at the next poll.
    async fn refresh_cache(&self, draft: &EventDraft, series: bool) {
        let cfg = CalendarDaemonConfig { settings_path: self.settings_path.clone(), ..CalendarDaemonConfig::default() };
        let daemon = CalendarDaemon::new(cfg, self.db.clone(), self.provider.clone());
        let r = if series { daemon.refresh_full_window().await } else { daemon.refresh_around(draft.start.date()).await };
        if let Err(e) = r {
            warn!("Calendar outbox {}: event created, cache not refreshed yet: {}", self.account_id, e);
        }
        sync_state::bump_mail("calendar event created");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::{OmarchyError, Result};
    use crate::models::CalendarEvent;
    use async_trait::async_trait;
    use std::sync::Mutex;

    struct Fake {
        script: Mutex<Vec<Result<crate::calendar_write::CreatedEvent>>>,
        seen: Mutex<Vec<(String, String)>>,   // (subject, transaction id)
    }
    #[async_trait]
    impl CalendarProvider for Fake {
        async fn fetch_events(&self, _l: usize) -> Result<Vec<CalendarEvent>> { Ok(vec![]) }
        async fn fetch_all_events(&self, _tx: tokio::sync::mpsc::Sender<Vec<CalendarEvent>>) -> Result<usize> { Ok(0) }
        async fn fetch_window(&self, _f: chrono::NaiveDate, _t: chrono::NaiveDate) -> Result<Vec<CalendarEvent>> { Ok(vec![]) }
        async fn is_token_valid(&self) -> Result<bool> { Ok(true) }
        async fn create_event(&self, d: &EventDraft) -> Result<crate::calendar_write::CreatedEvent> {
            self.seen.lock().unwrap().push((d.subject.clone(), d.transaction_id.clone()));
            self.script.lock().unwrap().remove(0)
        }
    }

    fn setup(script: Vec<Result<crate::calendar_write::CreatedEvent>>) -> (CalendarOutboxWorker, Arc<Fake>, Arc<Database>) {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("omalook-caloutbox-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Database::open_for_account(dir.join("m.db").to_str().unwrap(), "exchange-test").unwrap());
        let fake = Arc::new(Fake { script: Mutex::new(script), seen: Mutex::new(vec![]) });
        (CalendarOutboxWorker::new("exchange-test", db.clone(), fake.clone(), None), fake, db)
    }

    fn payload(subject: &str) -> String {
        serde_json::json!({"subject": subject, "start": "2026-10-28T10:00", "end": "2026-10-28T10:30", "time_zone": "America/New_York"}).to_string()
    }
    fn ok(id: &str) -> Result<crate::calendar_write::CreatedEvent> {
        Ok(crate::calendar_write::CreatedEvent { id: id.into(), ..Default::default() })
    }

    #[tokio::test]
    async fn a_due_create_is_sent_once_and_recorded() {
        let (w, fake, db) = setup(vec![ok("EVT1")]);
        db.cal_outbox_enqueue("c1", "exchange-test", "create", &payload("Standup"), 0, "tx-1", 0).unwrap();
        assert_eq!(w.drain().await, 1);
        assert_eq!(w.drain().await, 0, "nothing left to send");
        assert_eq!(fake.seen.lock().unwrap().as_slice(), &[("Standup".to_string(), "tx-1".to_string())]);
        let (state, err, id) = db.cal_outbox_status("c1").unwrap().unwrap();
        assert_eq!((state.as_str(), err, id.as_deref()), ("sent", None, Some("EVT1")));
    }

    #[tokio::test]
    async fn inside_the_undo_window_nothing_is_sent_and_undo_wins() {
        let (w, fake, db) = setup(vec![]);
        db.cal_outbox_enqueue("c1", "exchange-test", "create", &payload("Later"), 0, "tx-1", Database::now_ms() + 60_000).unwrap();
        assert_eq!(w.drain().await, 0);
        assert!(db.cal_outbox_cancel("c1").unwrap());
        assert!(fake.seen.lock().unwrap().is_empty());
        assert_eq!(db.cal_outbox_status("c1").unwrap().unwrap().0, "cancelled");
    }

    #[tokio::test]
    async fn undo_after_the_claim_is_refused() {
        let (_w, _f, db) = setup(vec![]);
        db.cal_outbox_enqueue("c1", "exchange-test", "create", &payload("X"), 0, "tx-1", 0).unwrap();
        assert_eq!(db.cal_outbox_claim_due("exchange-test", Database::now_ms()).unwrap().len(), 1);
        assert!(!db.cal_outbox_cancel("c1").unwrap(), "already sending");
    }

    #[tokio::test]
    async fn a_transient_failure_retries_with_the_same_transaction_id() {
        let (w, fake, db) = setup(vec![Err(OmarchyError::HttpError("Graph API error: 429 Too Many Requests — {}".into())), ok("EVT2")]);
        db.cal_outbox_enqueue("c1", "exchange-test", "create", &payload("Retry"), 0, "tx-7", 0).unwrap();
        w.drain().await;
        assert_eq!(db.cal_outbox_status("c1").unwrap().unwrap().0, "pending");
        db.cal_outbox_mark_retry("c1", "again", 0).unwrap();   // skip the backoff wait
        w.drain().await;
        assert_eq!(db.cal_outbox_status("c1").unwrap().unwrap().0, "sent");
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].1, seen[1].1, "same transaction id on the retry");
    }

    #[tokio::test]
    async fn a_rejected_write_fails_with_the_providers_words_and_is_not_retried() {
        let (w, fake, db) = setup(vec![Err(OmarchyError::HttpError(r#"Graph API error: 400 Bad Request — {"error":{"message":"Start must be before end"}}"#.into()))]);
        db.cal_outbox_enqueue("c1", "exchange-test", "create", &payload("Bad"), 0, "tx-1", 0).unwrap();
        w.drain().await;
        let (state, err, _) = db.cal_outbox_status("c1").unwrap().unwrap();
        assert_eq!(state, "failed");
        assert_eq!(err.as_deref(), Some("Start must be before end"));
        assert_eq!(fake.seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_write_interrupted_by_a_crash_is_reported_not_repeated() {
        let (w, fake, db) = setup(vec![]);
        db.cal_outbox_enqueue("c1", "exchange-test", "create", &payload("Crash"), 0, "tx-1", 0).unwrap();
        db.cal_outbox_claim_due("exchange-test", Database::now_ms()).unwrap();   // worker died here
        assert_eq!(db.cal_outbox_fail_interrupted("exchange-test").unwrap(), 1);
        assert_eq!(w.drain().await, 0);
        assert!(fake.seen.lock().unwrap().is_empty());
        assert!(db.cal_outbox_status("c1").unwrap().unwrap().1.unwrap().contains("check your calendar"));
    }
}
