//! Main application entry point (Rust + QML)
//!
//! OmaLook: Lightweight Outlook clone
//! - Rust backend: Auth, Graph API, SQLite cache
//! - QML frontend: Native Qt UI with hot-reload support

use omalook::{init_logging, AuthManager, Database, SettingsManager, email_daemon::{EmailDaemon, DaemonConfig}, providers::graph::GraphEmailProvider};
use log::{debug, error, info, warn};
use std::env;
use std::path::PathBuf;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use tokio::runtime::Runtime;

/// Process-wide sync scheduler (one mail + one calendar thread per signed-in account).
static SCHEDULER: std::sync::OnceLock<Arc<omalook::scheduler::SyncScheduler>> = std::sync::OnceLock::new();

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
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    i += 3;
                    continue;
                }
            }
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn main() {
    // `omalook mcp`: stdio MCP forwarder to the running daemon. Not a backend; stdout is the protocol.
    if env::args().nth(1).as_deref() == Some("mcp") {
        omalook::mcp::shim::run(&get_config_dir());
        return;
    }

    // Initialize logging
    init_logging();
    let config_dir = get_config_dir();

    // A backend left running in the background (Settings → General) already serves the UI:
    // just open a window against it instead of starting a second backend.
    if omalook::background::backend_listening(omalook::background::http_port()) {
        info!("OmaLook backend already running; opening the UI only");
        launch_qml_app(&config_dir);
        return;
    }

    omalook::perf::init(&config_dir);
    info!("OmaLook starting");
    
    let db_path = config_dir.join("omarchy.db");
    let settings_path = config_dir.join("settings.toml");
    
    info!("Config dir: {}", config_dir.display());
    info!("Database: {}", db_path.display());
    
    // Initialize core backend components
    match initialize_app(&config_dir, db_path.to_str().unwrap(), settings_path.to_str().unwrap()) {
        Ok(_) => {
            info!("Application backend initialized successfully");
            
            // Launch Qt/QML runtime (returns when the window closes)
            launch_qml_app(&config_dir);

            // With "Keep running in background" on, the backend outlives the window.
            omalook::background::stay_while_enabled(
                &settings_path,
                &resolve_qml_dir(),
                std::time::Duration::from_secs(2),
            );
        }
        Err(e) => {
            error!("Failed to initialize application: {}", e);
            std::process::exit(1);
        }
    }
}

fn get_config_dir() -> PathBuf {
    // Use XDG standard: ~/.config/omalook
    if let Ok(xdg_config) = env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg_config).join("omalook")
    } else if let Ok(home) = env::var("HOME") {
        PathBuf::from(home).join(".config").join("omalook")
    } else {
        PathBuf::from("/tmp/omalook")
    }
}

/// Directory holding the Omarchy shell UI kit (`Commons/`, `Ui/`).
const OMARCHY_KIT_DIR: &str = "/usr/share/omarchy/shell";

/// Locate the QML app directory: `QML_DIR` env, then `./qml`, then the `qml/` beside the installed
/// binary (`<prefix>/bin/omalook` -> `<prefix>/qml`). Nothing is baked in at build time: a path
/// remembered from the machine that built the binary (e.g. a project folder on a network share)
/// would break whenever that location is unavailable. When none exists the caller reports
/// "QML entry point not found" for `./qml`.
fn resolve_qml_dir() -> PathBuf {
    if let Ok(dir) = env::var("QML_DIR") {
        return PathBuf::from(dir);
    }
    let cwd_qml = PathBuf::from("qml");
    if cwd_qml.join("shell.qml").exists() {
        return cwd_qml;
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(prefix) = exe.parent().and_then(|bin| bin.parent()) {
            let beside = prefix.join("qml");
            if beside.join("shell.qml").exists() {
                return beside;
            }
        }
    }
    cwd_qml
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
                format!("{} not found - OmaLook needs the Omarchy shell UI kit", target.display()),
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
    omalook::perf::mark("spawning quickshell");
    let mut cmd = Command::new("quickshell");
    // Native QML plugins (OmaLook.Compose): the dev build next to the source tree, then the
    // installed location. Existing QML_IMPORT_PATH entries are kept.
    {
        let mut paths: Vec<std::path::PathBuf> = Vec::new();
        if let Some(root) = std::path::Path::new(&qml_dir).parent() {
            paths.push(root.join("plugin/omalook-compose/build/qml"));
        }
        paths.push(std::path::PathBuf::from("/usr/lib/omalook/qml"));
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

fn initialize_app(config_dir: &PathBuf, db_path: &str, settings_path: &str) -> omalook::errors::Result<()> {
    // Initialize database
    let db = {
        let _t = omalook::perf::span("Database::open (schema init)");
        Arc::new(Database::open(db_path)?)
    };
    info!("Database initialized");
    
    // Initialize settings
    let _settings = SettingsManager::open(settings_path)?;
    info!("Settings initialized");
    
    // Initialize auth (check if already authenticated, but don't auto-prompt)
    let _auth_span = omalook::perf::span("AuthManager::new + keyring check");
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

    let config_dir_sms = config_dir.clone();
    // MCP listener: opens its own port only while MCP is on and "Keep running in background" is on.
    omalook::mcp::server::start(config_dir);
    // Start HTTP trigger server (QML posts to this to trigger auth/logout)
    let config_dir_http = config_dir.clone();
    std::thread::spawn(move || {
        start_http_trigger_server(&config_dir_http);
    });
    
    // Pre-render: mail from "always HTML" senders is drawn ahead of time into the render cache (OL-028).
    let config_dir_render = config_dir.clone();
    std::thread::Builder::new().name("prerender".into()).spawn(move || {
        let db = config_dir_render.join("messages.db");
        std::thread::sleep(std::time::Duration::from_secs(45));      // let the first sync and the window settle
        loop {
            // the size is read again every round, so a changed setting applies without a restart
            let cache = omalook::render_cache::open_configured(&config_dir_render);
            cache.trim();
            omalook::render_cache::prerender_round(db.to_str().unwrap_or("messages.db"), &cache, &render_helper_path(), 3);
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
    }).ok();

    // Per-account sync (mail + calendar threads for every signed-in account)
    let scheduler = Arc::new(omalook::scheduler::SyncScheduler::new(config_dir));
    let _ = SCHEDULER.set(Arc::clone(&scheduler));
    std::thread::Builder::new().name("scheduler".into()).spawn(move || scheduler.start_all()).ok();

    // SMS: mirrors the phone's messages (through the local KDE Connect daemon) into the cache.
    omalook::sms_daemon::start(config_dir_sms);
    
    Ok(())
}

/// Watch for device flow and logout trigger files
fn watch_for_device_flow_trigger(config_dir: &PathBuf) {
    use std::fs;
    use std::thread;
    use std::time::Duration;
    
    let trigger_file = "/tmp/omalook-trigger-device-flow";
    let logout_trigger_file = "/tmp/omalook-trigger-logout";
    
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
                    if let Err(e) = omalook::account_ops::begin_add_account(config_dir, &provider, Arc::clone(scheduler)) {
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
                        omalook::account_ops::sign_out_all(config_dir, scheduler).map(|n| info!("✅ Signed out {} account(s)", n))
                    } else {
                        omalook::account_ops::sign_out_account(config_dir, &id, scheduler)
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

/// Invitation lookup of a message as JSON (see `InviteInfo::to_json`), or `{"error":"..."}`.
/// Ordinary mail is cached for good; calendar mail for a few minutes, because the answer can
/// change elsewhere (Outlook, phone) and the event can be cancelled or deleted.
fn invite_info_json(config_dir: &PathBuf, id: &str) -> String {
    use omalook::invites::{cache_ttl_secs, InviteInfo};
    let err = |m: &str| format!("{{\"error\":{}}}", serde_json::to_string(m).unwrap());
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
    if omalook::accounts::provider_of(&account) != "exchange" {
        return InviteInfo::none().to_json().to_string();
    }
    let cached = db.invite_info(id).ok().flatten();
    if let Some((json, at)) = &cached {
        let kind = serde_json::from_str::<serde_json::Value>(json)
            .ok().and_then(|v| v["type"].as_str().map(str::to_string)).unwrap_or_default();
        if !kind.is_empty() && chrono::Utc::now().timestamp() - at < cache_ttl_secs(&kind) {
            return json.clone();
        }
    }
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => return err(&format!("runtime: {}", e)),
    };
    match rt.block_on(GraphEmailProvider::new(AuthManager::for_account(&account)).fetch_invite_info(id)) {
        Ok(info) => {
            let json = info.to_json().to_string();
            if let Err(e) = db.set_invite_info(id, &json) {
                warn!("Could not cache invite info of {}: {}", id, e);
            }
            json
        }
        Err(e) => {
            warn!("Invite lookup failed for {}: {}", id, e);
            // A stale answer beats none (offline, token trouble).
            match cached {
                Some((json, _)) => json,
                None => err("could not read this message from the server"),
            }
        }
    }
}

/// List lookup of a message for reply-to-list (OL-053): `{is_list, list_id, post, no_post}` plus a `refusal`
/// sentence when a reply cannot go to the list, or `{"error"}`. The headers are fetched from the provider once
/// per message (the cache keeps no List-* headers) and the answer is cached for good in `list_info`.
fn list_info_json(config_dir: &PathBuf, id: &str) -> String {
    use omalook::list_headers::{refusal, ListInfo};
    let err = |m: &str| format!("{{\"error\":{}}}", serde_json::to_string(m).unwrap());
    if id.is_empty() {
        return err("missing id");
    }
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let with_refusal = |info: ListInfo| {
        let mut j = info.to_json();
        j["refusal"] = serde_json::json!(refusal(&info));
        j.to_string()
    };
    if let Some(cached) = db.list_info_get(id).ok().flatten() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&cached) {
            return with_refusal(ListInfo::from_json(&v));
        }
    }
    let account = match db.message_account(id) {
        Ok(Some(a)) => a,
        _ => return err("unknown message"),
    };
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => return err(&format!("runtime: {}", e)),
    };
    let headers = match omalook::accounts::provider_of(&account) {
        "gmail" => rt.block_on(omalook::providers::gmail::GmailProvider::new(&account, None).fetch_list_headers(id)),
        "exchange" => rt.block_on(GraphEmailProvider::new(AuthManager::for_account(&account)).fetch_list_headers(id)),
        _ => return err("this account type cannot tell whether a message came from a list"),
    };
    match headers {
        Ok(h) => {
            let info = ListInfo::from_headers(&h);
            if let Err(e) = db.list_info_set(id, &info.to_json().to_string()) {
                warn!("Could not cache list info of {}: {}", id, e);
            }
            with_refusal(info)
        }
        Err(e) => {
            warn!("List lookup failed for {}: {}", id, e);
            err("could not read this message's headers from the server")
        }
    }
}

/// Re-read the account's calendar around `date` after an invitation changed it. Returns
/// `"refreshed"` or `"not refreshed: ..."`.
fn refresh_calendar_note(rt: &tokio::runtime::Runtime, db_path: &str, account: &str, date: Option<chrono::NaiveDate>) -> String {
    match Database::open_for_account(db_path, account) {
        Err(e) => format!("not refreshed: {}", e),
        Ok(cal_db) => {
            let provider = Arc::new(omalook::providers::GraphCalendarProvider::new(AuthManager::for_account(account)));
            let cal = omalook::calendar_daemon::CalendarDaemon::new(Default::default(), Arc::new(cal_db), provider);
            let near = rt.block_on(cal.refresh_near_window());
            let around = match date {
                Some(d) => rt.block_on(cal.refresh_around(d)).map(|_| ()),
                None => Ok(()),
            };
            match near.map(|_| ()).and(around) {
                Ok(()) => "refreshed".to_string(),
                Err(e) => { warn!("Calendar refresh after invitation change failed: {}", e); format!("not refreshed: {}", e) }
            }
        }
    }
}

/// `y` on a meeting cancellation: remove the cancelled meeting (one occurrence or a whole series,
/// as the cancellation says) from the user's calendar, then refresh the calendar. A cancellation
/// whose meeting is no longer on the calendar succeeds without touching anything, so the caller
/// can still mark the mail read. Returns the lookup JSON plus `"ok":true`, `"removed":bool`,
/// `"series":bool`, `"calendar":...`, or `{"error":...}`.
fn remove_cancelled_json(config_dir: &PathBuf, id: &str) -> String {
    use omalook::invites::status_of;
    let err = |m: &str| format!("{{\"error\":{}}}", serde_json::to_string(m).unwrap());
    let db_path = config_dir.join("messages.db");
    let db = match Database::open(db_path.to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let account = match db.message_account(id) {
        Ok(Some(a)) => a,
        _ => return err("unknown message"),
    };
    if omalook::accounts::provider_of(&account) != "exchange" {
        return err("removing cancelled meetings is only supported for Microsoft accounts");
    }
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => return err(&format!("runtime: {}", e)),
    };
    let graph = GraphEmailProvider::new(AuthManager::for_account(&account));
    // Fresh lookup, never the cache: a stale answer here could delete the wrong thing.
    let mut info = match rt.block_on(graph.fetch_invite_info(id)) {
        Ok(i) => i,
        Err(e) => {
            warn!("Invite lookup before removing failed for {}: {}", id, e);
            return err("could not read this message from the server");
        }
    };
    if info.kind != "meetingCancelled" {
        return err("this message is not a meeting cancellation");
    }
    let mut removed = false;
    if let Some(event_id) = info.event_id.clone().filter(|_| info.removable()) {
        match rt.block_on(graph.delete_event(&event_id)) {
            Ok(()) => removed = true,
            // Already gone (removed in Outlook, or the server dropped it): the goal is met.
            Err(e) if status_of(&e.to_string()) == Some(404) => {}
            Err(e) => {
                warn!("Removing cancelled meeting of {} failed: {}", id, e);
                return match status_of(&e.to_string()) {
                    Some(403) => err("not allowed: grant Calendars.ReadWrite to this account (sign in again after the Azure consent)"),
                    _ => err("the server refused to remove the meeting"),
                };
            }
        }
    }
    if removed {
        info.event_id = None;
        info.response = "none".to_string();
    }
    let mut out = info.to_json();
    if let Err(e) = db.set_invite_info(id, &out.to_string()) {
        warn!("Could not cache invite info of {}: {}", id, e);
    }
    let note = if removed {
        refresh_calendar_note(&rt, db_path.to_str().unwrap_or("messages.db"), &account, info.start_date())
    } else {
        "refreshed".to_string()
    };
    out["ok"] = serde_json::json!(true);
    out["removed"] = serde_json::json!(removed);
    out["calendar"] = serde_json::json!(note);
    out.to_string()
}

/// Answer an invitation (choice = accept | tentative | decline) on its Graph event, then
/// refresh the calendar so the change shows at once. Returns the new `InviteInfo` JSON plus
/// `"ok":true` and `"calendar":"refreshed"|"not refreshed: ..."`, or `{"error":...}`.
/// A 403 means the account has not granted Calendars.ReadWrite yet.
fn respond_invite_json(config_dir: &PathBuf, id: &str, choice: &str) -> String {
    use omalook::invites::{response_op, status_of};
    let err = |m: &str| format!("{{\"error\":{}}}", serde_json::to_string(m).unwrap());
    if id.is_empty() {
        return err("missing id");
    }
    if choice == "remove" {
        return remove_cancelled_json(config_dir, id);
    }
    let Some((op, new_response)) = response_op(choice) else {
        return err("unknown response");
    };
    let db_path = config_dir.join("messages.db");
    let db = match Database::open(db_path.to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let account = match db.message_account(id) {
        Ok(Some(a)) => a,
        _ => return err("unknown message"),
    };
    if omalook::accounts::provider_of(&account) != "exchange" {
        return err("responding to invitations is only supported for Microsoft accounts");
    }
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => return err(&format!("runtime: {}", e)),
    };
    let graph = GraphEmailProvider::new(AuthManager::for_account(&account));
    // Always look the invitation up afresh: the cache may predate a cancellation.
    let mut info = match rt.block_on(graph.fetch_invite_info(id)) {
        Ok(i) => i,
        Err(e) => {
            warn!("Invite lookup before responding failed for {}: {}", id, e);
            return err("could not read this message from the server");
        }
    };
    let Some(event_id) = info.event_id.clone().filter(|_| info.actionable()) else {
        let why = if info.is_organizer { "you are the organizer of this meeting" }
            else if info.is_cancelled || info.kind == "meetingCancelled" { "this meeting was cancelled" }
            else if info.kind == "meetingRequest" { "the meeting is no longer on your calendar" }
            else { "this message is not an open invitation" };
        return err(&format!("cannot respond: {}", why));
    };
    if let Err(e) = rt.block_on(graph.respond_to_event(&event_id, op)) {
        warn!("Respond {} to {} failed: {}", op, id, e);
        return match status_of(&e.to_string()) {
            Some(403) => err("not allowed: grant Calendars.ReadWrite to this account (sign in again after the Azure consent)"),
            Some(404) => err("the meeting no longer exists on the server"),
            _ => err("the server rejected the response"),
        };
    }
    info.response = new_response.to_string();
    let mut out = info.to_json();
    if let Err(e) = db.set_invite_info(id, &out.to_string()) {
        warn!("Could not cache invite info of {}: {}", id, e);
    }
    // Every answer changes the calendar: accept/tentative add or update the event, decline removes it.
    let note = refresh_calendar_note(&rt, db_path.to_str().unwrap_or("messages.db"), &account, info.start_date());
    out["ok"] = serde_json::json!(true);
    out["calendar"] = serde_json::json!(note);
    out.to_string()
}

/// Contact name for each address ("" when the phone shared none).
fn sms_names_of(device: &str, addresses: &[String]) -> Vec<String> {
    addresses.iter().map(|a| omalook::sms_names::lookup(device, a).unwrap_or_default()).collect()
}

/// Display title of a thread: contact names (falling back to the number), comma-joined.
fn sms_title(addresses: &[String], names: &[String]) -> String {
    let mut seen: Vec<String> = vec![];
    for (a, n) in addresses.iter().zip(names) {
        let shown = if n.is_empty() { a.clone() } else { n.clone() };
        if !seen.contains(&shown) { seen.push(shown); }
    }
    seen.join(", ")
}

/// A short excerpt of `body` around the first word of `query` it contains.
fn sms_snippet(body: &str, query: &str) -> String {
    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = flat.chars().collect();
    let lower: String = flat.to_lowercase();
    let at = query.split_whitespace().filter_map(|w| lower.find(&w.to_lowercase())).min()
        .map(|b| lower[..b].chars().count()).unwrap_or(0);
    let start = at.saturating_sub(24);
    let end = (start + 90).min(chars.len());
    format!("{}{}{}", if start > 0 { "\u{2026}" } else { "" }, chars[start..end].iter().collect::<String>(), if end < chars.len() { "\u{2026}" } else { "" })
}

/// POST /sms/send — send a text through KDE Connect. JSON body:
/// `{"device"?: id, "thread"?: id, "to"?: ["number", ...], "text": "..."}`. With `thread` it replies in
/// that conversation; with `to` it starts one (or reuses the thread that already has exactly those
/// recipients). The text is stored as a pending row at once (queued -> the phone's echo replaces it,
/// or it turns into a failure). Answers `{ok, device, thread, uid, new_thread}` or `{ok:false, error, ...}`.
fn sms_send_response(config_dir: &PathBuf, stream: &mut std::net::TcpStream, initial: &[u8]) -> String {
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let raw = match read_post_body(stream, initial, 64 * 1024) { Ok(b) => b, Err(e) => return err(&e) };
    let req: serde_json::Value = match serde_json::from_str(&raw) { Ok(v) => v, Err(e) => return err(&format!("unreadable request: {}", e)) };
    let text = req["text"].as_str().unwrap_or("").trim_end().to_string();
    if text.trim().is_empty() { return err("nothing to send"); }
    if text.chars().count() > 5000 { return err("message too long"); }
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) { Ok(d) => d, Err(e) => return err(&format!("database: {}", e)) };
    let device = match req["device"].as_str().map(String::from).or_else(|| db.sms_default_device().ok().flatten()) {
        Some(d) => d, None => return err("no phone paired (Settings > SMS)"),
    };
    let to: Vec<String> = req["to"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
    let (mut thread, addresses) = match req["thread"].as_i64() {
        Some(t) => match db.sms_messages(Some(&device), t, None, 1).ok().and_then(|r| r.into_iter().next()) {
            Some(m) => (Some(t), m.addresses),
            None => return err("unknown conversation"),
        },
        None => {
            if to.is_empty() { return err("add at least one recipient"); }
            if let Some(bad) = to.iter().find(|a| a.chars().filter(|c| c.is_ascii_digit()).count() < 3) { return err(&format!("not a phone number: {}", bad)); }
            (db.sms_thread_for_addresses(&device, &to).ok().flatten(), to)
        }
    };
    if matches!(thread, Some(t) if t <= 0) { thread = None; }   // a stand-in thread: send as new
    let new_thread = thread.is_none();
    let shown_thread = thread.unwrap_or_else(|| omalook::db::sms::pending_thread_id(&addresses));
    let uid = match db.sms_add_pending(&device, shown_thread, &addresses, &text, false) { Ok(u) => u, Err(e) => return err(&e.to_string()) };
    omalook::sync_state::bump_sms("send queued");
    match omalook::sms_daemon::send_text(&device, thread, &addresses, &text) {
        Ok(()) => serde_json::json!({ "ok": true, "device": device, "thread": shown_thread, "uid": uid, "new_thread": new_thread }).to_string(),
        Err(e) => {
            warn!("SMS send failed: {}", e);
            let _ = db.sms_set_pending_state(&device, uid, true);
            omalook::sync_state::bump_sms("send failed");
            serde_json::json!({ "ok": false, "error": e, "device": device, "thread": shown_thread, "uid": uid, "new_thread": new_thread }).to_string()
        }
    }
}

/// Handle the `/sms/*` routes; always returns a JSON body.
fn sms_route(config_dir: &PathBuf, first_line: &str) -> String {
    let err = |m: &str| serde_json::json!({ "error": m }).to_string();
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let device = query_param(first_line, "device");
    let int = |k: &str| query_param(first_line, k).and_then(|v| v.parse::<i64>().ok());
    let json = |r: omalook::errors::Result<serde_json::Value>| match r {
        Ok(v) => v.to_string(),
        Err(e) => err(&e.to_string()),
    };
    let path = first_line.split_whitespace().nth(1).unwrap_or("").split('?').next().unwrap_or("");
    match (first_line.starts_with("POST"), path) {
        (false, "/sms/threads") => {
            let query = query_param(first_line, "q").unwrap_or_default();
            let query = query.trim().to_string();
            // Searching reads every thread, then keeps the ones that match by contact name,
            // phone number, or the text of any message in them.
            let limit = if query.is_empty() { int("limit").unwrap_or(500).clamp(1, 5000) } else { 5000 };
            let hits: std::collections::HashMap<(String, i64), String> = if query.is_empty() { Default::default() } else {
                db.sms_search_bodies(device.as_deref(), &query, 20000).unwrap_or_default()
                    .into_iter().map(|(d, t, b)| ((d, t), b)).collect()
            };
            let q_lower = query.to_lowercase();
            let q_digits: String = query.chars().filter(|c| c.is_ascii_digit()).collect();
            json(db.sms_threads(device.as_deref(), limit).map(|r| serde_json::Value::Array(r.into_iter().filter_map(|t| {
                let names = sms_names_of(&t.device_id, &t.addresses);
                let title = sms_title(&t.addresses, &names);
                let mut v = serde_json::json!(t);
                v["title"] = serde_json::json!(title);
                v["names"] = serde_json::json!(names);
                if !query.is_empty() {
                    let by_name = names.iter().chain(std::iter::once(&title)).any(|n| n.to_lowercase().contains(&q_lower));
                    let by_number = q_digits.len() >= 3 && t.addresses.iter().any(|a| {
                        a.chars().filter(|c| c.is_ascii_digit()).collect::<String>().contains(&q_digits)
                    });
                    let body = hits.get(&(t.device_id.clone(), t.thread_id));
                    if by_name || by_number { v["match"] = serde_json::json!(if by_name { "name" } else { "number" }); }
                    else if let Some(b) = body { v["match"] = serde_json::json!("text"); v["snippet"] = serde_json::json!(sms_snippet(b, &query)); }
                    else { return None; }
                    if by_name || by_number { if let Some(b) = body { v["snippet"] = serde_json::json!(sms_snippet(b, &query)); } }
                }
                Some(v)
            }).collect())))
        }
        (false, "/sms/contacts") => {
            // Recipient suggestions: the phone's contacts whose name or number contains `q`.
            let q = query_param(first_line, "q").unwrap_or_default().to_lowercase();
            let qd: String = q.chars().filter(|c| c.is_ascii_digit()).collect();
            let dev = device.clone().or_else(|| db.sms_default_device().ok().flatten()).unwrap_or_default();
            let list: Vec<serde_json::Value> = omalook::sms_names::contacts(&dev).into_iter()
                .filter(|(n, num)| q.is_empty() || n.to_lowercase().contains(&q)
                    || (qd.len() >= 2 && num.chars().filter(|c| c.is_ascii_digit()).collect::<String>().contains(&qd)))
                .take(12).map(|(n, num)| serde_json::json!({ "name": n, "number": num })).collect();
            serde_json::json!({ "device": dev, "contacts": list }).to_string()
        }
        (false, "/sms/messages") => match int("thread") {
            None => err("missing thread"),
            Some(t) => json(db.sms_messages(device.as_deref(), t, int("before"), int("limit").unwrap_or(100).clamp(1, 1000))
                .map(|r| serde_json::Value::Array(r.into_iter().map(|m| {
                    let names = sms_names_of(&m.device_id, &m.addresses);
                    let mut v = serde_json::json!(m);
                    v["names"] = serde_json::json!(names);
                    v
                }).collect()))),
        },
        // GET /sms/attachment?device=&part=&uid= : state of one MMS picture (the first call asks the phone).
        (false, "/sms/attachment") => match (device.as_deref(), int("part"), query_param(first_line, "uid")) {
            (Some(d), Some(part), Some(uid)) if !d.is_empty() && !uid.is_empty() => omalook::sms_daemon::attachment_status(d, part, &uid).to_string(),
            _ => err("need device, part and uid"),
        },
        (false, "/sms/unread") => json(db.sms_unread_totals().map(|(m, t)| serde_json::json!({ "messages": m, "threads": t }))),
        (true, "/sms/read") => match int("thread") {
            None => err("missing thread"),
            Some(t) => {
                let read = query_param(first_line, "read").as_deref() != Some("false");
                // Unread follows mail: only the newest received message of the thread goes back to unread.
                let changed = if read || int("uid").is_some() { db.sms_set_read(device.as_deref(), t, int("uid"), read) }
                              else { db.sms_mark_thread_unread(device.as_deref(), t) };
                json(changed.map(|n| {
                    if n > 0 { omalook::sync_state::bump_sms("read flag"); }
                    if n > 0 && read {
                        // Best effort: ask the phone's SMS app to mark the thread read (see sms_daemon).
                        if let Some(dev) = device.clone().or_else(|| db.sms_default_device().ok().flatten()) {
                            let mut needles: Vec<String> = vec![];
                            if let Ok(rows) = db.sms_messages(Some(&dev), t, None, 1) {
                                if let Some(m) = rows.first() {
                                    needles.extend(sms_names_of(&dev, &m.addresses));
                                    needles.extend(m.addresses.clone());
                                }
                            }
                            omalook::sms_daemon::sync_read_to_phone(dev, needles);
                        }
                    }
                    serde_json::json!({ "ok": true, "changed": n })
                }))
            }
        },
        (true, "/sms/retry") => {
            // Send a failed text again: one (`uid`) or every failed text of the thread.
            let dev = device.clone().or_else(|| db.sms_default_device().ok().flatten()).unwrap_or_default();
            let uids = match (int("uid"), int("thread")) {
                (Some(u), _) => vec![u],
                (None, Some(t)) => db.sms_failed_in_thread(&dev, t).unwrap_or_default(),
                _ => vec![],
            };
            let (mut sent, mut failed) = (0, 0);
            for u in uids {
                let Ok(Some((thread, addrs, body))) = db.sms_failed_row(&dev, u) else { continue };
                let _ = db.sms_set_pending_state(&dev, u, false);
                match omalook::sms_daemon::send_text(&dev, Some(thread), &addrs, &body) {
                    Ok(()) => sent += 1,
                    Err(e) => { warn!("SMS retry failed: {}", e); let _ = db.sms_set_pending_state(&dev, u, true); failed += 1; }
                }
            }
            omalook::sync_state::bump_sms("retry");
            serde_json::json!({ "ok": failed == 0, "sent": sent, "failed": failed }).to_string()
        }
        (true, "/sms/sync") => serde_json::json!({ "ok": omalook::sms_daemon::request_sync() }).to_string(),
        _ => err("unknown sms route"),
    }
}

/// Path of the WebKit render helper (the reading pane looks in the same place).
fn render_helper_path() -> PathBuf { resolve_qml_dir().join("..").join("bin").join("omalook-render") }

/// GET /render/get?id&width&scale&images -> {"hit":false} or {"hit":true,"file":PNG,"meta":{width,height,scale,bg,links}}.
/// Also remembers the width and scale so the background pre-render draws at the size the pane really uses.
fn render_get_json(config_dir: &PathBuf, line: &str) -> String {
    let miss = "{\"hit\":false}".to_string();
    let id = query_param(line, "id").unwrap_or_default();
    let width: u32 = query_param(line, "width").and_then(|v| v.parse().ok()).unwrap_or(0);
    let scale: f64 = query_param(line, "scale").and_then(|v| v.parse().ok()).unwrap_or(1.0);
    let images = query_param(line, "images").as_deref() == Some("1");
    if id.is_empty() || width < 100 { return miss; }
    let cache = omalook::render_cache::open_configured(config_dir);
    cache.write_hint(omalook::render_cache::Hint { width, scale });
    let Ok(db) = Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) else { return miss };
    let Ok(Some((t, body))) = db.cached_body(&id) else { return miss };
    if t != "html" { return miss; }
    let k = omalook::render_cache::key(&id, &body, width, scale, images);
    match cache.lookup(&k) {
        Some((png, meta)) => {
            let m: serde_json::Value = serde_json::from_str(&meta).unwrap_or(serde_json::Value::Null);
            serde_json::json!({ "hit": true, "file": png.to_string_lossy(), "meta": m }).to_string()
        }
        None => miss,
    }
}

