//! Widening MCP needs the user's confirmation in the UI (security review 2026-11-12, SEC-003 /
//! OL-058, roadmap R1.6).
//!
//! A Settings change that would let an agent do more is not applied when it is requested: it is
//! parked here as the one pending change for this config dir, and the UI shows a dialog with its
//! summary. Only `POST /settings/mcp/confirm {id}` applies it; `/cancel` or a newer request drops it;
//! it expires after `TTL`. Narrowing and neutral changes apply at once, as before.
//!
//! "Wider" is judged on the *effective* policy (`Policy::decide` for every registered tool, the
//! active switch, the reachable accounts), before vs after, so presets, domain levels, tool modes,
//! the master switch and "keep running in background" are all judged the same way, and a change
//! that has no effect yet (a domain raised while MCP is off) is not asked about until the change
//! that does take effect (turning MCP on), whose summary then lists everything it opens.
//! Copying the MCP token to the clipboard and regenerating it always ask.
//!
//! Residual (accepted, review section 4.2 Option A): the UI and any same-uid process holding the
//! session token look the same to the backend, so such a process can confirm its own request.
//! What this removes is the one-request widening (any authenticated POST used to apply at once),
//! widening from a query string, and widening the user never sees: every pending change bumps
//! `serial()`, which the UI polls, so a change it did not start is shown to the user too.

use super::policy::{Decision, McpSettings, Policy};
use super::registry;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A pending change is dropped this long after it was requested.
pub const TTL: Duration = Duration::from_secs(120);
/// Most summary lines shown; the rest are counted.
const MAX_LINES: usize = 12;

#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// `[mcp]` from `base` to `next`. Refused at confirm time if the stored settings are no longer `base`.
    Settings { base: McpSettings, next: McpSettings },
    /// `[sync] run_in_background` to `next`, judged against `base` `[mcp]` settings.
    Background { base: McpSettings, next: bool },
    RegenerateToken,
    CopyToken,
}

struct Pending {
    id: String,
    change: Change,
    summary: Vec<String>,
    created: Instant,
}

static PENDING: Mutex<Option<HashMap<PathBuf, Pending>>> = Mutex::new(None);
static SERIAL: AtomicU64 = AtomicU64::new(0);

/// Moves whenever a pending change appears, is confirmed, cancelled or replaced. Polled by the UI
/// through `GET /sync/serial` (`mcp_pending`).
pub fn serial() -> u64 {
    SERIAL.load(Ordering::SeqCst)
}

fn bump() {
    SERIAL.fetch_add(1, Ordering::SeqCst);
}

fn with<T>(f: impl FnOnce(&mut HashMap<PathBuf, Pending>) -> T) -> T {
    let mut g = PENDING.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

fn new_id() -> String {
    let mut buf = [0u8; 16];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf)).is_err() {
        // No randomness: an id nobody can guess is still required; refuse by making it unusable.
        return String::new();
    }
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && !a.is_empty() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Park `change` as the pending change for `config_dir` (replacing any earlier one). Returns its id.
pub fn propose(config_dir: &Path, change: Change, summary: Vec<String>) -> String {
    let id = new_id();
    with(|m| m.insert(config_dir.to_path_buf(), Pending { id: id.clone(), change, summary, created: Instant::now() }));
    bump();
    log::info!("MCP settings: a wider permission is waiting for confirmation in the UI");
    id
}

/// The pending change for `config_dir` as `(id, summary)`, if one is waiting and not expired.
pub fn current(config_dir: &Path) -> Option<(String, Vec<String>)> {
    with(|m| {
        let expired = m.get(config_dir).map(|p| p.created.elapsed() > TTL).unwrap_or(false);
        if expired {
            m.remove(config_dir);
            bump();
        }
        m.get(config_dir).map(|p| (p.id.clone(), p.summary.clone()))
    })
}

/// Take the pending change with this id (it is removed whatever happens next).
pub fn take(config_dir: &Path, id: &str) -> Result<Change, String> {
    with(|m| {
        let Some(p) = m.get(config_dir) else { return Err("nothing is waiting for confirmation".to_string()) };
        if !same(&p.id, id) {
            return Err("that confirmation is not the one waiting".to_string());
        }
        let p = m.remove(config_dir).expect("checked above");
        bump();
        if p.created.elapsed() > TTL {
            return Err("the confirmation expired; make the change again".to_string());
        }
        Ok(p.change)
    })
}

