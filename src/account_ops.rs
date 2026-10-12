//! Account lifecycle operations used by the HTTP layer: list, add (device-flow
//! login under a fresh `<provider>-<suffix>` id), sign out, remove.

use crate::accounts::{new_account_id, provider_of, provider_slug};
use crate::auth::{any_account_authenticated, broker_for, AuthManager};
use crate::db::Database;
use crate::errors::{OmarchyError, Result};
use crate::scheduler::{is_supported, SyncScheduler};
use log::{error, info, warn};
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

fn db_path(config_dir: &Path) -> String {
    config_dir.join("messages.db").to_string_lossy().to_string()
}

/// Email address of an account's mailbox: Graph `/me` for Microsoft accounts, Google's
/// userinfo endpoint for Gmail.
pub fn fetch_account_email(account_id: &str) -> Option<String> {
    let token = AuthManager::for_account(account_id).get_token().ok()?;
    if provider_of(account_id) == "gmail" {
        return crate::google_auth::fetch_email(&token).ok();
    }
    let me: serde_json::Value = ureq::get("https://graph.microsoft.com/v1.0/me?$select=mail,userPrincipalName")
        .set("Authorization", &format!("Bearer {}", token))
        .call()
        .ok()?
        .into_json()
        .ok()?;
    me["mail"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| me["userPrincipalName"].as_str())
        .map(|s| s.to_string())
}

/// auth_state.json is what the QML auth watcher reads. `is_authenticated` means
/// "at least one account is signed in"; `login_serial` bumps after each completed login.
pub fn write_auth_state(config_dir: &Path, error: Option<&str>, login_serial: Option<u64>) {
    write_auth_state_with(config_dir, error, login_serial, None)
}

/// As `write_auth_state`, plus an optional `confirm_reauth` prompt for the UI.
pub fn write_auth_state_with(config_dir: &Path, error: Option<&str>, login_serial: Option<u64>, confirm_reauth: Option<serde_json::Value>) {
    let mut state = serde_json::json!({ "is_authenticated": any_account_authenticated() });
    if let Some(c) = confirm_reauth {
        state["confirm_reauth"] = c;
    }
    if let Some(e) = error {
        state["error"] = e.into();
    }
    if let Some(s) = login_serial {
        state["login_serial"] = s.into();
    }
    if let Ok(json) = serde_json::to_string_pretty(&state) {
        let _ = std::fs::write(config_dir.join("auth_state.json"), json);
    }
}

fn now_serial() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ───────────────────────────────────────────────────────────── listing

/// JSON for GET /accounts. Rows still missing an email get it resolved in the background.
pub fn accounts_json(config_dir: &Path) -> String {
    let db = match Database::open(&db_path(config_dir)) {
        Ok(db) => db,
        Err(e) => {
            warn!("GET /accounts: DB open failed: {}", e);
            return "[]".to_string();
        }
    };
    let accounts = db.list_accounts().unwrap_or_default();
    let rows: Vec<serde_json::Value> = accounts
        .iter()
        .map(|a| {
            let has_token = broker_for(&a.id).is_authenticated();
            // Signed in = not logged out AND credentials present. A logged-out account keeps its token.
            let signed_in = a.enabled && has_token;
            if has_token && a.email.as_deref().unwrap_or("").is_empty() {
                resolve_email_in_background(config_dir, &a.id);
            }
            serde_json::json!({
                "id": a.id,
                "provider": a.provider,
                "email": a.email.clone().unwrap_or_default(),
                "signed_in": signed_in,
            })
        })
        .collect();
    serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string())
}

fn resolve_email_in_background(config_dir: &Path, account_id: &str) {
    static IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let set = IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()));
    if !set.lock().unwrap_or_else(|p| p.into_inner()).insert(account_id.to_string()) {
        return;
    }
    let (path, id) = (db_path(config_dir), account_id.to_string());
    std::thread::spawn(move || {
        if let Some(email) = fetch_account_email(&id) {
            if let Ok(db) = Database::open(&path) {
                let _ = db.set_account_email(&id, &email);
            }
        }
        set.lock().unwrap_or_else(|p| p.into_inner()).remove(&id);
    });
}

