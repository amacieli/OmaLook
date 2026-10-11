//! Main application entry point (Rust + QML)
//!
//! OmarchyLook: Lightweight Outlook clone
//! - Rust backend: Auth, Graph API, SQLite cache
//! - QML frontend: Native Qt UI with hot-reload support

use omarchylook::{init_logging, AuthManager, Database, SettingsManager, email_daemon::{EmailDaemon, DaemonConfig}, providers::graph::GraphEmailProvider};
use log::{debug, error, info, warn};
use std::env;
use std::path::PathBuf;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use tokio::runtime::Runtime;

/// Process-wide sync scheduler (one mail + one calendar thread per signed-in account).
static SCHEDULER: std::sync::OnceLock<Arc<omarchylook::scheduler::SyncScheduler>> = std::sync::OnceLock::new();

/// Value of `?key=` in a request line (percent-decoded).
fn query_param(first_line: &str, key: &str) -> Option<String> {
    let needle = format!("{}=", key);
    first_line.split_once('?').and_then(|(_, q)| {
        q.split(|c| c == ' ' || c == '\r' || c == '\n').next().and_then(|q| {
            q.split('&').find_map(|kv| kv.strip_prefix(needle.as_str()).map(url_decode))
        })
    }).filter(|v| !v.is_empty())
}

/// Percent-decode a URL query parameter value (e.g. %3D → =, %2F → /)
fn url_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i+1..i+3]) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte as char);
                    i += 3;
                    continue;
                }
            }
        } else if bytes[i] == b'+' {
            out.push(' ');
            i += 1;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn main() {
    // Initialize logging
    init_logging();
    let config_dir = get_config_dir();
    omarchylook::perf::init(&config_dir);
    info!("OmarchyLook starting");
    
    let db_path = config_dir.join("omarchy.db");
    let settings_path = config_dir.join("settings.toml");
    
    info!("Config dir: {}", config_dir.display());
    info!("Database: {}", db_path.display());
    
    // Initialize core backend components
    match initialize_app(&config_dir, db_path.to_str().unwrap(), settings_path.to_str().unwrap()) {
        Ok(_) => {
            info!("Application backend initialized successfully");
            
            // Launch Qt/QML runtime
            launch_qml_app(&config_dir);
        }
        Err(e) => {
            error!("Failed to initialize application: {}", e);
            std::process::exit(1);
        }
    }
}

fn get_config_dir() -> PathBuf {
    // Use XDG standard: ~/.config/omarchylook
    if let Ok(xdg_config) = env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg_config).join("omarchylook")
    } else if let Ok(home) = env::var("HOME") {
        PathBuf::from(home).join(".config").join("omarchylook")
    } else {
        PathBuf::from("/tmp/omarchylook")
    }
}

/// Directory holding the Omarchy shell UI kit (`Commons/`, `Ui/`).
const OMARCHY_KIT_DIR: &str = "/usr/share/omarchy/shell";

/// Locate the QML app directory: `QML_DIR` env, then `./qml`, then the
/// `qml/` next to the sources this binary was built from (so it also works
/// when launched from the app menu with an arbitrary cwd).
fn resolve_qml_dir() -> PathBuf {
    if let Ok(dir) = env::var("QML_DIR") {
        return PathBuf::from(dir);
    }
    let cwd_qml = PathBuf::from("qml");
    if cwd_qml.join("shell.qml").exists() {
        return cwd_qml;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("qml")
}

/// The UI is built on the Omarchy shell kit (`qs.Commons`, `qs.Ui`). Quickshell
/// resolves `qs.*` relative to the config root, so link the installed kit into
/// the QML dir (idempotent; the links are git-ignored).
fn ensure_omarchy_kit(qml_dir: &PathBuf) -> std::io::Result<()> {
    use std::os::unix::fs::symlink;
    for name in ["Commons", "Ui"] {
        let target = PathBuf::from(OMARCHY_KIT_DIR).join(name);
        if !target.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} not found - OmarchyLook needs the Omarchy shell UI kit", target.display()),
            ));
        }
        let link = qml_dir.join(name);
        if link.symlink_metadata().is_err() {
            symlink(&target, &link)?;
            debug!("Linked {} -> {}", link.display(), target.display());
        }
    }
    Ok(())
}

/// Launch the QML application window (Quickshell used as a standalone Qt Quick host).
fn launch_qml_app(config_dir: &PathBuf) {
    use std::process::{Command, Stdio};

    info!("Launching Quickshell UI...");

    let qml_dir = resolve_qml_dir();
    info!("QML directory: {}", qml_dir.display());

    let shell_qml = qml_dir.join("shell.qml");
    if !shell_qml.exists() {
        eprintln!("❌ QML entry point not found: {}", shell_qml.display());
        std::process::exit(1);
    }

    if let Err(e) = ensure_omarchy_kit(&qml_dir) {
        eprintln!("❌ {}", e);
        eprintln!("   Install/upgrade Omarchy (pacman -Q omarchy) so {} exists", OMARCHY_KIT_DIR);
        std::process::exit(1);
    }

    // `quickshell -p <dir>` runs <dir>/shell.qml as its own instance, independent
    // of the running omarchy-shell. CONFIG_DIR tells the UI where the backend
    // writes auth_state.json / device_code.json.
    omarchylook::perf::mark("spawning quickshell");
    let mut cmd = Command::new("quickshell");
    // Native QML plugins (OmarchyLook.Compose): the dev build next to the source tree, then the
    // installed location. Existing QML_IMPORT_PATH entries are kept.
    {
        let mut paths: Vec<std::path::PathBuf> = Vec::new();
        if let Some(root) = std::path::Path::new(&qml_dir).parent() {
            paths.push(root.join("plugin/omarchylook-compose/build/qml"));
        }
        paths.push(std::path::PathBuf::from("/usr/lib/omarchylook/qml"));
        let mut joined: Vec<String> = paths.iter().filter(|p| p.exists()).map(|p| p.display().to_string()).collect();
        if let Ok(existing) = std::env::var("QML_IMPORT_PATH") { if !existing.is_empty() { joined.push(existing); } }
        if !joined.is_empty() { cmd.env("QML_IMPORT_PATH", joined.join(":")); }
    }
    cmd.arg("-p").arg(&qml_dir)
        .env("QML_DIR", &qml_dir)
        .env("CONFIG_DIR", config_dir.to_str().unwrap())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    match cmd.status() {
        Ok(status) => {
            if !status.success() {
                eprintln!("⚠️  Quickshell exited with status: {}", status);
            }
            info!("UI window closed");
        }
        Err(e) => {
            eprintln!("❌ Failed to launch quickshell: {}", e);
            eprintln!("   Install it with: pacman -S quickshell");
            std::process::exit(1);
        }
    }
}

