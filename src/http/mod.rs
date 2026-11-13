//! The UI's loopback HTTP server plumbing (roadmap R1): request reading (`request`), the
//! connection cap and the dispatch policy. Routes still live in `main.rs` until the typed router (R3.1).

pub mod guard;
pub mod request;
pub mod session;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

/// Most connections handled at once. Beyond this a new connection gets `503` straight away instead
/// of waiting, so a pile of stuck clients cannot hold the accept loop.
pub const MAX_CONNECTIONS: usize = 32;

static ACTIVE: AtomicUsize = AtomicUsize::new(0);

/// One handler slot; released when dropped.
pub struct Slot(());

impl Slot {
    pub fn try_take() -> Option<Slot> {
        let mut cur = ACTIVE.load(Ordering::Relaxed);
        loop {
            if cur >= MAX_CONNECTIONS {
                return None;
            }
            match ACTIVE.compare_exchange_weak(cur, cur + 1, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => return Some(Slot(())),
                Err(now) => cur = now,
            }
        }
    }
    pub fn active() -> usize {
        ACTIVE.load(Ordering::Relaxed)
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Routes run one at a time, as they did on the old single accept thread, so read-modify-write of
/// settings.toml and similar keep their old guarantees. Reading the request happens before this
/// lock (a slow or idle client blocks nobody), and the read-only routes in `runs_unlocked` skip it.
static DISPATCH: Mutex<()> = Mutex::new(());

/// Read-only routes that may run beside anything else: the UI's poll, perf marks, and the routes that
/// wait on a provider or scan the cache inline (ARCH-H1), which used to hold up every other request.
pub fn runs_unlocked(method: &str, path: &str) -> bool {
    match method {
        "GET" => matches!(
            path,
            "/sync/serial"
                | "/calendar/find"
                | "/compose/originals"
                | "/contacts/directory"
                | "/contacts/get"
                | "/drafts"
                | "/drafts/get"
                | "/rules"
                | "/rules/preview"
                | "/rules/filter-preview"
                | "/local-rules"
        ),
        "POST" => path == "/perf",
        _ => false,
    }
}

/// Take the dispatch lock unless the route may run unlocked. A handler that panicked earlier leaves
/// the lock poisoned; the data it guards is the files and DB, not the `()`, so carry on.
pub fn dispatch_guard(method: &str, path: &str) -> Option<MutexGuard<'static, ()>> {
    if runs_unlocked(method, path) {
        None
    } else {
        Some(DISPATCH.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// A complete error answer (no body) for a request that could not be read.
pub fn error_response(status: &str) -> String {
    format!("HTTP/1.1 {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_capped_and_released() {
        let before = Slot::active();
        let mut held = Vec::new();
        while let Some(s) = Slot::try_take() {
            held.push(s);
            assert!(held.len() <= MAX_CONNECTIONS);
        }
        assert_eq!(Slot::active(), MAX_CONNECTIONS);
        held.pop();
        assert!(Slot::try_take().is_some());
        drop(held);
        assert_eq!(Slot::active(), before);
    }

    #[test]
    fn only_listed_reads_skip_the_lock() {
        assert!(runs_unlocked("GET", "/sync/serial"));
        assert!(runs_unlocked("GET", "/calendar/find"));
        assert!(runs_unlocked("POST", "/perf"));
        assert!(!runs_unlocked("POST", "/rules/add"));
        assert!(!runs_unlocked("POST", "/calendar/find"));
        assert!(!runs_unlocked("GET", "/settings/mcp"));
        assert!(!runs_unlocked("GET", "/contacts/photo"));
        assert!(dispatch_guard("GET", "/sync/serial").is_none());
        assert!(dispatch_guard("POST", "/settings/mail").is_some());
    }
}
