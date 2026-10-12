//! Per-account token storage and refresh.
//!
//! - `TokenStore`: where an account's tokens persist (OS keyring entry
//!   `acct:<account_id>`; in-memory for tests).
//! - `TokenBroker`: one per account, shared process-wide. Keeps tokens in
//!   memory (no keyring round-trip per call), refreshes with single-flight
//!   semantics (the state lock is held across the refresh, so concurrent
//!   callers — mail daemon, calendar daemon, HTTP handlers — trigger exactly
//!   one network refresh), and only wipes stored tokens when the server says
//!   the grant is dead (`invalid_grant`/consent/interaction required).
//!   Transient failures (network, 5xx) keep the stored tokens and back off.
//! - `migrate_legacy`: moves the pre-multi-account keyring entry
//!   (`omalook/auth_cache`) to the per-account entry.
//!
//! Account ids are opaque strings. The migrated legacy account is
//! `exchange-primary`; accounts added later use `<provider>-<random suffix>`.

use crate::errors::{OmarchyError, Result};
use crate::models::{CachedToken, TokenResponse};
use log::{debug, info, warn};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Account id used for the single account that existed before multi-account support.
pub const DEFAULT_ACCOUNT: &str = "exchange-primary";

// ───────────────────────────────────────────────────────────────── stores

pub trait TokenStore: Send + Sync {
    fn load(&self, account_id: &str) -> Result<Option<CachedToken>>;
    fn save(&self, account_id: &str, token: &CachedToken) -> Result<()>;
    fn delete(&self, account_id: &str) -> Result<()>;
}

/// OS keyring (Secret Service), one entry per account.
pub struct KeyringStore;

const SERVICE_NAME: &str = "omalook";

impl KeyringStore {
    fn entry(account_id: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE_NAME, &format!("acct:{}", account_id))
            .map_err(|e| OmarchyError::KeyringError(e.to_string()))
    }
}

impl TokenStore for KeyringStore {
    fn load(&self, account_id: &str) -> Result<Option<CachedToken>> {
        match Self::entry(account_id)?.get_password() {
            Ok(json) => Ok(serde_json::from_str(&json).ok()),
            Err(keyring::error::Error::NoEntry) => Ok(None),
            Err(e) => {
                warn!("Failed to read token for {} from keyring: {}", account_id, e);
                Ok(None)
            }
        }
    }

    fn save(&self, account_id: &str, token: &CachedToken) -> Result<()> {
        let json = serde_json::to_string(token)?;
        Self::entry(account_id)?
            .set_password(&json)
            .map_err(|e| OmarchyError::KeyringError(e.to_string()))
    }

    fn delete(&self, account_id: &str) -> Result<()> {
        match Self::entry(account_id)?.delete_password() {
            Ok(_) | Err(keyring::error::Error::NoEntry) => Ok(()),
            Err(e) => Err(OmarchyError::KeyringError(e.to_string())),
        }
    }
}

/// In-memory store (tests, and a fallback when no keyring is available).
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, CachedToken>>);

impl TokenStore for MemoryStore {
    fn load(&self, id: &str) -> Result<Option<CachedToken>> {
        Ok(self.0.lock().unwrap().get(id).cloned())
    }
    fn save(&self, id: &str, t: &CachedToken) -> Result<()> {
        self.0.lock().unwrap().insert(id.to_string(), t.clone());
        Ok(())
    }
    fn delete(&self, id: &str) -> Result<()> {
        self.0.lock().unwrap().remove(id);
        Ok(())
    }
}

// ────────────────────────────────────────────────────────────── migration

