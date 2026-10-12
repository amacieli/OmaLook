//! Authentication manager using Device Flow OAuth (Microsoft public client)

use crate::errors::{OmarchyError, Result};
use crate::models::{DeviceFlowResponse, TokenResponse};
use crate::keyring_mgr;
use crate::token_store::{self, KeyringStore, RefreshOutcome, Refresher, TokenBroker, DEFAULT_ACCOUNT};
use log::{info, debug, warn, error};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, Duration};

// omalook Azure App Registration (multi-tenant + personal accounts)
// Registered once by the developer; all users authenticate via Device Flow with no Azure interaction.
const PUBLIC_CLIENT_ID: &str = "9c277d6f-edb2-4f82-bda5-901b4c11c457";
// const TENANT_ID: &str = "common";  // Unused; part of OAuth spec but not needed for public client flow
// Explicit Graph scopes — NOT .default. These map to the delegated permissions on the app registration.
const GRAPH_SCOPE: &str = "https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.ReadWrite https://graph.microsoft.com/Tasks.ReadWrite https://graph.microsoft.com/User.Read offline_access";
// Read-only fallback set (accounts that consented before the write scopes were added). A silent
// refresh falls back to this set when the account has not yet consented to a newer scope (e.g.
// Calendars.ReadWrite), so mail/calendar/contacts keep working read-only instead of the whole
// login being thrown away. Mail.ReadWrite already covers Mail.Read; Mail.Send is unchanged.
const GRAPH_SCOPE_BASE: &str = "https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.Read https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/User.Read offline_access";
const DEVICE_AUTH_URL: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/devicecode";
// Top tier: the full set plus the mailbox-settings permission, which is what reading and writing
// the category list (names + colours) needs. Accounts that have not consented to it (or an app
// registration that does not list it) fall back to the full / base tiers without losing anything.
const MAILBOX_SCOPE: &str = "https://graph.microsoft.com/MailboxSettings.ReadWrite";

fn plus_scope() -> String {
    format!("{} {}", GRAPH_SCOPE, MAILBOX_SCOPE)
}

const TOKEN_URL: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/token";

/// Refreshes Microsoft tokens; classifies failures so the broker only wipes
/// stored tokens when the grant is genuinely dead.
struct MsRefresher {
    /// Set once this account was found not to have consented to the full scope set;
    /// later refreshes then go straight to the base scopes (one instance per account).
    /// Also persisted in `marker` so the next launch doesn't repeat the doomed first call
    /// (a full extra round trip + TLS handshake before every cold refresh).
    full_scope_unavailable: std::sync::atomic::AtomicBool,
    /// `<config dir>/token_scope_<account id>`; exists only while the base tier applies.
    /// Holds no secret. Removed by a fresh sign-in (`reset`), the only way to gain scopes.
    marker: std::path::PathBuf,
    /// Same idea one tier up: set once the account turned out not to cover `plus_scope()`.
    plus_unavailable: std::sync::atomic::AtomicBool,
    plus_marker: std::path::PathBuf,
}

impl MsRefresher {
    fn new(account_id: &str) -> Self {
        Self::with_marker(crate::google_auth::default_config_dir().join(format!("token_scope_{}", account_id)))
    }

    fn with_marker(marker: std::path::PathBuf) -> Self {
        let base_tier = marker.exists();
        let mut plus_name = marker.file_name().map(|n| n.to_os_string()).unwrap_or_default();
        plus_name.push("_plus");
        let plus_marker = marker.with_file_name(plus_name);
        Self {
            full_scope_unavailable: std::sync::atomic::AtomicBool::new(base_tier),
            plus_unavailable: std::sync::atomic::AtomicBool::new(plus_marker.exists()),
            marker,
            plus_marker,
        }
    }

