//! Google (Gmail) sign-in: OAuth 2.0 "installed app" flow.
//!
//! The user types their address, the system browser opens Google's own sign-in page,
//! Google redirects back to a one-shot listener on `127.0.0.1:<random port>`, and the
//! code is exchanged (PKCE) for tokens that the account's `TokenBroker` stores in the
//! keyring. Nothing has to be enabled in the Google account itself.
//!
//! Google's device-code flow is not usable here: it refuses Gmail/Calendar scopes.
//!
//! The OAuth client (a "Desktop app" client created once by the developer in Google
//! Cloud Console, like the Azure registration for Graph) is compiled into the binary by
//! build.rs, so end users configure nothing. `<config dir>/google_client.json` (Google's
//! "Download JSON" file works as-is) overrides it for developers using their own project.
//! Desktop clients carry a secret that Google documents as non-confidential; it is still
//! kept out of the git history (build.rs reads it from a git-ignored file or env vars).

use crate::errors::{OmarchyError, Result};
use crate::models::TokenResponse;
use crate::token_store::{RefreshOutcome, Refresher};
use log::{debug, info, warn};
use oauth2::{CsrfToken, PkceCodeChallenge};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const USERINFO_URL: &str = "https://openidconnect.googleapis.com/v1/userinfo";

/// Everything the planned mail / calendar / people / tasks sync needs, requested once up
/// front so adding a feature later never forces a second consent screen (the Graph side
/// learned that the hard way). `gmail.modify` covers read, label, trash and send.
pub const SCOPES: &[&str] = &[
    "openid",
    "email",
    "https://www.googleapis.com/auth/gmail.modify",
    "https://www.googleapis.com/auth/calendar",
    "https://www.googleapis.com/auth/contacts",
    "https://www.googleapis.com/auth/tasks",
];

/// How long the browser sign-in may take before the attempt is abandoned.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

/// File the QML watches for the sign-in URL (fallback if the browser did not open).
pub const LOGIN_FILE: &str = "google_login.json";

// ───────────────────────────────────────────────────────────── client config

#[derive(Debug, Clone, PartialEq)]
pub struct GoogleClient {
    pub client_id: String,
    pub client_secret: String,
}

pub fn default_config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg).join("omalook")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config").join("omalook")
    } else {
        PathBuf::from("/tmp/omalook")
    }
}

/// Parse Google's downloaded client JSON (`{"installed": {...}}`, `{"web": {...}}`) or a flat
/// `{"client_id": ..., "client_secret": ...}` object.
pub fn parse_client_json(raw: &str) -> Result<GoogleClient> {
    let v: serde_json::Value = serde_json::from_str(raw)?;
    let obj = v.get("installed").or_else(|| v.get("web")).unwrap_or(&v);
    let get = |k: &str| obj[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    match (get("client_id"), get("client_secret")) {
        (Some(client_id), Some(client_secret)) => Ok(GoogleClient { client_id, client_secret }),
        _ => Err(OmarchyError::AuthError("google_client.json has no client_id / client_secret".into())),
    }
}

impl GoogleClient {
    /// The client compiled into this binary (see build.rs), if the build had one.
    pub fn embedded() -> Option<Self> {
        match (option_env!("OMALOOK_GOOGLE_CLIENT_ID"), option_env!("OMALOOK_GOOGLE_CLIENT_SECRET")) {
            (Some(id), Some(secret)) if !id.is_empty() && !secret.is_empty() => {
                Some(Self { client_id: id.to_string(), client_secret: secret.to_string() })
            }
            _ => None,
        }
    }

    /// `<config dir>/google_client.json` overrides the embedded client (developers testing their
    /// own project); otherwise the embedded one is used, so end users need no setup.
    pub fn load(config_dir: &Path) -> Result<Self> {
        let path = config_dir.join("google_client.json");
        match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|r| parse_client_json(&r).map_err(|e| e.to_string())) {
            Ok(c) => Ok(c),
            Err(why) => Self::embedded().ok_or_else(|| {
                OmarchyError::AuthError(format!(
                    "Google sign-in isn't set up (this build has no embedded client and {} is unusable: {})",
                    path.display(), why
                ))
            }),
        }
    }
}