/// POST /render/adopt?id&width&scale&images&file=PNG: the pane just rendered a message itself (helper output beside the
/// PNG as PNG.json); keep a copy in the render cache. Only the pane's own temp files (XDG_RUNTIME_DIR/omalook-render-N.png).
fn render_adopt_json(config_dir: &PathBuf, line: &str) -> String {
    let err = |m: &str| format!("{{\"ok\":false,\"error\":{}}}", serde_json::to_string(m).unwrap());
    let id = query_param(line, "id").unwrap_or_default();
    let width: u32 = query_param(line, "width").and_then(|v| v.parse().ok()).unwrap_or(0);
    let scale: f64 = query_param(line, "scale").and_then(|v| v.parse().ok()).unwrap_or(1.0);
    let images = query_param(line, "images").as_deref() == Some("1");
    let file = PathBuf::from(query_param(line, "file").unwrap_or_default());
    let run = PathBuf::from(env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into()));
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if file.parent() != Some(run.as_path()) || !name.starts_with("omalook-render-") || !name.ends_with(".png") { return err("not a render output"); }
    let Ok(json) = std::fs::read_to_string(format!("{}.json", file.display())) else { return err("no helper output") };
    let Ok(db) = Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) else { return err("database") };
    let Ok(Some((t, body))) = db.cached_body(&id) else { return err("no cached body") };
    if t != "html" { return err("not html"); }
    let received: String = rusqlite::Connection::open(config_dir.join("messages.db")).ok()
        .and_then(|c| c.query_row("SELECT COALESCE(received_at,'') FROM messages WHERE id = ?1", [&id], |r| r.get(0)).ok()).unwrap_or_default();
    let cache = omalook::render_cache::open_configured(config_dir);
    let k = omalook::render_cache::key(&id, &body, width, scale, images);
    match cache.store(&k, &id, &received, &file, &json) {
        Ok(kept) => format!("{{\"ok\":true,\"kept\":{}}}", kept),
        Err(e) => err(&e.to_string()),
    }
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
    let fetched = if omalook::accounts::provider_of(&account) == "gmail" {
        rt.block_on(omalook::providers::GmailProvider::new(&account, None).fetch_message_body(id))
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

/// The Graph provider for message `id`, or a user-facing reason it cannot be had.
fn graph_for_message(db: &Database, id: &str) -> Result<GraphEmailProvider, String> {
    let account = match db.message_account(id) {
        Ok(Some(a)) => a,
        _ => return Err("unknown message".into()),
    };
    if omalook::accounts::provider_of(&account) == "gmail" {
        return Err("attachments are not supported for Gmail accounts yet".into());
    }
    Ok(GraphEmailProvider::new(AuthManager::for_account(&account)))
}

/// The attachment list of a message: cached, else fetched (and cached). The HTML body is
/// consulted so pictures embedded in it are not listed as attachments.
fn attachment_list(config_dir: &PathBuf, db: &Database, id: &str) -> Result<Vec<omalook::attachments::Attachment>, String> {
    if let Ok(Some(list)) = db.cached_attachments(id) {
        return Ok(list);
    }
    let provider = graph_for_message(db, id)?;
    if !matches!(db.cached_body(id), Ok(Some(_))) {
        let _ = message_body_json(config_dir, id); // fetches and caches the body
    }
    let body = match db.cached_body(id) { Ok(Some((t, c))) if t == "html" => Some(c), _ => None };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| format!("runtime: {}", e))?;
    match rt.block_on(provider.fetch_attachments(id, body.as_deref())) {
        Ok(list) => {
            if let Err(e) = db.store_attachments(id, &list) {
                warn!("Could not cache attachments of {}: {}", id, e);
            }
            Ok(list)
        }
        Err(e) => {
            warn!("Attachment list failed for {}: {}", id, e);
            Err(if e.to_string().contains("404") { "this message no longer exists on the server".into() } else { "could not fetch the attachment list".into() })
        }
    }
}

/// JSON for GET /messages/attachments?id=ID: `{"attachments":[...],"dir":"~/Downloads"}` or `{"error":"..."}`.
fn message_attachments_json(config_dir: &PathBuf, id: &str) -> String {
    let err = |m: &str| format!("{{\"error\":{}}}", serde_json::to_string(m).unwrap());
    if id.is_empty() { return err("missing id"); }
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    match attachment_list(config_dir, &db, id) {
        Ok(list) => {
            let dir = omalook::attachments::download_dir(&config_dir.join("settings.toml"));
            serde_json::json!({ "attachments": list, "dir": omalook::attachments::display_dir(&dir) }).to_string()
        }
        Err(e) => err(&e),
    }
}

/// JSON for POST /messages/attachments/save?id=ID&att=ATT (one) or no `att` (every attachment
/// that is not inline): `{"saved":[{"id","name","path"}],"failed":[{"id","name","error"}],"dir":"~/Downloads"}`.
/// One failure never stops the others.
fn save_attachments_json(config_dir: &PathBuf, id: &str, att: Option<&str>) -> String {
    let err = |m: &str| format!("{{\"error\":{}}}", serde_json::to_string(m).unwrap());
    if id.is_empty() { return err("missing id"); }
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let list = match attachment_list(config_dir, &db, id) { Ok(l) => l, Err(e) => return err(&e) };
    let wanted: Vec<_> = match att {
        Some(a) => list.iter().filter(|x| x.id == a).cloned().collect(),
        None => list.iter().filter(|x| !x.inline).cloned().collect(),
    };
    if wanted.is_empty() { return err("no such attachment"); }
    let provider = match graph_for_message(&db, id) { Ok(p) => p, Err(e) => return err(&e) };
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => return err(&format!("runtime: {}", e)),
    };
    let dir = omalook::attachments::download_dir(&config_dir.join("settings.toml"));
    let (mut saved, mut failed) = (Vec::new(), Vec::new());
    for a in &wanted {
        match rt.block_on(provider.download_attachment(id, a, &dir)) {
            Ok(path) => saved.push(serde_json::json!({ "id": a.id, "name": a.name, "path": path.display().to_string() })),
            Err(e) => {
                warn!("Attachment save failed ({}): {}", a.name, e);
                failed.push(serde_json::json!({ "id": a.id, "name": a.name, "error": e.to_string() }));
            }
        }
    }
    serde_json::json!({ "saved": saved, "failed": failed, "dir": omalook::attachments::display_dir(&dir) }).to_string()
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

/// POST /drafts/save — the compose pane's draft (`OutgoingMessage` JSON). `draft_id` empty = a new
/// draft. Writes the local copy (`draft_sync::save_local`); the draft worker pushes it to Drafts.
/// Answers `{ok, id, state, attachments, missing}` or `{ok:false, error}`.
fn drafts_save_response(config_dir: &PathBuf, stream: &mut std::net::TcpStream, initial: &[u8], first_line: &str) -> String {
    use omalook::compose::{OutgoingMessage, MAX_BODY_BYTES};
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let raw = match read_post_body(stream, initial, MAX_BODY_BYTES + 64 * 1024) {
        Ok(b) => b,
        Err(e) => return err(&e),
    };
    let mut msg: OutgoingMessage = match serde_json::from_str(&raw) {
        Ok(m) => m,
        Err(e) => return err(&format!("unreadable draft: {}", e)),
    };
    if msg.account_id.is_empty() {
        return err("the draft has no account");
    }
    let state = query_param(first_line, "state").unwrap_or_else(|| "saved".to_string());
    let db_path = config_dir.join("messages.db");
    let db = match Database::open_for_account(db_path.to_str().unwrap_or("messages.db"), &msg.account_id) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    match omalook::draft_sync::save_local(&db, &omalook::draft_sync::default_root(), &mut msg, &state) {
        Ok(id) => serde_json::json!({
            "ok": true, "id": id, "state": state, "attachments": msg.attachments,
            "missing": omalook::attach_spool::missing_copies(&msg.attachments),
        }).to_string(),
        Err(e) => err(&e),
    }
}