    /// One refresh attempt for `scope`. The bool is true when the failure only means a
    /// requested scope hasn't been consented to yet (not that the login is dead).
    fn attempt(refresh_token: &str, scope: &str) -> (RefreshOutcome, bool) {
        let params = [
            ("grant_type", "refresh_token"),
            ("client_id", PUBLIC_CLIENT_ID),
            ("refresh_token", refresh_token),
            ("scope", scope),
        ];
        match ureq::post(TOKEN_URL).send_form(&params) {
            Ok(resp) => match resp.into_json::<TokenResponse>() {
                Ok(t) => (RefreshOutcome::Ok(t), false),
                Err(e) => (RefreshOutcome::Transient(format!("bad token response: {}", e)), false),
            },
            Err(ureq::Error::Status(code, resp)) => classify_refresh_error(code, &resp.into_string().unwrap_or_default()),
            Err(e) => (RefreshOutcome::Transient(e.to_string()), false),
        }
    }
}

/// Map a token-endpoint error to an outcome (+ "only a new scope is missing consent").
fn classify_refresh_error(code: u16, body: &str) -> (RefreshOutcome, bool) {
    let json = serde_json::from_str::<serde_json::Value>(body).ok();
    let err = json.as_ref().and_then(|v| v["error"].as_str()).unwrap_or("");
    let desc = json.as_ref().and_then(|v| v["error_description"].as_str()).unwrap_or("").to_lowercase();
    match err {
        // The app registration does not list a requested scope: same effect as missing consent.
        "invalid_scope" => (RefreshOutcome::InvalidGrant(format!("{}: {}", code, err)), true),
        "invalid_grant" | "interaction_required" | "consent_required" => {
            // AADSTS65001: the user has not consented to (one of) the requested scopes.
            let consent_missing = err == "consent_required" || desc.contains("aadsts65001") || desc.contains("consent");
            (RefreshOutcome::InvalidGrant(format!("{}: {}", code, err)), consent_missing)
        }
        _ => (RefreshOutcome::Transient(format!("HTTP {}: {}", code, body)), false),
    }
}

impl Refresher for MsRefresher {
    fn refresh(&self, refresh_token: &str) -> RefreshOutcome {
        use std::sync::atomic::Ordering;
        if !self.plus_unavailable.load(Ordering::Relaxed) {
            let (outcome, consent_missing) = Self::attempt(refresh_token, &plus_scope());
            if !consent_missing {
                return outcome;
            }
            debug!("Account has not consented to the mailbox-settings scope; category colours need a fresh sign-in");
            self.plus_unavailable.store(true, Ordering::Relaxed);
            if let Err(e) = std::fs::write(&self.plus_marker, b"plus") {
                warn!("Could not record the scope tier ({}): {}", self.plus_marker.display(), e);
            }
        }
        if !self.full_scope_unavailable.load(Ordering::Relaxed) {
            let (outcome, consent_missing) = Self::attempt(refresh_token, GRAPH_SCOPE);
            if !consent_missing {
                return outcome;
            }
            warn!("Account has not consented to all scopes (e.g. Calendars/Contacts/Tasks ReadWrite); refreshing with read-only base scopes. \
                   Sign in to the account again to grant write access.");
            self.full_scope_unavailable.store(true, Ordering::Relaxed);
            if let Err(e) = std::fs::write(&self.marker, b"base") {
                warn!("Could not record the scope tier ({}): {}", self.marker.display(), e);
            }
        }
        Self::attempt(refresh_token, GRAPH_SCOPE_BASE).0
    }