// ───────────────────────────────────────────────────────── pure helpers

pub fn looks_like_email(s: &str) -> bool {
    let s = s.trim();
    match s.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.') && !s.contains(' ')
        }
        None => false,
    }
}

pub fn build_auth_url(client: &GoogleClient, redirect_uri: &str, state: &str, challenge: &str, login_hint: Option<&str>) -> String {
    let scope = SCOPES.join(" ");
    let mut params: Vec<(&str, &str)> = vec![
        ("client_id", &client.client_id),
        ("redirect_uri", redirect_uri),
        ("response_type", "code"),
        ("scope", &scope),
        ("state", state),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        // Offline access + forced consent: Google only returns a refresh token on consent, and
        // without one the account would be signed out within the hour.
        ("access_type", "offline"),
        ("prompt", "consent"),
    ];
    if let Some(h) = login_hint {
        params.push(("login_hint", h));
    }
    url::Url::parse_with_params(AUTH_URL, &params).expect("static URL").to_string()
}

/// Result of inspecting one HTTP request that reached the loopback listener.
#[derive(Debug, PartialEq)]
pub enum Callback {
    Code(String),
    /// Google reported an error (`access_denied`, …) or the state did not match.
    Failed(String),
    /// Not the redirect (favicon, port scanners …): ignore and keep listening.
    Unrelated,
}

pub fn parse_callback(request: &str, expected_state: &str) -> Callback {
    let first = request.lines().next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if method != "GET" || !target.starts_with("/?") {
        return Callback::Unrelated;
    }
    let Ok(u) = url::Url::parse(&format!("http://127.0.0.1{}", target)) else { return Callback::Unrelated };
    let q: std::collections::HashMap<_, _> = u.query_pairs().collect();
    if q.get("state").map(|s| s.as_ref()) != Some(expected_state) {
        return Callback::Failed("Sign-in response did not match this request (state mismatch)".into());
    }
    if let Some(err) = q.get("error") {
        return Callback::Failed(match err.as_ref() {
            "access_denied" => "Google sign-in was cancelled or access was denied".to_string(),
            other => format!("Google sign-in failed: {}", other),
        });
    }
    match q.get("code") {
        Some(c) if !c.is_empty() => Callback::Code(c.to_string()),
        _ => Callback::Failed("Google returned no authorisation code".into()),
    }
}

// ───────────────────────────────────────────────────── login (browser + loopback)

/// Bumped by every new login and by `cancel_login`; a waiting login whose generation is no
/// longer current gives up, which also frees its listener port.
static LOGIN_GEN: AtomicU64 = AtomicU64::new(0);

pub fn cancel_login(config_dir: &Path) {
    LOGIN_GEN.fetch_add(1, Ordering::SeqCst);
    let _ = std::fs::remove_file(config_dir.join(LOGIN_FILE));
}

pub struct PendingLogin {
    client: GoogleClient,
    listener: TcpListener,
    redirect_uri: String,
    state: String,
    verifier: String,
    generation: u64,
    pub auth_url: String,
}

pub struct GoogleLogin {
    pub tokens: TokenResponse,
    pub email: String,
}

