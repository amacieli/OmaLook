//! Supervision for long-running sync threads (ARCH-C1, R2.5).
//!
//! A daemon thread that panics used to die silently: its `JoinHandle` was dropped and the UI kept
//! showing "synced". `spawn_supervised` runs the thread body under `catch_unwind`, restarts it with
//! exponential backoff after a panic, and keeps a health record that `GET /sync/health` serves.
//! The body is re-run from scratch on each restart, so it must rebuild its own state.
//!
//! A body that returns normally has chosen to stop: the thread ends and is marked `Stopped`.
//! After `GIVE_UP_PANICS` panics within `GIVE_UP_WINDOW` the thread is marked `Stopped` and is not
//! restarted again; it stays visible in the health report.

use log::{error, warn};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(not(test))]
const BACKOFF_START: Duration = Duration::from_secs(5);
#[cfg(not(test))]
const BACKOFF_MAX: Duration = Duration::from_secs(300);
#[cfg(not(test))]
const SLEEP_STEP: Duration = Duration::from_secs(1);

#[cfg(test)]
const BACKOFF_START: Duration = Duration::from_millis(10);
#[cfg(test)]
const BACKOFF_MAX: Duration = Duration::from_millis(40);
#[cfg(test)]
const SLEEP_STEP: Duration = Duration::from_millis(2);

/// A body that ran at least this long before panicking counts as healthy: backoff starts over.
const HEALTHY_RUN: Duration = Duration::from_secs(600);
/// This many panics inside `GIVE_UP_WINDOW` stop the thread for good.
const GIVE_UP_PANICS: usize = 10;
const GIVE_UP_WINDOW: Duration = Duration::from_secs(3600);
/// Panic payload text kept in the health record (only `&str` and `String` payloads are kept).
const PANIC_TEXT_MAX: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Restarting,
    Stopped,
}

