//! Cross-thread sync signals.
//!
//! * `MAIL_SERIAL` is bumped whenever a sync stage changed what the mail UI shows (new
//!   messages, flipped read flags, new folder counts). The UI polls `GET /sync/serial`
//!   (tiny, no DB access) and reloads when the number moves, so fresh mail appears
//!   within a poll tick of being stored instead of waiting for the slow refresh timer.
//! * The per-account *gate* orders the stages of a launch: the account's mail daemon opens
//!   it once the recent-50 fetch, folder sync and read-state reconcile are done; calendar
//!   and contacts wait on it so mail gets Graph's throttle and the DB write lock first.

use crate::perf;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static MAIL_SERIAL: AtomicU64 = AtomicU64::new(0);
static READ_PUSH_SERIAL: AtomicU64 = AtomicU64::new(0);
static READ_PUSH: tokio::sync::Notify = tokio::sync::Notify::const_new();
static SMS_SERIAL: AtomicU64 = AtomicU64::new(0);
static OUTBOX_SERIAL: AtomicU64 = AtomicU64::new(0);
static OUTBOX: tokio::sync::Notify = tokio::sync::Notify::const_new();
static GATE: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static META_ACTIVE: Mutex<Option<HashSet<String>>> = Mutex::new(None);

pub fn mail_serial() -> u64 {
    MAIL_SERIAL.load(Ordering::SeqCst)
}

/// Tell the UI the mail data changed. `reason` is only for the perf log.
pub fn bump_mail(reason: &str) {
    let n = MAIL_SERIAL.fetch_add(1, Ordering::SeqCst) + 1;
    perf::mark(&format!("sync serial -> {} ({})", n, reason));
}

pub fn sms_serial() -> u64 {
    SMS_SERIAL.load(Ordering::SeqCst)
}

/// Tell the UI the SMS cache changed (new/updated messages, read flags, removed threads).
pub fn bump_sms(reason: &str) {
    let n = SMS_SERIAL.fetch_add(1, Ordering::SeqCst) + 1;
    perf::mark(&format!("sms serial -> {} ({})", n, reason));
}

/// The one-time repopulate of message details is running (or not) for `account`.
pub fn set_meta_active(account: &str, active: bool) {
    let mut g = META_ACTIVE.lock().unwrap_or_else(|p| p.into_inner());
    let set = g.get_or_insert_with(HashSet::new);
    if active { set.insert(account.to_string()); } else { set.remove(account); }
}

/// True while any account is repopulating message details (drives the status-bar progress).
pub fn meta_active() -> bool {
    META_ACTIVE.lock().unwrap_or_else(|p| p.into_inner()).as_ref().map(|s| !s.is_empty()).unwrap_or(false)
}

/// Mail's priority work for this account is done; calendar/contacts may start.
pub fn open_gate(account: &str) {
    let mut g = GATE.lock().unwrap_or_else(|p| p.into_inner());
    if g.get_or_insert_with(HashSet::new).insert(account.to_string()) {
        perf::mark(&format!("gate[{}] opened: mail priority stages done", account));
    }
}

fn is_open(account: &str) -> bool {
    GATE.lock().unwrap_or_else(|p| p.into_inner()).as_ref().map(|g| g.contains(account)).unwrap_or(false)
}

/// Wait (async) until the account's gate opens, or `max` passes so a failing mail sync
/// can never starve calendar/contacts. Returns true if it opened.
pub async fn wait_gate(account: &str, max: Duration) -> bool {
    let t = Instant::now();
    while !is_open(account) {
        if t.elapsed() >= max {
            perf::mark(&format!("gate[{}] wait timed out after {:?}", account, max));
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    true
}

/// A read/unread change was just stored locally: wake the mail daemons so they push it to the
/// provider now instead of at the next poll.
pub fn request_read_push() {
    READ_PUSH_SERIAL.fetch_add(1, Ordering::SeqCst);
    READ_PUSH.notify_waiters();
}

pub fn read_push_serial() -> u64 {
    READ_PUSH_SERIAL.load(Ordering::SeqCst)
}

/// Resolves on the next `request_read_push()`. Callers compare `read_push_serial()` to catch
/// requests made while they were not waiting.
pub fn read_push_requested() -> tokio::sync::futures::Notified<'static> {
    READ_PUSH.notified()
}

/// A message was queued (or its timing changed): wake the outbox workers so they send it the
/// moment it is due instead of at the next poll.
pub fn request_outbox_run() {
    OUTBOX_SERIAL.fetch_add(1, Ordering::SeqCst);
    OUTBOX.notify_waiters();
}

/// Resolves on the next `request_outbox_run()`.
pub fn outbox_requested() -> tokio::sync::futures::Notified<'static> {
    OUTBOX.notified()
}

#[cfg(test)]
mod read_push_tests {
    use super::*;

    #[test]
    fn request_read_push_wakes_a_waiting_daemon_and_bumps_the_serial() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let before = read_push_serial();
            let mut wake = Box::pin(read_push_requested());
            let _ = wake.as_mut().enable();
            request_read_push();
            tokio::time::timeout(Duration::from_secs(1), wake).await.expect("waiter was not woken");
            assert!(read_push_serial() > before);
        });
    }
}