// ──────────────────────────────────────────────────────────── add account

#[derive(Debug, PartialEq)]
pub enum LoginOutcome {
    /// A new account row was created under the login's id.
    Added(String),
    /// The mailbox already existed but was signed out; its tokens were replaced.
    Rebound(String),
    /// The mailbox is already signed in under account `existing_id`. Nothing is changed until
    /// the user confirms replacing that account's credentials with the new login.
    ConfirmReauth { existing_id: String, email: String },
}

/// Decide what a completed login means. Pure of I/O beyond the DB so it can be tested:
/// `is_signed_in(id)` reports token presence, `move_tokens(from, to)` transfers credentials.
pub fn register_login(
    db: &Database,
    id: &str,
    provider: &str,
    email: Option<&str>,
    is_signed_in: &dyn Fn(&str) -> bool,
    move_tokens: &dyn Fn(&str, &str) -> Result<()>,
) -> Result<LoginOutcome> {
    if let Some(email) = email {
        let existing = db.list_accounts()?.into_iter().find(|a| {
            a.id != id
                && a.provider == provider
                && a.email.as_deref().map(|e| e.eq_ignore_ascii_case(email)).unwrap_or(false)
        });
        if let Some(existing) = existing {
            if existing.enabled && is_signed_in(&existing.id) {
                return Ok(LoginOutcome::ConfirmReauth { existing_id: existing.id, email: email.to_string() });
            }
            move_tokens(id, &existing.id)?;
            return Ok(LoginOutcome::Rebound(existing.id));
        }
    }
    db.insert_account(id, provider, email, None, "{}")?;
    Ok(LoginOutcome::Added(id.to_string()))
}

/// Providers a user can sign in to (sync may lag behind: see `scheduler::is_supported`).
pub fn can_sign_in(provider: &str) -> bool {
    is_supported(provider) || provider == "gmail"
}

/// Start a login for a brand-new account. The id is generated here
/// (`<provider>-<suffix>`, even for the first account). Microsoft accounts use the
/// device flow (the QML polls device_code.json); Gmail uses the browser flow
/// (the QML shows the URL from google_login.json while the browser is open).
pub fn begin_add_account(config_dir: &Path, provider: &str, scheduler: Arc<SyncScheduler>) -> Result<()> {
    begin_add_account_with_hint(config_dir, provider, None, scheduler)
}

/// As `begin_add_account`; `email` is the address the user typed (Gmail: required, used as
/// Google's `login_hint`).
pub fn begin_add_account_with_hint(config_dir: &Path, provider: &str, email: Option<&str>, scheduler: Arc<SyncScheduler>) -> Result<()> {
    let provider = provider_slug(provider);
    if !can_sign_in(&provider) {
        let msg = format!("{} accounts aren't supported yet", provider);
        write_auth_state(config_dir, Some(&msg), None);
        return Err(OmarchyError::AuthError(msg));
    }
    let id = new_account_id(&provider);
    info!("Adding {} account (pending id {})", provider, id);
    write_auth_state(config_dir, None, None); // clear any stale error

    if provider == "gmail" {
        return begin_gmail_login(config_dir, &id, email, scheduler);
    }
    let dir = config_dir.to_path_buf();
    let mut auth = AuthManager::for_account(&id);
    let cb: Box<dyn FnOnce(&str) + Send> = Box::new(move |id| finish_login(&dir, id, &provider, &scheduler));
    auth.start_device_flow_with_callback(config_dir, Some(cb)).map(|_| ())
}