fn initialize_app(config_dir: &PathBuf, db_path: &str, settings_path: &str) -> omarchylook::errors::Result<()> {
    // Initialize database
    let db = {
        let _t = omarchylook::perf::span("Database::open (schema init)");
        Arc::new(Database::open(db_path)?)
    };
    info!("Database initialized");
    
    // Initialize settings
    let _settings = SettingsManager::open(settings_path)?;
    info!("Settings initialized");
    
    // Initialize auth (check if already authenticated, but don't auto-prompt)
    let _auth_span = omarchylook::perf::span("AuthManager::new + keyring check");
    let auth = AuthManager::new();
    let authed = auth.is_authenticated();
    drop(_auth_span);
    if authed {
        info!("User already authenticated (cached tokens available)");
        auth.write_state_file(config_dir)?;
        
        // Note: Email daemon will start in the background thread below
        // It will self-trigger on token availability
    } else {
        info!("User not authenticated; waiting for UI trigger");
    }
    
    // Start background thread to watch for device flow trigger file
    let config_dir_clone = config_dir.clone();
    std::thread::spawn(move || {
        watch_for_device_flow_trigger(&config_dir_clone);
    });

    // Start HTTP trigger server (QML posts to this to trigger auth/logout)
    let config_dir_http = config_dir.clone();
    std::thread::spawn(move || {
        start_http_trigger_server(&config_dir_http);
    });
    
    // Per-account sync (mail + calendar threads for every signed-in account)
    let scheduler = Arc::new(omarchylook::scheduler::SyncScheduler::new(config_dir));
    let _ = SCHEDULER.set(Arc::clone(&scheduler));
    std::thread::Builder::new().name("scheduler".into()).spawn(move || scheduler.start_all()).ok();
    
    Ok(())
}

/// Watch for device flow and logout trigger files
fn watch_for_device_flow_trigger(config_dir: &PathBuf) {
    use std::fs;
    use std::thread;
    use std::time::Duration;
    
    let trigger_file = "/tmp/omarchylook-trigger-device-flow";
    let logout_trigger_file = "/tmp/omarchylook-trigger-logout";
    
    loop {
        thread::sleep(Duration::from_millis(500));
        
        // Legacy trigger files go through the same account code as the HTTP routes, so a
        // login creates a normal <provider>-<suffix> account and starts its sync.
        // login:  file content = provider (default "exchange")
        // logout: file content = account id (empty = sign out every account)
        if fs::metadata(trigger_file).is_ok() {
            let provider = fs::read_to_string(trigger_file).unwrap_or_default().trim().to_string();
            let provider = if provider.is_empty() { "exchange".to_string() } else { provider };
            let _ = fs::remove_file(trigger_file);
            info!("🔔 Trigger file: add {} account", provider);
            match SCHEDULER.get() {
                Some(scheduler) => {
                    if let Err(e) = omarchylook::account_ops::begin_add_account(config_dir, &provider, Arc::clone(scheduler)) {
                        error!("❌ Add account failed: {}", e);
                    }
                }
                None => warn!("Trigger file ignored: scheduler not ready yet"),
            }
        }

        if fs::metadata(logout_trigger_file).is_ok() {
            let id = fs::read_to_string(logout_trigger_file).unwrap_or_default().trim().to_string();
            let _ = fs::remove_file(logout_trigger_file);
            info!("🔔 Trigger file: log out {}", if id.is_empty() { "all accounts" } else { &id });
            match SCHEDULER.get() {
                Some(scheduler) => {
                    let result = if id.is_empty() {
                        omarchylook::account_ops::sign_out_all(config_dir, scheduler).map(|n| info!("✅ Signed out {} account(s)", n))
                    } else {
                        omarchylook::account_ops::sign_out_account(config_dir, &id, scheduler)
                    };
                    if let Err(e) = result {
                        error!("❌ Logout failed: {}", e);
                    }
                }
                None => warn!("Trigger file ignored: scheduler not ready yet"),
            }
        }
    }
}

/// JSON for GET /messages/invite: `{"type":"meetingRequest"|...,"actionable":bool}`, or
/// `{"error":"..."}`. The type is cached per message, so only the first look hits Graph.
fn invite_info_json(config_dir: &PathBuf, id: &str) -> String {
    let err = |m: &str| format!("{{\"error\":{}}}", serde_json::to_string(m).unwrap());
    if id.is_empty() {
        return err("missing id");
    }
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let kind = match db.invite_type(id) {
        Ok(Some(k)) => k,
        _ => {
            let account = match db.message_account(id) {
                Ok(Some(a)) => a,
                _ => return err("unknown message"),
            };
            if omarchylook::accounts::provider_of(&account) != "graph" {
                return format!("{{\"type\":\"none\",\"actionable\":false}}");
            }
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => return err(&format!("runtime: {}", e)),
            };
            match rt.block_on(GraphEmailProvider::new(AuthManager::for_account(&account)).fetch_meeting_type(id)) {
                Ok(k) => {
                    let k = if k.is_empty() { "none".to_string() } else { k };
                    if let Err(e) = db.set_invite_type(id, &k) {
                        warn!("Could not cache invite type of {}: {}", id, e);
                    }
                    k
                }
                Err(e) => {
                    warn!("Invite lookup failed for {}: {}", id, e);
                    return err("could not read this message from the server");
                }
            }
        }
    };
    format!("{{\"type\":{},\"actionable\":{}}}",
        serde_json::to_string(&kind).unwrap(),
        omarchylook::invites::is_actionable(&kind))
}

/// Answer an invitation (choice = accept | tentative | decline) on Graph, record the new state
/// and, for accept/tentative, re-expand the calendar window so the event shows at once.
/// Returns JSON `{"ok":true,"type":...}` or `{"error":...}`. A 403 means the account has not
/// granted Calendars.ReadWrite yet, so the caller can explain it instead of failing silently.
fn respond_invite_json(config_dir: &PathBuf, id: &str, choice: &str) -> String {
    let err = |m: &str| format!("{{\"error\":{}}}", serde_json::to_string(m).unwrap());
    let Some((op, new_type)) = omarchylook::invites::response_op(choice) else {
        return err("unknown response");
    };
    if id.is_empty() {
        return err("missing id");
    }
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let account = match db.message_account(id) {
        Ok(Some(a)) => a,
        _ => return err("unknown message"),
    };
    if omarchylook::accounts::provider_of(&account) != "graph" {
        return err("responding to invitations is only supported for Microsoft accounts");
    }
    match db.invite_type(id) {
        Ok(Some(k)) if !omarchylook::invites::is_actionable(&k) => {
            return err("this message is not an open invitation");
        }
        _ => {}
    }
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => return err(&format!("runtime: {}", e)),
    };
    let graph = GraphEmailProvider::new(AuthManager::for_account(&account));
    if let Err(e) = rt.block_on(graph.respond_to_invite(id, op)) {
        warn!("Respond {} to {} failed: {}", op, id, e);
        let msg = e.to_string();
        return if msg.contains("403") {
            err("not allowed: grant Calendars.ReadWrite to this account (sign in again after the Azure consent)")
        } else {
            err("the server rejected the response")
        };
    }
    let _ = db.set_invite_type(id, new_type);
    if omarchylook::invites::adds_to_calendar(choice) {
        let provider = Arc::new(omarchylook::providers::GraphCalendarProvider::new(AuthManager::for_account(&account)));
        let cal_db = match Database::open_for_account(config_dir.join("messages.db").to_str().unwrap_or("messages.db"), &account) {
            Ok(d) => Arc::new(d),
            Err(e) => return format!("{{\"ok\":true,\"type\":{},\"calendar\":{}}}", serde_json::to_string(new_type).unwrap(), serde_json::to_string(&format!("not refreshed: {}", e)).unwrap()),
        };
        let cal = omarchylook::calendar_daemon::CalendarDaemon::new(Default::default(), cal_db, provider);
        let note = match rt.block_on(cal.refresh_near_window()) {
            Ok(_) => "refreshed".to_string(),
            Err(e) => { warn!("Calendar refresh after RSVP failed: {}", e); format!("not refreshed: {}", e) }
        };
        return format!("{{\"ok\":true,\"type\":{},\"calendar\":{}}}", serde_json::to_string(new_type).unwrap(), serde_json::to_string(&note).unwrap());
    }
    format!("{{\"ok\":true,\"type\":{}}}", serde_json::to_string(new_type).unwrap())
}