/// GET /drafts, GET /drafts/get?id=, POST /drafts/discard?id= (see the route comment in the loop).
/// Discard deletes the provider's copy first and the local row only after that succeeded.
fn drafts_route(config_dir: &PathBuf, first_line: &str) -> String {
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let db_str = config_dir.join("messages.db").to_str().unwrap_or("messages.db").to_string();
    let db = match Database::open(&db_str) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    if first_line.contains("POST /drafts/discard") {
        let id = match query_param(first_line, "id") {
            Some(i) => i,
            None => return err("missing id"),
        };
        let row = match db.draft_get(&id) {
            Ok(Some(r)) => r,
            Ok(None) => return serde_json::json!({ "ok": true, "discarded": false }).to_string(),
            Err(e) => return err(&e.to_string()),
        };
        let account_db = match Database::open_for_account(&db_str, &row.account_id) {
            Ok(d) => d,
            Err(e) => return err(&format!("database: {}", e)),
        };
        let provider: Arc<dyn omalook::providers::EmailProvider> = if omalook::accounts::provider_of(&row.account_id) == "gmail" {
            Arc::new(omalook::providers::gmail::GmailProvider::new(&row.account_id, Some(config_dir.join("messages.db"))))
        } else {
            Arc::new(GraphEmailProvider::new(AuthManager::for_account(&row.account_id)))
        };
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => return err(&format!("runtime: {}", e)),
        };
        let spool = omalook::draft_sync::default_root();
        return match rt.block_on(omalook::draft_sync::discard(&account_db, provider.as_ref(), &spool, &id)) {
            Ok(()) => serde_json::json!({ "ok": true, "discarded": true, "id": id }).to_string(),
            Err(e) => err(&e.to_string()),
        };
    }
    if first_line.contains("GET /drafts/get") {
        let id = match query_param(first_line, "id") {
            Some(i) => i,
            None => return err("missing id"),
        };
        return match db.draft_get(&id) {
            Ok(Some(r)) => serde_json::json!({ "ok": true, "draft": omalook::draft_sync::draft_json(&r) }).to_string(),
            Ok(None) => err("no such draft"),
            Err(e) => err(&e.to_string()),
        };
    }
    let rows = match query_param(first_line, "account") {
        Some(a) => db.draft_list(&a),
        None => db.draft_list_all(),
    };
    match rows {
        Ok(rows) => serde_json::json!({
            "ok": true,
            "drafts": rows.iter().map(omalook::draft_sync::draft_json).collect::<Vec<_>>(),
        }).to_string(),
        Err(e) => err(&e.to_string()),
    }
}

/// The send-as identities of an account (OL-051): cached for six hours, read from the provider when older (or `force`),
/// the stale copy when the provider cannot be reached, and just the account address when nothing was ever read.
fn identities_for(db: &Database, account: &str, force: bool) -> Vec<omalook::identities::Identity> {
    use omalook::identities::{ensure_primary, Identity};
    const TTL_SECS: i64 = 6 * 3600;
    let email = db.account_email(account).ok().flatten().unwrap_or_default();
    let cached = db.identities_get(account).ok().flatten();
    if let Some((list, at)) = &cached {
        if !force && chrono::Utc::now().timestamp() - at < TTL_SECS {
            return list.clone();
        }
    }
    let fetched: Option<Result<Vec<Identity>, String>> = tokio::runtime::Builder::new_current_thread().enable_all().build().ok().map(|rt| {
        match omalook::accounts::provider_of(account) {
            "gmail" => rt.block_on(omalook::providers::gmail::GmailProvider::new(account, None).fetch_identities(&email)).map_err(|e| e.to_string()),
            "exchange" => rt.block_on(GraphEmailProvider::new(AuthManager::for_account(account)).fetch_identities(&email)).map_err(|e| e.to_string()),
            _ => Ok(Vec::new()),
        }
    });
    match fetched {
        Some(Ok(list)) => {
            let list = ensure_primary(list, &email);
            if let Err(e) = db.identities_replace(account, &list) {
                warn!("Could not cache identities of {}: {}", account, e);
            }
            list
        }
        other => {
            if let Some(Err(e)) = other {
                warn!("Identities of {} not read: {}", account, e);
            }
            match cached {
                Some((list, _)) => list,
                None => ensure_primary(Vec::new(), &email),
            }
        }
    }
}

/// GET /compose/identities?account=ID[&refresh=1] -> `{identities:[{email,name,primary}], default}`. POST
/// /compose/identity_default?account=ID&email=ADDR remembers the account's default From (empty email clears it).
fn identities_response(config_dir: &PathBuf, first_line: &str) -> String {
    let err = |m: &str| serde_json::json!({ "error": m }).to_string();
    let account = query_param(first_line, "account").unwrap_or_default();
    if account.is_empty() {
        return err("missing account");
    }
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let email = db.account_email(&account).ok().flatten().unwrap_or_default();
    let list = identities_for(&db, &account, query_param(first_line, "refresh").as_deref() == Some("1"));
    if first_line.contains("POST /compose/identity_default") {
        let want = query_param(first_line, "email").unwrap_or_default();
        let chosen = match omalook::identities::choose_from(&list, &email, &want) {
            Ok(c) => c,
            Err(e) => return err(&e),
        };
        // The account address itself is stored as "no preference", like the provider default.
        return match db.identity_pref_set(&account, if chosen.is_empty() && !want.trim().is_empty() { &email } else { &chosen }) {
            Ok(()) => serde_json::json!({ "ok": true }).to_string(),
            Err(e) => err(&e.to_string()),
        };
    }
    let pref = db.identity_pref_get(&account).ok().flatten();
    serde_json::json!({
        "identities": list.iter().map(|i| serde_json::json!({ "email": i.email, "name": i.name, "primary": i.primary })).collect::<Vec<_>>(),
        "default": omalook::identities::default_identity(&list, pref.as_deref(), &email),
    }).to_string()
}

/// POST /compose/send — queue a composed message. The body is the JSON the compose pane builds
/// (`omalook::compose::OutgoingMessage`). It is stored in the outbox to go out after the
/// `[mail] send_delay_secs` setting (0 = now) and the account's outbox worker is woken, so the
/// send never waits for a poll. Answers `{ok, id, delay_secs}` or `{ok:false, error}`.
fn compose_send_response(config_dir: &PathBuf, stream: &mut std::net::TcpStream, initial: &[u8]) -> String {
    use omalook::compose::{OutgoingMessage, MAX_BODY_BYTES};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();

    let raw = match read_post_body(stream, initial, MAX_BODY_BYTES + 64 * 1024) {
        Ok(b) => b,
        Err(e) => return err(&e),
    };
    let mut msg: OutgoingMessage = match serde_json::from_str(&raw) {
        Ok(m) => m,
        Err(e) => return err(&format!("unreadable message: {}", e)),
    };
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    // Distribution lists become their members here, at queue time: the chip in the compose pane
    // stays one chip, and what is stored in the outbox is the final address list.
    let lists = db.lists_all().unwrap_or_default();
    let expanded = match msg.expand_lists(&|id| {
        lists.iter().find(|l| l.id == id).map(|l| (l.name.clone(), l.members.iter().map(|m| omalook::compose::Address { email: m.email.clone(), name: m.name.clone(), list: 0 }).collect()))
    }) {
        Ok(r) => r,
        Err(e) => return err(&e),
    };
    // Pictures the body does not show go, and so does any `<img>` that would not load for the
    // recipient (a local path); see `settle_inline`.
    msg.settle_inline();
    if let Err(e) = msg.validate() {
        return err(&e);
    }
    // Sent as an alias (OL-051): it must be one of the account's identities; the account's own address needs no override.
    if !msg.from.trim().is_empty() {
        let acct_email = db.account_email(&msg.account_id).ok().flatten().unwrap_or_default();
        match omalook::identities::choose_from(&identities_for(&db, &msg.account_id, false), &acct_email, &msg.from) {
            Ok(f) => msg.from = f,
            Err(e) => return err(&e),
        }
    }
    // A signed-out account has no worker, so the message would sit unsent without anyone knowing.
    if !SCHEDULER.get().map(|s| s.is_running(&msg.account_id)).unwrap_or(false) {
        return err("that account is not signed in, so it cannot send");
    }
    let delay = omalook::settings::read_send_delay(&config_dir.join("settings.toml"));
    let now = Database::now_ms();
    let id = format!("ob-{:x}-{:03x}", now, COUNTER.fetch_add(1, Ordering::Relaxed) & 0xfff);
    // Copy the attached files aside so the message goes out with exactly these bytes, whatever
    // happens to the originals during the undo window (docs/PLAN-compose-attachments.md 3.2).
    let spool_root = omalook::attach_spool::default_root();
    let mut msg = msg;
    msg.attachments = match omalook::attach_spool::snapshot(&spool_root, &id, &msg.attachments) {
        Ok(a) => a,
        Err(e) => return err(&e),
    };
    let payload = match serde_json::to_string(&msg) {
        Ok(p) => p,
        Err(e) => {
            omalook::attach_spool::remove(&spool_root, &id);
            return err(&e.to_string());
        }
    };
    if let Err(e) = db.outbox_enqueue(&id, &msg.account_id, &payload, now + delay as i64 * 1000) {
        omalook::attach_spool::remove(&spool_root, &id);
        return err(&format!("could not queue the message: {}", e));
    }
    omalook::sync_state::request_outbox_run();
    info!("Compose: queued {} ({} → {} recipient(s)), sends in {}s", id, msg.account_id, msg.to.len() + msg.cc.len() + msg.bcc.len(), delay);
    serde_json::json!({
        "ok": true, "id": id, "delay_secs": delay,
        "recipients": msg.to.len() + msg.cc.len() + msg.bcc.len(),
        "lists": expanded.iter().map(|(n, c)| serde_json::json!({ "name": n, "count": c })).collect::<Vec<_>>(),
    }).to_string()
}

/// GET /compose/originals?id=MESSAGE — the files attached to the message being forwarded, for the
/// forward's chips: `{"originals":[{"id","name","size"}]}` (ids: Graph attachment id, Gmail MIME part
/// id) or `{"error"}`. Pictures embedded in the HTML are left out: they were never the user's to pick.
fn compose_originals_json(config_dir: &PathBuf, id: &str) -> String {
    let err = |m: &str| serde_json::json!({ "error": m }).to_string();
    if id.is_empty() { return err("missing id"); }
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let account = match db.message_account(id) { Ok(Some(a)) => a, _ => return err("unknown message") };
    let list: Result<Vec<(String, String, i64)>, String> = if omalook::accounts::provider_of(&account) == "gmail" {
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() { Ok(rt) => rt, Err(e) => return err(&format!("runtime: {}", e)) };
        let p = omalook::providers::gmail::GmailProvider::new(&account, None);
        rt.block_on(p.list_attachments(id)).map(|v| v.into_iter().map(|(i, n, s)| (i, n, s as i64)).collect()).map_err(|e| { warn!("Forward: attachment list failed for {}: {}", id, e); "could not read the original's attachments".to_string() })
    } else {
        attachment_list(config_dir, &db, id).map(|v| v.into_iter().filter(|a| !a.inline && a.saveable()).map(|a| (a.id, a.name, a.size)).collect())
    };
    match list {
        Ok(v) => serde_json::json!({ "originals": v.into_iter().map(|(i, n, s)| serde_json::json!({ "id": i, "name": n, "size": s })).collect::<Vec<_>>() }).to_string(),
        Err(e) => err(&e),
    }
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
    if cancelled {
        // The UI restores its own chips (original paths); the copies are no longer needed.
        omalook::attach_spool::remove(&omalook::attach_spool::default_root(), &id);
    }
    match db.outbox_status(&id) {
        Ok(Some((state, error, attempts))) => serde_json::json!({
            "ok": true, "cancelled": cancelled, "state": state, "error": error, "attempts": attempts
        }).to_string(),
        Ok(None) => serde_json::json!({ "ok": false, "cancelled": false, "state": "unknown", "error": "no such message" }).to_string(),
        Err(e) => err(&e.to_string()),
    }
}

/// A write to the People view's address book: opens the account's database, runs `f` on it, wakes the
/// outbox worker. `account` is the account the write belongs to.
fn contacts_write_call(config_dir: &PathBuf, account: &str, f: impl FnOnce(&Database) -> serde_json::Value) -> String {
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    if account.is_empty() {
        return err("no contact account selected");
    }
    if !SCHEDULER.get().map(|s| s.is_running(account)).unwrap_or(false) {
        return err("that account is not signed in, so it cannot save contacts");
    }
    let path = config_dir.join("messages.db");
    let db = match Database::open_for_account(path.to_str().unwrap_or("messages.db"), account) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let out = f(&db);
    omalook::sync_state::request_contact_outbox_run();
    out.to_string()
}

/// The account a contact belongs to (its row says so).
fn account_of_contact(config_dir: &PathBuf, id: &str) -> String {
    let path = config_dir.join("messages.db");
    Database::open(path.to_str().unwrap_or("messages.db"))
        .ok()
        .and_then(|db| db.get_contact_record(id).ok().flatten())
        .map(|r| r.account_id)
        .unwrap_or_default()
}

/// Create, rename or delete a contact folder (done at once, not queued: folders are rare and the answer matters).
fn contacts_folder_call(config_dir: &PathBuf, first_line: &str, req: &serde_json::Value) -> String {
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let account = req["account_id"].as_str().unwrap_or("").to_string();
    if account.is_empty() { return err("no contact account selected"); }
    if !SCHEDULER.get().map(|s| s.is_running(&account)).unwrap_or(false) { return err("that account is not signed in"); }
    let name = req["name"].as_str().unwrap_or("").trim().to_string();
    let id = req["id"].as_str().unwrap_or("").to_string();
    let parent = req["parent_id"].as_str().filter(|p| !p.is_empty()).map(String::from);
    let path = config_dir.join("messages.db");
    let db = match Database::open_for_account(path.to_str().unwrap_or("messages.db"), &account) { Ok(d) => d, Err(e) => return err(&format!("database: {}", e)) };
    let provider = omalook::scheduler::contacts_provider_for(&account);
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() { Ok(r) => r, Err(e) => return err(&e.to_string()) };
    let out: Result<serde_json::Value, omalook::errors::OmarchyError> = rt.block_on(async {
        if first_line.contains("/contacts/folder/create") {
            if name.is_empty() { return Ok(serde_json::json!({ "ok": false, "error": "the folder needs a name" })); }
            let f = provider.create_folder(parent.as_deref(), &name).await?;
            let _ = db.upsert_contact_folder(&f);
            Ok(serde_json::json!({ "ok": true, "id": f.id }))
        } else if first_line.contains("/contacts/folder/rename") {
            if name.is_empty() || id.is_empty() { return Ok(serde_json::json!({ "ok": false, "error": "the folder needs a name" })); }
            provider.rename_folder(&id, &name).await?;
            let old = db.list_contact_folders_ext(Some(&account)).unwrap_or_default().into_iter().find(|f| f["id"] == id.as_str());
            let _ = db.upsert_contact_folder(&omalook::models::ContactFolder { id: id.clone(), display_name: name.clone(), parent_folder_id: old.and_then(|f| f["parent_id"].as_str().map(String::from)) });
            Ok(serde_json::json!({ "ok": true }))
        } else if first_line.contains("/contacts/folder/delete") {
            if id.is_empty() { return Ok(serde_json::json!({ "ok": false, "error": "no folder given" })); }
            provider.delete_folder(&id).await?;
            let _ = db.delete_contact_folder(&id);
            Ok(serde_json::json!({ "ok": true }))
        } else {
            Ok(serde_json::json!({ "ok": false, "error": "unknown folder action" }))
        }
    });
    match out { Ok(v) => v.to_string(), Err(e) => err(&omalook::contacts_outbox::short_error(&e)) }
}

fn photo_cache_file(id: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    let base = std::env::var("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache"));
    base.join("omalook").join("contact-photos").join(format!("{:016x}.jpg", h.finish()))
}

/// `GET /contacts/photo?id=`: the path of the contact's photo on disk, fetched in the background the first time.
fn contact_photo_path(config_dir: &PathBuf, id: &str) -> String {
    let file = photo_cache_file(id);
    let none_marker = file.with_extension("none");
    let fresh = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|a| a < std::time::Duration::from_secs(24 * 3600)).unwrap_or(false);
    if file.exists() && fresh(&file) { return serde_json::json!({ "ok": true, "path": file.to_string_lossy() }).to_string(); }
    if none_marker.exists() && fresh(&none_marker) { return serde_json::json!({ "ok": true, "path": "" }).to_string(); }
    let account = account_of_contact(config_dir, id);
    if account.is_empty() || id.starts_with("local-") || !SCHEDULER.get().map(|s| s.is_running(&account)).unwrap_or(false) {
        return serde_json::json!({ "ok": true, "path": "" }).to_string();
    }
    let id = id.to_string();
    std::thread::spawn(move || {
        let provider = omalook::scheduler::contacts_provider_for(&account);
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
        if let Some(dir) = file.parent() { let _ = std::fs::create_dir_all(dir); }
        match rt.block_on(provider.get_photo(&id)) {
            Ok(Some(bytes)) if !bytes.is_empty() => { let _ = std::fs::write(&file, bytes); let _ = std::fs::remove_file(&none_marker); }
            Ok(_) => { let _ = std::fs::write(&none_marker, b""); }
            Err(_) => {}
        }
    });
    serde_json::json!({ "ok": true, "path": "", "pending": true }).to_string()
}

/// `POST /contacts/photo/set`: a JPEG file becomes the contact's photo (at once, not queued).
fn contact_photo_set(config_dir: &PathBuf, id: &str, path: &str) -> String {
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let expanded = if let Some(rest) = path.trim().strip_prefix("~/") { std::env::var("HOME").map(|h| format!("{h}/{rest}")).unwrap_or_default() } else { path.trim().to_string() };
    let bytes = match std::fs::read(&expanded) { Ok(b) => b, Err(_) => return err("cannot read that file") };
    if bytes.len() > 4 * 1024 * 1024 { return err("the picture is larger than 4 MB"); }
    if !(bytes.len() > 3 && bytes[0] == 0xFF && bytes[1] == 0xD8) { return err("only JPEG pictures can be set (a .jpg file)"); }
    let account = account_of_contact(config_dir, id);
    if account.is_empty() || id.starts_with("local-") { return err("that contact is not saved yet"); }
    if !SCHEDULER.get().map(|s| s.is_running(&account)).unwrap_or(false) { return err("that account is not signed in"); }
    let provider = omalook::scheduler::contacts_provider_for(&account);
    let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return err("could not start") };
    match rt.block_on(provider.set_photo(id, &bytes)) {
        Ok(_) => {
            let f = photo_cache_file(id);
            let _ = std::fs::write(&f, &bytes);
            let _ = std::fs::remove_file(f.with_extension("none"));
            serde_json::json!({ "ok": true, "path": f.to_string_lossy() }).to_string()
        }
        Err(e) => err(&omalook::contacts_outbox::short_error(&e)),
    }
}