fn begin_gmail_login(config_dir: &Path, id: &str, email: Option<&str>, scheduler: Arc<SyncScheduler>) -> Result<()> {
    let fail = |msg: String| {
        write_auth_state(config_dir, Some(&msg), None);
        Err(OmarchyError::AuthError(msg))
    };
    let Some(email) = email.map(str::trim).filter(|e| crate::google_auth::looks_like_email(e)) else {
        return fail("Enter your Gmail address first".into());
    };
    let pending = match crate::google_auth::start_login(config_dir, Some(email)) {
        Ok(p) => p,
        Err(e) => return fail(e.to_string()),
    };
    let (dir, id) = (config_dir.to_path_buf(), id.to_string());
    std::thread::spawn(move || match pending.finish(&dir) {
        Ok(login) => {
            // Tokens go to the keyring under this login's id; finish_login then registers the
            // account (same Added / Rebound / ConfirmReauth rules as Microsoft).
            if let Err(e) = broker_for(&id).store_tokens(&login.tokens) {
                error!("Gmail sign-in: could not store tokens: {}", e);
                write_auth_state(&dir, Some("Signed in, but the token could not be saved to the keyring"), None);
                return;
            }
            info!("Gmail sign-in complete for {}", login.email);
            finish_login(&dir, &id, "gmail", &scheduler);
        }
        Err(e) => {
            warn!("Gmail sign-in failed: {}", e);
            write_auth_state(&dir, Some(&e.to_string()), None);
        }
    });
    Ok(())
}

fn finish_login(config_dir: &Path, id: &str, provider: &str, scheduler: &SyncScheduler) {
    let email = fetch_account_email(id);
    let db = match Database::open(&db_path(config_dir)) {
        Ok(db) => db,
        Err(e) => {
            error!("finish_login: DB open failed: {}", e);
            write_auth_state(config_dir, Some("Signed in, but the account could not be saved"), None);
            return;
        }
    };
    let move_tokens = |from: &str, to: &str| -> Result<()> {
        let snapshot = broker_for(from)
            .snapshot()
            .ok_or_else(|| OmarchyError::TokenError("no tokens to move".into()))?;
        broker_for(to).adopt(&snapshot)?;
        broker_for(from).sign_out()
    };
    match register_login(&db, id, provider, email.as_deref(), &|a| broker_for(a).is_authenticated(), &move_tokens) {
        Ok(LoginOutcome::Added(id)) | Ok(LoginOutcome::Rebound(id)) => {
            let _ = db.set_account_enabled(&id, true);
            scheduler.start_account(&id);
            write_auth_state(config_dir, None, Some(now_serial()));
        }
        Ok(LoginOutcome::ConfirmReauth { existing_id, email }) => {
            // Hold the new credentials under the pending id and ask before replacing anything.
            hold_pending_reauth(config_dir, id, &existing_id, &email);
        }
        Err(e) => {
            error!("finish_login failed: {}", e);
            let _ = broker_for(id).sign_out();
            write_auth_state(config_dir, Some(&format!("Could not add account: {}", e)), Some(now_serial()));
        }
    }
}

// ─────────────────────────────────────────── re-authenticate in place

struct PendingReauth {
    existing_id: String,
}

fn pending() -> &'static Mutex<std::collections::HashMap<String, PendingReauth>> {
    static P: OnceLock<Mutex<std::collections::HashMap<String, PendingReauth>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// How long an unanswered "replace sign-in?" prompt keeps the new credentials around.
const REAUTH_PROMPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// Keep the freshly obtained credentials under `pending_id` and ask the UI to confirm.
/// The prompt cancels itself after `REAUTH_PROMPT_TIMEOUT` so the credentials never linger.
fn hold_pending_reauth(config_dir: &Path, pending_id: &str, existing_id: &str, email: &str) {
    pending()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(pending_id.to_string(), PendingReauth { existing_id: existing_id.to_string() });
    write_auth_state_with(
        config_dir,
        None,
        None,
        Some(serde_json::json!({ "pending_id": pending_id, "account_id": existing_id, "email": email })),
    );

    let (dir, pid) = (config_dir.to_path_buf(), pending_id.to_string());
    std::thread::spawn(move || {
        std::thread::sleep(REAUTH_PROMPT_TIMEOUT);
        if pending().lock().unwrap_or_else(|p| p.into_inner()).contains_key(&pid) {
            warn!("Re-authentication prompt for {} timed out; discarding the new credentials", pid);
            let _ = discard_pending(&dir, &pid);
        }
    });
}