    fn reset(&self) {
        self.full_scope_unavailable.store(false, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_file(&self.marker);
        self.plus_unavailable.store(false, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_file(&self.plus_marker);
    }
}

#[cfg(test)]
struct OfflineRefresher;
#[cfg(test)]
impl Refresher for OfflineRefresher {
    fn refresh(&self, _refresh_token: &str) -> RefreshOutcome {
        RefreshOutcome::Transient("network disabled in unit tests".into())
    }
}

/// Process-wide broker per account, so every daemon / handler shares one
/// in-memory token cache and one refresh at a time.
fn brokers() -> &'static Mutex<HashMap<String, Arc<TokenBroker>>> {
    static BROKERS: OnceLock<Mutex<HashMap<String, Arc<TokenBroker>>>> = OnceLock::new();
    BROKERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// True when any account loaded in this process has credentials.
pub fn any_account_authenticated() -> bool {
    let list: Vec<Arc<TokenBroker>> =
        brokers().lock().unwrap_or_else(|p| p.into_inner()).values().cloned().collect();
    list.iter().any(|b| b.is_authenticated())
}

pub fn broker_for(account_id: &str) -> Arc<TokenBroker> {
    let mut map = brokers().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(b) = map.get(account_id) {
        return Arc::clone(b);
    }

    // Unit tests get an in-memory store and no network: they must never read, write or
    // migrate the developer's real keyring (or crash the keyring daemon).
    #[cfg(test)]
    let (store, refresher): (Arc<dyn token_store::TokenStore>, Arc<dyn Refresher>) =
        (Arc::new(token_store::MemoryStore::default()), Arc::new(OfflineRefresher));
    #[cfg(not(test))]
    let (store, refresher): (Arc<dyn token_store::TokenStore>, Arc<dyn Refresher>) = (
        Arc::new(KeyringStore),
        if crate::accounts::provider_of(account_id) == "gmail" {
            Arc::new(crate::google_auth::GoogleRefresher::new())
        } else {
            Arc::new(MsRefresher::new(account_id))
        },
    );

    #[cfg(not(test))]
    if account_id == DEFAULT_ACCOUNT {
        // Pre-multi-account installs kept one token under `auth_cache`.
        if let Err(e) = token_store::migrate_legacy(
            &*store,
            account_id,
            keyring_mgr::get_cached_token,
            keyring_mgr::clear_cache,
        ) {
            warn!("Legacy token migration failed (legacy entry kept): {}", e);
        }
    }
    let broker = Arc::new(TokenBroker::new(account_id, store, refresher));
    map.insert(account_id.to_string(), Arc::clone(&broker));
    broker
}

pub struct AuthManager {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    token_expires_at: Option<SystemTime>,
    broker: Arc<TokenBroker>,
}

impl AuthManager {
    /// Manager for the default (legacy single) account.
    pub fn new() -> Self {
        Self::for_account(DEFAULT_ACCOUNT)
    }

    pub fn account_id(&self) -> &str {
        self.broker.account_id()
    }

    pub fn for_account(account_id: &str) -> Self {
        Self {
            access_token: None,
            refresh_token: None,
            token_expires_at: None,
            broker: broker_for(account_id),
        }
    }
    
    /// Initiate Device Flow login (user sees code on screen)
    pub fn login(&mut self, device_code_callback: Option<&dyn Fn(&str, &str)>) -> Result<bool> {
        info!("Starting Device Flow login...");
        
        // Request device code from Microsoft
        let device_response = self.acquire_device_code()?;
        
        // Invoke callback if provided (for QML integration)
        if let Some(cb) = device_code_callback {
            cb(&device_response.user_code, &device_response.verification_uri);
        } else {
            // Print to terminal for CLI testing
            println!("\n{}", "=".repeat(70));
            println!("🔐 Device Login Required");
            println!("{}", "=".repeat(70));
            println!("1. Open this URL on any device (phone, tablet, another computer):");
            println!("   → {}", device_response.verification_uri);
            println!("2. Enter this code when prompted:");
            println!("   → {}", device_response.user_code);
            println!("\nWaiting for authentication...");
            println!("{}\n", "=".repeat(70));
        }
        
        // Poll for token
        match self.poll_for_token(&device_response)? {
            Some(_token_data) => {
                info!("Device Flow login successful");
                Ok(true)
            }
            None => {
                warn!("Device Flow login timed out or was denied");
                Ok(false)
            }
        }
    }
    
    /// Request device code from Microsoft
    fn acquire_device_code(&self) -> Result<DeviceFlowResponse> {
        debug!("Requesting device code from {}", DEVICE_AUTH_URL);
        
        // Ask for the top tier first; an app registration that does not list the mailbox
        // permission rejects it, and signing in with the full set must still work then.
        let plus = plus_scope();
        let mut scope = plus.as_str();
        let params = [
            ("client_id", PUBLIC_CLIENT_ID),
            ("scope", scope),
        ];
        let first = ureq::post(DEVICE_AUTH_URL).send_form(&params);
        let first = match first {
            Err(ureq::Error::Status(code, r)) => {
                let body = r.into_string().unwrap_or_default();
                if body.contains("invalid_scope") || body.contains("AADSTS70011") || body.contains("AADSTS650053") {
                    warn!("Device code with the mailbox-settings scope refused ({}); using the standard scopes", code);
                    scope = GRAPH_SCOPE;
                    let retry = [("client_id", PUBLIC_CLIENT_ID), ("scope", scope)];
                    ureq::post(DEVICE_AUTH_URL).send_form(&retry)
                } else {
                    Err(ureq::Error::Status(code, ureq::Response::new(code, "error", &body).unwrap_or_else(|_| ureq::Response::new(500, "error", "").unwrap())))
                }
            }
            other => other,
        };
        
        // ureq 2.x returns Err(ureq::Error::Status(code, response)) for non-2xx —
        // we must handle that arm to read the error body, not just convert to string.
        let resp = match first {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => {
                let body = r.into_string().unwrap_or_default();
                error!("Device code request failed with status {}: {}", code, body);
                return Err(OmarchyError::InvalidDeviceFlow(
                    format!("Status {}: {}", code, body)
                ));
            }
            Err(e) => {
                error!("Device code request transport error: {}", e);
                return Err(OmarchyError::HttpError(e.to_string()));
            }
        };

        let device_response: DeviceFlowResponse = resp
            .into_json()
            .map_err(|e| OmarchyError::HttpError(e.to_string()))?;
        
        debug!("Device code received, expires in {} seconds", device_response.expires_in);
        
        // Log the actual device code for debugging
        info!("Device Code: {} | Verification URI: {}", device_response.user_code, device_response.verification_uri);
        
        Ok(device_response)
    }
    
    /// Poll Microsoft for token (blocks until success, timeout, or denial)
    fn poll_for_token(&mut self, device_response: &DeviceFlowResponse) -> Result<Option<()>> {
        let start_time = SystemTime::now();
        let expires_at = start_time + Duration::from_secs(device_response.expires_in as u64);
        let interval = Duration::from_secs((device_response.interval.max(5)) as u64);
        let mut poll_count = 0;
        
        // Give user 10 seconds to authenticate before we start polling
        // This prevents race condition where backend polls before user enters code
        debug!("Waiting 10 seconds for user to authenticate...");
        std::thread::sleep(Duration::from_secs(10));
        
        loop {
            if SystemTime::now() >= expires_at {
                warn!("Device code expired");
                return Err(OmarchyError::AuthError("Device code expired".to_string()));
            }
            
            std::thread::sleep(interval);
            poll_count += 1;
            
            let params = [
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", PUBLIC_CLIENT_ID),
                ("device_code", &device_response.device_code),
            ];
            
            let resp = match ureq::post(TOKEN_URL).send_form(&params) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("🔍 DEBUG: Error debug: {:?}", e);
                    // Check if this is a Status error (400) which we can recover from
                    match e {
                        ureq::Error::Status(code, response) => {
                            eprintln!("🔴 HTTP {} from token endpoint (will handle below)", code);
                            response
                        }
                        ureq::Error::Transport(te) => {
                            eprintln!("❌ Transport error: {:?}", te);
                            return Err(OmarchyError::HttpError(format!("Transport error: {}", te)));
                        }
                    }
                }
            };
            
            match resp.status() {
                200 => {
                    let token_data: TokenResponse = resp
                        .into_json()
                        .map_err(|e| OmarchyError::HttpError(e.to_string()))?;
                    
                    info!("Device Flow complete (polled {} times)", poll_count);
                    self.cache_tokens(&token_data)?;
                    self.access_token = Some(token_data.access_token);
                    self.refresh_token = token_data.refresh_token;
                    self.token_expires_at = Some(SystemTime::now() + Duration::from_secs(token_data.expires_in as u64));
                    println!("✅ Authentication successful!\n");
                    return Ok(Some(()));
                }
                400 => {
                    let body_str = resp.into_string().unwrap_or_default();
                    eprintln!("🔴 HTTP 400 raw response: {}", body_str);
                    
                    match serde_json::from_str::<serde_json::Value>(&body_str) {
                        Ok(error_json) => {
                            debug!("🔍 Token polling received 400 error: {}", error_json);
                            
                            if let Some(error) = error_json.get("error").and_then(|v| v.as_str()) {
                                match error {
                                    "authorization_pending" => {
                                        // Still waiting; continue polling
                                        debug!("⏳ authorization_pending - continuing to poll");
                                        continue;
                                    }
                                    "expired_token" => {
                                        return Err(OmarchyError::AuthError("Device code expired".to_string()));
                                    }
                                    "access_denied" => {
                                        return Err(OmarchyError::AuthError("Authentication denied by user".to_string()));
                                    }
                                    _ => {
                                        debug!("❌ Unrecognized error: {}", error);
                                        return Err(OmarchyError::AuthError(format!("Auth failed: {}", error)));
                                    }
                                }
                            }
                        }
                        Err(json_err) => {
                            eprintln!("❌ Failed to parse JSON from 400 response: {}", json_err);
                            eprintln!("Raw body was: {}", body_str);
                            return Err(OmarchyError::HttpError(format!("HTTP 400 with unparseable JSON: {}", body_str)));
                        }
                    }
                }
                _ => {
                    return Err(OmarchyError::HttpError(
                        format!("Unexpected status {}: {}", resp.status(), resp.into_string().unwrap_or_default())
                    ));
                }
            }
        }
    }
    