/// JSON for GET /messages/body: `{"type":"html"|"text","content":"..."}`, or
/// `{"error":"..."}` when the body cannot be had.
fn message_body_json(config_dir: &PathBuf, id: &str) -> String {
    let err = |m: &str| format!("{{\"error\":{}}}", serde_json::to_string(m).unwrap());
    if id.is_empty() {
        return err("missing id");
    }
    let path = config_dir.join("messages.db");
    let db = match Database::open(path.to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let ok = |t: &str, c: &str| format!("{{\"type\":\"{}\",\"content\":{}}}", t, serde_json::to_string(c).unwrap());
    if let Ok(Some((t, c))) = db.cached_body(id) {
        return ok(&t, &c);
    }
    let account = match db.message_account(id) {
        Ok(Some(a)) => a,
        _ => return err("unknown message"),
    };
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => return err(&format!("runtime: {}", e)),
    };
    let fetched = if omarchylook::accounts::provider_of(&account) == "gmail" {
        rt.block_on(omarchylook::providers::GmailProvider::new(&account, None).fetch_message_body(id))
    } else {
        rt.block_on(GraphEmailProvider::new(AuthManager::for_account(&account)).fetch_message_body(id))
    };
    match fetched {
        Ok((t, c)) => {
            if let Err(e) = db.store_body(id, &t, &c) {
                warn!("Could not cache body of {}: {}", id, e);
            }
            ok(&t, &c)
        }
        Err(e) => {
            warn!("Body fetch failed for {}: {}", id, e);
            // The cached preview still lets the pane show something.
            let reason = if e.to_string().contains("404") {
                "this message no longer exists on the server"
            } else {
                "could not fetch the message body"
            };
            format!(
                "{{\"error\":{},\"preview\":{}}}",
                serde_json::to_string(reason).unwrap(),
                serde_json::to_string(&db.message_preview(id).unwrap_or_default()).unwrap()
            )
        }
    }
}

/// Read the body of a POST whose headers (and maybe part of the body) are already in `initial`.
/// The server's first read is only 1 KiB, which is enough for every route that carries its data
/// in the query string; the compose route carries a whole message.
fn read_post_body(stream: &mut std::net::TcpStream, initial: &[u8], limit: usize) -> Result<String, String> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok();
    let mut data = initial.to_vec();
    let find = |d: &[u8]| d.windows(4).position(|w| w == b"\r\n\r\n");
    let header_end = loop {
        if let Some(i) = find(&data) { break i + 4; }
        if data.len() > 64 * 1024 { return Err("request headers too large".into()); }
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk).map_err(|e| format!("read failed: {}", e))?;
        if n == 0 { return Err("connection closed before the request was complete".into()); }
        data.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&data[..header_end]).to_ascii_lowercase();
    let len = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .ok_or_else(|| "missing Content-Length".to_string())?;
    if len > limit { return Err("message too large".into()); }
    while data.len() < header_end + len {
        let mut chunk = [0u8; 16 * 1024];
        let n = stream.read(&mut chunk).map_err(|e| format!("read failed: {}", e))?;
        if n == 0 { return Err("connection closed before the message was complete".into()); }
        data.extend_from_slice(&chunk[..n]);
    }
    String::from_utf8(data[header_end..header_end + len].to_vec()).map_err(|_| "message is not valid UTF-8".into())
}

/// POST /compose/send — queue a composed message. The body is the JSON the compose pane builds
/// (`omarchylook::compose::OutgoingMessage`). It is stored in the outbox to go out after the
/// `[mail] send_delay_secs` setting (0 = now) and the account's outbox worker is woken, so the
/// send never waits for a poll. Answers `{ok, id, delay_secs}` or `{ok:false, error}`.
fn compose_send_response(config_dir: &PathBuf, stream: &mut std::net::TcpStream, initial: &[u8]) -> String {
    use omarchylook::compose::{OutgoingMessage, MAX_BODY_BYTES};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();

    let raw = match read_post_body(stream, initial, MAX_BODY_BYTES + 64 * 1024) {
        Ok(b) => b,
        Err(e) => return err(&e),
    };
    let msg: OutgoingMessage = match serde_json::from_str(&raw) {
        Ok(m) => m,
        Err(e) => return err(&format!("unreadable message: {}", e)),
    };
    if let Err(e) = msg.validate() {
        return err(&e);
    }
    // A signed-out account has no worker, so the message would sit unsent without anyone knowing.
    if !SCHEDULER.get().map(|s| s.is_running(&msg.account_id)).unwrap_or(false) {
        return err("that account is not signed in, so it cannot send");
    }
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let delay = omarchylook::settings::read_send_delay(&config_dir.join("settings.toml"));
    let now = Database::now_ms();
    let id = format!("ob-{:x}-{:03x}", now, COUNTER.fetch_add(1, Ordering::Relaxed) & 0xfff);
    let payload = match serde_json::to_string(&msg) {
        Ok(p) => p,
        Err(e) => return err(&e.to_string()),
    };
    if let Err(e) = db.outbox_enqueue(&id, &msg.account_id, &payload, now + delay as i64 * 1000) {
        return err(&format!("could not queue the message: {}", e));
    }
    omarchylook::sync_state::request_outbox_run();
    info!("Compose: queued {} ({} → {} recipient(s)), sends in {}s", id, msg.account_id, msg.to.len() + msg.cc.len() + msg.bcc.len(), delay);
    serde_json::json!({ "ok": true, "id": id, "delay_secs": delay }).to_string()
}

/// POST /compose/cancel?id=ID and GET /compose/status?id=ID.
fn compose_outbox_response(config_dir: &PathBuf, first_line: &str) -> String {
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let id = match query_param(first_line, "id") { Some(i) => i, None => return err("missing id") };
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let cancelling = first_line.contains("POST /compose/cancel");
    let cancelled = if cancelling {
        match db.outbox_cancel(&id) { Ok(c) => c, Err(e) => return err(&e.to_string()) }
    } else { false };
    match db.outbox_status(&id) {
        Ok(Some((state, error, attempts))) => serde_json::json!({
            "ok": true, "cancelled": cancelled, "state": state, "error": error, "attempts": attempts
        }).to_string(),
        Ok(None) => serde_json::json!({ "ok": false, "cancelled": false, "state": "unknown", "error": "no such message" }).to_string(),
        Err(e) => err(&e.to_string()),
    }
}