/// Drop the pending change with this id. False if it was not the one waiting.
pub fn cancel(config_dir: &Path, id: &str) -> bool {
    with(|m| {
        let hit = m.get(config_dir).map(|p| same(&p.id, id)).unwrap_or(false);
        if hit {
            m.remove(config_dir);
            bump();
        }
        hit
    })
}

fn rank(d: Decision) -> u8 {
    match d {
        Decision::Hidden => 0,
        Decision::Ask => 1,
        Decision::Allow => 2,
    }
}

fn word(d: Decision) -> &'static str {
    match d {
        Decision::Hidden => "off",
        Decision::Ask => "asks first",
        Decision::Allow => "allowed",
    }
}

/// What agents could do after the change that they could not before, one line each (empty = the
/// change does not widen anything and needs no confirmation).
pub fn widening(cur: &McpSettings, cur_bg: bool, next: &McpSettings, next_bg: bool) -> Vec<String> {
    let before = Policy::new(cur.clone(), cur_bg);
    let after = Policy::new(next.clone(), next_bg);
    let mut lines = Vec::new();
    if !before.active() && after.active() {
        lines.push("the MCP server turns on".to_string());
    }
    for a in &next.accounts {
        if after.account_allowed(a) && !before.account_allowed(a) {
            lines.push(format!("account {} becomes reachable", a));
        }
    }
    for t in registry::TOOLS {
        let (b, n) = (before.decide(t.name, t.domain, t.class), after.decide(t.name, t.domain, t.class));
        if rank(n) > rank(b) {
            lines.push(format!("{}: {} -> {}", t.name, word(b), word(n)));
        }
    }
    if lines.len() > MAX_LINES {
        let more = lines.len() - MAX_LINES;
        lines.truncate(MAX_LINES);
        lines.push(format!("and {} more", more));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(enabled: bool, domains: &[(&str, &str)], accounts: &[&str]) -> McpSettings {
        let mut m = McpSettings { enabled, ..Default::default() };
        for (k, v) in domains { m.domains.insert(k.to_string(), v.to_string()); }
        m.accounts = accounts.iter().map(|a| a.to_string()).collect();
        m
    }

    #[test]
    fn effective_widening_only() {
        let off = s(false, &[], &[]);
        // Raising a domain while MCP is off changes nothing effective: no confirmation.
        assert!(widening(&off, true, &s(false, &[("mail", "read")], &[]), true).is_empty());
        // Turning it on with mail readable: asks, and says so.
        let w = widening(&s(false, &[("mail", "read")], &[]), true, &s(true, &[("mail", "read")], &[]), true);
        assert!(w.iter().any(|l| l.contains("turns on")), "{:?}", w);
        // Background on with MCP enabled activates it: asks.
        let on = s(true, &[("mail", "read")], &[]);
        assert!(!widening(&on, false, &on, true).is_empty());
        // Narrowing never asks.
        assert!(widening(&on, true, &off, true).is_empty());
        assert!(widening(&on, true, &on, false).is_empty());
        // read -> write on an active server asks.
        assert!(!widening(&on, true, &s(true, &[("mail", "write")], &[]), true).is_empty());
        // An account added while active asks; removed does not.
        let acc = s(true, &[("mail", "read")], &["graph-1"]);
        assert!(widening(&on, true, &acc, true).iter().any(|l| l.contains("graph-1")));
        assert!(widening(&acc, true, &on, true).is_empty());
    }

    #[test]
    fn summary_is_capped() {
        let w = widening(&McpSettings::default(), true, &super::super::policy::with_preset(&s(true, &[], &[]), "approval").unwrap(), true);
        assert!(w.len() <= MAX_LINES + 1);
    }

    #[test]
    fn propose_take_cancel_expire() {
        let d = std::env::temp_dir().join(format!("omalook-confirm-{}", std::process::id()));
        let n0 = serial();
        let id = propose(&d, Change::RegenerateToken, vec!["x".into()]);
        assert_eq!(id.len(), 32);
        assert!(serial() > n0);
        assert_eq!(current(&d).map(|c| c.0), Some(id.clone()));
        assert!(take(&d, "wrong").is_err());
        // A wrong id does not consume the pending change.
        assert_eq!(take(&d, &id), Ok(Change::RegenerateToken));
        assert!(take(&d, &id).is_err(), "single use");
        let id2 = propose(&d, Change::CopyToken, vec![]);
        let id3 = propose(&d, Change::RegenerateToken, vec![]);
        assert!(take(&d, &id2).is_err(), "a newer request replaces the older one");
        assert!(cancel(&d, &id3));
        assert!(current(&d).is_none());
        assert!(!same("", ""), "an empty id never matches");
    }
}
