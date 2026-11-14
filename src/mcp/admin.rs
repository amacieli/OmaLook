//! The Settings → MCP page's backend: state for the page and the changes it can make, served on
//! the UI's own port (27182) under `/settings/mcp`. Not part of MCP itself.
//!
//! Every change is a POST with a JSON object body (R1.6: nothing is read from the query string):
//! GET  /settings/mcp                              → state (below)
//! POST /settings/mcp/enabled   {"value": true|false}        → master switch
//! POST /settings/mcp/domain    {"domain": "mail", "level": "off|read|write"}
//! POST /settings/mcp/tool      {"name": "mail_send", "mode": "off|ask|allow|default"}
//! POST /settings/mcp/account   {"id": ID, "value": true|false} → allow / disallow an account
//! POST /settings/mcp/preset    {"name": "locked|read_only|approval"}
//! POST /settings/mcp/token/regenerate
//! POST /settings/mcp/copy      {"what": "stdio|hermes|token"} → puts text on the clipboard (the
//!                                                      token never leaves the daemon)
//! POST /settings/mcp/confirm   {"id": ID}           → apply the pending wider change (`confirm.rs`)
//! POST /settings/mcp/cancel    {"id": ID}           → drop it
//! GET|POST /settings/background {"enabled": bool}   → `handle_background`
//!
//! A change that would let agents do more (`confirm::widening`), a token copy and a token
//! regenerate are not applied: they wait in `confirm` until the UI's dialog confirms them.
//! Every reply is the full state, plus `"error"` when the request was refused, plus `"pending":
//! {"id", "summary": [..]}` while a change is waiting (whoever requested it).

use super::confirm::{self, Change};
use super::policy::{self, Class, Domain, Level, McpSettings, Mode};
use super::{registry, token, Ctx};
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// `Some(body)` when `path` is a `/settings/mcp` route; `None` otherwise.
/// `body` is the parsed JSON request body (`Value::Null` for none); parameters come only from it.
pub fn handle(config_dir: &Path, method: &str, path: &str, body: &Value) -> Option<String> {
    let rest = path.strip_prefix("/settings/mcp")?;
    if !(rest.is_empty() || rest.starts_with('/')) { return None; }
    let q = |k: &str| param(body, k);
    let result = match (method, rest) {
        ("GET", "") => Ok(()),
        ("POST", "/confirm") => confirm_pending(config_dir, &q("id").unwrap_or_default()),
        ("POST", "/cancel") => {
            if confirm::cancel(config_dir, &q("id").unwrap_or_default()) { Ok(()) } else { Err("that confirmation is not the one waiting".into()) }
        }
        ("POST", r) => apply(config_dir, r, &q),
        _ => Err("unknown MCP settings route".to_string()),
    };
    let mut state = state(config_dir);
    if let Err(e) = result { state["error"] = json!(e); }
    Some(state.to_string())
}

/// A body parameter as text: strings as given (empty = missing), booleans as "true"/"false".
fn param(body: &Value, k: &str) -> Option<String> {
    match body.get(k) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Bool(b)) => Some(b.to_string()),
        _ => None,
    }
}

fn pending_json(config_dir: &Path) -> Value {
    match confirm::current(config_dir) {
        Some((id, summary)) => json!({ "id": id, "summary": summary }),
        None => Value::Null,
    }
}

/// Apply `next` now if it widens nothing; otherwise park it for the UI to confirm.
fn commit(config_dir: &Path, base: McpSettings, next: McpSettings) -> Result<(), String> {
    let bg = crate::settings::read_run_in_background(&config_dir.join("settings.toml"));
    let wider = confirm::widening(&base, bg, &next, bg);
    if wider.is_empty() { return save(config_dir, &next); }
    confirm::propose(config_dir, Change::Settings { base, next }, wider);
    Ok(())
}