    /// Get a valid access token (refreshed through the shared broker if needed)
    pub fn get_token(&mut self) -> Result<String> {
        let token = self.broker.get_token()?;
        self.access_token = Some(token.clone());
        self.token_expires_at = self.broker.expires_at();
        Ok(token)
    }
    
    /// Check if user is authenticated (has stored credentials; no network)
    pub fn is_authenticated(&self) -> bool {
        self.broker.is_authenticated()
    }
    
    /// Trigger device flow (can be called from external signal)
    pub fn trigger_device_flow(&mut self, config_dir: &std::path::Path) -> Result<bool> {
        info!("Device Flow triggered from Settings");
        
        // First logout any existing session
        let _ = self.logout();
        
        // Now start device flow
        self.start_device_flow_interactive(config_dir)
    }
    
    /// Logout and clear tokens
    pub fn logout(&mut self) -> Result<()> {
        info!("Logging out...");
        self.access_token = None;
        self.refresh_token = None;
        self.token_expires_at = None;
        self.broker.sign_out()?;
        // Drop any leftover pre-multi-account entry so it can't resurrect the login.
        let _ = keyring_mgr::clear_cache();
        info!("Logout successful");
        Ok(())
    }
    
    /// Write auth state to a JSON file for QML to read
    pub fn write_state_file(&self, config_dir: &std::path::Path) -> Result<()> {
        use std::fs;
        
        // Prefer in-memory token over keyring read — the background auth thread
        // has just written the token to keyring and set self.access_token.
        // Using is_authenticated() (which reads keyring) can race or fail silently.
        let authenticated = self.access_token.is_some() || self.is_authenticated() || any_account_authenticated();
        
        let state = serde_json::json!({
            "is_authenticated": authenticated,
            "has_access_token": self.access_token.is_some(),
            "token_expires_at": self.token_expires_at.and_then(|t| {
                t.duration_since(SystemTime::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs())
            }),
        });
        
        let state_file = config_dir.join("auth_state.json");
        let json_str = serde_json::to_string_pretty(&state)
            .map_err(|e| OmarchyError::JsonError(e))?;
        
        fs::write(&state_file, json_str)
            .map_err(OmarchyError::from)?;
        
        debug!("Auth state written to {}", state_file.display());
        Ok(())
    }
    