fn discard_pending(config_dir: &Path, pending_id: &str) -> Result<()> {
    pending().lock().unwrap_or_else(|p| p.into_inner()).remove(pending_id);
    broker_for(pending_id).sign_out()?;
    write_auth_state(config_dir, None, Some(now_serial()));
    Ok(())
}

/// Apply the user's answer to a "replace sign-in?" prompt.
/// `replace`: move the new credentials onto the existing account (it keeps its id, settings and
/// cached data; running sync picks the new token up). Otherwise discard them.
pub fn resolve_reauth(config_dir: &Path, pending_id: &str, replace: bool, scheduler: &SyncScheduler) -> Result<()> {
    let entry = pending().lock().unwrap_or_else(|p| p.into_inner()).remove(pending_id);
    let Some(entry) = entry else {
        return Err(OmarchyError::SettingsError("no pending re-authentication (it may have timed out)".into()));
    };
    if !replace {
        broker_for(pending_id).sign_out()?;
        write_auth_state(config_dir, None, Some(now_serial()));
        return Ok(());
    }
    let snapshot = broker_for(pending_id)
        .snapshot()
        .ok_or_else(|| OmarchyError::TokenError("new credentials are no longer available".into()))?;
    broker_for(&entry.existing_id).adopt(&snapshot)?;
    broker_for(pending_id).sign_out()?;
    if let Ok(db) = Database::open(&db_path(config_dir)) {
        let _ = db.set_account_enabled(&entry.existing_id, true);
    }
    scheduler.start_account(&entry.existing_id); // no-op if it is already running
    info!("Re-authenticated account {} in place", entry.existing_id);
    write_auth_state(config_dir, None, Some(now_serial()));
    Ok(())
}

// ───────────────────────────────────────────────── sign out / remove

/// Log out: stop syncing and mark the account signed out. The token stays in the keyring
/// and the cached data stays in the database, so `log_in_existing` can resume instantly.
/// Only `remove_account` deletes the keyring entry.
pub fn sign_out_account(config_dir: &Path, id: &str, scheduler: &SyncScheduler) -> Result<()> {
    scheduler.stop_account(id);
    Database::open(&db_path(config_dir))?.set_account_enabled(id, false)?;
    write_auth_state(config_dir, None, Some(now_serial()));
    Ok(())
}

#[derive(Debug, PartialEq)]
pub enum ResumeOutcome {
    /// Stored credentials were reused; sync restarted.
    Resumed,
    /// No usable credentials; the caller must run a device-flow login.
    LoginRequired(String),
}

/// Log in to an existing account. Reuses the stored token when there is one (no network
/// call here: if the grant turned out to be dead, the sync daemon's refresh clears it and
/// the next "Log in" falls through to a device flow).
pub fn log_in_existing(config_dir: &Path, id: &str, scheduler: &SyncScheduler) -> Result<ResumeOutcome> {
    let db = Database::open(&db_path(config_dir))?;
    let account = db
        .list_accounts()?
        .into_iter()
        .find(|a| a.id == id)
        .ok_or_else(|| OmarchyError::SettingsError(format!("unknown account {}", id)))?;
    if !broker_for(id).is_authenticated() {
        return Ok(ResumeOutcome::LoginRequired(account.provider));
    }
    db.set_account_enabled(id, true)?;
    scheduler.start_account(id);
    write_auth_state(config_dir, None, Some(now_serial()));
    Ok(ResumeOutcome::Resumed)
}