fn confirm_pending(config_dir: &Path, id: &str) -> Result<(), String> {
    let settings_path = config_dir.join("settings.toml");
    let changed = || "the MCP settings changed meanwhile; make the change again".to_string();
    let result = match confirm::take(config_dir, id)? {
        Change::Settings { base, next } => {
            if load(config_dir)? != base { return Err(changed()); }
            save(config_dir, &next)
        }
        Change::Background { base, next } => {
            if load(config_dir)? != base { return Err(changed()); }
            crate::settings::write_run_in_background(&settings_path, next).map(|_| ()).map_err(|e| e.to_string())
        }
        Change::RegenerateToken => token::regenerate(config_dir).map(|_| ()).ok_or_else(|| "could not write the token file".to_string()),
        Change::CopyToken => copy(config_dir, "token"),
    };
    if result.is_ok() { log::info!("MCP settings: a wider permission was confirmed in the UI"); }
    result
}

/// `GET|POST /settings/background` (body `{"enabled": bool}`): keep the backend running after the
/// window closes. Turning it on activates MCP when `[mcp] enabled` is set, so that case waits for
/// confirmation like any other widening. Answers `{"run_in_background", "pending"[, "error"]}`.
pub fn handle_background(config_dir: &Path, method: &str, body: &Value) -> String {
    let settings_path = config_dir.join("settings.toml");
    let cur = crate::settings::read_run_in_background(&settings_path);
    let mut error: Option<String> = None;
    if method == "POST" {
        match body.get("enabled").and_then(|v| v.as_bool()) {
            None => error = Some("missing enabled".into()),
            Some(wanted) if wanted != cur => {
                let s = crate::settings::read_mcp_settings(&settings_path).unwrap_or_default();
                let wider = confirm::widening(&s, cur, &s, wanted);
                if wider.is_empty() {
                    if let Err(e) = crate::settings::write_run_in_background(&settings_path, wanted) {
                        log::warn!("POST /settings/background: could not save: {}", e);
                        error = Some("could not save the setting".into());
                    }
                } else {
                    confirm::propose(config_dir, Change::Background { base: s, next: wanted }, wider);
                }
            }
            Some(_) => {}
        }
    }
    let mut out = json!({
        "run_in_background": crate::settings::read_run_in_background(&settings_path),
        "pending": pending_json(config_dir),
    });
    if let Some(e) = error { out["error"] = json!(e); }
    out.to_string()
}

fn load(config_dir: &Path) -> Result<McpSettings, String> {
    crate::settings::read_mcp_settings(&config_dir.join("settings.toml"))
        .ok_or_else(|| "settings.toml cannot be read; fix it before changing MCP settings".to_string())
}

fn save(config_dir: &Path, s: &McpSettings) -> Result<(), String> {
    crate::settings::write_mcp_settings(&config_dir.join("settings.toml"), s).map(|_| ()).map_err(|e| e.to_string())
}

