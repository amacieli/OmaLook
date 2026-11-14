//! The UI's loopback HTTP server plumbing (roadmap R1): request reading (`request`), the
//! connection cap, the guard, and the typed router (`router`, `routes`, R3.1). Every route is in
//! the table; each says whether it takes the dispatch lock (`router::Lock`).

pub mod guard;
pub mod request;
pub mod router;
pub mod routes;
pub mod session;

use std::sync::atomic::{AtomicUsize, Ordering};

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
}