/// The `/contacts/*` routes except `POST /contacts/favorite` (kept where it was). None = not one of ours.
/// GET /contacts/get?id=, GET /contacts/folders?account=; POST /contacts/save, /delete, /undo, /retry, /discard.
fn contacts_route(config_dir: &PathBuf, stream: &mut std::net::TcpStream, initial: &[u8], first_line: &str) -> Option<String> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let seq = || SEQ.fetch_add(1, Ordering::Relaxed) & 0xfff;
    let post = first_line.starts_with("POST ");
    if post && first_line.contains("/contacts/merge") {
        let body = match read_post_body(stream, initial, 1024 * 1024) { Ok(b) => b, Err(e) => return Some(serde_json::json!({ "ok": false, "error": e }).to_string()) };
        let req: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        let ids: Vec<String> = req["ids"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        let account = ids.first().map(|i| account_of_contact(config_dir, i)).unwrap_or_default();
        return Some(contacts_write_call(config_dir, &account, |db| omalook::contacts_write::merge(db, &ids, seq())));
    }
    if post && first_line.contains("/contacts/quick") {
        let body = match read_post_body(stream, initial, 64 * 1024) { Ok(b) => b, Err(e) => return Some(serde_json::json!({ "ok": false, "error": e }).to_string()) };
        let req: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        let account = req["account_id"].as_str().unwrap_or("").to_string();
        let (name, address) = (req["name"].as_str().unwrap_or("").to_string(), req["address"].as_str().unwrap_or("").to_string());
        return Some(contacts_write_call(config_dir, &account, |db| omalook::contacts_api::quick_add(db, &name, &address, seq())));
    }
    if post && first_line.contains("/contacts/export") {
        let body = match read_post_body(stream, initial, 1024 * 1024) { Ok(b) => b, Err(e) => return Some(serde_json::json!({ "ok": false, "error": e }).to_string()) };
        let req: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        let ids: Vec<String> = req["ids"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        let format = req["format"].as_str().unwrap_or("vcf").to_string();
        let dir = omalook::attachments::download_dir(&config_dir.join("settings.toml"));
        let path = config_dir.join("messages.db");
        return Some(match Database::open(path.to_str().unwrap_or("messages.db")) {
            Ok(db) => omalook::contacts_api::export(&db, &ids, &format, &dir).to_string(),
            Err(e) => serde_json::json!({ "ok": false, "error": format!("database: {}", e) }).to_string(),
        });
    }
    if post && first_line.contains("/contacts/import") {
        let body = match read_post_body(stream, initial, 64 * 1024) { Ok(b) => b, Err(e) => return Some(serde_json::json!({ "ok": false, "error": e }).to_string()) };
        let req: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        let raw_path = req["path"].as_str().unwrap_or("").trim().to_string();
        let expanded = if let Some(rest) = raw_path.strip_prefix("~/") { std::env::var("HOME").map(|h| format!("{h}/{rest}")).unwrap_or(raw_path.clone()) } else { raw_path.clone() };
        let (account, folder) = (req["account_id"].as_str().unwrap_or("").to_string(), req["folder_id"].as_str().unwrap_or("").to_string());
        return Some(contacts_write_call(config_dir, &account, |db| {
            let folder = if folder.is_empty() { db.default_contact_folder().ok().flatten().unwrap_or_default() } else { folder.clone() };
            if folder.is_empty() { return serde_json::json!({ "ok": false, "error": "this account's contact folder is not known yet" }); }
            omalook::contacts_api::import(db, std::path::Path::new(&expanded), &folder, seq())
        }));
    }
    if first_line.contains("GET /contacts/photo") {
        let id = query_param(first_line, "id").unwrap_or_default();
        return Some(contact_photo_path(config_dir, &id));
    }
    if post && first_line.contains("/contacts/photo/set") {
        let body = match read_post_body(stream, initial, 64 * 1024) { Ok(b) => b, Err(e) => return Some(serde_json::json!({ "ok": false, "error": e }).to_string()) };
        let req: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        return Some(contact_photo_set(config_dir, req["id"].as_str().unwrap_or(""), req["path"].as_str().unwrap_or("")));
    }
    if post && first_line.contains("/contacts/save") {
        let body = match read_post_body(stream, initial, 1024 * 1024) { Ok(b) => b, Err(e) => return Some(serde_json::json!({ "ok": false, "error": e }).to_string()) };
        let req: serde_json::Value = match serde_json::from_str(&body) { Ok(v) => v, Err(e) => return Some(serde_json::json!({ "ok": false, "error": format!("unreadable contact: {}", e) }).to_string()) };
        let id = req["id"].as_str().unwrap_or("");
        let account = if id.is_empty() { req["account_id"].as_str().unwrap_or("").to_string() } else { account_of_contact(config_dir, id) };
        return Some(contacts_write_call(config_dir, &account, |db| omalook::contacts_write::save(db, &req, seq())));
    }
    if post && (first_line.contains("/contacts/categories") || first_line.contains("/contacts/move")) {
        let body = match read_post_body(stream, initial, 1024 * 1024) { Ok(b) => b, Err(e) => return Some(serde_json::json!({ "ok": false, "error": e }).to_string()) };
        let req: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        let list = |k: &str| -> Vec<String> { req[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default() };
        let ids = list("ids");
        let moving = first_line.contains("/contacts/move");
        let folder = req["folder_id"].as_str().unwrap_or("").to_string();
        let target_account = if moving {
            let path = config_dir.join("messages.db");
            Database::open(path.to_str().unwrap_or("messages.db")).ok().and_then(|d| d.contact_folder_account(&folder).ok().flatten()).unwrap_or_default()
        } else { String::new() };
        let mut by: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        let mut elsewhere = 0;
        for i in ids {
            let a = account_of_contact(config_dir, &i);
            if moving && a != target_account { elsewhere += 1; continue; }
            by.entry(a).or_default().push(i);
        }
        let (add, remove) = (list("add"), list("remove"));
        let mut changed = 0;
        let mut last_err = String::new();
        for (acct, ids) in by {
            let out = contacts_write_call(config_dir, &acct, |db| if moving { omalook::contacts_write::move_contacts(db, &ids, &folder, seq()) } else { omalook::contacts_write::set_categories(db, &ids, &add, &remove, seq()) });
            let r: serde_json::Value = serde_json::from_str(&out).unwrap_or_default();
            if r["ok"] == true { changed += r["moved"].as_i64().or(r["changed"].as_i64()).unwrap_or(0); } else { last_err = r["error"].as_str().unwrap_or("").to_string(); }
        }
        return Some(if changed == 0 && !last_err.is_empty() { serde_json::json!({ "ok": false, "error": last_err }).to_string() }
                    else { serde_json::json!({ "ok": true, "changed": changed, "skipped_other_account": elsewhere }).to_string() });
    }
    if post && first_line.contains("/contacts/folder/") {
        let body = match read_post_body(stream, initial, 64 * 1024) { Ok(b) => b, Err(e) => return Some(serde_json::json!({ "ok": false, "error": e }).to_string()) };
        let req: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        return Some(contacts_folder_call(config_dir, first_line, &req));
    }
    if post && first_line.contains("/contacts/delete") {
        let body = match read_post_body(stream, initial, 1024 * 1024) { Ok(b) => b, Err(e) => return Some(serde_json::json!({ "ok": false, "error": e }).to_string()) };
        let req: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        let permanent = req["permanent"].as_bool().unwrap_or(false);
        let ids: Vec<String> = req["ids"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        // one account at a time: group by the account each contact belongs to
        let mut by: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for i in ids { by.entry(account_of_contact(config_dir, &i)).or_default().push(i); }
        let mut deleted = 0;
        let mut last_err = String::new();
        for (acct, list) in by {
            let r: serde_json::Value = serde_json::from_str(&contacts_write_call(config_dir, &acct, |db| omalook::contacts_write::delete_ext(db, &list, seq(), permanent))).unwrap_or_default();
            if r["ok"] == true { deleted += r["deleted"].as_i64().unwrap_or(0); } else { last_err = r["error"].as_str().unwrap_or("").to_string(); }
        }
        return Some(if deleted == 0 && !last_err.is_empty() { serde_json::json!({ "ok": false, "error": last_err }).to_string() } else { serde_json::json!({ "ok": true, "deleted": deleted }).to_string() });
    }
    if post && first_line.contains("/contacts/undo") {
        // the account whose newest write is the most recent
        let path = config_dir.join("messages.db");
        let mut best: Option<(i64, String)> = None;
        if let Ok(db) = Database::open(path.to_str().unwrap_or("messages.db")) {
            for (t, a) in db.contact_outbox_newest_per_account().unwrap_or_default() {
                if best.as_ref().map(|b| t > b.0).unwrap_or(true) { best = Some((t, a)); }
            }
        }
        return Some(match best {
            Some((_, a)) => contacts_write_call(config_dir, &a, |db| omalook::contacts_write::undo(db, seq())),
            None => serde_json::json!({ "ok": false, "error": "nothing to undo" }).to_string(),
        });
    }
    if post && (first_line.contains("/contacts/retry") || first_line.contains("/contacts/discard")) {
        let contact = query_param(first_line, "id").unwrap_or_default();
        let account = account_of_contact(config_dir, &contact);
        let retry = first_line.contains("/contacts/retry");
        return Some(contacts_write_call(config_dir, &account, |db| {
            match db.contact_last_error(&contact) {
                Ok(Some((op, _, _))) => {
                    if retry { let _ = db.contact_outbox_retry(&op); } else { let _ = db.contact_outbox_discard(&op); }
                    serde_json::json!({ "ok": true })
                }
                _ => serde_json::json!({ "ok": false, "error": "nothing to retry" }),
            }
        }));
    }
    let _ = (&stream, &initial);
    let db_path = config_dir.join("messages.db");
    let open = || Database::open(db_path.to_str().unwrap_or("messages.db"));
    let err = |m: String| serde_json::json!({ "ok": false, "error": m }).to_string();
    if first_line.contains("GET /contacts/directory") {
        let (account, q) = (query_param(first_line, "account").unwrap_or_default(), query_param(first_line, "q").unwrap_or_default());
        if account.is_empty() { return Some(err("no account selected".into())); }
        if !SCHEDULER.get().map(|s| s.is_running(&account)).unwrap_or(false) { return Some(err("that account is not signed in".into())); }
        let provider = omalook::scheduler::contacts_provider_for(&account);
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return Some(err("could not start".into())) };
        return Some(match rt.block_on(provider.directory_search(&q)) {
            Ok(list) => serde_json::json!({ "ok": true, "people": list }).to_string(),
            Err(e) => err(omalook::contacts_outbox::short_error(&e)),
        });
    }
    if first_line.contains("GET /contacts/get") {
        let id = query_param(first_line, "id").unwrap_or_default();
        let live = query_param(first_line, "live").as_deref() == Some("1");
        return Some(match open() {
            Ok(db) if live => {
                // the editor wants the extra MAPI fields too: one request to the server, merged into the answer
                let mut d = omalook::contacts_api::detail(&db, &id);
                let account = d["account_id"].as_str().unwrap_or("").to_string();
                if d["ok"] == true && !account.is_empty() && !id.starts_with("local-") && SCHEDULER.get().map(|s| s.is_running(&account)).unwrap_or(false) {
                    let provider = omalook::scheduler::contacts_provider_for(&account);
                    if let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() {
                        if let Ok(list) = rt.block_on(provider.get_extended(&id, &omalook::contacts_write::ext_ids())) {
                            d["contact"]["singleValueExtendedProperties"] = serde_json::Value::Array(list.into_iter().map(|(i, v)| serde_json::json!({ "id": i, "value": v })).collect());
                        }
                    }
                }
                d.to_string()
            }
            Ok(db) => omalook::contacts_api::detail(&db, &id).to_string(),
            Err(e) => err(format!("database: {}", e)),
        });
    }
    if first_line.contains("GET /contacts/folders") {
        let account = query_param(first_line, "account");
        return Some(match open() {
            Ok(db) => omalook::contacts_api::folders(&db, account.as_deref()).to_string(),
            Err(e) => err(format!("database: {}", e)),
        });
    }
    None
}

/// POST /calendar/event — queue a new calendar event from the editor (docs/PLAN-calendar-events.md
/// sections 6 and 10). Body: the editor's JSON (`EventDraft::from_request`) plus `account_id` and an
/// optional `transaction_id` (the editor makes one per draft, so a repeated press is one event).
/// The write waits `[mail] send_delay_secs` (the undo window) and the account's calendar outbox worker
/// is woken. `?dry_run=1` answers the exact Graph body and queues nothing. Answers
/// `{ok, id, delay_secs, attendees}` or `{ok:false, error}`.
fn calendar_event_response(config_dir: &PathBuf, stream: &mut std::net::TcpStream, initial: &[u8], first_line: &str) -> String {
    use omalook::calendar_write::EventDraft;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();

    let raw = match read_post_body(stream, initial, 256 * 1024) {
        Ok(b) => b,
        Err(e) => return err(&e),
    };
    let mut req: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return err(&format!("unreadable event: {}", e)),
    };
    let account_id = req["account_id"].as_str().unwrap_or("").to_string();
    if account_id.is_empty() {
        return err("no calendar account selected");
    }
    if req["time_zone"].as_str().unwrap_or("").is_empty() {
        req["time_zone"] = serde_json::json!(omalook::providers::calendar::local_time_zone());
    }
    let now = Database::now_ms();
    let tx = match req["transaction_id"].as_str().filter(|t| !t.is_empty()) {
        Some(t) => t.to_string(),
        None => format!("ol-{:x}-{:03x}", now, COUNTER.fetch_add(1, Ordering::Relaxed) & 0xfff),
    };
    let draft = match EventDraft::from_request(&req, &tx) {
        Ok(d) => d,
        Err(e) => return err(&e),
    };
    let graph_body = match draft.to_graph_json() {
        Ok(b) => b,
        Err(e) => return err(&e.to_string()),
    };
    if query_param(first_line, "dry_run").as_deref() == Some("1") {
        return serde_json::json!({ "ok": true, "dry_run": true, "graph": graph_body }).to_string();
    }
    // A signed-out account has no worker, so the event would sit unsaved without anyone knowing.
    if !SCHEDULER.get().map(|s| s.is_running(&account_id)).unwrap_or(false) {
        return err("that account is not signed in, so it cannot save events");
    }
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let delay = omalook::settings::read_send_delay(&config_dir.join("settings.toml"));
    let id = format!("co-{:x}-{:03x}", now, COUNTER.fetch_add(1, Ordering::Relaxed) & 0xfff);
    let payload = req.to_string();
    if let Err(e) = db.cal_outbox_enqueue(&id, &account_id, "create", &payload, draft.attendees.len(), &tx, now + delay as i64 * 1000) {
        return err(&format!("could not queue the event: {}", e));
    }
    omalook::sync_state::request_cal_outbox_run();
    info!("Calendar: queued {} ({}, {} attendee(s)), saves in {}s", id, account_id, draft.attendees.len(), delay);
    serde_json::json!({ "ok": true, "id": id, "delay_secs": delay, "attendees": draft.attendees.len() }).to_string()
}

/// PATCH /calendar/event?id=ID&scope=occurrence|series[&dry_run=1] — edit an event (phase 3): the body is the editor's
/// JSON (as for a create) with `account_id`. `scope=series` changes the whole series (the id resolves to its master);
/// `occurrence` makes one exception. POST /calendar/event/cancel?id=ID&scope=…&kind=cancel|decline|delete[&dry_run=1] —
/// cancel a meeting you organise (sends the cancellation), decline one you were invited to, or delete an entry; the
/// body is `{account_id, comment}`. Every one waits the undo window (`POST /calendar/event/undo?id=`) and is sent by
/// the account's calendar outbox. `invitees` counts the people the change reaches, shown before the user confirms.
/// `?dry_run=1` answers what would be sent and queues nothing. Answers `{ok, id, target, series, invitees, delay_secs}`.
fn calendar_change_response(config_dir: &PathBuf, stream: &mut std::net::TcpStream, initial: &[u8], first_line: &str) -> String {
    use omalook::calendar_write::{invitee_count, EventDraft};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();

    let raw = match read_post_body(stream, initial, 256 * 1024) {
        Ok(b) => b,
        Err(e) => return err(&e),
    };
    let mut req: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return err(&format!("unreadable change: {}", e)),
    };
    let account_id = req["account_id"].as_str().unwrap_or("").to_string();
    if account_id.is_empty() {
        return err("no calendar account selected");
    }
    let event_id = query_param(first_line, "id").unwrap_or_default();
    if event_id.is_empty() {
        return err("missing id");
    }
    let whole_series = query_param(first_line, "scope").as_deref() == Some("series");
    let update = first_line.starts_with("PATCH ");
    let kind = if update { "update".to_string() } else { query_param(first_line, "kind").unwrap_or_else(|| "cancel".into()) };
    if !matches!(kind.as_str(), "update" | "cancel" | "decline" | "delete") {
        return err("unknown change");
    }
    let now = Database::now_ms();
    let tx = format!("ol-{:x}-{:03x}", now, COUNTER.fetch_add(1, Ordering::Relaxed) & 0xfff);
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let target = match db.cal_event_ref(&event_id, whole_series) {
        Ok(Some(r)) => r,
        Ok(None) => return err("that event is not in the calendar cache; refresh the calendar and try again"),
        Err(e) => return err(&e),
    };
    // What the change reaches, and (for an edit) what it will send to the provider, checked before anything queues.
    let (invitees, graph) = if update {
        if req["time_zone"].as_str().unwrap_or("").is_empty() {
            req["time_zone"] = serde_json::json!(omalook::providers::calendar::local_time_zone());
        }
        let draft = match EventDraft::from_request(&req, &tx) {
            Ok(d) => d,
            Err(e) => return err(&e),
        };
        let graph = match draft.to_graph_patch_json(target.series) {
            Ok(b) => b,
            Err(e) => return err(&e.to_string()),
        };
        (invitee_count(&draft.attendees), Some(graph))
    } else {
        // A decline tells only the organiser; a cancel or a delete of a meeting tells every cached invitee.
        (if kind == "decline" { 0 } else { target.invitees }, None)
    };
    if query_param(first_line, "dry_run").as_deref() == Some("1") {
        return serde_json::json!({ "ok": true, "dry_run": true, "kind": kind, "id": event_id, "target": target.target,
            "series": target.series, "invitees": invitees, "graph": graph }).to_string();
    }
    if !SCHEDULER.get().map(|s| s.is_running(&account_id)).unwrap_or(false) {
        return err("that account is not signed in, so it cannot change events");
    }
    let delay = omalook::settings::read_send_delay(&config_dir.join("settings.toml"));
    let out_id = format!("cu-{:x}-{:03x}", now, COUNTER.fetch_add(1, Ordering::Relaxed) & 0xfff);
    let payload = if update {
        req["account_id"] = serde_json::json!(account_id);
        req["transaction_id"] = serde_json::json!(tx);
        req["series"] = serde_json::json!(target.series);
        req.clone()
    } else {
        serde_json::json!({ "account_id": account_id, "comment": req["comment"].as_str().unwrap_or(""),
            "subject": target.subject, "start": target.start, "end": target.end, "all_day": target.all_day,
            "location": target.location, "transaction_id": tx, "series": target.series })
    };
    if let Err(e) = db.cal_queue_event_change(&out_id, &kind, &target.target, &payload, invitees, now + delay as i64 * 1000) {
        return err(&e);
    }
    omalook::sync_state::request_cal_outbox_run();
    info!("Calendar: queued {} {} of {} ({} invitee(s)), runs in {}s", kind, out_id, target.target, invitees, delay);
    serde_json::json!({ "ok": true, "id": out_id, "kind": kind, "target": target.target, "series": target.series,
        "invitees": invitees, "delay_secs": delay }).to_string()
}

