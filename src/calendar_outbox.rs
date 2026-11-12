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
            if op == "set_tags" {
                self.run_set_tags(&id, &payload, attempt).await;
                continue;
            }
            if matches!(op.as_str(), "update" | "cancel" | "decline" | "delete") {
                self.run_change(&id, &op, &payload, &tx, attempt).await;
                continue;
            }
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
                    self.refresh_cache(draft.start.date(), created.is_series || draft.recurrence.is_some()).await;
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

    /// One "set the tags" write: send it, read the provider's answer back, and accept it only when it is what was asked
    /// (case-insensitive, order ignored). Transient failures retry; the write is idempotent (it sets, never adds).
    async fn run_set_tags(&self, id: &str, payload: &str, attempt: i64) {
        let parsed: Option<(String, Vec<String>)> = serde_json::from_str::<serde_json::Value>(payload).ok().and_then(|v| {
            Some((v["event_id"].as_str()?.to_string(), v["tags"].as_array()?.iter().filter_map(|t| t.as_str().map(String::from)).collect()))
        });
        let Some((event_id, wanted)) = parsed else {
            let _ = self.db.cal_outbox_mark_failed(id, "the queued tag change could not be read");
            return;
        };
        let wanted = crate::calendar_tags::clean(wanted);
        match self.provider.set_event_tags(&event_id, &wanted).await {
            Ok(stored) => {
                let key = |v: &[String]| { let mut k: Vec<String> = v.iter().map(|t| crate::db::tags::tag_key(t)).collect(); k.sort(); k };
                if key(&stored) != key(&wanted) {
                    let _ = self.db.cal_set_event_tags_local(&event_id, &stored);
                    let _ = self.db.cal_outbox_mark_failed(id, &format!("the calendar kept different tags than asked ({}); the page shows what it holds", stored.join(", ")));
                } else {
                    if let Err(e) = self.db.cal_set_event_tags_local(&event_id, &stored) { error!("Calendar outbox {}: tags set but not cached: {}", self.account_id, e); }
                    let _ = self.db.cal_outbox_mark_sent(id, &event_id);
                    info!("Calendar outbox {}: tags of {} set ({})", self.account_id, id, stored.join(", "));
                }
                sync_state::bump_mail("calendar event tags changed");
            }
            Err(e) if is_transient(&e) && attempt < MAX_ATTEMPTS => {
                let wait = retry_delay_secs(attempt);
                warn!("Calendar outbox {}: tags {} not set ({}); retrying in {}s", self.account_id, id, e, wait);
                let _ = self.db.cal_outbox_mark_retry(id, &short_error(&e), Database::now_ms() + wait * 1000);
            }
            Err(e) => {
                error!("Calendar outbox {}: tags {} failed: {}", self.account_id, id, e);
                let _ = self.db.cal_outbox_mark_failed(id, &short_error(&e));
            }
        }
    }

    /// One edit, cancel, decline or delete (plan phase 3). The same rules as a create: a transient failure retries
    /// with the same pending mark; a refusal fails with the provider's words. All four are safe to repeat.
    async fn run_change(&self, id: &str, op: &str, payload: &str, tx: &str, attempt: i64) {
        let v = match serde_json::from_str::<serde_json::Value>(payload) {
            Ok(v) => v,
            Err(_) => {
                let _ = self.db.cal_outbox_mark_failed(id, "the queued change could not be read");
                return;
            }
        };
        let target = v["target"].as_str().unwrap_or_default().to_string();
        let series = v["series"].as_bool().unwrap_or(false);
        let day = v["start"].as_str().and_then(|s| s.get(..10)).and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
        let comment = v["comment"].as_str().unwrap_or_default().to_string();
        // `Some(draft)` for an update, which changes the cache by its new time; `None` for the removals.
        let outcome: crate::errors::Result<Option<EventDraft>> = match op {
            "update" => match EventDraft::from_request(&v, tx) {
                Ok(draft) => self.provider.update_event(&target, &draft, series).await.map(|_| Some(draft)),
                Err(e) => Err(crate::errors::OmarchyError::HttpError(format!("the queued event could not be read: {e}"))),
            },
            "cancel" => self.provider.cancel_event(&target, &comment).await.map(|_| None),
            "decline" => self.provider.decline_event(&target, &comment).await.map(|_| None),
            "delete" => self.provider.delete_event(&target).await.map(|_| None),
            other => Err(crate::errors::OmarchyError::HttpError(format!("unknown operation: {other}"))),
        };
        match outcome {
            Ok(draft) => {
                info!("Calendar outbox {}: {} of {} done (attempt {})", self.account_id, op, id, attempt);
                if let Err(e) = self.db.cal_outbox_mark_sent(id, &target) {
                    error!("Calendar outbox {}: {} done but not recorded: {}", self.account_id, id, e);
                }
                let day = draft.as_ref().map(|d| d.start.date()).or(day);
                match day {
                    Some(day) => self.refresh_cache(day, series).await,
                    None => sync_state::bump_mail("calendar event changed"),
                }
            }
            Err(e) if is_transient(&e) && attempt < MAX_ATTEMPTS => {
                let wait = retry_delay_secs(attempt);
                warn!("Calendar outbox {}: {} of {} not done ({}); retrying in {}s", self.account_id, op, id, e, wait);
                let _ = self.db.cal_outbox_mark_retry(id, &short_error(&e), Database::now_ms() + wait * 1000);
            }
            Err(e) => {
                error!("Calendar outbox {}: {} of {} failed: {}", self.account_id, op, id, e);
                let _ = self.db.cal_outbox_mark_failed(id, &short_error(&e));
            }
        }
    }

    /// Make the new event visible now instead of at the next poll: the few days around one occurrence, or the
    /// whole configured window for a series.
    async fn refresh_cache(&self, day: chrono::NaiveDate, series: bool) {
        let cfg = CalendarDaemonConfig { settings_path: self.settings_path.clone(), ..CalendarDaemonConfig::default() };
        let daemon = CalendarDaemon::new(cfg, self.db.clone(), self.provider.clone());
        let r = if series {
            // calendarView lags a few seconds behind a new series (seen live: the first expansion right after
            // the create missed it), so show the near window now and expand the whole window after a pause.
            let near = daemon.refresh_near_window().await;
            tokio::time::sleep(Duration::from_secs(6)).await;
            near.and(daemon.refresh_full_window().await)
        } else {
            daemon.refresh_around(day).await
        };
        if let Err(e) = r {
            warn!("Calendar outbox {}: event written, cache not refreshed yet: {}", self.account_id, e);
        }
        sync_state::bump_mail("calendar event written");
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
    async fn a_failed_write_can_be_taken_back_which_dismisses_its_mark() {
        let (_w, _f, db) = setup(vec![]);
        db.cal_outbox_enqueue("c1", "exchange-test", "create", &payload("X"), 0, "tx-1", 0).unwrap();
        db.cal_outbox_claim_due("exchange-test", Database::now_ms()).unwrap();
        db.cal_outbox_mark_failed("c1", "refused").unwrap();
        assert!(db.cal_outbox_cancel("c1").unwrap());
        assert!(db.cal_outbox_list().unwrap().iter().all(|r| r.0 != "c1"), "no longer listed");
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

    // ── tags on events (OL-040 phase 6b) ──

    struct TagFake { script: Mutex<Vec<Result<Vec<String>>>>, seen: Mutex<Vec<(String, Vec<String>)>> }
    #[async_trait]
    impl CalendarProvider for TagFake {
        async fn fetch_events(&self, _l: usize) -> Result<Vec<CalendarEvent>> { Ok(vec![]) }
        async fn fetch_all_events(&self, _tx: tokio::sync::mpsc::Sender<Vec<CalendarEvent>>) -> Result<usize> { Ok(0) }
        async fn fetch_window(&self, _f: chrono::NaiveDate, _t: chrono::NaiveDate) -> Result<Vec<CalendarEvent>> { Ok(vec![]) }
        async fn is_token_valid(&self) -> Result<bool> { Ok(true) }
        async fn set_event_tags(&self, id: &str, tags: &[String]) -> Result<Vec<String>> {
            self.seen.lock().unwrap().push((id.to_string(), tags.to_vec()));
            self.script.lock().unwrap().remove(0)
        }
    }
    fn t(v: &[&str]) -> Vec<String> { v.iter().map(|x| x.to_string()).collect() }

    fn tag_setup(script: Vec<Result<Vec<String>>>) -> (CalendarOutboxWorker, Arc<TagFake>, Arc<Database>) {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("omalook-caltags-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Database::open_for_account(dir.join("m.db").to_str().unwrap(), "exchange-test").unwrap());
        let ev = |id: &str, kind: &str, master: Option<&str>| CalendarEvent { id: id.into(), subject: id.into(), start: "2026-10-05T09:00:00".into(), end: "2026-10-05T10:00:00".into(),
            event_type: kind.into(), series_master_id: master.map(String::from), ..Default::default() };
        db.upsert_event(&ev("M", "seriesMaster", None)).unwrap();
        db.upsert_event(&ev("O1", "occurrence", Some("M"))).unwrap();
        db.upsert_event(&ev("O2", "occurrence", Some("M"))).unwrap();
        db.upsert_event(&ev("S", "singleInstance", None)).unwrap();
        let fake = Arc::new(TagFake { script: Mutex::new(script), seen: Mutex::new(vec![]) });
        (CalendarOutboxWorker::new("exchange-test", db.clone(), fake.clone(), None), fake, db)
    }
    fn cached(db: &Database, id: &str) -> Vec<String> {
        db.cal_event_tags(id)
    }

    #[tokio::test]
    async fn tagging_an_occurrence_tags_the_whole_series_and_reads_back() {
        let (w, fake, db) = tag_setup(vec![Ok(t(&["Family", "Panama"]))]);
        let (target, series) = db.cal_queue_event_tags("t1", "O2", &t(&["Family", "Panama"]), 0).unwrap();
        assert_eq!((target.as_str(), series), ("M", true), "an occurrence resolves to its master");
        assert_eq!(w.drain().await, 1);
        assert_eq!(fake.seen.lock().unwrap().as_slice(), &[("M".to_string(), t(&["Family", "Panama"]))]);
        assert_eq!(db.cal_outbox_status("t1").unwrap().unwrap().0, "sent");
        for id in ["M", "O1", "O2"] { assert_eq!(cached(&db, id), t(&["Family", "Panama"]), "{id}") }
        assert!(cached(&db, "S").is_empty(), "other events untouched");
    }

    #[tokio::test]
    async fn a_single_event_is_not_a_series() {
        let (_w, _f, db) = tag_setup(vec![]);
        assert_eq!(db.cal_queue_event_tags("t1", "S", &t(&["X"]), 0).unwrap(), ("S".to_string(), false));
        assert!(db.cal_queue_event_tags("t2", "nope", &t(&["X"]), 0).is_err());
        assert!(db.cal_queue_event_tags("t3", "S", &t(&["Y"]), 0).unwrap_err().contains("already waiting"), "no two changes racing on one event");
    }

    #[tokio::test]
    async fn if_the_provider_keeps_other_tags_the_write_fails_and_the_cache_shows_the_truth() {
        let (w, _f, db) = tag_setup(vec![Ok(t(&["Family"]))]);
        db.cal_queue_event_tags("t1", "S", &t(&["Family", "Panama"]), 0).unwrap();
        w.drain().await;
        let (state, err, _) = db.cal_outbox_status("t1").unwrap().unwrap();
        assert_eq!(state, "failed");
        assert!(err.unwrap().contains("kept different tags"));
        assert_eq!(cached(&db, "S"), t(&["Family"]));
    }

    #[tokio::test]
    async fn removing_every_tag_is_a_write_too_and_case_order_do_not_matter_for_the_check() {
        let (w, fake, db) = tag_setup(vec![Ok(vec![]), Ok(t(&["b", "A"]))]);
        db.cal_queue_event_tags("t1", "S", &[], 0).unwrap();
        w.drain().await;
        assert_eq!(db.cal_outbox_status("t1").unwrap().unwrap().0, "sent");
        db.cal_queue_event_tags("t2", "S", &t(&["A", "B"]), 0).unwrap();
        w.drain().await;
        assert_eq!(db.cal_outbox_status("t2").unwrap().unwrap().0, "sent");
        assert!(fake.seen.lock().unwrap()[0].1.is_empty());
    }

    #[tokio::test]
    async fn a_transient_failure_retries_and_a_refusal_does_not() {
        let (w, fake, db) = tag_setup(vec![
            Err(OmarchyError::HttpError("Graph API error: 429 Too Many Requests — {}".into())), Ok(t(&["X"])),
            Err(OmarchyError::HttpError(r#"Google API error: 403 — {"error":{"message":"Forbidden"}}"#.into()))]);
        db.cal_queue_event_tags("t1", "S", &t(&["X"]), 0).unwrap();
        w.drain().await;
        assert_eq!(db.cal_outbox_status("t1").unwrap().unwrap().0, "pending");
        db.cal_outbox_mark_retry("t1", "again", 0).unwrap();
        w.drain().await;
        assert_eq!(db.cal_outbox_status("t1").unwrap().unwrap().0, "sent");
        db.cal_queue_event_tags("t2", "S", &t(&["Z"]), 0).unwrap();
        w.drain().await;
        assert_eq!(db.cal_outbox_status("t2").unwrap().unwrap().0, "failed");
        assert_eq!(fake.seen.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn tag_writes_are_listed_with_their_op_so_the_editor_can_tell_them_from_creates() {
        let (_w, _f, db) = tag_setup(vec![]);
        db.cal_queue_event_tags("t1", "S", &t(&["X"]), 0).unwrap();
        assert!(db.cal_outbox_list().unwrap().iter().any(|r| r.2 == "set_tags"), "the outbox list carries the op; draft.js only draws creates as marks");
    }

    // ── edit, cancel, decline, delete (plan phase 3) ──

    #[derive(Default)]
    struct ChangeFake { script: Mutex<Vec<Result<()>>>, seen: Mutex<Vec<String>> }
    #[async_trait]
    impl CalendarProvider for ChangeFake {
        async fn fetch_events(&self, _l: usize) -> Result<Vec<CalendarEvent>> { Ok(vec![]) }
        async fn fetch_all_events(&self, _tx: tokio::sync::mpsc::Sender<Vec<CalendarEvent>>) -> Result<usize> { Ok(0) }
        async fn fetch_window(&self, _f: chrono::NaiveDate, _t: chrono::NaiveDate) -> Result<Vec<CalendarEvent>> { Ok(vec![]) }
        async fn is_token_valid(&self) -> Result<bool> { Ok(true) }
        async fn update_event(&self, target: &str, d: &EventDraft, series: bool) -> Result<crate::calendar_write::CreatedEvent> {
            self.seen.lock().unwrap().push(format!("update {target} series={series} subject={}", d.subject));
            self.script.lock().unwrap().remove(0)?;
            Ok(crate::calendar_write::CreatedEvent { id: target.into(), ..Default::default() })
        }
        async fn delete_event(&self, target: &str) -> Result<()> { self.seen.lock().unwrap().push(format!("delete {target}")); self.script.lock().unwrap().remove(0) }
        async fn cancel_event(&self, target: &str, comment: &str) -> Result<()> { self.seen.lock().unwrap().push(format!("cancel {target} [{comment}]")); self.script.lock().unwrap().remove(0) }
        async fn decline_event(&self, target: &str, _c: &str) -> Result<()> { self.seen.lock().unwrap().push(format!("decline {target}")); self.script.lock().unwrap().remove(0) }
    }

    fn change_setup(script: Vec<Result<()>>) -> (CalendarOutboxWorker, Arc<ChangeFake>, Arc<Database>) {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("omalook-calchange-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Database::open_for_account(dir.join("m.db").to_str().unwrap(), "exchange-test").unwrap());
        let ev = |id: &str, kind: &str, master: Option<&str>| CalendarEvent { id: id.into(), subject: id.into(), start: "2026-10-05T09:00:00".into(), end: "2026-10-05T10:00:00".into(),
            event_type: kind.into(), series_master_id: master.map(String::from), ..Default::default() };
        db.upsert_event(&ev("M", "seriesMaster", None)).unwrap();
        db.upsert_event(&ev("O1", "occurrence", Some("M"))).unwrap();
        db.upsert_event(&ev("S", "singleInstance", None)).unwrap();
        let fake = Arc::new(ChangeFake { script: Mutex::new(script), ..Default::default() });
        (CalendarOutboxWorker::new("exchange-test", db.clone(), fake.clone(), None), fake, db)
    }

    fn change_payload(subject: &str) -> serde_json::Value {
        serde_json::json!({"subject": subject, "start": "2026-10-05T09:00", "end": "2026-10-05T10:00", "time_zone": "America/New_York"})
    }

    #[test]
    fn an_occurrence_edit_targets_the_occurrence_and_a_series_edit_the_master() {
        let (_w, _f, db) = change_setup(vec![]);
        let one = db.cal_event_ref("O1", false).unwrap().unwrap();
        assert_eq!((one.target.as_str(), one.series), ("O1", false));
        let all = db.cal_event_ref("O1", true).unwrap().unwrap();
        assert_eq!((all.target.as_str(), all.series), ("M", true), "'all occurrences' resolves to the master");
        let master = db.cal_event_ref("M", false).unwrap().unwrap();
        assert_eq!((master.target.as_str(), master.series), ("M", true), "a master always changes as a whole");
        let single = db.cal_event_ref("S", true).unwrap().unwrap();
        assert_eq!((single.target.as_str(), single.series), ("S", false));
        assert!(db.cal_event_ref("nope", false).unwrap().is_none());
    }

    #[tokio::test]
    async fn an_occurrence_edit_is_sent_once_with_its_target_and_only_one_change_waits_per_event() {
        let (w, fake, db) = change_setup(vec![Ok(())]);
        let r = db.cal_event_ref("O1", false).unwrap().unwrap();
        db.cal_queue_event_change("u1", "update", &r.target, &change_payload("Moved"), 0, 0).unwrap();
        assert!(db.cal_queue_event_change("u2", "delete", &r.target, &serde_json::json!({}), 0, 0).unwrap_err().contains("already waiting"));
        assert_eq!(w.drain().await, 1);
        assert_eq!(fake.seen.lock().unwrap().as_slice(), &["update O1 series=false subject=Moved".to_string()]);
        assert_eq!(db.cal_outbox_status("u1").unwrap().unwrap().0, "sent");
    }

    #[tokio::test]
    async fn cancel_and_delete_reach_the_provider_with_their_comment_and_are_recorded() {
        let (w, fake, db) = change_setup(vec![Ok(()), Ok(())]);
        db.cal_queue_event_change("c1", "cancel", "S", &serde_json::json!({"comment": "Moved to Friday"}), 2, 0).unwrap();
        db.cal_queue_event_change("d1", "delete", "O1", &serde_json::json!({"start": "2026-10-05T09:00"}), 0, 0).unwrap();
        assert_eq!(w.drain().await, 2);
        assert_eq!(fake.seen.lock().unwrap().as_slice(), &["cancel S [Moved to Friday]".to_string(), "delete O1".to_string()]);
        assert_eq!(db.cal_outbox_status("c1").unwrap().unwrap().0, "sent");
        assert_eq!(db.cal_outbox_status("d1").unwrap().unwrap().0, "sent");
    }

    #[tokio::test]
    async fn a_transient_failure_retries_and_a_refusal_fails_with_the_providers_words() {
        let (w, _f, db) = change_setup(vec![
            Err(OmarchyError::HttpError("Graph API error: 503 Service Unavailable — {}".into())),
            Err(OmarchyError::HttpError("Graph API error: 403 Forbidden — {}".into())),
        ]);
        db.cal_queue_event_change("d1", "delete", "S", &serde_json::json!({}), 0, 0).unwrap();
        w.drain().await;
        assert_eq!(db.cal_outbox_status("d1").unwrap().unwrap().0, "pending");
        db.cal_outbox_mark_retry("d1", "again", 0).unwrap();   // skip the backoff wait
        w.drain().await;
        let (state, err, _) = db.cal_outbox_status("d1").unwrap().unwrap();
        assert_eq!(state, "failed");
        assert!(err.unwrap().contains("not allowed"), "a 403 says what to do, not the raw Graph body");
    }
}