fn account_ids(config_dir: &Path) -> Vec<String> {
    let Ok(c) = rusqlite::Connection::open_with_flags(config_dir.join("messages.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) else { return vec![] };
    let Ok(mut st) = c.prepare("SELECT id FROM accounts") else { return vec![] };
    st.query_map([], |r| r.get::<_, String>(0)).map(|it| it.flatten().collect()).unwrap_or_default()
}

fn apply(config_dir: &Path, route: &str, q: &dyn Fn(&str) -> Option<String>) -> Result<(), String> {
    let arg = |k: &str| q(k).ok_or_else(|| format!("missing {}", k));
    let on = |k: &str| arg(k).map(|v| v == "true");
    match route {
        "/enabled" => { let base = load(config_dir)?; let mut s = base.clone(); s.enabled = on("value")?; commit(config_dir, base, s) }
        "/domain" => {
            let (d, l) = (arg("domain")?, arg("level")?);
            let domain = Domain::ALL.iter().find(|x| x.key() == d).ok_or("unknown domain")?;
            if !matches!(l.as_str(), "off" | "read" | "write") { return Err("level must be off, read or write".into()); }
            let l = if *domain == Domain::Accounts { Level::parse(&l).min(Level::Read).key().to_string() } else { l };
            let base = load(config_dir)?;
            let mut s = base.clone();
            s.domains.insert(d, l);
            commit(config_dir, base, s)
        }
        "/tool" => {
            let (name, mode) = (arg("name")?, arg("mode")?);
            let spec = registry::find(&name).ok_or("unknown tool")?;
            if !spec.class.is_write() { return Err("read tools are always allowed; only write tools can be changed".into()); }
            let base = load(config_dir)?;
            let mut s = base.clone();
            if mode == "default" { s.tools.remove(&name); }
            else if Mode::parse(&mode).is_some() { s.tools.insert(name, mode); }
            else { return Err("mode must be off, ask, allow or default".into()); }
            commit(config_dir, base, s)
        }
        "/account" => {
            let (id, allow) = (arg("id")?, on("value")?);
            let base = load(config_dir)?;
            let mut s = base.clone();
            if allow {
                if !account_ids(config_dir).contains(&id) { return Err("unknown account".into()); }
                if !s.accounts.contains(&id) { s.accounts.push(id); }
            } else {
                s.accounts.retain(|a| *a != id);
            }
            commit(config_dir, base, s)
        }
        "/preset" => {
            let s = load(config_dir)?;
            let p = policy::with_preset(&s, &arg("name")?).ok_or("unknown preset")?;
            commit(config_dir, s, p)
        }
        "/token/regenerate" => {
            confirm::propose(config_dir, Change::RegenerateToken, vec!["a new MCP token: clients using the old one stop working".into()]);
            Ok(())
        }
        "/copy" => match arg("what")?.as_str() {
            "token" => {
                confirm::propose(config_dir, Change::CopyToken, vec!["the MCP token is put on the clipboard, where other programs can read it".into()]);
                Ok(())
            }
            other => copy(config_dir, other),
        },
        _ => Err("unknown MCP settings route".into()),
    }
}

/// Client configuration for the stdio command (no secret in it: the command reads the token file).
pub fn stdio_json(exe: &str) -> String {
    serde_json::to_string_pretty(&json!({ "mcpServers": { "omalook": { "command": exe, "args": ["mcp"] } } })).unwrap_or_default()
}

pub fn hermes_yaml(exe: &str) -> String {
    format!("mcp_servers:\n  omalook:\n    command: {}\n    args: [\"mcp\"]\n", serde_json::to_string(exe).unwrap_or_default())
}

fn exe_path() -> String {
    std::env::current_exe().ok().map(|p| p.display().to_string()).unwrap_or_else(|| "omalook".into())
}

fn copy(config_dir: &Path, what: &str) -> Result<(), String> {
    let text = match what {
        "stdio" => stdio_json(&exe_path()),
        "hermes" => hermes_yaml(&exe_path()),
        "token" => token::load_or_create(config_dir).ok_or("could not read the token file")?,
        _ => return Err("nothing to copy".into()),
    };
    clipboard(&text)
}

/// Put text on the Wayland (or X11) clipboard. Closes the pipe before returning so the tool sees EOF
/// (the known hang, OL-005, is exactly a child kept waiting on an open stdin).
fn clipboard(text: &str) -> Result<(), String> {
    for (cmd, args) in [("wl-copy", vec![]), ("xclip", vec!["-selection", "clipboard"])] {
        let Ok(mut child) = Command::new(cmd).args(&args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else { continue };
        if let Some(mut stdin) = child.stdin.take() {
            if stdin.write_all(text.as_bytes()).is_err() { let _ = child.kill(); continue; }
        } // stdin dropped here
        std::thread::spawn(move || { let _ = child.wait(); });
        return Ok(());
    }
    Err("no clipboard tool found (install wl-clipboard)".into())
}

/// Everything the page shows. No secrets.
pub fn state(config_dir: &Path) -> Value {
    let ctx = Ctx::new(config_dir);
    let path = ctx.settings_path();
    let bg = crate::settings::read_run_in_background(&path);
    let parsed = crate::settings::read_mcp_settings(&path);
    let readable = parsed.is_some();
    let s = parsed.unwrap_or_default();
    let policy = policy::Policy::new(s.clone(), bg);

    let levels: serde_json::Map<String, Value> = Domain::ALL
        .iter()
        .map(|d| {
            let lv = s.domains.get(d.key()).map(|v| Level::parse(v)).unwrap_or(Level::Off);
            let lv = if *d == Domain::Accounts { lv.min(Level::Read) } else { lv };
            (d.key().to_string(), json!(lv.key()))
        })
        .collect();

    let tools: Vec<Value> = registry::TOOLS
        .iter()
        .filter(|t| t.class.is_write())
        .map(|t| {
            let default = t.class.default_mode();
            let mode = s.tools.get(t.name).and_then(|v| Mode::parse(v)).unwrap_or(default);
            json!({
                "name": t.name, "domain": t.domain.key(), "description": t.description, "implemented": t.implemented,
                "class": match t.class { Class::R => "R", Class::W1 => "W1", Class::W2 => "W2", Class::W3 => "W3" },
                "mode": mode.key(), "default": default.key(),
            })
        })
        .collect();
    let read_counts: serde_json::Map<String, Value> = Domain::ALL
        .iter()
        .map(|d| (d.key().to_string(), json!(registry::TOOLS.iter().filter(|t| t.domain == *d && !t.class.is_write() && t.implemented).count())))
        .collect();

    let port = super::mcp_port();
    let listening = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), std::time::Duration::from_millis(150)).is_ok();
    json!({
        "settings_readable": readable,
        "run_in_background": bg,
        "enabled": s.enabled,
        "active": policy.active(),
        "listening": listening,
        "preset": policy::preset_of(&s),
        "domains": levels,
        "read_tools": read_counts,
        "tools": tools,
        "accounts": s.accounts,
        "http_url": format!("http://127.0.0.1:{}/mcp", port),
        "command": format!("{} mcp", exe_path()),
        "pending": pending_json(config_dir),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn dir(name: &str, bg: bool) -> PathBuf {
        let d = std::env::temp_dir().join(format!("omalook-mcp-admin-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        crate::settings::write_run_in_background(&d.join("settings.toml"), bg).unwrap();
        drop(crate::db::Database::open(d.join("messages.db").to_str().unwrap()).unwrap());
        rusqlite::Connection::open(d.join("messages.db")).unwrap()
            .execute_batch("INSERT INTO accounts (id, provider, email) VALUES ('graph-1','graph','a@x.test'), ('gmail-2','gmail','b@x.test');").unwrap();
        d
    }

    fn post(d: &Path, path: &str, kv: &[(&str, &str)]) -> Value {
        let body: serde_json::Map<String, Value> = kv.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
        serde_json::from_str(&handle(d, "POST", path, &Value::Object(body)).unwrap()).unwrap()
    }

    fn get(d: &Path) -> Value { serde_json::from_str(&handle(d, "GET", "/settings/mcp", &Value::Null).unwrap()).unwrap() }

    #[test]
    fn other_paths_are_not_ours() {
        let d = dir("path", true);
        assert!(handle(&d, "GET", "/settings/mail", &Value::Null).is_none());
        assert!(handle(&d, "GET", "/settings/mcpx", &Value::Null).is_none());
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn fresh_state_is_locked_and_inactive_and_has_no_secret() {
        let d = dir("fresh", true);
        let s = get(&d);
        assert_eq!(s["enabled"], false);
        assert_eq!(s["active"], false);
        assert_eq!(s["preset"], "locked");
        assert_eq!(s["domains"]["mail"], "off");
        assert!(s["tools"].as_array().unwrap().iter().all(|t| t["class"] != "R"), "only write tools are listed");
        assert!(s["tools"].as_array().unwrap().iter().any(|t| t["name"] == "mail_create_draft"));
        assert!(!s.to_string().contains(&token::load_or_create(&d).unwrap()));
        assert_eq!(s["read_tools"]["mail"], 7);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn master_switch_only_activates_with_background_on() {
        let d = dir("master", false);
        let s = post(&d, "/settings/mcp/enabled", &[("value", "true")]);
        assert_eq!((s["enabled"].as_bool(), s["active"].as_bool()), (Some(true), Some(false)), "stored, but inactive while background is off");
        crate::settings::write_run_in_background(&d.join("settings.toml"), true).unwrap();
        assert_eq!(get(&d)["active"], true, "turning background on later activates it; the [mcp] section survived the rewrite");
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn domain_levels_validate_and_accounts_cannot_write() {
        let d = dir("domain", true);
        assert_eq!(post_confirmed(&d, "/settings/mcp/domain", &[("domain", "mail"), ("level", "write")])["domains"]["mail"], "write");
        assert_eq!(post_confirmed(&d, "/settings/mcp/domain", &[("domain", "accounts"), ("level", "write")])["domains"]["accounts"], "read");
        assert!(post_confirmed(&d, "/settings/mcp/domain", &[("domain", "settings"), ("level", "read")])["error"].is_string());
        assert!(post_confirmed(&d, "/settings/mcp/domain", &[("domain", "mail"), ("level", "root")])["error"].is_string());
        assert_eq!(get(&d)["domains"]["mail"], "write", "a refused change leaves things as they were");
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn tool_modes_can_be_set_and_reset_but_not_for_read_tools() {
        let d = dir("tool", true);
        let mode = |s: &Value, n: &str| s["tools"].as_array().unwrap().iter().find(|t| t["name"] == n).unwrap()["mode"].clone();
        assert_eq!(mode(&get(&d), "mail_send"), "ask");
        assert_eq!(mode(&get(&d), "mail_delete_permanent"), "off");
        let s = post_confirmed(&d, "/settings/mcp/tool", &[("name", "mail_send"), ("mode", "allow")]);
        assert_eq!(mode(&s, "mail_send"), "allow");
        assert_eq!(preset_after(&s), "custom");
        let s = post_confirmed(&d, "/settings/mcp/tool", &[("name", "mail_send"), ("mode", "default")]);
        assert_eq!(mode(&s, "mail_send"), "ask");
        assert!(post_confirmed(&d, "/settings/mcp/tool", &[("name", "mail_search"), ("mode", "off")])["error"].is_string());
        assert!(post_confirmed(&d, "/settings/mcp/tool", &[("name", "nope"), ("mode", "off")])["error"].is_string());
        assert!(post_confirmed(&d, "/settings/mcp/tool", &[("name", "mail_send"), ("mode", "yes")])["error"].is_string());
        let _ = fs::remove_dir_all(d);
    }

    fn preset_after(s: &Value) -> String { s["preset"].as_str().unwrap().to_string() }

    #[test]
    fn accounts_can_be_allowed_only_if_they_exist() {
        let d = dir("acct", true);
        assert_eq!(post_confirmed(&d, "/settings/mcp/account", &[("id", "graph-1"), ("value", "true")])["accounts"], json!(["graph-1"]));
        assert!(post_confirmed(&d, "/settings/mcp/account", &[("id", "ghost"), ("value", "true")])["error"].is_string());
        post_confirmed(&d, "/settings/mcp/account", &[("id", "gmail-2"), ("value", "true")]);
        assert_eq!(post_confirmed(&d, "/settings/mcp/account", &[("id", "graph-1"), ("value", "false")])["accounts"], json!(["gmail-2"]));
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn presets_apply_and_are_detected() {
        let d = dir("preset", true);
        post_confirmed(&d, "/settings/mcp/tool", &[("name", "mail_send"), ("mode", "allow")]);
        let s = post_confirmed(&d, "/settings/mcp/preset", &[("name", "approval")]);
        assert_eq!(s["preset"], "approval");
        assert_eq!(s["domains"]["sms"], "write");
        assert_eq!(s["tools"].as_array().unwrap().iter().find(|t| t["name"] == "mail_send").unwrap()["mode"], "ask", "overrides cleared");
        assert_eq!(post_confirmed(&d, "/settings/mcp/preset", &[("name", "read_only")])["preset"], "read_only");
        assert_eq!(post_confirmed(&d, "/settings/mcp/preset", &[("name", "locked")])["preset"], "locked");
        assert!(post_confirmed(&d, "/settings/mcp/preset", &[("name", "yolo")])["error"].is_string());
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn regenerating_changes_the_token() {
        let d = dir("regen", true);
        let t1 = token::load_or_create(&d).unwrap();
        let r = post(&d, "/settings/mcp/token/regenerate", &[]);
        assert_eq!(token::load_or_create(&d).unwrap(), t1, "not applied until confirmed");
        let id = r["pending"]["id"].as_str().unwrap().to_string();
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
        let r = post(&d, "/settings/mcp/confirm", &[("id", &id)]);
        assert!(r["error"].is_null(), "{}", r);
        assert_ne!(token::load_or_create(&d).unwrap(), t1);
        assert!(post(&d, "/settings/mcp/confirm", &[("id", &id)])["error"].is_string(), "single use");
        let _ = fs::remove_dir_all(d);
    }

    /// Post, and if the change is parked, confirm it; returns the reply that applied it.
    fn post_confirmed(d: &Path, path: &str, kv: &[(&str, &str)]) -> Value {
        let r = post(d, path, kv);
        match r["pending"]["id"].as_str() {
            Some(id) => post(d, "/settings/mcp/confirm", &[("id", id)]),
            None => r,
        }
    }

    /// MCP on, background on, mail at `level`: all confirmed.
    fn active_dir(name: &str, level: &str) -> PathBuf {
        let d = dir(name, true);
        post_confirmed(&d, "/settings/mcp/domain", &[("domain", "mail"), ("level", level)]);
        let s = post_confirmed(&d, "/settings/mcp/enabled", &[("value", "true")]);
        assert_eq!((s["active"].as_bool(), s["pending"].is_null()), (Some(true), true), "{}", s);
        assert_eq!(s["domains"]["mail"], level);
        d
    }

    #[test]
    fn widening_waits_for_confirmation() {
        let d = dir("widen", true);
        // Enabling with everything off still "turns the server on": it asks.
        let r = post(&d, "/settings/mcp/enabled", &[("value", "true")]);
        assert_eq!(r["enabled"], false);
        assert!(r["pending"]["id"].is_string());
        let r = post(&d, "/settings/mcp/confirm", &[("id", r["pending"]["id"].as_str().unwrap())]);
        assert_eq!((r["enabled"].clone(), r["active"].clone(), r["pending"].clone()), (json!(true), json!(true), Value::Null));

        let r = post(&d, "/settings/mcp/domain", &[("domain", "mail"), ("level", "read")]);
        let id = r["pending"]["id"].as_str().expect("pending").to_string();
        assert!(!r["pending"]["summary"].as_array().unwrap().is_empty());
        assert_eq!(r["domains"]["mail"], "off", "state still shows mail off");
        assert_eq!(get(&d)["domains"]["mail"], "off");
        assert_eq!(get(&d)["pending"]["id"], id.as_str(), "pending shows on plain GET too");
        assert!(!fs::read_to_string(d.join("settings.toml")).unwrap().contains("read"));

        let r = post(&d, "/settings/mcp/confirm", &[("id", &id)]);
        assert!(r["error"].is_null(), "{}", r);
        assert_eq!(r["domains"]["mail"], "read");
        assert!(r["pending"].is_null());
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn narrowing_applies_at_once() {
        let d = active_dir("narrow", "write");
        let r = post(&d, "/settings/mcp/domain", &[("domain", "mail"), ("level", "off")]);
        assert_eq!(r["domains"]["mail"], "off");
        assert!(r["pending"].is_null() && r["error"].is_null(), "{}", r);
        let d2 = active_dir("narrow2", "write");
        let r = post(&d2, "/settings/mcp/preset", &[("name", "locked")]);
        assert_eq!(r["preset"], "locked");
        assert!(r["pending"].is_null() && r["error"].is_null(), "{}", r);
        let _ = fs::remove_dir_all(d);
        let _ = fs::remove_dir_all(d2);
    }

    #[test]
    fn wrong_or_cancelled_id_applies_nothing() {
        let d = active_dir("wrongid", "off");
        let id = post(&d, "/settings/mcp/domain", &[("domain", "mail"), ("level", "write")])["pending"]["id"].as_str().unwrap().to_string();
        let r = post(&d, "/settings/mcp/confirm", &[("id", "0000")]);
        assert!(r["error"].is_string());
        assert_eq!(r["pending"]["id"], id.as_str(), "still pending");
        assert_eq!(r["domains"]["mail"], "off");
        let r = post(&d, "/settings/mcp/cancel", &[("id", &id)]);
        assert!(r["pending"].is_null() && r["error"].is_null(), "{}", r);
        assert_eq!(r["domains"]["mail"], "off");
        let r = post(&d, "/settings/mcp/confirm", &[("id", &id)]);
        assert!(r["error"].is_string());
        assert_eq!(r["domains"]["mail"], "off");
        assert!(post(&d, "/settings/mcp/cancel", &[("id", &id)])["error"].is_string());
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn settings_changed_meanwhile_refuses_confirm() {
        let d = active_dir("meanwhile", "read");
        let id = post(&d, "/settings/mcp/domain", &[("domain", "mail"), ("level", "write")])["pending"]["id"].as_str().unwrap().to_string();
        // A narrowing request applies at once and changes the stored [mcp].
        let r = post(&d, "/settings/mcp/domain", &[("domain", "mail"), ("level", "off")]);
        assert_eq!(r["domains"]["mail"], "off");
        assert_eq!(r["pending"]["id"], id.as_str(), "narrowing leaves the pending change alone");
        let r = post(&d, "/settings/mcp/confirm", &[("id", &id)]);
        assert!(r["error"].as_str().unwrap().contains("changed meanwhile"), "{}", r);
        assert_eq!(r["domains"]["mail"], "off", "nothing widened");
        assert!(r["pending"].is_null());

        // Same via a direct write of [mcp].
        let id = post(&d, "/settings/mcp/domain", &[("domain", "mail"), ("level", "write")])["pending"]["id"].as_str().unwrap().to_string();
        let mut s = crate::settings::read_mcp_settings(&d.join("settings.toml")).unwrap();
        s.domains.insert("sms".into(), "read".into());
        crate::settings::write_mcp_settings(&d.join("settings.toml"), &s).unwrap();
        let r = post(&d, "/settings/mcp/confirm", &[("id", &id)]);
        assert!(r["error"].as_str().unwrap().contains("changed meanwhile"), "{}", r);
        assert_eq!(r["domains"]["mail"], "off");
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn copy_token_waits() {
        let d = dir("copy", true);
        let r = post(&d, "/settings/mcp/copy", &[("what", "token")]);
        assert!(r["pending"]["id"].is_string(), "{}", r); // not confirmed: that would run wl-copy
        assert!(r["error"].is_null());
        let d2 = dir("copy2", true);
        let r = post(&d2, "/settings/mcp/copy", &[("what", "stdio")]);
        assert!(r["pending"].is_null(), "{}", r); // may carry a clipboard error when headless
        let _ = fs::remove_dir_all(d);
        let _ = fs::remove_dir_all(d2);
    }

    fn bg_post(d: &Path, enabled: bool) -> Value {
        serde_json::from_str(&handle_background(d, "POST", &json!({ "enabled": enabled }))).unwrap()
    }

    #[test]
    fn background_on_with_mcp_enabled_waits() {
        let d = dir("bgwait", false);
        // No widening while background is off.
        post_confirmed(&d, "/settings/mcp/domain", &[("domain", "mail"), ("level", "read")]);
        let s = post_confirmed(&d, "/settings/mcp/enabled", &[("value", "true")]);
        assert_eq!((s["enabled"].clone(), s["active"].clone(), s["pending"].clone()), (json!(true), json!(false), Value::Null), "{}", s);

        let r = bg_post(&d, true);
        assert_eq!(r["run_in_background"], false);
        let id = r["pending"]["id"].as_str().expect("pending").to_string();
        let r = post(&d, "/settings/mcp/confirm", &[("id", &id)]);
        assert!(r["error"].is_null(), "{}", r);
        assert!(crate::settings::read_run_in_background(&d.join("settings.toml")));
        let r: Value = serde_json::from_str(&handle_background(&d, "GET", &Value::Null)).unwrap();
        assert_eq!((r["run_in_background"].clone(), r["pending"].clone()), (json!(true), Value::Null));

        // Background off applies at once.
        let r = bg_post(&d, false);
        assert_eq!((r["run_in_background"].clone(), r["pending"].clone()), (json!(false), Value::Null));

        // MCP disabled: background on applies at once.
        let d2 = dir("bgwait2", false);
        let r = bg_post(&d2, true);
        assert_eq!((r["run_in_background"].clone(), r["pending"].clone()), (json!(true), Value::Null), "{}", r);

        // Missing enabled.
        let r: Value = serde_json::from_str(&handle_background(&d2, "POST", &json!({}))).unwrap();
        assert!(r["error"].is_string());
        let _ = fs::remove_dir_all(d);
        let _ = fs::remove_dir_all(d2);
    }

    #[test]
    fn query_string_is_ignored() {
        let d = dir("query", true);
        let r: Value = serde_json::from_str(&handle(&d, "POST", "/settings/mcp/enabled?value=true", &Value::Null).unwrap()).unwrap_or(Value::Null);
        // The route with a query suffix is not a known route; the plain route with no body is "missing value".
        assert!(r["error"].is_string(), "{}", r);
        let r: Value = serde_json::from_str(&handle(&d, "POST", "/settings/mcp/enabled", &Value::Null).unwrap()).unwrap();
        assert!(r["error"].as_str().unwrap().contains("missing value"), "{}", r);
        assert_eq!(r["enabled"], false);
        assert!(r["pending"].is_null());
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn a_broken_settings_file_is_reported_and_not_overwritten() {
        let d = dir("broken", true);
        fs::write(d.join("settings.toml"), "not [toml").unwrap();
        let s = post(&d, "/settings/mcp/enabled", &[("value", "true")]);
        assert!(s["error"].as_str().unwrap().contains("cannot be read"));
        assert_eq!(s["settings_readable"], false);
        assert_eq!(fs::read_to_string(d.join("settings.toml")).unwrap(), "not [toml");
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn client_snippets_have_no_secret_and_are_well_formed() {
        let j: Value = serde_json::from_str(&stdio_json("/opt/omalook/bin/omalook")).unwrap();
        assert_eq!(j["mcpServers"]["omalook"]["command"], "/opt/omalook/bin/omalook");
        assert_eq!(j["mcpServers"]["omalook"]["args"], json!(["mcp"]));
        let y = hermes_yaml("/opt/omalook/bin/omalook");
        assert!(y.starts_with("mcp_servers:\n  omalook:\n    command: \"/opt/omalook/bin/omalook\""));
        assert!(y.contains("args: [\"mcp\"]"));
    }
}