/// POST /calendar/event/undo?id=ID (take a queued write back while it is inside its undo window),
/// GET /calendar/outbox (queued, running and failed writes, and the ones that finished in the last
/// minute: the editor draws them as pending marks) and GET /calendar/event/status?id=ID.
fn calendar_outbox_response(config_dir: &PathBuf, first_line: &str) -> String {
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    if first_line.contains("GET /calendar/outbox") {
        return match db.cal_outbox_list() {
            Ok(rows) => serde_json::json!({ "ok": true, "items": rows.iter().map(|(id, account, op, state, attendees, error, due_at, payload, result)| {
                let p: serde_json::Value = serde_json::from_str(payload).unwrap_or(serde_json::Value::Null);
                serde_json::json!({
                    "id": id, "account_id": account, "op": op, "state": state, "attendees": attendees,
                    "error": error, "due_at": due_at, "result_id": result,
                    "subject": p["subject"], "start": p["start"], "end": p["end"], "all_day": p["all_day"],
                    "location": p["location"], "transaction_id": p["transaction_id"],
                    "target": p["target"], "series": p["series"],
                })
            }).collect::<Vec<_>>() }).to_string(),
            Err(e) => err(&e.to_string()),
        };
    }
    let id = match query_param(first_line, "id") { Some(i) => i, None => return err("missing id") };
    let undoing = first_line.contains("POST /calendar/event/undo");
    let cancelled = if undoing {
        match db.cal_outbox_cancel(&id) { Ok(c) => c, Err(e) => return err(&e.to_string()) }
    } else { false };
    match db.cal_outbox_status(&id) {
        Ok(Some((state, error, result))) => serde_json::json!({ "ok": true, "cancelled": cancelled, "state": state, "error": error, "result_id": result }).to_string(),
        Ok(None) => serde_json::json!({ "ok": false, "cancelled": false, "state": "unknown", "error": "no such event write" }).to_string(),
        Err(e) => err(&e.to_string()),
    }
}

/// POST /calendar/tags?account=A&id=EVENT&tags=["Family","Panama"] (OL-040 phase 6b): set the tags of an event. A recurring
/// event is tagged as a whole (an occurrence resolves to its series). The change waits `[mail] send_delay_secs`, the same
/// undo window as every calendar write (`POST /calendar/event/undo?id=`), then the account's calendar outbox writes it
/// (Exchange categories / Google private property) and reads it back. `?dry_run=1` answers what would be written and queues
/// nothing. Answers `{ok, id, event_id, series, tags, delay_secs}` or `{ok:false, error}`.
fn calendar_tags_response(config_dir: &PathBuf, first_line: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let account = query_param(first_line, "account").unwrap_or_default();
    let event = query_param(first_line, "id").unwrap_or_default();
    if account.is_empty() || event.is_empty() { return err("missing account or id") }
    let tags: Vec<String> = match serde_json::from_str::<Vec<String>>(&query_param(first_line, "tags").unwrap_or_else(|| "[]".into())) {
        Ok(t) => omalook::calendar_tags::clean(t),
        Err(_) => return err("tags must be a JSON list of names"),
    };
    if tags.len() > 25 { return err("too many tags on one event (25 at most)") }
    if let Some(bad) = tags.iter().find(|t| !omalook::rules::valid_tag_name(t)) { return err(&format!("\"{}\" is not a usable tag name", bad)) }
    let db = match Database::open_for_account(config_dir.join("messages.db").to_str().unwrap_or("messages.db"), &account) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let now = Database::now_ms();
    let id = format!("ct-{:x}-{:03x}", now, COUNTER.fetch_add(1, Ordering::Relaxed) & 0xfff);
    let dry = query_param(first_line, "dry_run").as_deref() == Some("1");
    if !dry && !SCHEDULER.get().map(|s| s.is_running(&account)).unwrap_or(false) {
        return err("that account is not signed in, so it cannot save tags");
    }
    let delay = omalook::settings::read_send_delay(&config_dir.join("settings.toml"));
    // a dry run queues far in the future and is cancelled at once: it exercises the same lookups without a write
    let queued = db.cal_queue_event_tags(&id, &event, &tags, if dry { i64::MAX / 2 } else { now + delay as i64 * 1000 });
    let (target, series) = match queued { Ok(v) => v, Err(e) => return err(&e) };
    if dry {
        let _ = db.cal_outbox_cancel(&id);
        return serde_json::json!({ "ok": true, "dry_run": true, "event_id": target, "series": series, "tags": tags }).to_string();
    }
    omalook::sync_state::request_cal_outbox_run();
    info!("Calendar: queued tags {:?} on {} ({}), saves in {}s", tags, target, account, delay);
    serde_json::json!({ "ok": true, "id": id, "event_id": target, "series": series, "tags": tags, "delay_secs": delay }).to_string()
}

/// GET /calendar/find?account=A&attendees=a@x,b@y&optional=c@z&accounts=A2,A3&from=YYYY-MM-DD&to=YYYY-MM-DD&duration=MIN[&limit=5]
/// (OL-021 track A, docs/PLAN-calendar-events.md 5.6): ranked free slots. `account` is the account the answer is for
/// (Exchange; its getSchedule covers you and the attendees); `attendees` are required, `optional` optional; `accounts`
/// are other signed-in accounts whose calendars count as your busy time; `to` is exclusive. Read-only: nothing is
/// queued or sent. Answers the finder JSON (`calendar_finder::find_free_time`) or `{ok:false, error}`.
fn calendar_find_response(config_dir: &PathBuf, first_line: &str) -> String {
    use chrono::NaiveDate;
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let list = |key: &str| -> Vec<String> {
        query_param(first_line, key)
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };
    let date = |key: &str| query_param(first_line, key).and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok());
    let (from, to) = match (date("from"), date("to")) {
        (Some(f), Some(t)) => (f, t),
        _ => return err("from and to must be dates (YYYY-MM-DD, to exclusive)"),
    };
    let duration_min = match query_param(first_line, "duration").and_then(|s| s.parse::<i64>().ok()) {
        Some(d) => d,
        None => return err("duration (minutes) is missing"),
    };
    let account_id = query_param(first_line, "account").unwrap_or_default();
    if account_id.is_empty() {
        return err("missing account");
    }
    let mut attendees: Vec<(String, bool)> = list("attendees").into_iter().map(|a| (a, true)).collect();
    attendees.extend(list("optional").into_iter().map(|a| (a, false)));
    let req = omalook::calendar_finder::FindRequest {
        account_id,
        attendees,
        other_accounts: list("accounts"),
        from,
        to,
        duration_min,
        limit: query_param(first_line, "limit").and_then(|s| s.parse().ok()).unwrap_or(5),
    };
    match omalook::calendar_finder::find_free_time(config_dir, &req) {
        Ok(v) => v.to_string(),
        Err(e) => err(&e),
    }
}

