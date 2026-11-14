//! Main application entry point (Rust + QML)
//!
//! OmaLook: Lightweight Outlook clone
//! - Rust backend: Auth, Graph API, SQLite cache
//! - QML frontend: Native Qt UI with hot-reload support

use omalook::{init_logging, AuthManager, Database, SettingsManager};
use log::{debug, error, info, warn};
use std::env;
use std::path::PathBuf;
use std::io::Write;
use std::net::TcpListener;
use std::sync::Arc;

/// Process-wide sync scheduler (one mail + one calendar thread per signed-in account); lives in
/// the library so moved route groups reach it too.
use omalook::scheduler::GLOBAL as SCHEDULER;

fn main() {
    // R2.6: owner-only files and dirs from here on. Before anything else creates a file.
    omalook::fsperm::set_private_umask();

    // `omalook mcp`: stdio MCP forwarder to the running daemon. Not a backend; stdout is the protocol.
    if env::args().nth(1).as_deref() == Some("mcp") {
        omalook::mcp::shim::run(&get_config_dir());
        return;
    }

    // Initialize logging
    init_logging();
    let config_dir = get_config_dir();
    // R2.6: tighten existing config/cache dirs and files (never follows symlinks). Failure is not
    // fatal: the backend still runs, and the umask above covers anything it creates.
    {
        let cache_dir = dirs::cache_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join("omalook");
        if let Err(e) = omalook::fsperm::harden_permissions(&config_dir, &cache_dir) {
            warn!("could not tighten permissions on config/cache dirs: {}", e);
        }
    }

    // A backend left running in the background (Settings → General) already serves the UI:
    // just open a window against it instead of starting a second backend. Its session token is in
    // the runtime file it wrote at start (http::session).
    let port = omalook::background::http_port();
    let running_token = omalook::http::session::read(&config_dir, port);
    if omalook::background::backend_listening(port, running_token.as_deref()) {
        info!("OmaLook backend already running; opening the UI only");
        launch_qml_app(&config_dir, running_token.as_deref().unwrap_or_default());
        return;
    }

    // Fail closed (SEC-015): without our own port there is no backend, and a UI must never talk to
    // whatever else holds it. Bound here, before anything starts, so a refusal changes nothing.
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("❌ OmaLook cannot start: port 127.0.0.1:{} is not available ({}).", port, e);
            eprintln!("   Another program holds it, or an OmaLook backend that this launch cannot verify.");
            error!("HTTP: bind 127.0.0.1:{} failed: {}; exiting", port, e);
            std::process::exit(2);
        }
    };
    // Per-launch session token (R1.4): the UI gets it in its environment; every request must carry it.
    let token = match omalook::http::session::create(&config_dir, port) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("❌ OmaLook cannot start: could not write its session token ({}).", e);
            error!("HTTP: session token not written: {}; exiting", e);
            std::process::exit(1);
        }
    };

    omalook::perf::init(&config_dir);
    info!("OmaLook starting");
    
    let db_path = config_dir.join("omarchy.db");
    let settings_path = config_dir.join("settings.toml");
    
    info!("Config dir: {}", config_dir.display());
    info!("Database: {}", db_path.display());
    
    // Initialize core backend components
    match initialize_app(&config_dir, db_path.to_str().unwrap(), settings_path.to_str().unwrap(), listener, port, token.clone()) {
        Ok(_) => {
            info!("Application backend initialized successfully");
            
            // Launch Qt/QML runtime (returns when the window closes)
            launch_qml_app(&config_dir, &token);

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
fn launch_qml_app(config_dir: &PathBuf, token: &str) {
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
        // The backend's session token: Backend.qml sends it on every request (http::guard).
        .env(omalook::http::session::ENV, token)
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

fn initialize_app(config_dir: &PathBuf, db_path: &str, settings_path: &str, listener: TcpListener, port: u16, token: String) -> omalook::errors::Result<()> {
    // R3.9: before anything opens the mail cache, rebuild it if the user asked or it cannot be read;
    // a full quick_check runs in the background a minute after start-up.
    let mail_db = config_dir.join("messages.db");
    omalook::db_health::prepare_at_startup(config_dir, &mail_db);
    omalook::db_health::check_in_background(mail_db, std::time::Duration::from_secs(60));

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
        start_http_trigger_server(&config_dir_http, listener, port, token);
    });
    
    // Pre-render: mail from "always HTML" senders is drawn ahead of time into the render cache (OL-028).
    let config_dir_render = config_dir.clone();
    omalook::supervisor::spawn_supervised("prerender", None, || false, move || {
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

/// Path of the WebKit render helper (the reading pane looks in the same place).
fn render_helper_path() -> PathBuf { resolve_qml_dir().join("..").join("bin").join("omalook-render") }

/// The UI's backend HTTP server on 127.0.0.1:27182 (`OMALOOK_PORT` overrides).
/// Every UI request comes from the one QML client, `qml/state/Backend.qml`.
/// `listener` was bound by `main` (which exits when it cannot bind); `token` is this start's session token.
fn start_http_trigger_server(config_dir: &PathBuf, listener: TcpListener, port: u16, token: String) {
    // OMALOOK_PORT is for tests that run a second instance beside the real one; the UI's
    // Backend.qml reads the same variable, so a scratch UI talks only to its own backend.
    info!("🌐 HTTP server listening on http://127.0.0.1:{}", port);
    omalook::perf::mark("http server bound");
    let token = Arc::new(token);

    // One thread per connection (at most http::MAX_CONNECTIONS at once): the request is read there,
    // with timeouts and size caps, so an idle or slow client no longer freezes the other routes (SEC-004).
    let config_dir = Arc::new(config_dir.clone());
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let Some(slot) = omalook::http::Slot::try_take() else {
                    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(1)));
                    let _ = stream.write_all(omalook::http::error_response("503 Service Unavailable").as_bytes());
                    continue;
                };
                let config_dir = Arc::clone(&config_dir);
                let token = Arc::clone(&token);
                let spawned = std::thread::Builder::new().name("http-conn".into()).spawn(move || {
                    let _slot = slot;
                    handle_http_connection(&config_dir, port, &token, stream);
                });
                if let Err(e) = spawned { warn!("HTTP: could not start a connection thread: {}", e); }
            }
            Err(e) => warn!("HTTP trigger server accept error: {}", e),
        }
    }
}