/// Bind the loopback listener, build the sign-in URL, publish it for the UI and open the
/// browser. Returns immediately; call `finish` (blocking) to wait for the result.
pub fn start_login(config_dir: &Path, login_hint: Option<&str>) -> Result<PendingLogin> {
    let client = GoogleClient::load(config_dir)?;
    let generation = LOGIN_GEN.fetch_add(1, Ordering::SeqCst) + 1; // supersedes any login still waiting
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let redirect_uri = format!("http://127.0.0.1:{}", listener.local_addr()?.port());

    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let state = CsrfToken::new_random().secret().clone();
    let auth_url = build_auth_url(&client, &redirect_uri, &state, challenge.as_str(), login_hint);

    let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let info = serde_json::json!({
        "auth_url": auth_url,
        "expires_in": LOGIN_TIMEOUT.as_secs(),
        "started": started,
    });
    std::fs::write(config_dir.join(LOGIN_FILE), serde_json::to_string_pretty(&info)?)?;

    match std::process::Command::new("xdg-open").arg(&auth_url).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn() {
        Ok(mut child) => { std::thread::spawn(move || { let _ = child.wait(); }); }
        Err(e) => warn!("Could not launch a browser ({}); the sign-in URL is shown in the app", e),
    }
    info!("Google sign-in waiting on {}", redirect_uri);

    Ok(PendingLogin { client, listener, redirect_uri, state, verifier: verifier.secret().clone(), generation, auth_url })
}

const SUCCESS_PAGE: &str = "<!doctype html><meta charset=utf-8><title>omalook</title>\
<body style=\"font-family:sans-serif;background:#000;color:#ddd;text-align:center;padding-top:20vh\">\
<h2>Signed in</h2><p>You can close this tab and return to omalook.</p></body>";
const FAILURE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>omalook</title>\
<body style=\"font-family:sans-serif;background:#000;color:#ddd;text-align:center;padding-top:20vh\">\
<h2>Sign-in failed</h2><p>Return to omalook for details.</p></body>";

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status, body.len(), body
    );
}