/// JSON body for the /settings/senders endpoints (see the route comment).
fn sender_prefs_response(config_dir: &PathBuf, first_line: &str) -> String {
    use omarchylook::db::SenderAdd;
    let err = |m: &str| format!("{{\"ok\":false,\"error\":{}}}", serde_json::to_string(m).unwrap());
    let path = config_dir.join("messages.db");
    let db = match Database::open(path.to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let email = query_param(first_line, "email").unwrap_or_default();
    let result = if first_line.contains("POST /settings/senders/add") {
        db.add_sender(&email).map(|r| {
            let status = match r { SenderAdd::Added => "added", SenderAdd::AlreadyListed => "exists", SenderAdd::Invalid => "invalid" };
            format!("{{\"ok\":{},\"status\":\"{}\"}}", r != SenderAdd::Invalid, status)
        })
    } else if first_line.contains("POST /settings/senders/set") {
        let field = query_param(first_line, "field").unwrap_or_default();
        let value = query_param(first_line, "value").as_deref() == Some("true");
        db.set_sender_pref(&email, &field, value).map(|ok| format!("{{\"ok\":{}}}", ok))
    } else if first_line.contains("POST /settings/senders/remove") {
        db.remove_sender(&email).map(|ok| format!("{{\"ok\":{}}}", ok))
    } else {
        db.list_senders().map(|rows| {
            let items: Vec<String> = rows.iter().map(|s| format!(
                "{{\"email\":{},\"always_html\":{},\"always_images\":{}}}",
                serde_json::to_string(&s.email).unwrap(), s.always_html, s.always_images
            )).collect();
            format!("[{}]", items.join(","))
        })
    };
    result.unwrap_or_else(|e| err(&e.to_string()))
}

/// Minimal HTTP trigger server — listens on localhost:27182
/// QML uses XMLHttpRequest to POST to this endpoint to trigger auth/logout.
/// This avoids the limitation of QML not being able to write files directly.
///
/// Endpoints:
///   POST /auth/login   → triggers device flow
///   POST /auth/logout  → triggers logout
fn start_http_trigger_server(config_dir: &PathBuf) {
    // OMARCHYLOOK_PORT is for tests that run a second instance beside the real one; the UI always
    // talks to the default port.
    let addr = format!("127.0.0.1:{}", env::var("OMARCHYLOOK_PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(27182));
    eprintln!("[HTTP] Attempting to bind {}", addr);
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => {
            eprintln!("[HTTP] ✅ Bound OK");
            info!("🌐 HTTP trigger server listening on http://127.0.0.1:27182");
            omarchylook::perf::mark("http server bound");
            l
        }
        Err(e) => {
            eprintln!("[HTTP] ❌ BIND FAILED: {}", e);
            warn!("HTTP trigger server failed to bind: {} (falling back to file triggers)", e);
            return;
        }
    };

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let mut buf = [0u8; 1024];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                let first_line = request.lines().next().unwrap_or("");
                let req_t = std::time::Instant::now();
                // ── GET /sync/serial — cheap change counter the UI polls (no DB access)
                if first_line.contains("GET /sync/serial") {
                    let body = format!("{{\"mail\":{}}}", omarchylook::sync_state::mail_serial());
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }
                if !first_line.contains("POST /perf") {
                    omarchylook::perf::mark(&format!("http <- {}", first_line.split(" HTTP").next().unwrap_or("").chars().take(60).collect::<String>()));
                }

                // ── POST /perf?m=label — a mark from the QML side (shares this timeline)
                if first_line.contains("POST /perf") {
                    if let Some(m) = query_param(first_line, "m") {
                        omarchylook::perf::mark(&format!("UI: {}", m));
                    }
                    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 2\r\n\r\nok");
                    continue;
                }

                // ── GET /folders — return cached folder list as JSON ──────────────
                if first_line.contains("GET /folders") {
                    let db_path = config_dir.join("messages.db");
                    let body = match rusqlite::Connection::open(&db_path) {
                        Ok(conn) => {
                            // ?account=<id or email> limits to one account; every row says which
                            // account (id + email address) it belongs to.
                            let account = query_param(first_line, "account");
                            let mut stmt = conn.prepare(
                                "SELECT f.id, f.display_name, f.unread_item_count, f.well_known_name, f.total_item_count, \
                                        COALESCE(f.account_id, ''), COALESCE(a.email, '') \
                                 FROM folders f LEFT JOIN accounts a ON a.id = f.account_id \
                                 WHERE (?1 IS NULL OR f.account_id = ?1 OR lower(a.email) = lower(?1)) \
                                 ORDER BY f.account_id ASC, f.sort_order ASC, f.display_name ASC"
                            ).unwrap();
                            let rows: Vec<String> = stmt.query_map(rusqlite::params![account], |row| {
                                let id: String = row.get(0)?;
                                let display_name: String = row.get(1)?;
                                let unread: i32 = row.get::<_, Option<i32>>(2)?.unwrap_or(0);
                                let well_known: String = row.get::<_, Option<String>>(3)?.unwrap_or_default();
                                let total: i32 = row.get::<_, Option<i32>>(4)?.unwrap_or(0);
                                let account_id: String = row.get(5)?;
                                let account_email: String = row.get(6)?;
                                Ok(format!(
                                    "{{\"id\":{},\"display_name\":{},\"unread_item_count\":{},\"well_known_name\":{},\"total_item_count\":{},\"account_id\":{},\"account_email\":{}}}",
                                    serde_json::to_string(&id).unwrap(),
                                    serde_json::to_string(&display_name).unwrap(),
                                    unread,
                                    serde_json::to_string(&well_known).unwrap(),
                                    total,
                                    serde_json::to_string(&account_id).unwrap(),
                                    serde_json::to_string(&account_email).unwrap(),
                                ))
                            }).unwrap()
                            .filter_map(|r| r.ok())
                            .collect();
                            // "<Kind> - all accounts" rows go first: one per folder kind that two or
                            // more accounts have. Their account_id is empty, so move targets skip them.
                            let mut all_rows: Vec<String> = Vec::new();
                            if account.is_none() {
                                if let Ok(aggs) = omarchylook::db::all_accounts_folders(&conn) {
                                    for a in aggs {
                                        all_rows.push(format!(
                                            "{{\"id\":{},\"display_name\":{},\"unread_item_count\":{},\"well_known_name\":{},\"total_item_count\":{},\"account_id\":\"\",\"account_email\":\"All accounts\",\"all_accounts\":true}}",
                                            serde_json::to_string(&format!("all:{}", a.well_known_name)).unwrap(),
                                            serde_json::to_string(omarchylook::db::all_accounts_label(&a.well_known_name)).unwrap(),
                                            a.unread_item_count,
                                            serde_json::to_string(&a.well_known_name).unwrap(),
                                            a.total_item_count,
                                        ));
                                    }
                                }
                            }
                            all_rows.extend(rows);
                            format!("[{}]", all_rows.join(","))
                        }
                        Err(e) => {
                            warn!("GET /folders: DB open failed: {}", e);
                            "[]".to_string()
                        }
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── GET /accounts — configured accounts and whether each is signed in ──
                if first_line.contains("GET /accounts") {
                    let body = omarchylook::account_ops::accounts_json(config_dir);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── GET /contacts?view=all|favorites|lists&sort=first|last|company|recent ──
                if first_line.contains("GET /contacts") {
                    let view = query_param(first_line, "view").unwrap_or_else(|| "all".to_string());
                    let sort = query_param(first_line, "sort").unwrap_or_else(|| "first".to_string());
                    let body = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
                        Ok(db) => match db.query_contacts_for(&view, &sort, query_param(first_line, "account").as_deref()) {
                            Ok(rows) => serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string()),
                            Err(e) => {
                                warn!("GET /contacts: query failed: {}", e);
                                "[]".to_string()
                            }
                        },
                        Err(e) => {
                            warn!("GET /contacts: DB open failed: {}", e);
                            "[]".to_string()
                        }
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── POST /contacts/favorite?id=ID&value=true|false — local favorite flag ──
                if first_line.contains("POST /contacts/favorite") {
                    let id = query_param(first_line, "id").unwrap_or_default();
                    let value = query_param(first_line, "value").as_deref() == Some("true");
                    let ok = Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db"))
                        .and_then(|db| db.set_contact_favorite(&id, value))
                        .is_ok();
                    let body = if ok { "ok" } else { "error" };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── POST /messages/read?id=ID&read=true|false — mark a message read/unread.
                // Updates the local cache at once; the daemon pushes it to the provider. ──
                if first_line.contains("POST /messages/read") {
                    let id = query_param(first_line, "id").unwrap_or_default();
                    let read = query_param(first_line, "read").as_deref() == Some("true");
                    let ok = !id.is_empty()
                        && Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db"))
                            .and_then(|db| db.set_message_read(&id, read))
                            .map(|changed| if changed { omarchylook::sync_state::request_read_push() })
                            .is_ok();
                    let body = if ok { "ok" } else { "error" };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── GET /categories?account=ID — the account's categories (Exchange) / user labels
                // (Gmail) with colours, whether the Exchange category list is readable at all, and
                // the colour palette for new ones. ──
                if first_line.contains("GET /categories") {
                    let db_path = config_dir.join("messages.db");
                    let account = query_param(first_line, "account").unwrap_or_default();
                    let body = match rusqlite::Connection::open(&db_path) {
                        Ok(conn) => {
                            let cats: Vec<serde_json::Value> = conn
                                .prepare("SELECT name, color, pending FROM category_defs WHERE account_id = ?1 ORDER BY lower(name)")
                                .and_then(|mut st| {
                                    st.query_map(rusqlite::params![account], |r| Ok(serde_json::json!({
                                        "name": r.get::<_, String>(0)?, "color": r.get::<_, String>(1)?, "pending": r.get::<_, i64>(2)? != 0,
                                    }))).map(|it| it.flatten().collect())
                                })
                                .unwrap_or_default();
                            let master: bool = conn
                                .query_row("SELECT master_list FROM category_state WHERE account_id = ?1", rusqlite::params![account], |r| r.get::<_, i64>(0))
                                .map(|v| v != 0)
                                .unwrap_or(true);
                            let palette: Vec<serde_json::Value> = omarchylook::models::CATEGORY_PALETTE.iter()
                                .map(|c| serde_json::json!({ "key": c.key, "label": c.label, "hex": c.hex }))
                                .collect();
                            serde_json::json!({ "categories": cats, "master_list": master, "palette": palette }).to_string()
                        }
                        Err(_) => serde_json::json!({ "categories": [], "master_list": true, "palette": [] }).to_string(),
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── POST /categories/create?account=ID&name=X&color=KEY — a new category / label.
                // Usable at once; the daemon creates it on the provider. ──
                if first_line.contains("POST /categories/create") {
                    let account = query_param(first_line, "account").unwrap_or_default();
                    let name = query_param(first_line, "name").unwrap_or_default().trim().to_string();
                    let color = query_param(first_line, "color").and_then(|k| omarchylook::models::palette_color(&k));
                    let body = match color {
                        Some(c) if !account.is_empty() && !name.is_empty() && name.chars().count() <= 64 && !name.chars().any(|ch| ch.is_control() || ch == ';' || ch == ',') => {
                            match Database::open_for_account(config_dir.join("messages.db").to_str().unwrap_or("messages.db"), &account)
                                .and_then(|db| db.add_pending_category(&name, c.hex))
                            {
                                Ok(created) => {
                                    omarchylook::sync_state::request_read_push();
                                    serde_json::json!({ "created": created }).to_string()
                                }
                                Err(e) => serde_json::json!({ "error": format!("database: {}", e) }).to_string(),
                            }
                        }
                        _ => serde_json::json!({ "error": "need account, a name (max 64 chars, no ; or ,) and a palette colour" }).to_string(),
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── POST /messages/category?ids=a,b&op=add|remove&name=X — set / clear a category on
                // messages. Stored at once; the daemon pushes it to the provider. ──
                if first_line.contains("POST /messages/category") {
                    let ids: Vec<String> = query_param(first_line, "ids")
                        .map(|v| v.split(',').filter(|s| !s.is_empty()).map(String::from).collect())
                        .unwrap_or_default();
                    let name = query_param(first_line, "name").unwrap_or_default();
                    let add = match query_param(first_line, "op").as_deref() { Some("add") => Some(true), Some("remove") => Some(false), _ => None };
                    let body = match add {
                        Some(add) if !ids.is_empty() && !name.is_empty() => {
                            match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db"))
                                .and_then(|db| db.change_message_categories(&ids, add, &name))
                            {
                                Ok(changed) => {
                                    if changed > 0 { omarchylook::sync_state::request_read_push(); }
                                    serde_json::json!({ "changed": changed }).to_string()
                                }
                                Err(e) => serde_json::json!({ "error": format!("database: {}", e) }).to_string(),
                            }
                        }
                        _ => serde_json::json!({ "error": "need ids, name and op=add|remove" }).to_string(),
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── GET /messages/meta_progress — {"active":bool,"remaining":N}: the one-time
                // repopulate of message details (sender name, To/Cc/Bcc, categories…). ──
                if first_line.contains("GET /messages/meta_progress") {
                    let remaining: i64 = rusqlite::Connection::open(config_dir.join("messages.db"))
                        .and_then(|c| c.query_row("SELECT COUNT(*) FROM messages WHERE meta_ok = 0", [], |r| r.get(0)))
                        .unwrap_or(0);
                    let body = serde_json::json!({ "active": omarchylook::sync_state::meta_active(), "remaining": remaining }).to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── POST /messages/action?ids=a,b&op=archive|trash|delete|move[&dest=FOLDER] —
                // archive / trash / delete / move messages. The rows are hidden at once; the daemon
                // tells the provider when the undo window (the mail "send delay") is over.
                // POST /messages/action/undo?ids=a,b takes them back while that window is open. ──
                if first_line.contains("POST /messages/action") {
                    let db_path = config_dir.join("messages.db");
                    let ids: Vec<String> = query_param(first_line, "ids")
                        .map(|v| v.split(',').filter(|s| !s.is_empty()).map(String::from).collect())
                        .unwrap_or_default();
                    let delay = omarchylook::settings::read_send_delay(&config_dir.join("settings.toml"));
                    let body = match Database::open(db_path.to_str().unwrap_or("messages.db")) {
                        Ok(db) if first_line.contains("POST /messages/action/undo") => {
                            let undone = db.cancel_message_actions(&ids).unwrap_or(0);
                            if undone > 0 { omarchylook::sync_state::bump_mail("message action undone"); }
                            serde_json::json!({ "undone": undone }).to_string()
                        }
                        Ok(db) => {
                            use omarchylook::models::MessageAction;
                            let op = query_param(first_line, "op").unwrap_or_default();
                            let action = match op.as_str() {
                                "archive" => Some(MessageAction::Archive),
                                "trash" => Some(MessageAction::Trash),
                                "delete" => Some(MessageAction::Delete),
                                "move" => query_param(first_line, "dest").map(MessageAction::Move),
                                _ => None,
                            };
                            match action {
                                Some(action) if !ids.is_empty() => {
                                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
                                    let queued = db.queue_message_actions(&ids, &action, now + delay as i64).unwrap_or(0);
                                    // Wake the daemons the moment the window closes.
                                    std::thread::spawn(move || {
                                        std::thread::sleep(std::time::Duration::from_millis(delay * 1000 + 400));
                                        omarchylook::sync_state::request_read_push();
                                    });
                                    serde_json::json!({ "queued": queued, "delay_secs": delay }).to_string()
                                }
                                _ => serde_json::json!({ "error": "bad request: need ids and op (archive|trash|delete|move+dest)" }).to_string(),
                            }
                        }
                        Err(e) => serde_json::json!({ "error": format!("database: {}", e) }).to_string(),
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── POST /compose/send (JSON body) — queue a message; POST /compose/cancel?id= —
                // take it back while it is still inside its delay; GET /compose/status?id= ──
                if first_line.contains("POST /compose/send") {
                    let body = compose_send_response(config_dir, &mut stream, &buf[..n]);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }
                if first_line.contains("POST /compose/cancel") || first_line.contains("GET /compose/status") {
                    let body = compose_outbox_response(config_dir, first_line);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── GET|POST /settings/mail?send_delay=N — seconds a sent message waits before it goes ──
                if first_line.contains("GET /settings/mail") || first_line.contains("POST /settings/mail") {
                    use omarchylook::models::MailSettings;
                    let settings_path = config_dir.join("settings.toml");
                    let mut secs = omarchylook::settings::read_send_delay(&settings_path);
                    if first_line.contains("POST /settings/mail") {
                        if let Some(wanted) = query_param(first_line, "send_delay").and_then(|v| v.parse::<i32>().ok()) {
                            match omarchylook::settings::write_send_delay(&settings_path, wanted) {
                                Ok(stored) => secs = stored,
                                Err(e) => warn!("POST /settings/mail: could not save: {}", e),
                            }
                        }
                    }
                    let body = serde_json::json!({
                        "send_delay_secs": secs,
                        "max_send_delay_secs": MailSettings::MAX_SEND_DELAY_SECS,
                    }).to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── POST /auth/confirm?pending=ID&decision=replace|cancel — answer the
                // "this mailbox is already signed in — replace its sign-in?" prompt ──
                if first_line.contains("POST /auth/confirm") {
                    let pending_id = query_param(first_line, "pending").unwrap_or_default();
                    let replace = query_param(first_line, "decision").as_deref() == Some("replace");
                    let ok = SCHEDULER
                        .get()
                        .map(|sch| omarchylook::account_ops::resolve_reauth(config_dir, &pending_id, replace, sch))
                        .map(|r| r.map_err(|e| error!("❌ re-authentication answer failed: {}", e)).is_ok())
                        .unwrap_or(false);
                    let body = if ok { "ok" } else { "error" };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── POST /accounts/login?account=ID — log in an existing account ──
                // Reuses the kept token when there is one; otherwise tells the UI to run a device flow.
                if first_line.contains("POST /accounts/login") {
                    let id = query_param(first_line, "account").unwrap_or_default();
                    let body = match SCHEDULER.get() {
                        Some(scheduler) => match omarchylook::account_ops::log_in_existing(config_dir, &id, scheduler) {
                            Ok(omarchylook::account_ops::ResumeOutcome::Resumed) => "{\"result\":\"resumed\"}".to_string(),
                            Ok(omarchylook::account_ops::ResumeOutcome::LoginRequired(provider)) => {
                                serde_json::json!({ "result": "login_required", "provider": provider }).to_string()
                            }
                            Err(e) => {
                                error!("❌ log in {} failed: {}", id, e);
                                "{\"result\":\"error\"}".to_string()
                            }
                        },
                        None => "{\"result\":\"error\"}".to_string(),
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── GET /calendar/events?month=YYYY-MM — cached calendar events ──
                if first_line.contains("GET /calendar/events") {
                    let month: String = first_line
                        .split_once("month=")
                        .map(|(_, rest)| {
                            rest.split(|c| c == ' ' || c == '&' || c == '\r' || c == '\n')
                                .next()
                                .unwrap_or("")
                                .to_string()
                        })
                        .unwrap_or_default();

                    let body = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
                        Ok(db) => match db.get_events_for_month_by_account(&month, query_param(first_line, "account").as_deref()) {
                            Ok(events) => serde_json::to_string(
                                &events.iter().map(|(e, account_id, account_email)| serde_json::json!({
                                    "account_id": account_id,
                                    "account_email": account_email,
                                    "id": e.id,
                                    "subject": e.subject,
                                    "body": e.body,
                                    "start": e.start,
                                    "end": e.end,
                                    "is_all_day": e.is_all_day,
                                    "time_zone": e.time_zone,
                                })).collect::<Vec<_>>()
                            ).unwrap_or_else(|_| "[]".to_string()),
                            Err(e) => {
                                warn!("GET /calendar/events: query failed: {}", e);
                                "[]".to_string()
                            }
                        },
                        Err(e) => {
                            warn!("GET /calendar/events: DB open failed: {}", e);
                            "[]".to_string()
                        }
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── Sender preferences (Settings → Senders) ─────────────────────────────
                //   GET  /settings/senders                                  → [{email, always_html, always_images}]
                //   POST /settings/senders/add?email=E                      → {ok, status: added|exists|invalid}
                //   POST /settings/senders/set?email=E&field=html|images&value=true|false
                //   POST /settings/senders/remove?email=E
                if first_line.contains("/settings/senders") {
                    let body = sender_prefs_response(config_dir, first_line);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── GET /messages/invite?id=ID — meeting type of a message and whether it can be
                // answered. Cached after the first look. ──
                if first_line.contains("GET /messages/invite") {
                    let id = query_param(first_line, "id").unwrap_or_default();
                    let dir = config_dir.clone();
                    std::thread::spawn(move || {
                        let body = invite_info_json(&dir, &id);
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                            body.len(), body
                        );
                        let _ = stream.write_all(response.as_bytes());
                    });
                    continue;
                }

                // ── POST /messages/invite?id=ID&choice=accept|tentative|decline — answer an invitation
                // on Graph (silent reply), then refresh the calendar for accept/tentative. Slow, so it
                // runs on its own thread. ──
                if first_line.contains("POST /messages/invite") {
                    let id = query_param(first_line, "id").unwrap_or_default();
                    let choice = query_param(first_line, "choice").unwrap_or_default();
                    let dir = config_dir.clone();
                    std::thread::spawn(move || {
                        let body = respond_invite_json(&dir, &id, &choice);
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                            body.len(), body
                        );
                        let _ = stream.write_all(response.as_bytes());
                    });
                    continue;
                }

                // ── GET /messages/body?id=ID — full body of one message: {"type","content"}.
                // Served from the cache; otherwise fetched from the provider and cached.
                // Runs on its own thread so a slow fetch never stalls the list requests. ──
                if first_line.contains("GET /messages/body") {
                    let id = query_param(first_line, "id").unwrap_or_default();
                    let dir = config_dir.clone();
                    std::thread::spawn(move || {
                        let body = message_body_json(&dir, &id);
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                            body.len(), body
                        );
                        let _ = stream.write_all(response.as_bytes());
                    });
                    continue;
                }

                // ── GET /messages — return messages as JSON (optional ?folder_id=) ──────────────
                if first_line.contains("GET /messages") {
                    // Parse optional ?folder_id= query parameter
                    let folder_id: Option<String> = first_line
                        .split_once("folder_id=")
                        .map(|(_, rest)| {
                            let raw = rest.split(|c| c == ' ' || c == '&' || c == '\r' || c == '\n')
                                .next()
                                .unwrap_or("");
                            // URL-decode: folder IDs contain = and / which QML encodeURIComponent encodes
                            url_decode(raw)
                        })
                        .filter(|s| !s.is_empty());

                    let db_path = config_dir.join("messages.db");
                    let body = match rusqlite::Connection::open(&db_path) {
                        Ok(conn) => {
                            // Category colours per (account, lower-cased name), so each row can say
                            // which categories it carries and how to draw them.
                            let mut cat_colors: std::collections::HashMap<(String, String), String> = std::collections::HashMap::new();
                            if let Ok(mut st) = conn.prepare("SELECT account_id, name, color FROM category_defs") {
                                if let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))) {
                                    for (a, n, c) in rows.flatten() { cat_colors.insert((a, n.to_lowercase()), c); }
                                }
                            }
                            let map_row = |row: &rusqlite::Row| {
                                let account_id: String = row.get(6)?;
                                let cats: Vec<serde_json::Value> = omarchylook::db::categories_from_json(row.get::<_, Option<String>>(12)?.as_deref())
                                    .into_iter()
                                    .map(|name| {
                                        let color = cat_colors.get(&(account_id.clone(), name.to_lowercase())).cloned()
                                            .unwrap_or_else(|| omarchylook::models::fallback_category_color(&name).to_string());
                                        serde_json::json!([name, color])
                                    })
                                    .collect();
                                Ok(serde_json::json!({
                                    "id": row.get::<_, String>(0)?,
                                    "subject": row.get::<_, String>(1)?,
                                    "from_email": row.get::<_, String>(2)?,
                                    "from_name": row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                                    "received_at": row.get::<_, String>(4)?,
                                    "is_read": row.get::<_, bool>(5)?,
                                    "account_id": account_id,
                                    "account_email": row.get::<_, String>(7)?,
                                    "to_text": row.get::<_, Option<String>>(8)?.unwrap_or_default(),
                                    "cc_text": row.get::<_, Option<String>>(9)?.unwrap_or_default(),
                                    "bcc_text": row.get::<_, Option<String>>(10)?.unwrap_or_default(),
                                    "sent_at": row.get::<_, Option<String>>(11)?.unwrap_or_default(),
                                    // JSON text: [["Name","#rrggbb"],…] (a list model cannot hold nested lists cheaply)
                                    "cats": serde_json::Value::Array(cats).to_string(),
                                    "importance": row.get::<_, Option<String>>(13)?.unwrap_or_default(),
                                    "has_attachments": row.get::<_, Option<bool>>(14)?.unwrap_or(false),
                                    "conversation_id": row.get::<_, Option<String>>(15)?.unwrap_or_default(),
                                    "folder_id": row.get::<_, String>(16)?,
                                }).to_string())
                            };

                            // Paged: ?limit= (default 200, max 1000) & ?offset=. The cache holds
                            // tens of thousands of rows, so the UI pulls them a page at a time.
                            // Optional filters: ?folder_id= and ?account=<id or email address>.
                            let limit: i64 = query_param(first_line, "limit")
                                .and_then(|v| v.parse().ok())
                                .map(|n: i64| n.clamp(1, 1000))
                                .unwrap_or(200);
                            let offset: i64 = query_param(first_line, "offset")
                                .and_then(|v| v.parse().ok())
                                .map(|n: i64| n.max(0))
                                .unwrap_or(0);
                            let mut conds: Vec<String> = Vec::new();
                            let mut args: Vec<rusqlite::types::Value> = Vec::new();
                            if let Some(ref fid) = folder_id {
                                // "all:<kind>" = that kind of folder across every account (Inbox - all accounts).
                                if let Some(kind) = fid.strip_prefix("all:") {
                                    args.push(kind.to_string().into());
                                    conds.push(format!("m.folder_id IN (SELECT id FROM folders WHERE well_known_name = ?{})", args.len()));
                                } else {
                                    args.push(fid.clone().into());
                                    conds.push(format!("m.folder_id = ?{}", args.len()));
                                }
                            }
                            if let Some(acct) = query_param(first_line, "account") {
                                args.push(acct.into());
                                let n = args.len();
                                conds.push(format!("(m.account_id = ?{n} OR lower(a.email) = lower(?{n}))"));
                            }
                            // Archived / deleted / moved messages wait out their undo window hidden.
                            conds.push("m.action_pending IS NULL".to_string());
                            let where_sql = if conds.is_empty() { String::new() } else { format!(" WHERE {}", conds.join(" AND ")) };
                            args.push(limit.into());
                            args.push(offset.into());
                            let sql = format!(
                                "SELECT m.id, m.subject, m.from_email, m.from_name, m.received_at, m.is_read, \
                                        COALESCE(m.account_id, ''), COALESCE(a.email, ''), \
                                        m.to_text, m.cc_text, m.bcc_text, m.sent_at, m.categories, m.importance, \
                                        m.has_attachments, m.conversation_id, COALESCE(m.folder_id, '') \
                                 FROM messages m LEFT JOIN accounts a ON a.id = m.account_id{} \
                                 ORDER BY m.received_at DESC LIMIT ?{} OFFSET ?{}",
                                where_sql, args.len() - 1, args.len()
                            );
                            let rows: Vec<String> = match conn.prepare(&sql) {
                                Ok(mut stmt) => stmt.query_map(rusqlite::params_from_iter(args.iter()), map_row)
                                    .map(|it| it.filter_map(|r| r.ok()).collect())
                                    .unwrap_or_default(),
                                Err(e) => { warn!("GET /messages: bad query: {}", e); Vec::new() }
                            };
                            format!("[{}]", rows.join(","))
                        }
                        Err(e) => {
                            warn!("GET /messages: DB open failed: {}", e);
                            "[]".to_string()
                        }
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    omarchylook::perf::mark(&format!("http -> GET /messages answered ({} bytes, handler {:.1}ms)", body.len(), req_t.elapsed().as_secs_f64() * 1000.0));
                    continue;
                }

                // ── GET /settings/keys — the `[keys]` table of settings.toml:
                // {"serial":N,"preset":"","bindings":{"mail.archive":["e"]},"error":""}.
                // `serial` is the file's mtime; the UI polls this and re-applies when it moves, so
                // editing settings.toml rebinds keys without a restart. ──
                if first_line.contains("GET /settings/keys") {
                    let settings_path = config_dir.join("settings.toml");
                    let serial: u128 = std::fs::metadata(&settings_path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_nanos())
                        .unwrap_or(0);
                    let body = match std::fs::read_to_string(&settings_path) {
                        Ok(text) => match toml::from_str::<omarchylook::models::Settings>(&text) {
                            Ok(st) => {
                                let bindings: serde_json::Map<String, serde_json::Value> = st.keys.bindings.iter()
                                    .map(|(k, v)| (k.clone(), serde_json::json!(v.keys())))
                                    .collect();
                                serde_json::json!({ "serial": serial.to_string(), "preset": st.keys.preset, "bindings": bindings, "error": "" })
                            }
                            Err(e) => serde_json::json!({ "serial": serial.to_string(), "preset": "", "bindings": {}, "error": format!("settings.toml: {}", e) }),
                        },
                        Err(_) => serde_json::json!({ "serial": serial.to_string(), "preset": "", "bindings": {}, "error": "" }),
                    }.to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── GET /settings/ui — return UI settings as JSON ─────────────
                if first_line.contains("GET /settings/ui") {
                    let settings_path = config_dir.join("settings.toml");
                    let body = match std::fs::read_to_string(&settings_path)
                        .ok()
                        .and_then(|s| toml::from_str::<omarchylook::models::Settings>(&s).ok())
                    {
                        Some(settings) => format!(
                            "{{\"sidebar_expanded\":{},\"window_width\":{},\"window_height\":{},\"message_rendering\":\"{}\",\"folder_pane\":{}}}",
                            settings.ui.sidebar_expanded,
                            settings.ui.window_width,
                            settings.ui.window_height,
                            omarchylook::settings::normalize_message_rendering(&settings.ui.message_rendering).unwrap_or("system_sender"),
                            settings.ui.folder_pane,
                        ),
                        None => "{\"sidebar_expanded\":true,\"window_width\":1280,\"window_height\":800,\"message_rendering\":\"system_sender\",\"folder_pane\":false}".to_string(),
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── GET|POST /settings/calendar — recurring-meeting window (years back / ahead) ──
                if first_line.contains("GET /settings/calendar") || first_line.contains("POST /settings/calendar") {
                    use omarchylook::models::CalendarSettings;
                    let settings_path = config_dir.join("settings.toml");
                    let current = if first_line.contains("POST /settings/calendar") {
                        let cur = omarchylook::settings::read_calendar_settings(&settings_path);
                        let wanted = CalendarSettings {
                            recurrence_years_back: query_param(first_line, "back").and_then(|v| v.parse().ok()).unwrap_or(cur.recurrence_years_back),
                            recurrence_years_ahead: query_param(first_line, "ahead").and_then(|v| v.parse().ok()).unwrap_or(cur.recurrence_years_ahead),
                        };
                        match omarchylook::settings::write_calendar_settings(&settings_path, &wanted) {
                            Ok(stored) => stored,
                            Err(e) => {
                                warn!("POST /settings/calendar: could not save: {}", e);
                                cur
                            }
                        }
                    } else {
                        omarchylook::settings::read_calendar_settings(&settings_path)
                    };
                    let body = serde_json::json!({
                        "recurrence_years_back": current.recurrence_years_back,
                        "recurrence_years_ahead": current.recurrence_years_ahead,
                        "max_years_back": CalendarSettings::MAX_YEARS_BACK,
                        "max_years_ahead": CalendarSettings::MAX_YEARS_AHEAD,
                    }).to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── POST /settings/message_rendering?value=html|system|system_sender ──
                if first_line.contains("POST /settings/message_rendering") {
                    let wanted = query_param(first_line, "value").unwrap_or_default();
                    let body = match omarchylook::settings::write_message_rendering(&config_dir.join("settings.toml"), &wanted) {
                        Ok(stored) => format!("{{\"ok\":true,\"message_rendering\":\"{}\"}}", stored),
                        Err(e) => {
                            warn!("POST /settings/message_rendering {}: {}", wanted, e);
                            "{\"ok\":false}".to_string()
                        }
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── POST /settings/sidebar_expanded — persist sidebar state ────
                if first_line.contains("POST /settings/sidebar_expanded") {
                    // Body is "true" or "false" — read remaining request bytes
                    let body_start = request.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
                    let body_str = request[body_start..].trim();
                    let expanded = body_str == "true";

                    let settings_path = config_dir.join("settings.toml");
                    let result = std::fs::read_to_string(&settings_path)
                        .ok()
                        .and_then(|s| toml::from_str::<omarchylook::models::Settings>(&s).ok())
                        .map(|mut settings| {
                            settings.ui.sidebar_expanded = expanded;
                            toml::to_string_pretty(&settings)
                                .ok()
                                .map(|content| std::fs::write(&settings_path, content))
                        });
                    let ok = result.is_some();
                    debug!("POST /settings/sidebar_expanded {} → {}", expanded, ok);

                    let response = "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\nok";
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── POST /settings/folder_pane (body "true"|"false") — show/hide the folder pane ──
                if first_line.contains("POST /settings/folder_pane") {
                    let body_start = request.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
                    let shown = request[body_start..].trim() == "true";
                    let settings_path = config_dir.join("settings.toml");
                    let ok = std::fs::read_to_string(&settings_path)
                        .ok()
                        .and_then(|s| toml::from_str::<omarchylook::models::Settings>(&s).ok())
                        .and_then(|mut settings| {
                            settings.ui.folder_pane = shown;
                            toml::to_string_pretty(&settings).ok().map(|content| std::fs::write(&settings_path, content))
                        })
                        .is_some();
                    debug!("POST /settings/folder_pane {} → {}", shown, ok);
                    let response = "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\nok";
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── POST trigger routes ────────────────────────────────────
                let action = if first_line.contains("POST /auth/login") {
                    "login"
                } else if first_line.contains("POST /auth/cancel") {
                    "cancel"
                } else if first_line.contains("POST /auth/logout") {
                    "logout"
                } else if first_line.contains("POST /accounts/remove") {
                    "remove"
                } else {
                    ""
                };

                let status = if action.is_empty() { "404 Not Found" } else { "200 OK" };

                // Always respond with CORS headers so QML XHR doesn't block
                let response = format!(
                    "HTTP/1.1 {}\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\nok",
                    status
                );
                let _ = stream.write_all(response.as_bytes());
                drop(stream);

                match action {
                    "login" => {
                        // New account: its id is generated as <provider>-<suffix>
                        let provider = query_param(first_line, "provider").unwrap_or_else(|| "exchange".to_string());
                        let email = query_param(first_line, "email");
                        info!("🔔 HTTP trigger: add {} account requested", provider);
                        if let Some(scheduler) = SCHEDULER.get() {
                            let (config_dir, scheduler) = (config_dir.clone(), Arc::clone(scheduler));
                            std::thread::spawn(move || {
                                if let Err(e) = omarchylook::account_ops::begin_add_account_with_hint(&config_dir, &provider, email.as_deref(), scheduler) {
                                    error!("❌ Add account failed: {}", e);
                                }
                            });
                        }
                    }
                    "cancel" => {
                        info!("🔔 HTTP trigger: cancel sign-in");
                        omarchylook::google_auth::cancel_login(config_dir);
                    }
                    "logout" | "remove" => {
                        let id = query_param(first_line, "account")
                            .unwrap_or_else(|| omarchylook::token_store::DEFAULT_ACCOUNT.to_string());
                        info!("🔔 HTTP trigger: {} account {}", action, id);
                        if let Some(scheduler) = SCHEDULER.get() {
                            let result = if action == "remove" {
                                omarchylook::account_ops::remove_account(config_dir, &id, scheduler)
                            } else {
                                omarchylook::account_ops::sign_out_account(config_dir, &id, scheduler)
                            };
                            match result {
                                Ok(()) => info!("✅ {} succeeded for {}", action, id),
                                Err(e) => error!("❌ {} failed for {}: {}", action, id, e),
                            }
                        }
                    }
                    _ => {}
                }
            }
            Err(e) => warn!("HTTP trigger server accept error: {}", e),
        }
    }
}
