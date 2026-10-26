//! Keep the backend (mail/calendar/SMS sync, HTTP server) running after the UI window closes.
//!
//! The `omalook` process hosts the backend and runs the UI (`quickshell -p <qml>`) as a child.
//! With `[sync] run_in_background = true` the process stays alive once that child exits
//! ("headless"). Starting OmaLook again then finds the backend answering on its port and only
//! opens a UI against it ("attach"). Turning the setting off lets a headless backend exit as
//! soon as no UI is open.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

pub const DEFAULT_PORT: u16 = 27182;

/// Port of the backend's HTTP server (`OMALOOK_PORT` is for tests running a second instance).
pub fn http_port() -> u16 {
    std::env::var("OMALOOK_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT)
}

/// True when an OmaLook backend answers on `port`. Checks the reply, not just the connection,
/// so some other program holding the port is not mistaken for a backend.
pub fn backend_listening(port: u16) -> bool {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else { return false };
    let _ = s.set_read_timeout(Some(Duration::from_millis(800)));
    let _ = s.set_write_timeout(Some(Duration::from_millis(800)));
    if s.write_all(b"GET /sync/serial HTTP/1.1\r\nHost: localhost\r\n\r\n").is_err() { return false; }
    let mut buf = [0u8; 256];
    let n = s.read(&mut buf).unwrap_or(0);
    let reply = String::from_utf8_lossy(&buf[..n]);
    reply.starts_with("HTTP/1.1 200") && reply.contains("\"mail\"")
}

/// Whether `cmdline` (NUL-separated, as in /proc/<pid>/cmdline) is a `quickshell -p <qml_dir>` UI.
fn is_ui_cmdline(cmdline: &[u8], qml_dir: &str) -> bool {
    let args: Vec<&[u8]> = cmdline.split(|b| *b == 0).filter(|a| !a.is_empty()).collect();
    let Some(first) = args.first() else { return false };
    let exe = String::from_utf8_lossy(first);
    if Path::new(exe.as_ref()).file_name().and_then(|n| n.to_str()) != Some("quickshell") { return false; }
    args.windows(2).any(|w| w[0] == b"-p" && w[1] == qml_dir.as_bytes())
}

/// True while any OmaLook UI window process (`quickshell -p <qml_dir>`) is running.
pub fn ui_running(qml_dir: &Path) -> bool {
    let dir = qml_dir.display().to_string();
    let Ok(entries) = std::fs::read_dir("/proc") else { return false };
    entries.flatten().any(|e| {
        e.file_name().to_string_lossy().chars().all(|c| c.is_ascii_digit())
            && std::fs::read(e.path().join("cmdline")).map(|c| is_ui_cmdline(&c, &dir)).unwrap_or(false)
    })
}

/// Called when this process's UI has closed. With the setting on, blocks for as long as the
/// backend should keep running (forever, until the setting is turned off and no UI is open) and
/// returns afterwards so `main` can exit. With the setting off, returns at once.
pub fn stay_while_enabled(settings_path: &Path, qml_dir: &Path, poll: Duration) {
    if !crate::settings::read_run_in_background(settings_path) { return; }
    log::info!("UI closed; staying in the background (Settings → General → Keep running in background)");
    loop {
        std::thread::sleep(poll);
        if !crate::settings::read_run_in_background(settings_path) && !ui_running(qml_dir) {
            log::info!("Background running turned off and no UI open; exiting");
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_the_ui_process_only() {
        let ok = b"quickshell\0-p\0/home/user/omalook/qml\0";
        assert!(is_ui_cmdline(ok, "/home/user/omalook/qml"));
        assert!(is_ui_cmdline(b"/usr/bin/quickshell\0-p\0/x/qml\0", "/x/qml"));
        assert!(!is_ui_cmdline(ok, "/other/qml"), "a different quickshell instance");
        assert!(!is_ui_cmdline(b"omalook\0-p\0/x/qml\0", "/x/qml"), "not quickshell");
        assert!(!is_ui_cmdline(b"", "/x/qml"));
    }

    #[test]
    fn backend_detection_needs_a_real_answer() {
        use std::net::TcpListener;
        // nothing listening
        let free = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = free.local_addr().unwrap().port();
        drop(free);
        assert!(!backend_listening(port));
        // a listener that answers like the backend
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for s in l.incoming().take(2) {
                let mut s = s.unwrap();
                let mut b = [0u8; 256];
                let _ = s.read(&mut b);
                let body = if s.local_addr().is_ok() { "{\"mail\":1,\"sms\":0}" } else { "" };
                let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}", body.len(), body).as_bytes());
            }
        });
        assert!(backend_listening(port));
    }

    #[test]
    fn stays_only_while_enabled_and_exits_once_disabled() {
        let p = std::env::temp_dir().join(format!("omalook-bg-{}.toml", std::process::id()));
        std::fs::write(&p, "[sync]\nrun_in_background = false\n").unwrap();
        // disabled: returns immediately
        stay_while_enabled(&p, Path::new("/nonexistent/qml"), Duration::from_millis(10));
        // enabled: blocks until the file flips
        std::fs::write(&p, "[sync]\nrun_in_background = true\n").unwrap();
        let p2 = p.clone();
        let flip = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            std::fs::write(&p2, "[sync]\nrun_in_background = false\n").unwrap();
        });
        let t = std::time::Instant::now();
        stay_while_enabled(&p, Path::new("/nonexistent/qml"), Duration::from_millis(20));
        assert!(t.elapsed() >= Duration::from_millis(150), "must have stayed while enabled");
        flip.join().unwrap();
        let _ = std::fs::remove_file(&p);
    }
}