/// Log out every signed-in account (what the legacy logout trigger file means).
pub fn sign_out_all(config_dir: &Path, scheduler: &SyncScheduler) -> Result<usize> {
    let accounts = Database::open(&db_path(config_dir))?.list_accounts()?;
    let mut n = 0;
    for a in accounts.iter().filter(|a| a.enabled) {
        sign_out_account(config_dir, &a.id, scheduler)?;
        n += 1;
    }
    write_auth_state(config_dir, None, Some(now_serial()));
    Ok(n)
}

/// Remove the account: delete its keyring entry and everything cached for it.
pub fn remove_account(config_dir: &Path, id: &str, scheduler: &SyncScheduler) -> Result<()> {
    scheduler.stop_account(id);
    broker_for(id).sign_out()?;
    Database::open(&db_path(config_dir))?.delete_account(id)?;
    write_auth_state(config_dir, None, Some(now_serial()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn db() -> Database {
        Database::open(":memory:").unwrap()
    }

    #[test]
    fn new_mailbox_is_added_under_its_login_id() {
        let d = db();
        let out = register_login(&d, "exchange-aaaaaa", "exchange", Some("a@x.com"), &|_| false, &|_, _| Ok(())).unwrap();
        assert_eq!(out, LoginOutcome::Added("exchange-aaaaaa".into()));
        assert_eq!(d.list_accounts().unwrap().len(), 1);
    }

    #[test]
    fn second_account_of_same_provider_gets_its_own_row() {
        let d = db();
        register_login(&d, "exchange-aaaaaa", "exchange", Some("a@x.com"), &|_| true, &|_, _| Ok(())).unwrap();
        let out = register_login(&d, "exchange-bbbbbb", "exchange", Some("b@x.com"), &|_| true, &|_, _| Ok(())).unwrap();
        assert_eq!(out, LoginOutcome::Added("exchange-bbbbbb".into()));
        assert_eq!(d.list_accounts().unwrap().len(), 2);
    }

    #[test]
    fn signing_in_to_an_already_signed_in_mailbox_asks_for_confirmation_and_changes_nothing() {
        let d = db();
        register_login(&d, "exchange-aaaaaa", "exchange", Some("a@x.com"), &|_| true, &|_, _| Ok(())).unwrap();
        let moved = RefCell::new(0);
        let out = register_login(&d, "exchange-bbbbbb", "exchange", Some("A@X.com"), &|_| true, &|_, _| { *moved.borrow_mut() += 1; Ok(()) }).unwrap();
        assert_eq!(out, LoginOutcome::ConfirmReauth { existing_id: "exchange-aaaaaa".into(), email: "A@X.com".into() });
        assert_eq!(*moved.borrow(), 0, "credentials must not move before the user confirms");
        assert_eq!(d.list_accounts().unwrap().len(), 1, "no second row");
    }

    #[test]
    fn signed_out_mailbox_is_rebound_and_keeps_its_id() {
        let d = db();
        register_login(&d, "exchange-aaaaaa", "exchange", Some("a@x.com"), &|_| true, &|_, _| Ok(())).unwrap();
        let moved = RefCell::new(Vec::new());
        let out = register_login(&d, "exchange-bbbbbb", "exchange", Some("a@x.com"), &|_| false, &|from, to| {
            moved.borrow_mut().push((from.to_string(), to.to_string()));
            Ok(())
        }).unwrap();
        assert_eq!(out, LoginOutcome::Rebound("exchange-aaaaaa".into()));
        assert_eq!(moved.into_inner(), vec![("exchange-bbbbbb".to_string(), "exchange-aaaaaa".to_string())]);
        assert_eq!(d.list_accounts().unwrap().len(), 1);
    }

    #[test]
    fn logged_out_account_with_a_kept_token_is_still_rebound_not_duplicate() {
        let d = db();
        register_login(&d, "exchange-aaaaaa", "exchange", Some("a@x.com"), &|_| true, &|_, _| Ok(())).unwrap();
        d.set_account_enabled("exchange-aaaaaa", false).unwrap();
        // token still present (is_signed_in true) but the account is logged out
        let out = register_login(&d, "exchange-bbbbbb", "exchange", Some("a@x.com"), &|_| true, &|_, _| Ok(())).unwrap();
        assert_eq!(out, LoginOutcome::Rebound("exchange-aaaaaa".into()));
    }

    #[test]
    fn same_address_on_different_provider_is_a_separate_account() {
        let d = db();
        register_login(&d, "exchange-aaaaaa", "exchange", Some("a@x.com"), &|_| true, &|_, _| Ok(())).unwrap();
        let out = register_login(&d, "outlook-bbbbbb", "outlook", Some("a@x.com"), &|_| true, &|_, _| Ok(())).unwrap();
        assert_eq!(out, LoginOutcome::Added("outlook-bbbbbb".into()));
    }

    #[test]
    fn unknown_email_still_registers_the_account() {
        let d = db();
        let out = register_login(&d, "exchange-aaaaaa", "exchange", None, &|_| false, &|_, _| Ok(())).unwrap();
        assert_eq!(out, LoginOutcome::Added("exchange-aaaaaa".into()));
    }

    // ── re-authentication prompt (test brokers use in-memory stores, never the real keyring) ──
    fn tok(a: &str, r: &str) -> crate::models::CachedToken {
        crate::models::CachedToken { access_token: a.into(), refresh_token: Some(r.into()), expires_at: None }
    }
    fn setup(tag: &str) -> (std::path::PathBuf, SyncScheduler, String, String) {
        let dir = std::env::temp_dir().join(format!("omalook-reauth-{}-{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // unsupported provider => the scheduler never spawns real sync threads in tests
        let (existing, pending_id) = (format!("gmail-{}1", tag), format!("gmail-{}2", tag));
        broker_for(&existing).adopt(&tok("old", "r_old")).unwrap();
        broker_for(&pending_id).adopt(&tok("new", "r_new")).unwrap();
        (dir.clone(), SyncScheduler::new(&dir), existing, pending_id)
    }

    #[test]
    fn confirming_replaces_the_existing_accounts_credentials_in_place() {
        let (dir, sched, existing, pending_id) = setup("yes");
        hold_pending_reauth(&dir, &pending_id, &existing, "a@x.com");
        assert!(std::fs::read_to_string(dir.join("auth_state.json")).unwrap().contains("confirm_reauth"));

        resolve_reauth(&dir, &pending_id, true, &sched).unwrap();
        assert_eq!(broker_for(&existing).snapshot().unwrap().refresh_token.as_deref(), Some("r_new"));
        assert!(!broker_for(&pending_id).is_authenticated(), "temporary credentials must be gone");
        assert!(!std::fs::read_to_string(dir.join("auth_state.json")).unwrap().contains("confirm_reauth"), "prompt cleared");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelling_keeps_the_existing_credentials_and_discards_the_new_ones() {
        let (dir, sched, existing, pending_id) = setup("no");
        hold_pending_reauth(&dir, &pending_id, &existing, "a@x.com");

        resolve_reauth(&dir, &pending_id, false, &sched).unwrap();
        assert_eq!(broker_for(&existing).snapshot().unwrap().refresh_token.as_deref(), Some("r_old"));
        assert!(!broker_for(&pending_id).is_authenticated());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn answering_twice_or_for_an_unknown_prompt_is_an_error_not_a_second_replace() {
        let (dir, sched, existing, pending_id) = setup("twice");
        hold_pending_reauth(&dir, &pending_id, &existing, "a@x.com");
        resolve_reauth(&dir, &pending_id, false, &sched).unwrap();
        assert!(resolve_reauth(&dir, &pending_id, true, &sched).is_err());
        assert_eq!(broker_for(&existing).snapshot().unwrap().refresh_token.as_deref(), Some("r_old"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