    /// Write auth error to a JSON file for QML to display
    fn write_auth_error(&self, config_dir: &std::path::Path, error_msg: &str) -> Result<()> {
        use std::fs;
        
        let error_state = serde_json::json!({
            "is_authenticated": any_account_authenticated(),
            "error": error_msg,
        });
        
        let state_file = config_dir.join("auth_state.json");
        let json_str = serde_json::to_string_pretty(&error_state)
            .map_err(|e| OmarchyError::JsonError(e))?;
        
        fs::write(&state_file, json_str)
            .map_err(OmarchyError::from)?;
        
        warn!("Auth error written: {}", error_msg);
        Ok(())
    }

    /// Start device flow and write code to file for QML display (non-blocking)
    pub fn start_device_flow_interactive(&mut self, config_dir: &std::path::Path) -> Result<bool> {
        self.start_device_flow_with_callback(config_dir, None)
    }

    /// Like `start_device_flow_interactive`; `on_success(account_id)` runs on the
    /// polling thread once tokens for this manager's account have been stored.
    pub fn start_device_flow_with_callback(
        &mut self,
        config_dir: &std::path::Path,
        on_success: Option<Box<dyn FnOnce(&str) + Send>>,
    ) -> Result<bool> {
        info!("Starting Device Flow login...");
        
        // Request device code from Microsoft
        let device_response = match self.acquire_device_code() {
            Ok(resp) => resp,
            Err(e) => {
                let err_msg = format!("Failed to get device code: {}", e);
                let _ = self.write_auth_error(config_dir, &err_msg);
                return Err(e);
            }
        };
        
        // Write device code to file for QML to read
        let device_code_file = config_dir.join("device_code.json");
        let device_code_data = serde_json::json!({
            "device_code": device_response.device_code,
            "user_code": device_response.user_code,
            "verification_uri": device_response.verification_uri,
            "expires_in": device_response.expires_in,
            "interval": device_response.interval,
        });
        
        let json_str = serde_json::to_string_pretty(&device_code_data)
            .map_err(|e| OmarchyError::JsonError(e))?;
        std::fs::write(&device_code_file, json_str).map_err(OmarchyError::from)?;
        
        info!("Device code written to {}", device_code_file.display());
        
        // Display to console for testing
        println!("\n{}", "=".repeat(70));
        println!("📱 Device Flow Authentication");
        println!("==================================");
        println!("User Code: {}", device_response.user_code);
        println!("Verification URL: {}", device_response.verification_uri);
        println!("Expires in: {} seconds", device_response.expires_in);
        println!("\n1. Open: {}", device_response.verification_uri);
        println!("2. Enter code: {}", device_response.user_code);
        println!("\nWaiting for authentication in background...");
        println!("{}\n", "=".repeat(70));
        
        // Spawn a background thread to poll for token (non-blocking)
        // This allows QML UI to remain responsive while polling happens
        let device_response_clone = device_response.clone();
        let config_dir_clone = config_dir.to_path_buf();
        let account_id = self.account_id().to_string();
        
        std::thread::spawn(move || {
            // Tokens go to the account this login was started for (not always the default one).
            let mut auth = AuthManager::for_account(&account_id);
            match auth.poll_for_token(&device_response_clone) {
                Ok(Some(_)) => {
                    match auth.write_state_file(&config_dir_clone) {
                        Ok(()) => {
                            info!("✅ Background token polling succeeded - state file written");
                            if let Some(cb) = on_success {
                                cb(&account_id);
                            }
                        }
                        Err(e) => {
                            warn!("Failed to write state file: {}", e);
                            let _ = auth.write_auth_error(&config_dir_clone, &format!("Token acquired but failed to write state: {}", e));
                        }
                    }
                }
                Ok(None) => {
                    let _ = auth.write_auth_error(&config_dir_clone, "Authentication failed or was cancelled");
                    warn!("Background token polling returned None");
                }
                Err(e) => {
                    let err_msg = format!("Background auth error: {}", e);
                    let _ = auth.write_auth_error(&config_dir_clone, &err_msg);
                    warn!("Background token polling failed: {}", e);
                }
            }
        });
        
        // Return immediately so QML can display code to user
        Ok(true)
    }
    