/// JSON body for the /settings/senders endpoints (see the route comment).
fn sender_prefs_response(config_dir: &PathBuf, first_line: &str) -> String {
    use omalook::db::SenderAdd;
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

/// JSON body for the `/local-rules` routes (Settings -> Rules, OL-040 phase 4d): rules that are exactly what a
/// provider can store (see `omalook::local_rules`), run by OmaLook on arrival for accounts without a server
/// engine and, on request, over existing Inbox mail.
fn local_rules_response(config_dir: &PathBuf, first_line: &str) -> String {
    use omalook::local_rules::{has_local_engine, LocalRule};
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let path = config_dir.join("messages.db");
    let db = match Database::open(path.to_str().unwrap_or("messages.db")) {
        Ok(d) => d,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let limit = omalook::settings::read_rules_confirm_over(&config_dir.join("settings.toml"));
    let id = query_param(first_line, "id").unwrap_or_default();
    // the rule from `rule=<json>`; a blank id means a new rule
    let parsed = || -> Result<LocalRule, String> {
        let raw = query_param(first_line, "rule").ok_or("need rule=<json>")?;
        let mut v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| format!("rule is not valid JSON: {e}"))?;
        if v["id"].as_str().unwrap_or("").is_empty() {
            let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
            v["id"] = serde_json::json!(format!("lr-{:x}", n));
        }
        serde_json::from_value::<LocalRule>(v).map_err(|e| format!("rule has the wrong shape: {e}"))
    };
    let named = |db: &Database| db.local_rules().map(|r| r.into_iter().find(|x| x.id == id));
    let r: Result<String, String> = (|| {
        if first_line.starts_with("GET /local-rules/preview") {
            let mut rule = parsed()?;
            rule.validate()?;
            rule.enabled = true; // a preview shows what the rule would do if it were on
            let d = db.dry_run(&[rule]).map_err(|e| e.to_string())?;
            return Ok(serde_json::json!({ "ok": true, "preview": d }).to_string());
        }
        if first_line.starts_with("POST /local-rules/save") {
            let mut rule = parsed()?;
            rule.validate()?;
            let existing = db.local_rules().map_err(|e| e.to_string())?;
            match existing.iter().find(|x| x.id == rule.id) {
                Some(old) => rule.sequence = old.sequence,
                None => rule.sequence = existing.iter().map(|x| x.sequence).max().unwrap_or(0) + 1,
            }
            db.save_local_rule(&rule).map_err(|e| e.to_string())?;
            return Ok(serde_json::json!({ "ok": true, "id": rule.id }).to_string());
        }
        if first_line.starts_with("POST /local-rules/delete") {
            if query_param(first_line, "confirm").as_deref() != Some("yes") { return Err("confirm=yes is required".into()); }
            let gone = db.delete_local_rule(&id).map_err(|e| e.to_string())?;
            return Ok(serde_json::json!({ "ok": gone, "error": if gone { "" } else { "no such rule" } }).to_string());
        }
        if first_line.starts_with("POST /local-rules/enable") {
            let mut rule = named(&db).map_err(|e| e.to_string())?.ok_or("no such rule")?;
            rule.enabled = query_param(first_line, "on").as_deref() == Some("true");
            db.save_local_rule(&rule).map_err(|e| e.to_string())?;
            return Ok(serde_json::json!({ "ok": true }).to_string());
        }
        if first_line.starts_with("POST /local-rules/move") {
            let up = match query_param(first_line, "dir").as_deref() { Some("up") => true, Some("down") => false, _ => return Err("dir must be up or down".into()) };
            let moved = db.move_local_rule(&id, up).map_err(|e| e.to_string())?;
            return Ok(serde_json::json!({ "ok": moved, "error": if moved { "" } else { "cannot move that way" } }).to_string());
        }
        if first_line.starts_with("POST /local-rules/apply") {
            // This rule alone over existing Inbox mail. Over the limit it writes nothing and says how many it would
            // change; the caller repeats with confirm=yes after the user agrees.
            let rule = named(&db).map_err(|e| e.to_string())?.ok_or("no such rule")?;
            if !rule.enabled { return Err("turn the rule on before applying it to existing mail".into()); }
            let confirmed = query_param(first_line, "confirm").as_deref() == Some("yes");
            let out = db.apply_local_rules(&[rule], limit, confirmed).map_err(|e| e.to_string())?;
            if matches!(out, omalook::db::local_rules::ApplyOutcome::Applied { messages, .. } if messages > 0) { omalook::sync_state::request_read_push(); }
            return Ok(serde_json::json!({ "ok": true, "outcome": out, "limit": limit }).to_string());
        }
        // GET /local-rules
        let stats = db.local_rule_stats().unwrap_or_default();
        let accounts: Vec<serde_json::Value> = db.accounts_basic().unwrap_or_default().into_iter().map(|(id, provider, email)| {
            let on_arrival = has_local_engine(omalook::accounts::provider_of(&id));
            serde_json::json!({ "id": id, "provider": provider, "email": email, "on_arrival": on_arrival })
        }).collect();
        let rules: Vec<serde_json::Value> = db.local_rules().map_err(|e| e.to_string())?.into_iter().map(|r| {
            let applied = stats.get(&r.id).copied().unwrap_or(0);
            let mut v = serde_json::to_value(&r).unwrap_or_default();
            v["applied"] = serde_json::json!(applied);
            v
        }).collect();
        let tags: Vec<String> = db.tags_overview().unwrap_or_default().into_iter().map(|t| t.name).collect();
        Ok(serde_json::json!({ "ok": true, "rules": rules, "accounts": accounts, "tags": tags, "limit": limit }).to_string())
    })();
    r.unwrap_or_else(|m| err(&m))
}

/// JSON body for `GET /rules`: every account's cached mailbox rules, summarised and grouped.
/// Marks one manual rule run as going (key = account|rule id); dropped when the run ends, even on a panic.
struct RunGuard(String);
static RUNNING_RULES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
impl RunGuard {
    fn take(key: &str) -> Option<RunGuard> {
        let mut g = RUNNING_RULES.lock().unwrap_or_else(|e| e.into_inner());
        if g.iter().any(|k| k == key) { return None }
        g.push(key.to_string());
        Some(RunGuard(key.to_string()))
    }
}
impl Drop for RunGuard {
    fn drop(&mut self) {
        let mut g = RUNNING_RULES.lock().unwrap_or_else(|e| e.into_inner());
        g.retain(|k| k != &self.0);
    }
}

fn rules_response(config_dir: &PathBuf, first_line: &str) -> String {
    use omalook::db::rules::RuleChange;
    use omalook::rules::{Match, NewRule};
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let path = config_dir.join("messages.db");
    if first_line.starts_with("POST /rules/") || first_line.starts_with("GET /rules/preview") || first_line.starts_with("GET /rules/run-preview") || first_line.starts_with("GET /rules/filter-preview") {
        let Some(account) = query_param(first_line, "account").filter(|a| !a.is_empty()) else { return err("need account=<id>") };
        let db = match Database::open_for_account(path.to_str().unwrap_or("messages.db"), &account) {
            Ok(d) => d,
            Err(e) => return err(&format!("database: {}", e)),
        };
        if !db.accounts_basic().unwrap_or_default().iter().any(|(id, _, _)| *id == account) { return err("no such account") }
        let id = query_param(first_line, "id").unwrap_or_default();
        // The Exchange routes (everything but filter-*) are Graph rule changes and runs; Gmail accounts have their own.
        let provider_of = db.accounts_basic().unwrap_or_default().into_iter().find(|(a, _, _)| *a == account).map(|(_, p, _)| p).unwrap_or_default();
        if !first_line.contains("/rules/filter-") && !provider_of.eq_ignore_ascii_case("exchange") {
            return err("this account's rules are not Exchange rules (Gmail accounts use filters)");
        }
        let new_rule = || -> Result<NewRule, String> {
            let matcher = match (query_param(first_line, "address"), query_param(first_line, "domain")) {
                (Some(a), None) => Match::Address(a),
                (None, Some(d)) => Match::Domain(d),
                _ => return Err("need exactly one of address=<email> or domain=<domain>".into()),
            };
            Ok(NewRule {
                tag: query_param(first_line, "tag").unwrap_or_default().trim().to_string(), matcher,
                enabled: query_param(first_line, "enabled").as_deref() != Some("false"),
                stop: query_param(first_line, "stop").as_deref() != Some("false"),
            })
        };
        let queued = |r: Result<i64, String>| match r {
            Ok(op) => { omalook::sync_state::request_read_push(); serde_json::json!({ "ok": true, "op": op }).to_string() }
            Err(m) => err(&m),
        };
        // Gmail filters (OL-040 phase 5b): the rule is a local-rule JSON, written to Gmail only when Gmail can hold it
        // exactly (omalook::gmail_filters). Everything is queued; the daemon does the checked write.
        if first_line.starts_with("GET /rules/filter-preview") || first_line.starts_with("POST /rules/filter-") {
            let rule = || -> Result<omalook::local_rules::LocalRule, String> {
                let raw = query_param(first_line, "rule").ok_or("need rule=<json>")?;
                let mut v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| format!("rule is not valid JSON: {e}"))?;
                if v["id"].as_str().unwrap_or("").is_empty() { v["id"] = serde_json::json!("gmail-filter"); }
                if v["name"].as_str().unwrap_or("").is_empty() { v["name"] = serde_json::json!("Gmail filter"); }
                let mut r: omalook::local_rules::LocalRule = serde_json::from_value(v).map_err(|e| format!("rule has the wrong shape: {e}"))?;
                r.applies_to = vec![account.clone()];
                r.enabled = true;
                Ok(r)
            };
            if first_line.starts_with("GET /rules/filter-preview") {
                return match rule() {
                    Err(m) => err(&m),
                    Ok(r) => {
                        let (writable, reason) = match omalook::gmail_filters::writable(&r) { Ok(()) => (true, String::new()), Err(m) => (false, m) };
                        match db.dry_run(&[r]) {
                            Ok(p) => serde_json::json!({ "ok": true, "writable": writable, "reason": reason, "preview": p }).to_string(),
                            Err(e) => err(&e.to_string()),
                        }
                    }
                };
            }
            use omalook::db::gmail_filters::FilterChange;
            let change = if first_line.starts_with("POST /rules/filter-add") {
                rule().map(FilterChange::Add)
            } else if first_line.starts_with("POST /rules/filter-edit") {
                rule().map(|r| FilterChange::Edit { id: id.clone(), rule: r })
            } else if first_line.starts_with("POST /rules/filter-delete") {
                if query_param(first_line, "confirm").as_deref() != Some("yes") { Err("confirm=yes is required".to_string()) } else { Ok(FilterChange::Delete { id: id.clone() }) }
            } else { Err("no such route".to_string()) };
            return match change { Ok(c) => queued(db.queue_filter_change(&c)), Err(m) => err(&m) };
        }
        // Manual "run now" of the account's Exchange tag rules over cached Inbox mail (all of them, or `id`).
        // Count first (run-preview); `run` writes nothing over the limit until confirm=yes. Tags only, added never removed.
        if first_line.starts_with("GET /rules/run-preview") || first_line.starts_with("POST /rules/run") {
            let only = if id.is_empty() { None } else { Some(id.as_str()) };
            let (rules, left_out) = match db.exchange_rules_to_run(&account, only) { Ok(x) => x, Err(m) => return err(&m) };
            if rules.is_empty() { return err("no enabled tag rules to run on this account") }
            let limit = omalook::settings::read_rules_confirm_over(&config_dir.join("settings.toml"));
            return if first_line.starts_with("GET /rules/run-preview") {
                match db.dry_run(&rules) {
                    Ok(p) => serde_json::json!({ "ok": true, "preview": p, "rules": rules.len(), "left_out": left_out, "limit": limit }).to_string(),
                    Err(e) => err(&e.to_string()),
                }
            } else {
                let confirmed = query_param(first_line, "confirm").as_deref() == Some("yes");
                // one run per (account, rule) at a time; different rules run side by side
                let _running = match RunGuard::take(&format!("{}|{}", account, id)) { Some(g) => g, None => return err("that run is already going") };
                match db.apply_local_rules(&rules, limit, confirmed) {
                    Ok(out) => {
                        if matches!(out, omalook::db::local_rules::ApplyOutcome::Applied { messages, .. } if messages > 0) { omalook::sync_state::request_read_push(); }
                        serde_json::json!({ "ok": true, "outcome": out, "rules": rules.len(), "left_out": left_out, "limit": limit }).to_string()
                    }
                    Err(e) => err(&e.to_string()),
                }
            };
        }
        return if first_line.starts_with("GET /rules/preview") {
            match new_rule().and_then(|n| db.rule_preview(&n)) {
                Ok(p) => serde_json::json!({ "ok": true, "preview": p }).to_string(),
                Err(m) => err(&m),
            }
        } else if first_line.starts_with("POST /rules/add") {
            match new_rule() { Ok(n) => queued(db.queue_rule_change(&RuleChange::Add(n))), Err(m) => err(&m) }
        } else if first_line.starts_with("POST /rules/enable") {
            queued(db.queue_rule_change(&RuleChange::Enable { id, on: query_param(first_line, "on").as_deref() == Some("true") }))
        } else if first_line.starts_with("POST /rules/delete") {
            if query_param(first_line, "confirm").as_deref() != Some("yes") { err("confirm=yes is required") }
            else { queued(db.queue_rule_change(&RuleChange::Delete { id })) }
        } else if first_line.starts_with("POST /rules/move") {
            match query_param(first_line, "dir").as_deref() {
                Some("up") => queued(db.queue_rule_change(&RuleChange::Move { id, up: true })),
                Some("down") => queued(db.queue_rule_change(&RuleChange::Move { id, up: false })),
                _ => err("dir must be up or down"),
            }
        } else { err("no such route") };
    }
    let db = match Database::open(path.to_str().unwrap_or("messages.db")) {
        Ok(d) => d,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let colors: std::collections::HashMap<(String, String), String> = rusqlite::Connection::open(&path).ok()
        .and_then(|c| c.prepare("SELECT account_id, name, color FROM category_defs WHERE delete_pending = 0").ok().map(|mut st| {
            st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
                .map(|it| it.flatten().map(|(a, n, c)| ((a, n.to_lowercase()), c)).collect()).unwrap_or_default()
        })).unwrap_or_default();
    let tags = db.tags_overview().unwrap_or_default();
    let mut accounts: Vec<serde_json::Value> = Vec::new();
    for (id, provider, email) in db.accounts_basic().unwrap_or_default() {
        let state = db.rules_state(&id).ok().flatten();
        let raw = db.rules_raw(&id).unwrap_or_default();
        let mut order: Vec<(i64, String, bool)> = raw.iter().filter(|_| !provider.eq_ignore_ascii_case("gmail")).map(|r| (r["sequence"].as_i64().unwrap_or(0), r["id"].as_str().unwrap_or_default().to_string(),
            omalook::rules::writable(r).is_err())).collect();
        order.sort();
        let order: Vec<serde_json::Value> = order.into_iter().map(|(_, rid, locked)| serde_json::json!({ "id": rid, "locked": locked })).collect();
        let ops = db.recent_rule_ops(&id, 86_400).unwrap_or_default();
        let is_gmail = provider.eq_ignore_ascii_case("gmail");
        // Gmail filters are not Graph rules: they are summarised by omalook::gmail_filters, not grouped as tag rules
        let filters: Vec<omalook::gmail_filters::FilterSummary> = if is_gmail { db.filter_summaries(&id) } else { Vec::new() };
        let summaries: Vec<omalook::rules::RuleSummary> = if is_gmail { Vec::new() } else { raw.iter().map(omalook::rules::summarize).collect() };
        let total = if is_gmail { filters.len() } else { summaries.len() };
        let org = omalook::rules::organise(summaries);
        let groups: Vec<serde_json::Value> = org.groups.iter().map(|g| {
            let color = colors.get(&(id.clone(), g.tag.to_lowercase())).cloned()
                .or_else(|| tags.iter().find(|t| omalook::db::tags::tag_key(&t.name) == omalook::db::tags::tag_key(&g.tag)).map(|t| t.color.clone()))
                .unwrap_or_else(|| omalook::models::fallback_category_color(&g.tag).to_string());
            serde_json::json!({ "tag": g.tag, "color": color, "rules": g.rules })
        }).collect();
        accounts.push(serde_json::json!({
            "account_id": id, "provider": provider, "email": email, "total": total,
            "state": state.as_ref().map(|s| s.state.clone()).unwrap_or_else(|| "none".into()),
            "error": state.as_ref().map(|s| s.error.clone()).unwrap_or_default(),
            "fetched_at": state.as_ref().map(|s| s.fetched_at).unwrap_or(0),
            "groups": groups, "other": org.other, "system": org.system,
            "can_write": (provider == "exchange" || is_gmail) && state.as_ref().map(|s| s.state == "ok").unwrap_or(false),
            "filters": filters,
            "order": order, "ops": ops,
            "tag_pushes_pending": db.tag_pushes_pending(&id).unwrap_or(0),
        }));
    }
    serde_json::json!({ "accounts": accounts }).to_string()
}

/// JSON body for the Tags page endpoints (see the route comment).
fn tags_response(config_dir: &PathBuf, first_line: &str) -> String {
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let path = config_dir.join("messages.db");
    let path = path.to_str().unwrap_or("messages.db");
    let overview = match Database::open(path).and_then(|d| d.tags_overview()) {
        Ok(v) => v,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let name = query_param(first_line, "name").unwrap_or_default().trim().to_string();
    let key = omalook::db::tags::tag_key(&name);
    let only_account = query_param(first_line, "account");
    let row = overview.iter().find(|t| omalook::db::tags::tag_key(&t.name) == key);
    let for_account = |id: &str| only_account.as_deref().map(|a| a == id).unwrap_or(true);

    if first_line.starts_with("POST /tags/color") {
        let Some(c) = query_param(first_line, "color").and_then(|k| omalook::models::palette_color(&k)) else { return err("color must be a palette key") };
        let Some(row) = row else { return err("no such tag") };
        let mut changed: Vec<String> = Vec::new();
        for a in row.accounts.iter().filter(|a| a.defined && for_account(&a.account_id)) {
            match Database::open_for_account(path, &a.account_id).and_then(|d| d.tag_set_color(&name, c.hex)) {
                Ok(true) => changed.push(a.account_id.clone()),
                Ok(false) => {}
                Err(e) => return err(&format!("database: {}", e)),
            }
        }
        if changed.is_empty() { return err("no account has this tag in its list (add it to one first)") }
        omalook::sync_state::request_read_push();
        serde_json::json!({ "ok": true, "accounts": changed }).to_string()
    } else if first_line.starts_with("POST /tags/delete") {
        let Some(row) = row else { return err("no such tag") };
        let counts: Vec<serde_json::Value> = row.accounts.iter().filter(|a| (a.defined || a.messages > 0) && for_account(&a.account_id))
            .map(|a| serde_json::json!({ "account": a.account_id, "messages": a.messages })).collect();
        if query_param(first_line, "confirm").as_deref() != Some("yes") {
            return serde_json::json!({ "ok": false, "error": "confirm=yes is required", "accounts": counts }).to_string();
        }
        let mut stripped = 0usize;
        for a in row.accounts.iter().filter(|a| (a.defined || a.messages > 0) && for_account(&a.account_id)) {
            match Database::open_for_account(path, &a.account_id).and_then(|d| d.tag_delete(&name)) {
                Ok((n, _)) => stripped += n,
                Err(e) => return err(&format!("database: {}", e)),
            }
        }
        omalook::sync_state::request_read_push();
        serde_json::json!({ "ok": true, "messages": stripped, "accounts": counts }).to_string()
    } else if first_line.starts_with("POST /tags/adopt") {
        let Some(to) = query_param(first_line, "to") else { return err("need to=<account id>") };
        let Some(row) = row else { return err("no such tag") };
        if name.chars().count() > 64 || name.chars().any(|ch| ch.is_control() || ch == ';' || ch == ',') { return err("not a usable tag name") }
        let Some(target) = row.accounts.iter().find(|a| a.account_id == to) else { return err("no such account") };
        if target.defined { return serde_json::json!({ "ok": true, "created": false }).to_string(); }
        let hex = omalook::models::CATEGORY_PALETTE.iter().find(|c| c.hex == row.color || c.gmail_bg == row.color)
            .map(|c| c.hex).unwrap_or_else(|| omalook::models::fallback_category_color(&name));
        match Database::open_for_account(path, &to).and_then(|d| d.add_pending_category(&row.name, hex)) {
            Ok(created) => { omalook::sync_state::request_read_push(); serde_json::json!({ "ok": true, "created": created }).to_string() }
            Err(e) => err(&format!("database: {}", e)),
        }
    } else if first_line.starts_with("GET /tags/similar") {
        match Database::open(path).and_then(|d| d.tags_similar(&name)) {
            Ok(v) => serde_json::json!({ "similar": v }).to_string(),
            Err(e) => err(&format!("database: {}", e)),
        }
    } else {
        let palette: Vec<serde_json::Value> = omalook::models::CATEGORY_PALETTE.iter()
            .map(|c| serde_json::json!({ "key": c.key, "label": c.label, "hex": c.hex })).collect();
        serde_json::json!({ "tags": overview, "palette": palette }).to_string()
    }
}

/// JSON body for the distribution-list endpoints (see the route comment).
fn lists_response(config_dir: &PathBuf, first_line: &str) -> String {
    use omalook::db::lists::ListOp;
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let db = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let status = |op: ListOp| match op {
        ListOp::Done => "done", ListOp::Exists => "exists", ListOp::Invalid => "invalid", ListOp::Missing => "missing",
    };
    let id: i64 = query_param(first_line, "id").and_then(|v| v.parse().ok()).unwrap_or(0);
    let name = query_param(first_line, "name").unwrap_or_default();
    let email = query_param(first_line, "email").unwrap_or_default();
    let r: Result<String, omalook::errors::OmarchyError> = if first_line.contains("POST /lists/targets/set") {
        let account = query_param(first_line, "account").unwrap_or_default();
        let on = query_param(first_line, "on").as_deref() == Some("true");
        if account.is_empty() { Ok(serde_json::json!({ "ok": false }).to_string()) }
        else { db.set_list_sync_target(&account, on).map(|_| serde_json::json!({ "ok": true }).to_string()) }
    } else if first_line.contains("GET /lists/targets") {
        db.list_sync_targets().map(|v| serde_json::json!(v).to_string())
    } else if first_line.contains("GET /lists/sync/status") {
        db.list_link_log(60).map(|log| serde_json::json!({ "reports": omalook::list_sync::last_reports(), "log": log }).to_string())
    } else if first_line.contains("POST /lists/sync/now") {
        Ok(serde_json::json!({ "ok": true }).to_string())
    } else if first_line.contains("POST /lists/create") {
        db.list_create(&name).map(|(op, id)| serde_json::json!({ "ok": op == ListOp::Done, "status": status(op), "id": id }).to_string())
    } else if first_line.contains("POST /lists/rename") {
        db.list_rename(id, &name).map(|op| serde_json::json!({ "ok": op == ListOp::Done, "status": status(op) }).to_string())
    } else if first_line.contains("POST /lists/delete") {
        db.list_delete(id).map(|ok| serde_json::json!({ "ok": ok }).to_string())
    } else if first_line.contains("POST /lists/members/add") {
        db.list_member_add(id, &email, &name).map(|op| serde_json::json!({ "ok": op == ListOp::Done || op == ListOp::Exists, "status": status(op) }).to_string())
    } else if first_line.contains("POST /lists/members/remove") {
        db.list_member_remove(id, &email).map(|ok| serde_json::json!({ "ok": ok }).to_string())
    } else {
        db.lists_all().map(|l| serde_json::to_string(&l).unwrap_or_else(|_| "[]".into()))
    };
    // a change to the lists (or to where they are mirrored) is followed by a run of the link
    let changed = ["/lists/targets/set", "/lists/create", "/lists/rename", "/lists/delete", "/lists/members/add", "/lists/members/remove", "/lists/sync/now"];
    if first_line.starts_with("POST ") && changed.iter().any(|p| first_line.contains(p)) {
        lists_link_run(config_dir.clone());
    }
    r.unwrap_or_else(|e| err(&e.to_string()))
}

/// Run the two-way list link for every mirror account that is signed in (off the request thread).
fn lists_link_run(config_dir: PathBuf) {
    std::thread::spawn(move || {
        let path = config_dir.join("messages.db");
        let path = path.to_str().unwrap_or("messages.db").to_string();
        let targets = match Database::open(&path).and_then(|d| d.list_sync_targets()) { Ok(t) => t, Err(_) => return };
        let create = omalook::settings::read_lists_create_contacts(&config_dir.join("settings.toml"));
        for account in targets {
            if omalook::accounts::provider_of(&account) == "gmail" { continue; }
            if !SCHEDULER.get().map(|s| s.is_running(&account)).unwrap_or(false) { continue; }
            if let Ok(db) = Database::open_for_account(&path, &account) {
                let walked = db.flag_is_set(omalook::contacts_daemon::FULL_FLAG).unwrap_or(false);
                let policy = if create { omalook::list_sync::CreatePolicy::Always } else { omalook::list_sync::CreatePolicy::Skip };
                omalook::list_sync::run_and_record(&db, walked, policy);
            }
        }
    });
}

/// Addresses seen in mail (From / To / Cc / Bcc), best first, for recipient suggestions. The
/// scan covers the newest messages and is reused for a minute.
static ADDRESS_CACHE: std::sync::OnceLock<std::sync::Mutex<Option<(std::time::Instant, String)>>> = std::sync::OnceLock::new();
const ADDRESS_SCAN_MESSAGES: usize = 40_000;
const ADDRESS_MAX_RETURNED: usize = 6_000;

fn addresses_json(config_dir: &PathBuf) -> String {
    let cache = ADDRESS_CACHE.get_or_init(|| std::sync::Mutex::new(None));
    if let Some((at, body)) = cache.lock().ok().and_then(|g| g.clone()) {
        if at.elapsed() < std::time::Duration::from_secs(60) { return body; }
    }
    let body = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db"))
        .and_then(|db| db.harvest_addresses(ADDRESS_SCAN_MESSAGES))
    {
        Ok(v) => {
            let rows: Vec<serde_json::Value> = v.iter().take(ADDRESS_MAX_RETURNED)
                .map(|s| serde_json::json!({ "email": s.email, "name": s.name, "score": s.score })).collect();
            serde_json::Value::Array(rows).to_string()
        }
        Err(e) => return serde_json::json!({ "ok": false, "error": e.to_string() }).to_string(),
    };
    if let Ok(mut g) = cache.lock() { *g = Some((std::time::Instant::now(), body.clone())); }
    body
}

fn json_error_response(status: &str, msg: &str) -> String {
    let body = serde_json::json!({ "error": msg }).to_string();
    format!(
        "HTTP/1.1 {}\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        status, body.len(), body
    )
}

/// Most message ids taken from one provider search (per account); beyond it the result is partial.
const REMOTE_SEARCH_CAP: usize = 1000;

/// Provider search results are reused for a few minutes: the list re-asks on every page and
/// refresh, and each ask would otherwise be a round trip to Graph / Gmail.
static REMOTE_SEARCH_CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, Vec<String>, bool)>>> = std::sync::OnceLock::new();
const REMOTE_SEARCH_TTL: std::time::Duration = std::time::Duration::from_secs(300);

/// Ids (as cached locally) of the messages whose full text matches `term`, asked of the provider
/// of every account in scope: the folder (`all:<kind>` = that kind of folder in every account),
/// or the whole mailbox when no folder is given; `account` (id or address) narrows it further.
/// The flag is true when any account hit the cap.
fn remote_body_ids(conn: &rusqlite::Connection, folder_id: Option<&str>, account: Option<&str>, term: &str) -> Result<(Vec<String>, bool), String> {
    use omalook::providers::EmailProvider;
    let db_err = |e: rusqlite::Error| format!("database: {e}");
    // (account id, provider folder id)
    let mut scopes: Vec<(String, Option<String>)> = Vec::new();
    match folder_id {
        Some(f) if f.starts_with("all:") => {
            let mut st = conn.prepare("SELECT account_id, id FROM folders WHERE well_known_name = ?1 AND account_id IS NOT NULL").map_err(db_err)?;
            let rows = st.query_map([&f[4..]], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(db_err)?;
            scopes.extend(rows.flatten().map(|(a, f)| (a, Some(f))));
        }
        Some(f) => {
            let acct: Option<String> = conn.query_row("SELECT account_id FROM folders WHERE id = ?1", [f], |r| r.get(0)).map_err(db_err)?;
            scopes.push((acct.ok_or("unknown folder")?, Some(f.to_string())));
        }
        None => {
            let mut st = conn.prepare("SELECT id FROM accounts WHERE enabled = 1").map_err(db_err)?;
            let rows = st.query_map([], |r| r.get::<_, String>(0)).map_err(db_err)?;
            scopes.extend(rows.flatten().map(|a| (a, None)));
        }
    }
    if let Some(acct) = account {
        let wanted: Option<String> = conn
            .query_row("SELECT id FROM accounts WHERE id = ?1 OR lower(email) = lower(?1)", [acct], |r| r.get(0))
            .ok();
        match wanted {
            Some(id) => scopes.retain(|(a, _)| *a == id),
            None => scopes.clear(),
        }
    }
    if scopes.is_empty() {
        return Err("no account to search".into());
    }

    let cache = REMOTE_SEARCH_CACHE.get_or_init(Default::default);
    let (mut all, mut more) = (Vec::<String>::new(), false);
    for (acct, folder) in scopes {
        let key = format!("{}|{}|{}", acct, folder.as_deref().unwrap_or(""), term.to_lowercase());
        let hit = cache.lock().ok().and_then(|c| c.get(&key).filter(|(t, _, _)| t.elapsed() < REMOTE_SEARCH_TTL).cloned());
        let (ids, m) = match hit {
            Some((_, ids, m)) => (ids, m),
            None => {
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| format!("runtime: {e}"))?;
                let found = if omalook::accounts::provider_of(&acct) == "gmail" {
                    rt.block_on(omalook::providers::GmailProvider::new(&acct, None).search_message_ids(folder.as_deref(), term, REMOTE_SEARCH_CAP))
                } else {
                    rt.block_on(GraphEmailProvider::new(AuthManager::for_account(&acct)).search_message_ids(folder.as_deref(), term, REMOTE_SEARCH_CAP))
                };
                let (ids, m) = found.map_err(|e| {
                    warn!("fullbody search failed for {}: {}", acct, e);
                    format!("the search at {} failed ({})", acct, e.to_string().chars().take(160).collect::<String>())
                })?;
                info!("fullbody search {:?} at {} ({:?}): {} ids{}", term, acct, folder.as_deref().map(|f| f.chars().take(12).collect::<String>()), ids.len(), if m { " (capped)" } else { "" });
                if let Ok(mut c) = cache.lock() {
                    c.insert(key, (std::time::Instant::now(), ids.clone(), m));
                }
                (ids, m)
            }
        };
        all.extend(ids);
        more |= m;
    }
    Ok((all, more))
}

/// The whole HTTP response for `GET /messages` (list, paging, `?q=` search). Runs on its own
/// thread: a `fullbody:` search waits on the mail provider and must not stall the server loop.
fn messages_response(config_dir: &PathBuf, first_line: &str, req_t: std::time::Instant) -> String {
                    // Optional ?q= keyword pattern (see src/search.rs). A bad pattern is a 400 with
                    // {"error": "..."} so the UI can show why; `fullbody:` needs the provider, which
                    // is not wired up yet, so it is refused rather than silently ignored.
                    let search_q = match query_param(first_line, "q").map(|q| omalook::search::parse(&q)) {
                        None => Ok(None),
                        Some(Ok(q)) => Ok(Some(q)),
                        Some(Err(e)) => Err(e),
                    };
                    let search_q = match search_q {
                        Ok(q) => q,
                        Err(e) => {
                            let body = serde_json::json!({ "error": e }).to_string();
                            let response = format!(
                                "HTTP/1.1 400 Bad Request\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                                body.len(), body
                            );
                            return response;
                        }
                    };
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
                    let mut truncated = false;   // a fullbody: result hit the provider-side cap
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
                                let cats: Vec<serde_json::Value> = omalook::db::categories_from_json(row.get::<_, Option<String>>(12)?.as_deref())
                                    .into_iter()
                                    .map(|name| {
                                        let color = cat_colors.get(&(account_id.clone(), name.to_lowercase())).cloned()
                                            .unwrap_or_else(|| omalook::models::fallback_category_color(&name).to_string());
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
                            if let Some(ref sq) = search_q {
                                conds.extend(sq.to_sql(&mut args));
                                // `fullbody:` terms are answered by the mail provider(s); the ids that
                                // come back (messages in scope, as cached here) become IN / NOT IN.
                                let account = query_param(first_line, "account");
                                for (neg, term) in &sq.fullbody {
                                    match remote_body_ids(&conn, folder_id.as_deref(), account.as_deref(), term) {
                                        Ok((ids, more)) => {
                                            if more && *neg {
                                                return json_error_response("400 Bad Request", &format!("-fullbody:{term} matches more than {REMOTE_SEARCH_CAP} messages, too many to exclude"));
                                            }
                                            truncated |= more;
                                            if ids.is_empty() {
                                                if !*neg { conds.push("0".to_string()); }
                                                continue;
                                            }
                                            let marks: Vec<String> = ids.into_iter().map(|id| { args.push(id.into()); format!("?{}", args.len()) }).collect();
                                            conds.push(format!("m.id {}IN ({})", if *neg { "NOT " } else { "" }, marks.join(",")));
                                        }
                                        Err(e) => return json_error_response("400 Bad Request", &format!("fullbody: {e}")),
                                    }
                                }
                            }
                            // Archived / deleted / moved messages wait out their undo window hidden.
                            conds.push("m.action_pending IS NULL".to_string());
                            // A draft that this app holds locally is shown from the local copy (GET /drafts), never twice.
                            conds.push("m.id NOT IN (SELECT remote_id FROM drafts WHERE remote_id IS NOT NULL AND state != 'sent')".to_string());
                            let where_sql = if conds.is_empty() { String::new() } else { format!(" WHERE {}", conds.join(" AND ")) };
                            args.push(limit.into());
                            args.push(offset.into());
                            let sql = format!(
                                "SELECT m.id, m.subject, m.from_email, m.from_name, m.received_at, m.is_read, \
                                        COALESCE(m.account_id, ''), COALESCE(a.email, ''), \
                                        m.to_text, m.cc_text, m.bcc_text, m.sent_at, m.categories, m.importance, \
                                        COALESCE((SELECT c.has_real FROM attachment_cache c WHERE c.message_id = m.id), m.has_attachments), \
                                        m.conversation_id, COALESCE(m.folder_id, '') \
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
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Expose-Headers: X-Search-Truncated\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        if truncated { "X-Search-Truncated: 1\r\n" } else { "" }, body.len(), body
                    );
                    omalook::perf::mark(&format!("http -> GET /messages answered ({} bytes, handler {:.1}ms)", body.len(), req_t.elapsed().as_secs_f64() * 1000.0));
                    response
}

/// Minimal HTTP trigger server — listens on localhost:27182
/// QML uses XMLHttpRequest to POST to this endpoint to trigger auth/logout.
/// This avoids the limitation of QML not being able to write files directly.
///
/// Endpoints:
///   POST /auth/login   → triggers device flow
///   POST /auth/logout  → triggers logout
fn start_http_trigger_server(config_dir: &PathBuf) {
    // OMALOOK_PORT is for tests that run a second instance beside the real one; the UI always
    // talks to the default port.
    let addr = format!("127.0.0.1:{}", env::var("OMALOOK_PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(27182));
    eprintln!("[HTTP] Attempting to bind {}", addr);
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => {
            eprintln!("[HTTP] ✅ Bound OK");
            info!("🌐 HTTP trigger server listening on http://127.0.0.1:27182");
            omalook::perf::mark("http server bound");
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
                    let body = format!("{{\"mail\":{},\"sms\":{}}}", omalook::sync_state::mail_serial(), omalook::sync_state::sms_serial());
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }
                if !first_line.contains("POST /perf") {
                    omalook::perf::mark(&format!("http <- {}", first_line.split(" HTTP").next().unwrap_or("").chars().take(60).collect::<String>()));
                }

                // ── POST /perf?m=label — a mark from the QML side (shares this timeline)
                if first_line.contains("POST /perf") {
                    if let Some(m) = query_param(first_line, "m") {
                        omalook::perf::mark(&format!("UI: {}", m));
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
                                if let Ok(aggs) = omalook::db::all_accounts_folders(&conn) {
                                    for a in aggs {
                                        all_rows.push(format!(
                                            "{{\"id\":{},\"display_name\":{},\"unread_item_count\":{},\"well_known_name\":{},\"total_item_count\":{},\"account_id\":\"\",\"account_email\":\"All accounts\",\"all_accounts\":true}}",
                                            serde_json::to_string(&format!("all:{}", a.well_known_name)).unwrap(),
                                            serde_json::to_string(omalook::db::all_accounts_label(&a.well_known_name)).unwrap(),
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
                    let body = omalook::account_ops::accounts_json(config_dir);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── SMS cache (KDE Connect). Reads: GET /sms/threads?device=&limit=,
                // GET /sms/messages?thread=&device=&before=&limit=, GET /sms/unread.
                // Writes: POST /sms/read?thread=&read=true|false[&uid=&device=] (local state only:
                // KDE Connect cannot mark messages read on the phone), POST /sms/sync (full re-pull). ──
                if first_line.contains(" /sms/") {
                    let body = if first_line.contains("POST /sms/send") { sms_send_response(config_dir, &mut stream, &buf[..n]) }
                               else { sms_route(config_dir, first_line) };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── GET /tasks[?account=ID] — cached To Do tasks of every account (JSON array). ──
                if first_line.contains("GET /tasks") {
                    let body = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
                        Ok(db) => match db.query_tasks(query_param(first_line, "account").as_deref()) {
                            Ok(rows) => serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string()),
                            Err(e) => { warn!("GET /tasks: query failed: {}", e); "[]".to_string() }
                        },
                        Err(e) => { warn!("GET /tasks: DB open failed: {}", e); "[]".to_string() }
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── /contacts/get, /contacts/folders (and the write routes of phase 2 onward): see `contacts_route` ──
                if let Some(body) = contacts_route(config_dir, &mut stream, &buf[..n], first_line) {
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── GET /contacts?view=all|favorites|lists&sort=first|last|company|recent&q=&folder=&category= ──
                if first_line.contains("GET /contacts") {
                    let view = query_param(first_line, "view").unwrap_or_else(|| "all".to_string());
                    let sort = query_param(first_line, "sort").unwrap_or_else(|| "first".to_string());
                    let dup_view = view == "duplicates";
                    let body = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
                        Ok(db) => match db.query_contacts_full(
                            if dup_view { "all" } else { &view }, &sort,
                            query_param(first_line, "account").as_deref(),
                            query_param(first_line, "q").as_deref(),
                            query_param(first_line, "folder").as_deref(),
                            query_param(first_line, "category").as_deref(),
                        ) {
                            Ok(rows) if dup_view => serde_json::to_string(&omalook::contacts_api::duplicates_view(&db, rows)).unwrap_or_else(|_| "[]".to_string()),
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

                // ── POST /mail/sync — the Refresh button / F5: poll the provider now, not at the next interval.
                // Returns at once; new mail reaches the UI through the mail sync serial. ──
                if first_line.contains("POST /mail/sync") {
                    omalook::sync_state::request_mail_sync();
                    let body = "ok";
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
                            .map(|changed| if changed { omalook::sync_state::request_read_push() })
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
                                .prepare("SELECT name, color, pending FROM category_defs WHERE account_id = ?1 AND delete_pending = 0 ORDER BY lower(name)")
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
                            let palette: Vec<serde_json::Value> = omalook::models::CATEGORY_PALETTE.iter()
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
                    let color = query_param(first_line, "color").and_then(|k| omalook::models::palette_color(&k));
                    let body = match color {
                        Some(c) if !account.is_empty() && !name.is_empty() && name.chars().count() <= 64 && !name.chars().any(|ch| ch.is_control() || ch == ';' || ch == ',') => {
                            match Database::open_for_account(config_dir.join("messages.db").to_str().unwrap_or("messages.db"), &account)
                                .and_then(|db| db.add_pending_category(&name, c.hex))
                            {
                                Ok(created) => {
                                    omalook::sync_state::request_read_push();
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
                                    if changed > 0 { omalook::sync_state::request_read_push(); }
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
                    let body = serde_json::json!({ "active": omalook::sync_state::meta_active(), "remaining": remaining }).to_string();
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
                    let delay = omalook::settings::read_send_delay(&config_dir.join("settings.toml"));
                    let body = match Database::open(db_path.to_str().unwrap_or("messages.db")) {
                        Ok(db) if first_line.contains("POST /messages/action/undo") => {
                            let undone = db.cancel_message_actions(&ids).unwrap_or(0);
                            if undone > 0 { omalook::sync_state::bump_mail("message action undone"); }
                            serde_json::json!({ "undone": undone }).to_string()
                        }
                        Ok(db) => {
                            use omalook::models::MessageAction;
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
                                        omalook::sync_state::request_read_push();
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

                // ── GET /fs/list?path=DIR[&hidden=1] — one directory for the compose file picker ──
                if first_line.contains("GET /fs/list") {
                    let path = query_param(first_line, "path").unwrap_or_default();
                    let hidden = query_param(first_line, "hidden").as_deref() == Some("1");
                    let body = match omalook::fs_browse::list(&path, hidden) {
                        Ok(l) => serde_json::to_string(&l).unwrap_or_else(|_| "{\"error\":\"could not list\"}".into()),
                        Err(e) => serde_json::json!({ "error": e }).to_string(),
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
                // ── GET /compose/originals?id=MESSAGE — the attachments of the message being forwarded ──
                if first_line.contains("GET /compose/originals") {
                    let body = compose_originals_json(config_dir, &query_param(first_line, "id").unwrap_or_default());
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }
                // ── GET /attach/limits?account=ID — what the account's provider accepts on one message ──
                if first_line.contains("GET /attach/limits") {
                    let provider = query_param(first_line, "account")
                        .map(|a| omalook::accounts::provider_of(&a).to_string())
                        .unwrap_or_default();
                    let body = serde_json::to_string(&omalook::compose::attachment_limits(&provider)).unwrap_or_else(|_| "{}".into());
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── Drafts (docs/PLAN-compose.md Phase B2). GET /drafts?account=ID (every account without it;
                // `recover` marks an unsaved autosave for the startup prompt), GET /drafts/get?id=,
                // POST /drafts/save?state=editing|saved (JSON body: the compose message), POST /drafts/discard?id= ──
                if first_line.contains("GET /drafts") || first_line.contains("POST /drafts/") {
                    let body = if first_line.contains("POST /drafts/save") {
                        drafts_save_response(config_dir, &mut stream, &buf[..n], first_line)
                    } else {
                        drafts_route(config_dir, first_line)
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── POST /calendar/event (JSON body; ?dry_run=1) — queue a new event; POST /calendar/event/undo?id= —
                // take it back inside its undo window; GET /calendar/outbox, GET /calendar/event/status?id= ──
                if first_line.contains("POST /calendar/event/undo") || first_line.contains("GET /calendar/outbox") || first_line.contains("GET /calendar/event/status") {
                    let body = calendar_outbox_response(config_dir, first_line);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }
                if first_line.contains("POST /calendar/tags") {
                    let body = calendar_tags_response(config_dir, first_line);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }
                // ── PATCH /calendar/event (edit) and POST /calendar/event/cancel?kind= (cancel, decline, delete) — phase 3 ──
                if first_line.starts_with("PATCH /calendar/event") || first_line.contains("POST /calendar/event/cancel") {
                    let body = calendar_change_response(config_dir, &mut stream, &buf[..n], first_line);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }
                if first_line.contains("POST /calendar/event") {
                    let body = calendar_event_response(config_dir, &mut stream, &buf[..n], first_line);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── GET|POST /settings/mail?send_delay=N&large_audience=N — seconds a sent message waits before it goes; recipient count above which Send asks to confirm (0 = never) ──
                if first_line.contains("GET /settings/mail") || first_line.contains("POST /settings/mail") {
                    use omalook::models::MailSettings;
                    let settings_path = config_dir.join("settings.toml");
                    let mut secs = omalook::settings::read_send_delay(&settings_path);
                    let mut large = omalook::settings::read_large_audience(&settings_path);
                    if first_line.contains("POST /settings/mail") {
                        if let Some(wanted) = query_param(first_line, "send_delay").and_then(|v| v.parse::<i32>().ok()) {
                            match omalook::settings::write_send_delay(&settings_path, wanted) {
                                Ok(stored) => secs = stored,
                                Err(e) => warn!("POST /settings/mail: could not save: {}", e),
                            }
                        }
                        if let Some(wanted) = query_param(first_line, "large_audience").and_then(|v| v.parse::<i32>().ok()) {
                            match omalook::settings::write_large_audience(&settings_path, wanted) {
                                Ok(stored) => large = stored,
                                Err(e) => warn!("POST /settings/mail: could not save: {}", e),
                            }
                        }
                    }
                    let body = serde_json::json!({
                        "send_delay_secs": secs,
                        "max_send_delay_secs": MailSettings::MAX_SEND_DELAY_SECS,
                        "large_audience_confirm": large,
                        "max_large_audience_confirm": MailSettings::MAX_LARGE_AUDIENCE,
                    }).to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── GET|POST /settings/render?cache_mb=N — size cap of the rendered-HTML cache; lowering it trims the cache at once ──
                if first_line.contains("GET /settings/render") || first_line.contains("POST /settings/render") {
                    let settings_path = config_dir.join("settings.toml");
                    if first_line.contains("POST /settings/render") {
                        if let Some(wanted) = query_param(first_line, "cache_mb").and_then(|v| v.parse::<i64>().ok()) {
                            match omalook::settings::write_render_cache_mb(&settings_path, wanted) {
                                Ok(_) => { omalook::render_cache::open_configured(config_dir).trim(); }
                                Err(e) => warn!("POST /settings/render: could not save: {}", e),
                            }
                        }
                    }
                    let cache = omalook::render_cache::open_configured(config_dir);
                    let body = serde_json::json!({
                        "cache_mb": omalook::settings::read_render_cache_mb(&settings_path),
                        "min_mb": omalook::settings::MIN_RENDER_CACHE_MB,
                        "max_mb": omalook::settings::MAX_RENDER_CACHE_MB,
                        "used_bytes": cache.total_bytes(),
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
                        .map(|sch| omalook::account_ops::resolve_reauth(config_dir, &pending_id, replace, sch))
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
                        Some(scheduler) => match omalook::account_ops::log_in_existing(config_dir, &id, scheduler) {
                            Ok(omalook::account_ops::ResumeOutcome::Resumed) => "{\"result\":\"resumed\"}".to_string(),
                            Ok(omalook::account_ops::ResumeOutcome::LoginRequired(provider)) => {
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

                // ── GET /calendar/find — free-time finder (OL-021 track A): ranked free slots, read-only ──
                if first_line.contains("GET /calendar/find") {
                    let body = calendar_find_response(config_dir, first_line);
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

                    // `?tag=` keeps events whose tags match (same syntax as mail search: lucia, =Family, none, -lucia)
                    let tag_filter = query_param(first_line, "tag").unwrap_or_default();
                    let body = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
                        Ok(db) => match db.get_events_for_month_by_account(&month, query_param(first_line, "account").as_deref()) {
                            Ok(events) => serde_json::to_string(
                                &events.iter().filter(|(e, _, _)| omalook::calendar_tags::matches(&e.tags, &tag_filter)).map(|(e, account_id, account_email)| serde_json::json!({
                                    "tags": e.tags,
                                    "account_id": account_id,
                                    "account_email": account_email,
                                    "id": e.id,
                                    "subject": e.subject,
                                    "body": e.body,
                                    "start": e.start,
                                    "end": e.end,
                                    "is_all_day": e.is_all_day,
                                    "time_zone": e.time_zone,
                                    "event_type": e.event_type,
                                    "series_master_id": e.series_master_id,
                                    "location": e.location,
                                    "organizer_name": e.organizer_name,
                                    "organizer_email": e.organizer_email,
                                    "is_organizer": e.is_organizer,
                                    "attendees": serde_json::from_str::<serde_json::Value>(&e.attendees_json).unwrap_or_else(|_| serde_json::json!([])),
                                    "my_response": e.my_response,
                                    "show_as": e.show_as,
                                    "online_provider": e.online_provider,
                                    "reminder_minutes": e.reminder_minutes,
                                    "web_link": e.web_link,
                                    "sensitivity": e.sensitivity,
                                    "join_url": omalook::join_link::detect(&e.online_url, &e.location, &e.body).map(|j| j.url),
                                    "join_provider": omalook::join_link::detect(&e.online_url, &e.location, &e.body).map(|j| j.provider),
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

                // ── Distribution lists (Settings → Lists; chips in the compose pane) ──────
                //   GET  /lists                                  → [{id, name, members:[{email,name}]}]
                //   POST /lists/create?name=N                    → {ok, status: done|exists|invalid, id}
                //   POST /lists/rename?id=I&name=N               → {ok, status}
                //   POST /lists/delete?id=I                      → {ok}
                //   POST /lists/members/add?id=I&email=E[&name=N] → {ok, status: done|exists|invalid|missing}
                //   POST /lists/members/remove?id=I&email=E      → {ok}
                //   GET  /lists/targets                          → [account id, ...] mirrored to besides this computer
                //   POST /lists/targets/set?account=A&on=true|false
                //   GET  /addresses                              → [{email, name, score}] seen in mail, best first
                if first_line.starts_with("GET /lists") || first_line.starts_with("POST /lists/") || first_line.starts_with("GET /addresses") {
                    let body = if first_line.starts_with("GET /addresses") { addresses_json(config_dir) } else { lists_response(config_dir, first_line) };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── Local rules (Settings -> Rules, "Rules OmaLook runs"; OL-040 phase 4d) ───
                //   GET  /local-rules → {ok, rules:[LocalRule + applied], accounts:[{id,provider,email,on_arrival}], tags:[..], limit}
                //   GET  /local-rules/preview?rule=<json>        → {ok, preview:{scanned,matched,messages,additions,by_tag,sample}}
                //   POST /local-rules/save?rule=<json>           create (blank id) or replace; validated; {ok,id}
                //   POST /local-rules/delete?id=R&confirm=yes · /enable?id=R&on=true|false · /move?id=R&dir=up|down
                //   POST /local-rules/apply?id=R[&confirm=yes]   this rule over existing Inbox mail; over `limit` messages
                //                                                it writes nothing ({outcome:{result:"refused",would_change,limit}}) until confirm=yes
                if first_line.starts_with("GET /local-rules") || first_line.starts_with("POST /local-rules/") {
                    // A retroactive apply scans and tags thousands of messages: its own thread, so it never holds up
                    // the other requests (or another rule's run).
                    if first_line.starts_with("POST /local-rules/apply") {
                        let (dir, line) = (config_dir.clone(), first_line.to_string());
                        std::thread::spawn(move || {
                            let body = local_rules_response(&dir, &line);
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                                body.len(), body
                            );
                            let _ = stream.write_all(response.as_bytes());
                        });
                        continue;
                    }
                    let body = local_rules_response(config_dir, first_line);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── Mailbox rules (Settings -> Rules; OL-040 phase 2, read-only) ─────────────
                //   GET /rules → {accounts:[{account_id,provider,email,state:ok|none|needs_signin|unsupported|error,error,fetched_at,total,
                //                            groups:[{tag,color,rules:[RuleSummary]}], other:[RuleSummary], system:[RuleSummary]}]}
                //   GET  /rules/preview?account=A&tag=T&address=E|domain=D  → {ok, preview:{matches,already_tagged,shadowed_by:[..],exists}}
                //   POST /rules/add?account=A&tag=T&address=E|domain=D[&enabled=false][&stop=false]   queue a new tag rule (appended last)
                //   POST /rules/enable?account=A&id=R&on=true|false
                //   POST /rules/delete?account=A&id=R&confirm=yes
                //   POST /rules/move?account=A&id=R&dir=up|down       swap with the neighbour in the list
                //   GET  /rules/run-preview?account=A[&id=R]  → {ok, preview:{scanned,matched,messages,additions,by_tag,..}, rules, left_out:[names], limit}
                //   POST /rules/run?account=A[&id=R][&confirm=yes]  run the enabled Exchange tag rules (or one) over cached Inbox mail now; tags added
                //                                              locally and pushed; over `limit` messages writes nothing until confirm=yes; never touches the server's rules
                //   Gmail accounts: GET /rules lists `filters:[{id,tags,text,editable,reason,rule}]` (rule = editable local-rule JSON) and no groups.
                //   GET  /rules/filter-preview?account=A&rule=<json>  → {ok, writable, reason, preview:{...}}  (Gmail refuses stop, several tags, subject/body, fragments)
                //   POST /rules/filter-add?account=A&rule=<json> · /filter-edit?account=A&id=F&rule=<json> (create new, check, then delete old) · /filter-delete?account=A&id=F&confirm=yes
                //   The write routes only queue ({ok, op}); the mail daemon does the checked write and the result shows in GET /rules `ops`
                //   ({id,kind,state:pending|done|failed|conflict|exists,message}). Plain tag rules only.
                if first_line.starts_with("GET /rules") || first_line.starts_with("POST /rules/") {
                    // Manual runs (count and run) work through the whole cached Inbox: each on its own thread, so
                    // several rules can run at once and nothing else waits for them.
                    if first_line.starts_with("GET /rules/run-preview") || first_line.starts_with("POST /rules/run") {
                        let (dir, line) = (config_dir.clone(), first_line.to_string());
                        std::thread::spawn(move || {
                            let body = rules_response(&dir, &line);
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                                body.len(), body
                            );
                            let _ = stream.write_all(response.as_bytes());
                        });
                        continue;
                    }
                    let body = rules_response(config_dir, first_line);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── Tags page (Settings → Tags; OL-040 phase 1) ──────────────────────────
                //   GET  /tags                                → {tags:[{name,color,total,accounts:[{account_id,email,provider,defined,pending,color_pending,color,messages}]}], palette}
                //   POST /tags/color?name=N&color=KEY[&account=A]   recolour on every account that has it (or A); pushed by the daemon
                //   POST /tags/delete?name=N&confirm=yes[&account=A] strip from messages, then delete on the provider
                //   POST /tags/adopt?name=N&to=A              make the tag on another account (same colour)
                //   GET  /tags/similar?name=N                 → {similar:[names that look like N]}
                if first_line.starts_with("GET /tags") || first_line.starts_with("POST /tags/") {
                    let body = tags_response(config_dir, first_line);
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

                // ── GET /compose/identities?account=ID, POST /compose/identity_default?account=ID&email= — send-as addresses (OL-051). Slow (provider call), own thread. ──
                if first_line.contains("GET /compose/identities") || first_line.contains("POST /compose/identity_default") {
                    let first = first_line.to_string();
                    let dir = config_dir.clone();
                    std::thread::spawn(move || {
                        let body = identities_response(&dir, &first);
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                            body.len(), body
                        );
                        let _ = stream.write_all(response.as_bytes());
                    });
                    continue;
                }

                // ── GET /messages/list-info?id=ID — List-Id / List-Post of a message (lazy provider fetch, cached). ──
                if first_line.contains("GET /messages/list-info") {
                    let id = query_param(first_line, "id").unwrap_or_default();
                    let dir = config_dir.clone();
                    std::thread::spawn(move || {
                        let body = list_info_json(&dir, &id);
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                            body.len(), body
                        );
                        let _ = stream.write_all(response.as_bytes());
                    });
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

                // ── POST /messages/invite?id=ID&choice=accept|tentative|decline|remove — answer an invitation (remove: drop a cancelled meeting from the calendar)
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

                // ── GET /messages/attachments?id=ID — the message's attachments (cached after the
                // first fetch); POST /messages/attachments/save?id=ID[&att=ATT] — save one, or all
                // non-inline ones, as individual files into the download folder. Own thread each. ──
                if first_line.contains("GET /messages/attachments") || first_line.contains("POST /messages/attachments/save") {
                    let saving = first_line.contains("POST /messages/attachments/save");
                    let id = query_param(first_line, "id").unwrap_or_default();
                    let att = query_param(first_line, "att");
                    let dir = config_dir.clone();
                    std::thread::spawn(move || {
                        let body = if saving { save_attachments_json(&dir, &id, att.as_deref()) } else { message_attachments_json(&dir, &id) };
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                            body.len(), body
                        );
                        let _ = stream.write_all(response.as_bytes());
                    });
                    continue;
                }

                // ── Render cache (OL-028): GET /render/get?id&width&scale&images · POST /render/adopt?id&width&scale&images&file ──
                if first_line.contains("GET /render/get") || first_line.contains("POST /render/adopt") {
                    let (dir, line) = (config_dir.clone(), first_line.to_string());
                    std::thread::spawn(move || {
                        let body = if line.contains("GET /render/get") { render_get_json(&dir, &line) } else { render_adopt_json(&dir, &line) };
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
                    let dir = config_dir.clone();
                    let line = first_line.to_string();
                    std::thread::spawn(move || {
                        let response = messages_response(&dir, &line, req_t);
                        let _ = stream.write_all(response.as_bytes());
                    });
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
                        Ok(text) => match toml::from_str::<omalook::models::Settings>(&text) {
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

                // ── GET /search/history — recent search patterns, newest first (JSON array);
                // POST /search/history?q=PATTERN — record one (called after it ran fine). ──
                if first_line.contains("GET /search/history") || first_line.contains("POST /search/history") {
                    let path = config_dir.join("search_history.json");
                    let list = if first_line.contains("POST ") {
                        match query_param(first_line, "q") {
                            Some(q) => omalook::search::push_history(&path, &q),
                            None => omalook::search::load_history(&path),
                        }
                    } else {
                        omalook::search::load_history(&path)
                    };
                    let body = serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string());
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── GET/POST /settings/view_state — where the user last was (see view_state.rs) ──
                if first_line.contains("GET /settings/view_state") || first_line.contains("POST /settings/view_state") {
                    let path = config_dir.join("view_state.json");
                    let (status, body) = if first_line.contains("POST ") {
                        let body_start = request.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
                        if omalook::view_state::save(&path, request[body_start..].trim()) { ("200 OK", "{}".to_string()) }
                        else { ("400 Bad Request", "{\"error\":\"not a flat JSON object\"}".to_string()) }
                    } else {
                        ("200 OK", omalook::view_state::load(&path))
                    };
                    let response = format!(
                        "HTTP/1.1 {}\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        status, body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── GET /settings/signatures — every account's signature: {account_id: text} (OL-050) ──
                // ── POST /settings/signature?account=ID with body {"text": "..."} — set or clear one (empty) ──
                if first_line.contains("GET /settings/signatures") || first_line.contains("POST /settings/signature") {
                    let settings_path = config_dir.join("settings.toml");
                    let result: std::result::Result<std::collections::BTreeMap<String, String>, String> = if first_line.contains("POST /settings/signature") {
                        let account = query_param(first_line, "account").unwrap_or_default();
                        let body_start = request.find("\r\n\r\n").map(|i| i + 4).unwrap_or(request.len());
                        let text = serde_json::from_str::<serde_json::Value>(request[body_start..].trim())
                            .ok()
                            .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()));
                        match (account.is_empty(), text) {
                            (true, _) => Err("missing account".to_string()),
                            (false, None) => Err("body must be {\"text\": \"...\"}".to_string()),
                            (false, Some(t)) => omalook::settings::write_signature(&settings_path, &account, &t).map_err(|e| e.to_string()),
                        }
                    } else {
                        Ok(omalook::settings::read_signatures(&settings_path))
                    };
                    let (status, body) = match result {
                        Ok(map) => ("200 OK", serde_json::to_string(&map).unwrap_or_else(|_| "{}".into())),
                        Err(e) => ("400 Bad Request", serde_json::json!({ "error": e }).to_string()),
                    };
                    let response = format!(
                        "HTTP/1.1 {}\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        status, body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── GET /settings/ui — return UI settings as JSON ─────────────
                if first_line.contains("GET /settings/ui") {
                    let settings_path = config_dir.join("settings.toml");
                    let body = match std::fs::read_to_string(&settings_path)
                        .ok()
                        .and_then(|s| toml::from_str::<omalook::models::Settings>(&s).ok())
                    {
                        Some(settings) => format!(
                            "{{\"sidebar_expanded\":{},\"window_width\":{},\"window_height\":{},\"message_rendering\":\"{}\",\"folder_pane\":{},\"list_width_offset\":{},\"calendar_pane_offset\":{},\"scale_factor\":{}}}",
                            settings.ui.sidebar_expanded,
                            settings.ui.window_width,
                            settings.ui.window_height,
                            omalook::settings::normalize_message_rendering(&settings.ui.message_rendering).unwrap_or("system_sender"),
                            settings.ui.folder_pane,
                            settings.ui.list_width_offset,
                            settings.ui.calendar_pane_offset,
                            omalook::settings::normalize_scale_factor(settings.font.scale_factor),
                        ),
                        None => "{\"sidebar_expanded\":true,\"window_width\":1280,\"window_height\":800,\"message_rendering\":\"system_sender\",\"folder_pane\":false,\"list_width_offset\":0,\"calendar_pane_offset\":0,\"scale_factor\":1.0}".to_string(),
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── /settings/mcp… — Settings → MCP (state + changes); see src/mcp/admin.rs ──
                if first_line.contains(" /settings/mcp") {
                    let method = if first_line.starts_with("POST") { "POST" } else { "GET" };
                    let path = first_line.split_whitespace().nth(1).unwrap_or("").split('?').next().unwrap_or("");
                    let line = first_line.to_string();
                    let dir = config_dir.clone();
                    let path = path.to_string();
                    std::thread::spawn(move || {
                        let body = omalook::mcp::admin::handle(&dir, method, &path, &|k| query_param(&line, k))
                            .unwrap_or_else(|| "{\"error\":\"unknown route\"}".to_string());
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                            body.len(), body
                        );
                        let _ = stream.write_all(response.as_bytes());
                    });
                    continue;
                }

                // ── GET|POST /settings/background?enabled=true|false — keep the backend running
                // after the UI window closes. Takes effect when the window closes; no restart. ──
                if first_line.contains("GET /settings/background") || first_line.contains("POST /settings/background") {
                    let settings_path = config_dir.join("settings.toml");
                    let enabled = if first_line.contains("POST /settings/background") {
                        let cur = omalook::settings::read_run_in_background(&settings_path);
                        let wanted = query_param(first_line, "enabled").map(|v| v == "true").unwrap_or(cur);
                        match omalook::settings::write_run_in_background(&settings_path, wanted) {
                            Ok(stored) => stored,
                            Err(e) => {
                                warn!("POST /settings/background: could not save: {}", e);
                                cur
                            }
                        }
                    } else {
                        omalook::settings::read_run_in_background(&settings_path)
                    };
                    let body = serde_json::json!({ "run_in_background": enabled }).to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── POST /settings/calendar_pane_offset (body: integer, characters) — calendar sidebar width.
                //    Must stay above /settings/calendar below: that route matches by prefix. ──
                if first_line.contains("POST /settings/calendar_pane_offset") {
                    let body_start = request.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
                    let cols: i32 = request[body_start..].trim().parse::<i32>().unwrap_or(0).clamp(-200, 400);
                    let settings_path = config_dir.join("settings.toml");
                    let ok = std::fs::read_to_string(&settings_path)
                        .ok()
                        .and_then(|s| toml::from_str::<omalook::models::Settings>(&s).ok())
                        .and_then(|mut settings| {
                            settings.ui.calendar_pane_offset = cols;
                            toml::to_string_pretty(&settings).ok().map(|content| std::fs::write(&settings_path, content))
                        })
                        .is_some();
                    debug!("POST /settings/calendar_pane_offset {} → {}", cols, ok);
                    let response = "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\nok";
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }

                // ── GET|POST /settings/calendar — recurring-meeting window (years back / ahead) ──
                if first_line.contains("GET /settings/calendar") || first_line.contains("POST /settings/calendar") {
                    use omalook::models::CalendarSettings;
                    let settings_path = config_dir.join("settings.toml");
                    let current = if first_line.contains("POST /settings/calendar") {
                        let cur = omalook::settings::read_calendar_settings(&settings_path);
                        let wanted = CalendarSettings {
                            recurrence_years_back: query_param(first_line, "back").and_then(|v| v.parse().ok()).unwrap_or(cur.recurrence_years_back),
                            recurrence_years_ahead: query_param(first_line, "ahead").and_then(|v| v.parse().ok()).unwrap_or(cur.recurrence_years_ahead),
                            default_minutes: query_param(first_line, "minutes").and_then(|v| v.parse().ok()).unwrap_or(cur.default_minutes),
                            default_reminder_minutes: query_param(first_line, "reminder").and_then(|v| v.parse().ok()).unwrap_or(cur.default_reminder_minutes),
                            default_series_months: query_param(first_line, "series_months").and_then(|v| v.parse().ok()).unwrap_or(cur.default_series_months),
                            default_account: query_param(first_line, "account").unwrap_or(cur.default_account.clone()),
                        };
                        match omalook::settings::write_calendar_settings(&settings_path, &wanted) {
                            Ok(stored) => stored,
                            Err(e) => {
                                warn!("POST /settings/calendar: could not save: {}", e);
                                cur
                            }
                        }
                    } else {
                        omalook::settings::read_calendar_settings(&settings_path)
                    };
                    let body = serde_json::json!({
                        "recurrence_years_back": current.recurrence_years_back,
                        "recurrence_years_ahead": current.recurrence_years_ahead,
                        "default_minutes": current.default_minutes,
                        "default_reminder_minutes": current.default_reminder_minutes,
                        "default_series_months": current.default_series_months,
                        "default_account": current.default_account,
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

                // ── POST /settings/scale_factor?value=1.1 — UI scale, clamped, stored in [font] ──
                if first_line.contains("POST /settings/scale_factor") {
                    let wanted = query_param(first_line, "value").and_then(|v| v.parse::<f64>().ok());
                    let body = match wanted.map(|w| omalook::settings::write_scale_factor(&config_dir.join("settings.toml"), w)) {
                        Some(Ok(stored)) => format!("{{\"ok\":true,\"scale_factor\":{}}}", stored),
                        other => {
                            if let Some(Err(e)) = other { warn!("POST /settings/scale_factor {:?}: {}", wanted, e); }
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

                // ── POST /settings/message_rendering?value=html|system|system_sender ──
                if first_line.contains("POST /settings/message_rendering") {
                    let wanted = query_param(first_line, "value").unwrap_or_default();
                    let body = match omalook::settings::write_message_rendering(&config_dir.join("settings.toml"), &wanted) {
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
                        .and_then(|s| toml::from_str::<omalook::models::Settings>(&s).ok())
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
                        .and_then(|s| toml::from_str::<omalook::models::Settings>(&s).ok())
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

                // ── POST /settings/list_width_offset (body: integer, characters) — message list width ──
                if first_line.contains("POST /settings/list_width_offset") {
                    let body_start = request.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
                    let cols: i32 = request[body_start..].trim().parse::<i32>().unwrap_or(0).clamp(-200, 400);
                    let settings_path = config_dir.join("settings.toml");
                    let ok = std::fs::read_to_string(&settings_path)
                        .ok()
                        .and_then(|s| toml::from_str::<omalook::models::Settings>(&s).ok())
                        .and_then(|mut settings| {
                            settings.ui.list_width_offset = cols;
                            toml::to_string_pretty(&settings).ok().map(|content| std::fs::write(&settings_path, content))
                        })
                        .is_some();
                    debug!("POST /settings/list_width_offset {} → {}", cols, ok);
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
                                if let Err(e) = omalook::account_ops::begin_add_account_with_hint(&config_dir, &provider, email.as_deref(), scheduler) {
                                    error!("❌ Add account failed: {}", e);
                                }
                            });
                        }
                    }
                    "cancel" => {
                        info!("🔔 HTTP trigger: cancel sign-in");
                        omalook::google_auth::cancel_login(config_dir);
                    }
                    "logout" | "remove" => {
                        let id = query_param(first_line, "account")
                            .unwrap_or_else(|| omalook::token_store::DEFAULT_ACCOUNT.to_string());
                        info!("🔔 HTTP trigger: {} account {}", action, id);
                        if let Some(scheduler) = SCHEDULER.get() {
                            let result = if action == "remove" {
                                omalook::account_ops::remove_account(config_dir, &id, scheduler)
                            } else {
                                omalook::account_ops::sign_out_account(config_dir, &id, scheduler)
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