/// Read one request from a UI connection, check who sent it, and route it. Requests that cannot be
/// read get an error status (or nothing, when the client sent nothing); requests from a browser or
/// to another host name get 403 (`http::guard`, R1.3), requests without the session token 401 (R1.4).
/// Either way the connection is then closed.
fn handle_http_connection(config_dir: &PathBuf, port: u16, token: &str, mut stream: std::net::TcpStream) {
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(10)));
    let limits = omalook::http::request::Limits::default();
    let req = match omalook::http::request::read_request(&mut stream, &limits) {
        Ok(r) => r,
        Err(e) => {
            if let Some(status) = e.status() {
                debug!("HTTP: request refused ({:?})", e);
                let _ = stream.write_all(omalook::http::error_response(status).as_bytes());
            }
            return;
        }
    };
    if let Err(why) = omalook::http::guard::check(&req, port, token) {
        // Method + route only: query strings may hold addresses or search terms.
        warn!("HTTP: refused {} {} ({:?})", req.method, req.path(), why);
        let _ = stream.write_all(omalook::http::error_response(why.status()).as_bytes());
        return;
    }
    if req.path() != "/perf" {
        omalook::perf::mark(&format!("http <- {}", omalook::perf::route_name(&req.first_line())));
    }
    use omalook::http::router::{self, Found};
    match router::table().find(&req.method, req.path()) {
        Found::Route(route) => {
            let _guard = router::lock_for(route);
            let (bytes, after) = (route.handler)(&router::Ctx::new(config_dir, &req)).into_parts();
            let _ = stream.write_all(&bytes);
            drop(stream);
            if let Some(f) = after { f(); }
        }
        // Unknown path, or a known path with another method: a 404 whose body is `ok`, kept byte for byte.
        Found::WrongMethod | Found::NotFound => {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\nok");
        }
    }
}