    /// Hand freshly issued tokens (device flow) to the broker, which persists them
    fn cache_tokens(&self, token_data: &TokenResponse) -> Result<()> {
        self.broker.store_tokens(token_data)?;
        debug!("Tokens stored for account {}", self.broker.account_id());
        Ok(())
    }
}

impl Default for AuthManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    #[test]
    fn missing_consent_is_distinguished_from_a_dead_grant() {
        let consent = r#"{"error":"invalid_grant","error_description":"AADSTS65001: The user or administrator has not consented to use the application"}"#;
        let (o, missing) = classify_refresh_error(400, consent);
        assert!(matches!(o, RefreshOutcome::InvalidGrant(_)) && missing);

        let revoked = r#"{"error":"invalid_grant","error_description":"AADSTS70008: The provided authorization code or refresh token has expired"}"#;
        let (o, missing) = classify_refresh_error(400, revoked);
        assert!(matches!(o, RefreshOutcome::InvalidGrant(_)) && !missing);

        let (o, missing) = classify_refresh_error(503, "service unavailable");
        assert!(matches!(o, RefreshOutcome::Transient(_)) && !missing);
    }

    #[test]
    fn full_scope_is_a_write_superset_of_the_base_scopes() {
        let full: Vec<&str> = GRAPH_SCOPE.split(' ').collect();
        for scope in GRAPH_SCOPE_BASE.split(' ') {
            // A ReadWrite scope in the full set satisfies the matching Read scope in the base set.
            let rw = scope.replace(".Read", ".ReadWrite");
            assert!(full.contains(&scope) || full.contains(&rw.as_str()), "{} missing from full scope", scope);
        }
        for w in ["Calendars.ReadWrite", "Contacts.ReadWrite", "Tasks.ReadWrite", "Mail.ReadWrite", "Mail.Send"] {
            assert!(GRAPH_SCOPE.contains(w), "{} missing", w);
        }
        assert!(!GRAPH_SCOPE.contains("Mail.Read ") && !GRAPH_SCOPE_BASE.contains("Mail.Read "));
        assert!(!GRAPH_SCOPE_BASE.contains("Tasks"));
    }
}