impl State {
    fn as_str(self) -> &'static str {
        match self {
            State::Running => "running",
            State::Restarting => "restarting",
            State::Stopped => "stopped",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Health {
    pub state: State,
    pub restarts: u32,
    pub last_panic_unix: Option<i64>,
    pub last_panic: Option<String>,
}

type Key = (&'static str, Option<String>);

static REGISTRY: Mutex<Option<HashMap<Key, Health>>> = Mutex::new(None);
static HEALTH_SERIAL: AtomicU64 = AtomicU64::new(0);

fn with_registry<R>(f: impl FnOnce(&mut HashMap<Key, Health>) -> R) -> R {
    // A poisoned lock only means some other thread panicked while holding it; the map is still valid.
    let mut guard = REGISTRY.lock().unwrap_or_else(|p| p.into_inner());
    f(guard.get_or_insert_with(HashMap::new))
}

fn update(key: &Key, change: impl FnOnce(&mut Health)) {
    with_registry(|map| {
        let h = map.entry(key.clone()).or_insert(Health {
            state: State::Running,
            restarts: 0,
            last_panic_unix: None,
            last_panic: None,
        });
        change(h);
    });
    HEALTH_SERIAL.fetch_add(1, Ordering::SeqCst);
}

/// A deliberate stop (sign-out, account removed, shutdown) is not a problem to report: the
/// thread's row leaves the health list instead of showing "stopped".
fn forget(key: &Key) {
    let removed = with_registry(|map| map.remove(key).is_some());
    if removed {
        HEALTH_SERIAL.fetch_add(1, Ordering::SeqCst);
    }
}

fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Text of a panic payload, if it is a `&str` or `String`; truncated to `PANIC_TEXT_MAX` chars.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> Option<String> {
    let raw = if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        return None;
    };
    Some(raw.chars().take(PANIC_TEXT_MAX).collect())
}

/// Sleep for `total`, checking `stop` before each step. Returns true if `stop` fired.
fn sleep_unless_stopped(total: Duration, stop: &impl Fn() -> bool) -> bool {
    let mut left = total;
    loop {
        if stop() {
            return true;
        }
        if left.is_zero() {
            return false;
        }
        let step = left.min(SLEEP_STEP);
        std::thread::sleep(step);
        left -= step;
    }
}

/// Run `body` on a named thread, restarting it after a panic until `stop()` returns true, the body
/// returns normally, or it has panicked `GIVE_UP_PANICS` times within `GIVE_UP_WINDOW`.
pub fn spawn_supervised(
    name: &'static str,
    account: Option<String>,
    stop: impl Fn() -> bool + Send + 'static,
    body: impl Fn() + Send + 'static,
) -> std::io::Result<JoinHandle<()>> {
    let key: Key = (name, account);
    std::thread::Builder::new().name(name.to_string()).spawn(move || {
        let mut backoff = BACKOFF_START;
        let mut recent_panics: Vec<Instant> = Vec::new();
        loop {
            if stop() {
                forget(&key);
                return;
            }
            update(&key, |h| h.state = State::Running);
            let started = Instant::now();
            match catch_unwind(AssertUnwindSafe(|| body())) {
                Ok(()) => {
                    // Ended because it was told to stop: not a problem. Ended on its own (e.g. the
                    // database could not be opened): visible as stopped.
                    if stop() { forget(&key); } else { update(&key, |h| h.state = State::Stopped); }
                    return;
                }
                Err(payload) => {
                    if started.elapsed() > HEALTHY_RUN {
                        backoff = BACKOFF_START;
                    }
                    let text = panic_text(payload.as_ref());
                    let now = Instant::now();
                    recent_panics.retain(|t| now.duration_since(*t) < GIVE_UP_WINDOW);
                    recent_panics.push(now);
                    let give_up = recent_panics.len() >= GIVE_UP_PANICS;
                    update(&key, |h| {
                        h.restarts += 1;
                        h.last_panic_unix = Some(now_unix());
                        h.last_panic = text.clone();
                        h.state = if give_up { State::Stopped } else { State::Restarting };
                    });
                    // The message is not logged: it can carry data from the provider or the cache.
                    if give_up {
                        error!("{}: panicked {} times within an hour; sync stopped until restart", name, GIVE_UP_PANICS);
                        return;
                    }
                    warn!("{}: thread panicked; restarting in {:?}", name, backoff);
                    if sleep_unless_stopped(backoff, &stop) {
                        forget(&key);
                        return;
                    }
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                }
            }
        }
    })
}

/// Health of every supervised thread, sorted by name then account, for `GET /sync/health`.
pub fn health_json() -> Value {
    let mut rows: Vec<(Key, Health)> = with_registry(|map| map.iter().map(|(k, h)| (k.clone(), h.clone())).collect());
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let threads: Vec<Value> = rows
        .into_iter()
        .map(|((name, account), h)| {
            json!({
                "name": name,
                "account": account,
                "state": h.state.as_str(),
                "restarts": h.restarts,
                "last_panic_unix": h.last_panic_unix,
                "last_panic": h.last_panic,
            })
        })
        .collect();
    json!({ "threads": threads })
}

/// Something else shown with the threads changed (database health, R3.9): the UI re-reads `/sync/health`.
pub fn bump_health_serial() {
    HEALTH_SERIAL.fetch_add(1, Ordering::SeqCst);
}

/// Changes every time a thread's health changes. Included in `GET /sync/serial`.
pub fn health_serial() -> u64 {
    HEALTH_SERIAL.load(Ordering::SeqCst)
}

/// Health record of one thread (tests and diagnostics).
pub fn health_of(name: &'static str, account: Option<&str>) -> Option<Health> {
    let key: Key = (name, account.map(str::to_string));
    with_registry(|map| map.get(&key).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::sync::Arc;

    fn wait_for(name: &'static str, account: Option<&str>, pred: impl Fn(&Health) -> bool) -> Health {
        for _ in 0..500 {
            if let Some(h) = health_of(name, account) {
                if pred(&h) {
                    return h;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("health of {name} never reached the expected state");
    }

    #[test]
    fn panics_then_return_restarts_twice_and_ends() {
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let never = || false;
        let h = spawn_supervised("supv-test-twice", None, never, move || {
            let n = c.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                panic!("boom {n}");
            }
        })
        .unwrap();
        h.join().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        let health = health_of("supv-test-twice", None).unwrap();
        assert_eq!(health.restarts, 2);
        assert_eq!(health.state, State::Stopped);
        assert_eq!(health.last_panic.as_deref(), Some("boom 1"));
        assert!(health.last_panic_unix.is_some());
    }

    #[test]
    fn stop_during_backoff_ends_the_thread() {
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        // stop() is polled at the loop top (call 1) and before each backoff step; it turns true on
        // the third poll, which lands inside the backoff after the first panic.
        let polls = Arc::new(AtomicU32::new(0));
        let p = polls.clone();
        let stop = move || p.fetch_add(1, Ordering::SeqCst) >= 2;
        let h = spawn_supervised("supv-test-stop", Some("acct-1".into()), stop, move || {
            c.fetch_add(1, Ordering::SeqCst);
            panic!("always");
        })
        .unwrap();
        h.join().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "no second run after stop");
        // A deliberate stop (sign-out, account removed) is not reported as a sync problem.
        assert!(health_of("supv-test-stop", Some("acct-1")).is_none(), "a stopped-on-purpose thread leaves the health list");
    }

    #[test]
    fn health_json_lists_the_thread() {
        let h = spawn_supervised("supv-test-json", Some("acct-json".into()), || false, || panic!("x")).unwrap();
        // Reaches the give-up limit eventually; wait for the record, not for a fixed time.
        wait_for("supv-test-json", Some("acct-json"), |h| h.state == State::Stopped);
        h.join().unwrap();
        let v = health_json();
        let found = v["threads"].as_array().unwrap().iter().find(|t| t["name"] == "supv-test-json").cloned().unwrap();
        assert_eq!(found["account"], "acct-json");
        assert_eq!(found["state"], "stopped");
        assert_eq!(found["restarts"], 10);
    }

    #[test]
    fn ten_panics_stop_the_thread() {
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let h = spawn_supervised("supv-test-giveup", None, || false, move || {
            c.fetch_add(1, Ordering::SeqCst);
            panic!("again");
        })
        .unwrap();
        h.join().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 10, "the tenth panic is the last run");
        let health = health_of("supv-test-giveup", None).unwrap();
        assert_eq!(health.state, State::Stopped);
        assert_eq!(health.restarts, 10);
    }

    #[test]
    fn health_serial_moves_on_change() {
        let before = health_serial();
        update(&("supv-test-serial", None), |h| h.restarts += 0);
        assert!(health_serial() > before);
    }
}