/// Move the legacy single-account entry to `account_id`.
///
/// 1. New entry already present → keep it; drop any stale legacy entry (so a
///    later logout can't be undone by a leftover legacy token).
/// 2. Otherwise copy legacy → new, read it back and compare. Only after the
///    read-back matches is the legacy entry deleted. Any failure leaves the
///    legacy entry untouched.
///
/// Returns true when a token was migrated.
pub fn migrate_legacy(
    store: &dyn TokenStore,
    account_id: &str,
    legacy_get: impl Fn() -> Result<Option<String>>,
    legacy_clear: impl Fn() -> Result<()>,
) -> Result<bool> {
    let legacy_json = match legacy_get()? {
        Some(j) => j,
        None => return Ok(false),
    };

    if store.load(account_id)?.is_some() {
        debug!("Token for {} already present; removing stale legacy entry", account_id);
        legacy_clear()?;
        return Ok(false);
    }

    let legacy: CachedToken = match serde_json::from_str(&legacy_json) {
        Ok(t) => t,
        Err(e) => {
            warn!("Legacy token entry is unreadable ({}); leaving it in place", e);
            return Ok(false);
        }
    };

    store.save(account_id, &legacy)?;
    let verified = store.load(account_id)?;
    let ok = verified
        .map(|v| v.access_token == legacy.access_token && v.refresh_token == legacy.refresh_token)
        .unwrap_or(false);
    if !ok {
        let _ = store.delete(account_id);
        return Err(OmarchyError::KeyringError(
            "token migration verification failed; legacy entry kept".into(),
        ));
    }

    legacy_clear()?;
    info!("Migrated legacy token to account {}", account_id);
    Ok(true)
}

// ───────────────────────────────────────────────────────────────── broker

pub enum RefreshOutcome {
    Ok(TokenResponse),
    /// The grant is dead (invalid_grant, consent/interaction required): re-login needed.
    InvalidGrant(String),
    /// Network / server trouble: keep stored tokens and retry later.
    Transient(String),
}

pub trait Refresher: Send + Sync {
    fn refresh(&self, refresh_token: &str) -> RefreshOutcome;

    /// A fresh sign-in just stored new tokens for the account (so any per-account
    /// knowledge about its old grant, e.g. "scope X was not consented", is stale).
    fn reset(&self) {}
}

/// A stored access token must have at least this long left to be reused at launch
/// (instead of refreshing): long enough to finish the startup requests.
const REUSE_MARGIN: Duration = Duration::from_secs(300);