#[cfg(test)]
mod scope_marker_tests {
    use super::*;
    use crate::token_store::Refresher;
    use std::sync::atomic::Ordering;

    fn marker(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("omalook-scope-{}-{}", tag, std::process::id()))
    }

    #[test]
    fn marker_selects_the_base_tier_and_a_fresh_sign_in_clears_it() {
        let path = marker("tier");
        let _ = std::fs::remove_file(&path);
        assert!(!MsRefresher::with_marker(path.clone()).full_scope_unavailable.load(Ordering::Relaxed));

        std::fs::write(&path, b"base").unwrap();
        let r = MsRefresher::with_marker(path.clone());
        assert!(r.full_scope_unavailable.load(Ordering::Relaxed), "next launch skips the doomed full-scope call");

        r.reset();
        assert!(!r.full_scope_unavailable.load(Ordering::Relaxed));
        assert!(!path.exists(), "re-login must retry the full scope set");
    }
}

#[cfg(test)]
mod scope_plus_tests {
    use super::*;
    use crate::token_store::Refresher;
    use std::sync::atomic::Ordering;

    #[test]
    fn plus_tier_is_the_full_set_plus_mailbox_settings_and_has_its_own_marker() {
        assert!(plus_scope().starts_with(GRAPH_SCOPE));
        assert!(plus_scope().ends_with("MailboxSettings.ReadWrite"));
        assert!(!GRAPH_SCOPE.contains("MailboxSettings"), "the lower tiers must not ask for it");
        let path = std::env::temp_dir().join(format!("omalook-plus-{}", std::process::id()));
        let plus = path.with_file_name(format!("{}_plus", path.file_name().unwrap().to_string_lossy()));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&plus);
        assert!(!MsRefresher::with_marker(path.clone()).plus_unavailable.load(Ordering::Relaxed));
        std::fs::write(&plus, b"plus").unwrap();
        let r = MsRefresher::with_marker(path.clone());
        assert!(r.plus_unavailable.load(Ordering::Relaxed), "next launch skips the doomed top-tier call");
        assert!(!r.full_scope_unavailable.load(Ordering::Relaxed), "and still tries the full tier");
        r.reset();
        assert!(!r.plus_unavailable.load(Ordering::Relaxed) && !plus.exists());
    }

    #[test]
    fn an_unlisted_scope_counts_as_missing_consent() {
        let body = r#"{"error":"invalid_scope","error_description":"AADSTS70011: The provided request must include a 'scope' input parameter"}"#;
        let (_, missing) = classify_refresh_error(400, body);
        assert!(missing);
    }
}