impl PendingLogin {
    fn wait_for_code(&self, timeout: Duration) -> Result<String> {
        let deadline = Instant::now() + timeout;
        loop {
            if LOGIN_GEN.load(Ordering::SeqCst) != self.generation {
                return Err(OmarchyError::AuthError("Sign-in cancelled".into()));
            }
            if Instant::now() > deadline {
                return Err(OmarchyError::AuthError("Timed out waiting for Google sign-in".into()));
            }
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
                    let mut buf = [0u8; 4096];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    match parse_callback(&String::from_utf8_lossy(&buf[..n]), &self.state) {
                        Callback::Code(c) => { respond(&mut stream, "200 OK", SUCCESS_PAGE); return Ok(c); }
                        Callback::Failed(m) => { respond(&mut stream, "400 Bad Request", FAILURE_PAGE); return Err(OmarchyError::AuthError(m)); }
                        Callback::Unrelated => respond(&mut stream, "404 Not Found", ""),
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(150)),
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Block until the browser sign-in completes, then exchange the code and look up the
    /// address Google actually signed in (which may differ from the hint).
    pub fn finish(self, config_dir: &Path) -> Result<GoogleLogin> {
        let result = (|| {
            let code = self.wait_for_code(LOGIN_TIMEOUT)?;
            debug!("Google authorisation code received; exchanging");
            let tokens = exchange_code(&self.client, &code, &self.verifier, &self.redirect_uri)?;
            if tokens.refresh_token.as_deref().unwrap_or("").is_empty() {
                return Err(OmarchyError::AuthError("Google did not issue a refresh token; please try again".into()));
            }
            let email = fetch_email(&tokens.access_token)?;
            Ok(GoogleLogin { tokens, email })
        })();
        if LOGIN_GEN.load(Ordering::SeqCst) == self.generation {
            let _ = std::fs::remove_file(config_dir.join(LOGIN_FILE)); // a newer login owns the file otherwise
        }
        result
    }
}

// ─────────────────────────────────────────────────────────────── token calls

fn token_post(params: &[(&str, &str)]) -> std::result::Result<TokenResponse, (u16, String)> {
    match ureq::post(TOKEN_URL).send_form(params) {
        Ok(r) => r.into_json::<TokenResponse>().map_err(|e| (0, format!("bad token response: {}", e))),
        Err(ureq::Error::Status(code, r)) => Err((code, r.into_string().unwrap_or_default())),
        Err(e) => Err((0, e.to_string())),
    }
}

pub fn exchange_code(client: &GoogleClient, code: &str, verifier: &str, redirect_uri: &str) -> Result<TokenResponse> {
    token_post(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("code_verifier", verifier),
        ("redirect_uri", redirect_uri),
        ("client_id", &client.client_id),
        ("client_secret", &client.client_secret),
    ])
    .map_err(|(code, body)| OmarchyError::AuthError(format!("Google token exchange failed ({}): {}", code, body)))
}

pub fn fetch_email(access_token: &str) -> Result<String> {
    let v: serde_json::Value = ureq::get(USERINFO_URL)
        .set("Authorization", &format!("Bearer {}", access_token))
        .call()
        .map_err(|e| OmarchyError::HttpError(format!("Google userinfo: {}", e)))?
        .into_json()?;
    v["email"].as_str().filter(|s| !s.is_empty()).map(String::from)
        .ok_or_else(|| OmarchyError::AuthError("Google did not report an email address".into()))
}

/// Classify a token-endpoint failure. Only `invalid_grant` (revoked, expired, or the 7-day
/// limit of apps still in "Testing") means the stored login is dead; anything else keeps it.
pub fn classify_refresh_error(code: u16, body: &str) -> RefreshOutcome {
    let err = serde_json::from_str::<serde_json::Value>(body).ok()
        .and_then(|v| v["error"].as_str().map(String::from)).unwrap_or_default();
    match err.as_str() {
        "invalid_grant" | "unauthorized_client" | "invalid_client" => RefreshOutcome::InvalidGrant(format!("{}: {}", code, err)),
        _ => RefreshOutcome::Transient(format!("HTTP {}: {}", code, body)),
    }
}

/// Refreshes Gmail-account tokens for the shared `TokenBroker`.
pub struct GoogleRefresher {
    config_dir: PathBuf,
}

impl GoogleRefresher {
    pub fn new() -> Self {
        Self { config_dir: default_config_dir() }
    }
}

impl Refresher for GoogleRefresher {
    fn refresh(&self, refresh_token: &str) -> RefreshOutcome {
        let client = match GoogleClient::load(&self.config_dir) {
            Ok(c) => c,
            Err(e) => return RefreshOutcome::Transient(e.to_string()), // keep the tokens; config may come back
        };
        match token_post(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &client.client_id),
            ("client_secret", &client.client_secret),
        ]) {
            Ok(t) => RefreshOutcome::Ok(t),
            Err((0, msg)) => RefreshOutcome::Transient(msg),
            Err((code, body)) => classify_refresh_error(code, &body),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> GoogleClient {
        GoogleClient { client_id: "cid.apps.googleusercontent.com".into(), client_secret: "sec".into() }
    }

    #[test]
    fn client_json_accepts_googles_download_and_flat_shapes() {
        let dl = r#"{"installed":{"client_id":"a","client_secret":"b","redirect_uris":["http://localhost"]}}"#;
        assert_eq!(parse_client_json(dl).unwrap(), GoogleClient { client_id: "a".into(), client_secret: "b".into() });
        assert_eq!(parse_client_json(r#"{"client_id":"a","client_secret":"b"}"#).unwrap().client_id, "a");
        assert!(parse_client_json(r#"{"installed":{"client_id":"a"}}"#).is_err());
        assert!(parse_client_json("not json").is_err());
    }

    #[test]
    fn load_prefers_the_file_then_the_embedded_client() {
        let dir = std::env::temp_dir().join(format!("omalook-gclient-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(dir.join("google_client.json"));
        // no file: embedded client if this build has one, else a clear error
        match GoogleClient::embedded() {
            Some(e) => assert_eq!(GoogleClient::load(&dir).unwrap(), e),
            None => assert!(GoogleClient::load(&dir).is_err()),
        }
        std::fs::write(dir.join("google_client.json"), r#"{"installed":{"client_id":"file-id","client_secret":"file-secret"}}"#).unwrap();
        assert_eq!(GoogleClient::load(&dir).unwrap().client_id, "file-id", "file overrides embedded");
        std::fs::write(dir.join("google_client.json"), "garbage").unwrap();
        assert_eq!(GoogleClient::load(&dir).ok(), GoogleClient::embedded(), "bad file falls back to embedded");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auth_url_carries_pkce_offline_access_scopes_and_hint() {
        let u = url::Url::parse(&build_auth_url(&client(), "http://127.0.0.1:5555", "st", "chal", Some("me@gmail.com"))).unwrap();
        let q: std::collections::HashMap<_, _> = u.query_pairs().collect();
        assert_eq!(u.host_str(), Some("accounts.google.com"));
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["code_challenge"], "chal");
        assert_eq!(q["access_type"], "offline");
        assert_eq!(q["prompt"], "consent");
        assert_eq!(q["login_hint"], "me@gmail.com");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:5555");
        for s in SCOPES { assert!(q["scope"].split(' ').any(|x| x == *s), "{}", s); }
        assert!(!build_auth_url(&client(), "http://127.0.0.1:1", "s", "c", None).contains("login_hint"));
    }

    #[test]
    fn callback_parsing() {
        let ok = "GET /?state=abc&code=4%2F0AX&scope=x HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        assert_eq!(parse_callback(ok, "abc"), Callback::Code("4/0AX".into()));
        assert!(matches!(parse_callback(ok, "other"), Callback::Failed(m) if m.contains("state")));
        let denied = "GET /?state=abc&error=access_denied HTTP/1.1\r\n\r\n";
        assert!(matches!(parse_callback(denied, "abc"), Callback::Failed(m) if m.contains("cancelled")));
        assert_eq!(parse_callback("GET /favicon.ico HTTP/1.1\r\n\r\n", "abc"), Callback::Unrelated);
        assert_eq!(parse_callback("POST /?code=1&state=abc HTTP/1.1\r\n\r\n", "abc"), Callback::Unrelated);
        assert!(matches!(parse_callback("GET /?state=abc HTTP/1.1\r\n\r\n", "abc"), Callback::Failed(_)));
    }

    #[test]
    fn email_shape_check() {
        assert!(looks_like_email("adam@gmail.com"));
        assert!(looks_like_email(" adam@example.co.uk "));
        for bad in ["", "adam", "adam@", "@gmail.com", "adam@gmail", "a b@gmail.com", "adam@.com"] {
            assert!(!looks_like_email(bad), "{}", bad);
        }
    }

    #[test]
    fn only_a_dead_grant_wipes_tokens() {
        assert!(matches!(classify_refresh_error(400, r#"{"error":"invalid_grant"}"#), RefreshOutcome::InvalidGrant(_)));
        assert!(matches!(classify_refresh_error(503, "oops"), RefreshOutcome::Transient(_)));
        assert!(matches!(classify_refresh_error(400, r#"{"error":"temporarily_unavailable"}"#), RefreshOutcome::Transient(_)));
    }

    /// End to end against a real loopback listener (no Google): the redirect is delivered by a
    /// plain TCP client, as the browser would.
    #[test]
    fn listener_returns_the_code_and_survives_stray_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let gen = LOGIN_GEN.fetch_add(1, Ordering::SeqCst) + 1;
        let p = PendingLogin {
            client: client(), listener, redirect_uri: format!("http://127.0.0.1:{}", port),
            state: "st8".into(), verifier: "v".into(), generation: gen, auth_url: String::new(),
        };
        let t = std::thread::spawn(move || p.wait_for_code(Duration::from_secs(10)));
        let get = |path: &str| {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(s, "GET {} HTTP/1.1\r\nHost: x\r\n\r\n", path).unwrap();
            let mut out = String::new();
            let _ = s.read_to_string(&mut out);
            out
        };
        assert!(get("/favicon.ico").starts_with("HTTP/1.1 404"));
        assert!(get("/?state=st8&code=THE_CODE").starts_with("HTTP/1.1 200"));
        assert_eq!(t.join().unwrap().unwrap(), "THE_CODE");
    }
}