fn unix_now() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `exp` (unix seconds) from a JWT access token; None for opaque tokens (Google's `ya29...`).
pub fn jwt_expiry(token: &str) -> Option<u64> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    if token.split('.').count() != 3 {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice::<serde_json::Value>(&bytes).ok()?["exp"].as_u64()
}

/// Expiry of a stored token: the recorded one, else the JWT claim.
fn stored_expiry(t: &CachedToken) -> Option<u64> {
    t.expires_at.or_else(|| jwt_expiry(&t.access_token))
}

struct State {
    stored: Option<CachedToken>,
    loaded: bool,
    access: Option<String>,
    expires_at: Option<SystemTime>,
    needs_reauth: bool,
    retry_after: Option<Instant>,
    last_error: String,
}

pub struct TokenBroker {
    account_id: String,
    store: Arc<dyn TokenStore>,
    refresher: Arc<dyn Refresher>,
    backoff: Duration,
    state: Mutex<State>,
}

impl TokenBroker {
    pub fn new(account_id: &str, store: Arc<dyn TokenStore>, refresher: Arc<dyn Refresher>) -> Self {
        Self::with_backoff(account_id, store, refresher, Duration::from_secs(15))
    }

    pub fn with_backoff(
        account_id: &str,
        store: Arc<dyn TokenStore>,
        refresher: Arc<dyn Refresher>,
        backoff: Duration,
    ) -> Self {
        Self {
            account_id: account_id.to_string(),
            store,
            refresher,
            backoff,
            state: Mutex::new(State {
                stored: None,
                loaded: false,
                access: None,
                expires_at: None,
                needs_reauth: false,
                retry_after: None,
                last_error: String::new(),
            }),
        }
    }

    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn ensure_loaded(&self, st: &mut State) {
        if !st.loaded {
            st.stored = self.store.load(&self.account_id).ok().flatten();
            st.loaded = true;
            // Launch fast path: the keyring holds the access token from the last run. If it is
            // still good for a while, use it and skip the network refresh entirely.
            if let Some(c) = st.stored.as_ref() {
                if !c.access_token.is_empty() {
                    if let Some(exp) = stored_expiry(c) {
                        let now = unix_now();
                        if exp > now + REUSE_MARGIN.as_secs() {
                            debug!("Reusing stored access token for {} ({} s left)", self.account_id, exp - now);
                            st.access = Some(c.access_token.clone());
                            st.expires_at = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(exp));
                        }
                    }
                }
            }
        }
    }

    /// True when credentials exist (no network, no keyring hit after first load).
    pub fn is_authenticated(&self) -> bool {
        let mut st = self.lock();
        self.ensure_loaded(&mut st);
        st.access.is_some()
            || st.stored.as_ref().map(|c| c.refresh_token.is_some() || !c.access_token.is_empty()).unwrap_or(false)
    }

    /// True after the server rejected the grant; cleared by a fresh login.
    pub fn needs_reauth(&self) -> bool {
        self.lock().needs_reauth
    }

    pub fn expires_at(&self) -> Option<SystemTime> {
        self.lock().expires_at
    }

    /// Valid access token, refreshing if needed. The lock is held across the
    /// refresh so concurrent callers share one network call.
    pub fn get_token(&self) -> Result<String> {
        let mut st = self.lock();
        self.ensure_loaded(&mut st);

        if let (Some(tok), Some(exp)) = (&st.access, st.expires_at) {
            if SystemTime::now() + Duration::from_secs(60) < exp {
                return Ok(tok.clone());
            }
        }

        let refresh_token = st
            .stored
            .as_ref()
            .and_then(|c| c.refresh_token.clone())
            .ok_or_else(|| OmarchyError::TokenError("No valid token available".into()))?;

        if let Some(until) = st.retry_after {
            if Instant::now() < until {
                return Err(OmarchyError::TokenError(format!(
                    "Token refresh backing off after error: {}",
                    st.last_error
                )));
            }
        }

        debug!("Refreshing token for account {}", self.account_id);
        match self.refresher.refresh(&refresh_token) {
            RefreshOutcome::Ok(resp) => {
                st.retry_after = None;
                st.needs_reauth = false;
                self.apply(&mut st, &resp);
                Ok(st.access.clone().unwrap_or_default())
            }
            RefreshOutcome::InvalidGrant(msg) => {
                warn!("Grant rejected for account {} ({}); sign-in required", self.account_id, msg);
                let _ = self.store.delete(&self.account_id);
                st.stored = None;
                st.access = None;
                st.expires_at = None;
                st.needs_reauth = true;
                Err(OmarchyError::TokenError(format!("Sign-in required: {}", msg)))
            }
            RefreshOutcome::Transient(msg) => {
                warn!("Token refresh failed for account {} (keeping stored tokens): {}", self.account_id, msg);
                st.retry_after = Some(Instant::now() + self.backoff);
                st.last_error = msg.clone();
                Err(OmarchyError::TokenError(format!("Token refresh failed: {}", msg)))
            }
        }
    }

    /// Record tokens from a completed login (device flow).
    pub fn store_tokens(&self, resp: &TokenResponse) -> Result<()> {
        // Reset the refresher's remembered scope tier only once the lock is held: a refresh that
        // is still running with the OLD credentials holds it, and may record "not consented" when
        // it finishes. Resetting before waiting for it let that stale verdict outlive the login.
        let mut st = self.lock();
        self.refresher.reset();
        self.ensure_loaded(&mut st);
        st.retry_after = None;
        st.needs_reauth = false;
        self.apply(&mut st, resp);
        Ok(())
    }

    /// Current stored credentials (for moving them to another account id).
    pub fn snapshot(&self) -> Option<CachedToken> {
        let mut st = self.lock();
        self.ensure_loaded(&mut st);
        st.stored.clone()
    }

    /// Take over credentials obtained under another id (re-login of an existing account).
    pub fn adopt(&self, token: &CachedToken) -> Result<()> {
        // Lock first, then reset (see `store_tokens`).
        let mut st = self.lock();
        self.refresher.reset();
        self.store.save(&self.account_id, token)?;
        st.stored = Some(token.clone());
        st.loaded = true;
        st.access = None;
        st.expires_at = None;
        st.needs_reauth = false;
        st.retry_after = None;
        Ok(())
    }

    /// Forget this account's tokens everywhere.
    pub fn sign_out(&self) -> Result<()> {
        let mut st = self.lock();
        st.stored = None;
        st.loaded = true;
        st.access = None;
        st.expires_at = None;
        st.needs_reauth = false;
        st.retry_after = None;
        self.store.delete(&self.account_id)
    }

    fn apply(&self, st: &mut State, resp: &TokenResponse) {
        // Servers may omit the refresh token on refresh — keep the old one then.
        let refresh = resp
            .refresh_token
            .clone()
            .or_else(|| st.stored.as_ref().and_then(|c| c.refresh_token.clone()));
        let cached = CachedToken {
            access_token: resp.access_token.clone(),
            refresh_token: refresh,
            expires_at: Some(unix_now() + resp.expires_in.max(0) as u64),
        };
        if let Err(e) = self.store.save(&self.account_id, &cached) {
            // Keep working from memory; losing the rotated token on disk is the
            // lesser evil versus failing a request that already succeeded.
            warn!("Could not persist tokens for {}: {}", self.account_id, e);
        }
        st.stored = Some(cached);
        st.access = Some(resp.access_token.clone());
        st.expires_at = Some(SystemTime::now() + Duration::from_secs(resp.expires_in.max(0) as u64));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeRefresher {
        calls: AtomicUsize,
        resets: AtomicUsize,
        outcome: Mutex<Box<dyn Fn() -> RefreshOutcome + Send>>,
        delay: Duration,
    }
    impl FakeRefresher {
        fn new(f: impl Fn() -> RefreshOutcome + Send + 'static) -> Arc<Self> {
            Arc::new(Self { calls: AtomicUsize::new(0), resets: AtomicUsize::new(0), outcome: Mutex::new(Box::new(f)), delay: Duration::ZERO })
        }
        fn slow(f: impl Fn() -> RefreshOutcome + Send + 'static, delay: Duration) -> Arc<Self> {
            Arc::new(Self { calls: AtomicUsize::new(0), resets: AtomicUsize::new(0), outcome: Mutex::new(Box::new(f)), delay })
        }
        fn calls(&self) -> usize { self.calls.load(Ordering::SeqCst) }
    }
    impl Refresher for FakeRefresher {
        fn reset(&self) {
            self.resets.fetch_add(1, Ordering::SeqCst);
        }
        fn refresh(&self, _rt: &str) -> RefreshOutcome {
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            (self.outcome.lock().unwrap())()
        }
    }

    fn resp(access: &str, refresh: Option<&str>) -> TokenResponse {
        TokenResponse {
            access_token: access.into(), token_type: "Bearer".into(), expires_in: 3600,
            refresh_token: refresh.map(|s| s.to_string()), scope: String::new(),
        }
    }
    fn seeded() -> Arc<MemoryStore> {
        let s = Arc::new(MemoryStore::default());
        s.save("a", &CachedToken { access_token: "old".into(), refresh_token: Some("r1".into()), expires_at: None }).unwrap();
        s
    }

    #[test]
    fn refresh_caches_in_memory_and_persists() {
        let store = seeded();
        let r = FakeRefresher::new(|| RefreshOutcome::Ok(resp("new", Some("r2"))));
        let b = TokenBroker::new("a", store.clone(), r.clone());
        assert_eq!(b.get_token().unwrap(), "new");
        assert_eq!(b.get_token().unwrap(), "new");
        assert_eq!(r.calls(), 1, "second call must hit the in-memory cache");
        let saved = store.load("a").unwrap().unwrap();
        assert_eq!(saved.refresh_token.as_deref(), Some("r2"));
    }

    #[test]
    fn keeps_old_refresh_token_when_server_omits_it() {
        let store = seeded();
        let b = TokenBroker::new("a", store.clone(), FakeRefresher::new(|| RefreshOutcome::Ok(resp("new", None))));
        b.get_token().unwrap();
        assert_eq!(store.load("a").unwrap().unwrap().refresh_token.as_deref(), Some("r1"));
    }

    #[test]
    fn concurrent_callers_share_one_refresh() {
        let store = seeded();
        let r = FakeRefresher::slow(|| RefreshOutcome::Ok(resp("new", Some("r2"))), Duration::from_millis(100));
        let b = Arc::new(TokenBroker::new("a", store, r.clone()));
        let handles: Vec<_> = (0..8).map(|_| { let b = b.clone(); std::thread::spawn(move || b.get_token().unwrap()) }).collect();
        for h in handles { assert_eq!(h.join().unwrap(), "new"); }
        assert_eq!(r.calls(), 1);
    }

    #[test]
    fn invalid_grant_clears_tokens_and_flags_reauth() {
        let store = seeded();
        let b = TokenBroker::new("a", store.clone(), FakeRefresher::new(|| RefreshOutcome::InvalidGrant("expired".into())));
        assert!(b.get_token().is_err());
        assert!(b.needs_reauth());
        assert!(!b.is_authenticated());
        assert!(store.load("a").unwrap().is_none());
    }

    #[test]
    fn transient_failure_keeps_tokens_and_backs_off() {
        let store = seeded();
        let r = FakeRefresher::new(|| RefreshOutcome::Transient("network down".into()));
        let b = TokenBroker::with_backoff("a", store.clone(), r.clone(), Duration::from_secs(60));
        assert!(b.get_token().is_err());
        assert!(b.get_token().is_err());
        assert_eq!(r.calls(), 1, "second call must be suppressed by backoff");
        assert!(b.is_authenticated());
        assert!(!b.needs_reauth());
        assert!(store.load("a").unwrap().is_some(), "transient errors must not wipe tokens");
    }

    #[test]
    fn recovers_after_transient_failure() {
        let store = seeded();
        let fail = Arc::new(AtomicUsize::new(1));
        let f2 = fail.clone();
        let r = FakeRefresher::new(move || {
            if f2.swap(0, Ordering::SeqCst) == 1 { RefreshOutcome::Transient("blip".into()) }
            else { RefreshOutcome::Ok(resp("new", Some("r2"))) }
        });
        let b = TokenBroker::with_backoff("a", store, r, Duration::ZERO);
        assert!(b.get_token().is_err());
        assert_eq!(b.get_token().unwrap(), "new");
    }

    #[test]
    fn sign_out_removes_tokens() {
        let store = seeded();
        let b = TokenBroker::new("a", store.clone(), FakeRefresher::new(|| RefreshOutcome::Ok(resp("new", Some("r2")))));
        b.get_token().unwrap();
        b.sign_out().unwrap();
        assert!(!b.is_authenticated());
        assert!(store.load("a").unwrap().is_none());
        assert!(b.get_token().is_err());
    }

    #[test]
    fn accounts_are_isolated() {
        let store = seeded();
        store.save("b", &CachedToken { access_token: "x".into(), refresh_token: Some("rb".into()), expires_at: None }).unwrap();
        let ra = FakeRefresher::new(|| RefreshOutcome::InvalidGrant("dead".into()));
        let a = TokenBroker::new("a", store.clone(), ra);
        let b = TokenBroker::new("b", store.clone(), FakeRefresher::new(|| RefreshOutcome::Ok(resp("newb", Some("rb2")))));
        assert!(a.get_token().is_err());
        assert_eq!(b.get_token().unwrap(), "newb");
        assert!(store.load("b").unwrap().is_some());
    }

    #[test]
    fn adopt_replaces_tokens_and_clears_reauth_flag() {
        let store = seeded();
        let b = TokenBroker::new("a", store.clone(), FakeRefresher::new(|| RefreshOutcome::InvalidGrant("dead".into())));
        assert!(b.get_token().is_err());
        assert!(b.needs_reauth());
        let fresh = CachedToken { access_token: "n".into(), refresh_token: Some("rn".into()), expires_at: None };
        b.adopt(&fresh).unwrap();
        assert!(b.is_authenticated());
        assert!(!b.needs_reauth());
        assert_eq!(store.load("a").unwrap().unwrap().refresh_token.as_deref(), Some("rn"));
        assert_eq!(b.snapshot().unwrap().refresh_token.as_deref(), Some("rn"));
    }

    /// A refresh running on the old credentials finishes AFTER a re-login starts; what it
    /// learned (here: "not consented") must not survive the new sign-in.
    #[test]
    fn relogin_clears_scope_verdict_of_a_refresh_still_running_on_old_credentials() {
        struct Marking { marked: std::sync::atomic::AtomicBool }
        impl Refresher for Marking {
            fn reset(&self) { self.marked.store(false, Ordering::SeqCst); }
            fn refresh(&self, _rt: &str) -> RefreshOutcome {
                std::thread::sleep(Duration::from_millis(300));
                self.marked.store(true, Ordering::SeqCst); // like writing token_scope_<id>
                RefreshOutcome::Ok(resp("x", Some("r1")))
            }
        }
        let r = Arc::new(Marking { marked: std::sync::atomic::AtomicBool::new(false) });
        let b = Arc::new(TokenBroker::new("a", seeded(), r.clone()));
        let b2 = b.clone();
        let h = std::thread::spawn(move || { let _ = b2.get_token(); });
        std::thread::sleep(Duration::from_millis(80)); // refresh now in flight, holding the lock
        b.adopt(&CachedToken { access_token: "n".into(), refresh_token: Some("rn".into()), expires_at: None }).unwrap();
        h.join().unwrap();
        assert!(!r.marked.load(Ordering::SeqCst), "stale verdict must be cleared by the re-login");
    }

    // ── launch fast path: reuse a still-valid stored access token ─────────
    fn jwt_with_exp(exp: u64) -> String {
        use base64::Engine;
        let e = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!("{}.{}.sig", e.encode(r#"{"alg":"none"}"#), e.encode(format!(r#"{{"exp":{}}}"#, exp)))
    }
    fn store_with(t: CachedToken) -> Arc<MemoryStore> {
        let s = Arc::new(MemoryStore::default());
        s.save("a", &t).unwrap();
        s
    }

    #[test]
    fn valid_stored_token_is_reused_without_a_refresh() {
        let t = CachedToken { access_token: "stored".into(), refresh_token: Some("r".into()), expires_at: Some(unix_now() + 3000) };
        let r = FakeRefresher::new(|| RefreshOutcome::Ok(resp("new", None)));
        let b = TokenBroker::new("a", store_with(t), r.clone());
        assert_eq!(b.get_token().unwrap(), "stored");
        assert_eq!(r.calls(), 0, "no network when the stored token is still good");
    }

    #[test]
    fn stored_token_close_to_expiry_is_refreshed() {
        let t = CachedToken { access_token: "stored".into(), refresh_token: Some("r".into()), expires_at: Some(unix_now() + 120) };
        let r = FakeRefresher::new(|| RefreshOutcome::Ok(resp("new", None)));
        let b = TokenBroker::new("a", store_with(t), r.clone());
        assert_eq!(b.get_token().unwrap(), "new");
        assert_eq!(r.calls(), 1);
    }

    #[test]
    fn expired_stored_token_is_refreshed() {
        let t = CachedToken { access_token: "stored".into(), refresh_token: Some("r".into()), expires_at: Some(unix_now().saturating_sub(10)) };
        let r = FakeRefresher::new(|| RefreshOutcome::Ok(resp("new", None)));
        let b = TokenBroker::new("a", store_with(t), r.clone());
        assert_eq!(b.get_token().unwrap(), "new");
        assert_eq!(r.calls(), 1);
    }

    #[test]
    fn legacy_entry_without_expiry_uses_the_jwt_claim() {
        let jwt = jwt_with_exp(unix_now() + 3000);
        let t = CachedToken { access_token: jwt.clone(), refresh_token: Some("r".into()), expires_at: None };
        let r = FakeRefresher::new(|| RefreshOutcome::Ok(resp("new", None)));
        let b = TokenBroker::new("a", store_with(t), r.clone());
        assert_eq!(b.get_token().unwrap(), jwt);
        assert_eq!(r.calls(), 0);
    }

    #[test]
    fn opaque_token_without_expiry_is_always_refreshed() {
        // e.g. an entry saved by an older version for Google (ya29... is not a JWT)
        let t = CachedToken { access_token: "ya29.opaque".into(), refresh_token: Some("r".into()), expires_at: None };
        let r = FakeRefresher::new(|| RefreshOutcome::Ok(resp("new", None)));
        let b = TokenBroker::new("a", store_with(t), r.clone());
        assert_eq!(b.get_token().unwrap(), "new");
        assert_eq!(r.calls(), 1);
    }

    #[test]
    fn refresh_persists_the_expiry_and_old_entries_still_deserialize() {
        let store = seeded();
        let b = TokenBroker::new("a", store.clone(), FakeRefresher::new(|| RefreshOutcome::Ok(resp("new", Some("r2")))));
        b.get_token().unwrap();
        let exp = store.load("a").unwrap().unwrap().expires_at.expect("expiry saved");
        assert!(exp > unix_now() + 3500 && exp <= unix_now() + 3600);
        // an entry written before this field existed
        let old: CachedToken = serde_json::from_str(r#"{"access_token":"x","refresh_token":"y"}"#).unwrap();
        assert_eq!(old.expires_at, None);
        assert!(!serde_json::to_string(&old).unwrap().contains("expires_at"));
    }

    #[test]
    fn fresh_sign_in_resets_the_refresher() {
        let store = seeded();
        let r = FakeRefresher::new(|| RefreshOutcome::Ok(resp("new", None)));
        let b = TokenBroker::new("a", store, r.clone());
        b.store_tokens(&resp("login", Some("rl"))).unwrap();
        b.adopt(&CachedToken { access_token: "n".into(), refresh_token: Some("rn".into()), expires_at: None }).unwrap();
        assert_eq!(r.resets.load(Ordering::SeqCst), 2);
    }

    // ── migration ────────────────────────────────────────────────
    fn legacy_json() -> String {
        serde_json::to_string(&CachedToken { access_token: "acc".into(), refresh_token: Some("ref".into()), expires_at: None }).unwrap()
    }

    #[test]
    fn migration_copies_then_clears_legacy() {
        let store = MemoryStore::default();
        let cleared = AtomicUsize::new(0);
        let moved = migrate_legacy(&store, DEFAULT_ACCOUNT, || Ok(Some(legacy_json())), || { cleared.fetch_add(1, Ordering::SeqCst); Ok(()) }).unwrap();
        assert!(moved);
        assert_eq!(store.load(DEFAULT_ACCOUNT).unwrap().unwrap().refresh_token.as_deref(), Some("ref"));
        assert_eq!(cleared.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn migration_noop_without_legacy() {
        let store = MemoryStore::default();
        assert!(!migrate_legacy(&store, DEFAULT_ACCOUNT, || Ok(None), || panic!("must not clear")).unwrap());
        assert!(store.load(DEFAULT_ACCOUNT).unwrap().is_none());
    }

    #[test]
    fn migration_does_not_overwrite_and_drops_stale_legacy() {
        let store = seeded();
        let cleared = AtomicUsize::new(0);
        let moved = migrate_legacy(&*store, "a", || Ok(Some(legacy_json())), || { cleared.fetch_add(1, Ordering::SeqCst); Ok(()) }).unwrap();
        assert!(!moved);
        assert_eq!(store.load("a").unwrap().unwrap().refresh_token.as_deref(), Some("r1"));
        assert_eq!(cleared.load(Ordering::SeqCst), 1, "stale legacy must go so logout can't be undone");
    }

    #[test]
    fn migration_keeps_legacy_when_unreadable() {
        let store = MemoryStore::default();
        let moved = migrate_legacy(&store, DEFAULT_ACCOUNT, || Ok(Some("not json".into())), || panic!("must not clear")).unwrap();
        assert!(!moved);
    }

    /// Store whose reads return nothing → verification must fail and keep legacy.
    struct BlackHole;
    impl TokenStore for BlackHole {
        fn load(&self, _: &str) -> Result<Option<CachedToken>> { Ok(None) }
        fn save(&self, _: &str, _: &CachedToken) -> Result<()> { Ok(()) }
        fn delete(&self, _: &str) -> Result<()> { Ok(()) }
    }
    #[test]
    fn migration_keeps_legacy_when_verification_fails() {
        let r = migrate_legacy(&BlackHole, DEFAULT_ACCOUNT, || Ok(Some(legacy_json())), || panic!("must not clear"));
        assert!(r.is_err());
    }
}

#[cfg(test)]
mod keyring_live {
    use super::*;

    /// Round-trips a throwaway account through the real OS keyring. Opt-in only: it talks
    /// to the live keyring daemon (a run coincided with a gnome-keyring crash once), so it
    /// needs both --ignored and OMARCHY_LIVE_KEYRING_TEST=1.
    /// Run with: OMARCHY_LIVE_KEYRING_TEST=1 cargo test keyring_live -- --ignored
    #[test]
    #[ignore]
    fn keyring_round_trip_with_throwaway_account() {
        if std::env::var("OMARCHY_LIVE_KEYRING_TEST").as_deref() != Ok("1") {
            eprintln!("skipped: set OMARCHY_LIVE_KEYRING_TEST=1 to touch the real keyring");
            return;
        }
        let id = format!("test-{}", std::process::id());
        let store = KeyringStore;
        let t = CachedToken { access_token: "a".into(), refresh_token: Some("r".into()), expires_at: None };
        store.save(&id, &t).unwrap();
        assert_eq!(store.load(&id).unwrap().unwrap().refresh_token.as_deref(), Some("r"));
        store.delete(&id).unwrap();
        assert!(store.load(&id).unwrap().is_none());
    }
}
