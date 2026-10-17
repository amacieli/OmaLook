//! The MCP listener: `POST /mcp` on 127.0.0.1 (port 27183), JSON-RPC in the body, bearer token required.
//!
//! It exists only while MCP is active (master switch on AND "Keep running in background" on): a
//! supervisor thread opens and closes the port as Settings change. Requests carrying an `Origin`
//! header (browsers) or a `Host` other than localhost are refused, so a web page cannot drive it.

use super::{rpc, token, Ctx};
use log::{info, warn};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::time::Duration;

const MAX_HEADER: usize = 16 * 1024;
const MAX_BODY: usize = 1024 * 1024;

pub struct Reply {
    pub status: u16,
    /// Empty for 202/4xx without a body.
    pub body: String,
}

fn reply(status: u16, body: &str) -> Reply { Reply { status, body: body.to_string() } }

fn host_ok(host: &str) -> bool {
    let h = host.trim().to_ascii_lowercase();
    let name = h.rsplit_once(':').map(|(n, p)| if p.chars().all(|c| c.is_ascii_digit()) { n } else { h.as_str() }).unwrap_or(&h);
    name == "127.0.0.1" || name == "localhost"
}

/// Everything after the bytes are read: checks, then JSON-RPC. `header` looks up a lowercase name.
pub fn respond(ctx: &Ctx, expected_token: Option<&str>, method: &str, path: &str, header: &dyn Fn(&str) -> Option<String>, body: &[u8]) -> Reply {
    if let Some(h) = header("host") { if !host_ok(&h) { return reply(403, "bad host"); } }
    if header("origin").is_some() { return reply(403, "browser requests are not accepted"); }
    if path != "/mcp" { return reply(404, "not found"); }
    if method != "POST" { return Reply { status: 405, body: "POST only".into() }; }
    let Some(expected) = expected_token else { return reply(503, "no token"); };
    let given = header("authorization").and_then(|a| a.strip_prefix("Bearer ").map(|t| t.trim().to_string()));
    match given {
        Some(t) if token::matches(&t, expected) => {}
        _ => return reply(401, "unauthorized"),
    }
    // MCP is off or locked: behave as if nothing is here.
    if !ctx.policy().active() { return reply(503, "MCP is off"); }

    let Ok(parsed) = serde_json::from_slice::<Value>(body) else {
        return Reply { status: 200, body: rpc::parse_error().to_string() };
    };
    // Batches are legal JSON-RPC; answer each, drop notifications.
    let out = match parsed {
        Value::Array(items) => {
            let rs: Vec<Value> = items.iter().filter_map(|r| rpc::handle(ctx, r)).collect();
            if rs.is_empty() { None } else { Some(Value::Array(rs)) }
        }
        one => rpc::handle(ctx, &one),
    };
    match out {
        Some(v) => Reply { status: 200, body: v.to_string() },
        None => Reply { status: 202, body: String::new() },
    }
}

fn status_text(code: u16) -> &'static str {
    match code { 200 => "OK", 202 => "Accepted", 401 => "Unauthorized", 403 => "Forbidden", 404 => "Not Found", 405 => "Method Not Allowed", 503 => "Service Unavailable", _ => "Error" }
}

/// Read one HTTP request off the socket, answer it, close.
pub fn serve_connection(mut stream: TcpStream, ctx: &Ctx) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") { break p; }
        if buf.len() > MAX_HEADER { return; }
        match stream.read(&mut chunk) { Ok(0) | Err(_) => return, Ok(n) => buf.extend_from_slice(&chunk[..n]) }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let (method, path) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())))
        .collect();
    let header = |name: &str| headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());

    let len = header("content-length").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    if len > MAX_BODY { write_reply(&mut stream, &reply(413, "too large")); return; }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        match stream.read(&mut chunk) { Ok(0) | Err(_) => return, Ok(n) => body.extend_from_slice(&chunk[..n]) }
    }
    body.truncate(len);

    let tok = token::load_or_create(&ctx.config_dir);
    let r = respond(ctx, tok.as_deref(), &method, &path, &header, &body);
    write_reply(&mut stream, &r);
}

fn write_reply(stream: &mut TcpStream, r: &Reply) {
    let text = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        r.status, status_text(r.status), r.body.len(), r.body
    );
    let _ = stream.write_all(text.as_bytes());
}

/// Start the supervisor: open the port while MCP is active, close it when it is not.
/// Returns at once; runs on its own thread.
pub fn start(config_dir: &Path) {
    let ctx = Ctx::new(config_dir);
    let port = super::mcp_port();
    std::thread::Builder::new()
        .name("mcp-supervisor".into())
        .spawn(move || {
            let mut listener: Option<TcpListener> = None;
            let mut last_check: Option<std::time::Instant> = None;
            loop {
                if last_check.map_or(true, |t| t.elapsed() >= Duration::from_secs(1)) {
                    last_check = Some(std::time::Instant::now());
                    let want = ctx.policy().active();
                    if want && listener.is_none() {
                        match TcpListener::bind(("127.0.0.1", port)) {
                            Ok(l) => {
                                let _ = l.set_nonblocking(true);
                                info!("MCP listening on 127.0.0.1:{}", port);
                                token::load_or_create(&ctx.config_dir);
                                listener = Some(l);
                            }
                            Err(e) => { warn!("MCP could not bind 127.0.0.1:{}: {}", port, e); std::thread::sleep(Duration::from_secs(5)); }
                        }
                    } else if !want && listener.is_some() {
                        info!("MCP switched off; closing port {}", port);
                        listener = None;
                    }
                }
                match listener.as_ref().map(|l| l.accept()) {
                    Some(Ok((s, _))) => {
                        let c = ctx.clone();
                        let _ = s.set_nonblocking(false);
                        std::thread::spawn(move || serve_connection(s, &c));
                    }
                    _ => std::thread::sleep(Duration::from_millis(if listener.is_some() { 10 } else { 100 })),
                }
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    const ON: &str = "[sync]\nrun_in_background = true\n[mcp]\nenabled = true\n[mcp.domains]\naccounts = \"read\"\n";

    fn ctx(name: &str, toml: &str) -> (Ctx, PathBuf) {
        let d = std::env::temp_dir().join(format!("omalook-mcp-srv-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("settings.toml"), toml).unwrap();
        (Ctx::new(&d), d)
    }

    fn hdrs(pairs: Vec<(&'static str, String)>) -> impl Fn(&str) -> Option<String> {
        move |n: &str| pairs.iter().find(|(k, _)| *k == n).map(|(_, v)| v.clone())
    }

    const PING: &[u8] = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;

    #[test]
    fn needs_the_right_token() {
        let (c, d) = ctx("tok", ON);
        let h = hdrs(vec![("host", "127.0.0.1:27183".into())]);
        assert_eq!(respond(&c, Some("secret"), "POST", "/mcp", &h, PING).status, 401);
        let h = hdrs(vec![("authorization", "Bearer wrong".into())]);
        assert_eq!(respond(&c, Some("secret"), "POST", "/mcp", &h, PING).status, 401);
        let h = hdrs(vec![("authorization", "Bearer secret".into()), ("host", "localhost:27183".into())]);
        let r = respond(&c, Some("secret"), "POST", "/mcp", &h, PING);
        assert_eq!(r.status, 200);
        assert!(r.body.contains("\"result\""));
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn browsers_and_foreign_hosts_are_refused_even_with_the_token() {
        let (c, d) = ctx("web", ON);
        let h = hdrs(vec![("authorization", "Bearer secret".into()), ("origin", "http://evil.example".into())]);
        assert_eq!(respond(&c, Some("secret"), "POST", "/mcp", &h, PING).status, 403);
        let h = hdrs(vec![("authorization", "Bearer secret".into()), ("host", "evil.example:27183".into())]);
        assert_eq!(respond(&c, Some("secret"), "POST", "/mcp", &h, PING).status, 403);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn only_post_mcp() {
        let (c, d) = ctx("route", ON);
        let h = hdrs(vec![("authorization", "Bearer secret".into())]);
        assert_eq!(respond(&c, Some("secret"), "GET", "/mcp", &h, b"").status, 405);
        assert_eq!(respond(&c, Some("secret"), "POST", "/other", &h, PING).status, 404);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn inactive_policy_serves_nothing() {
        let (c, d) = ctx("off", &ON.replace("enabled = true", "enabled = false"));
        let h = hdrs(vec![("authorization", "Bearer secret".into())]);
        assert_eq!(respond(&c, Some("secret"), "POST", "/mcp", &h, PING).status, 503);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn notification_is_202_and_garbage_is_a_parse_error() {
        let (c, d) = ctx("misc", ON);
        let h = hdrs(vec![("authorization", "Bearer secret".into())]);
        let r = respond(&c, Some("secret"), "POST", "/mcp", &h, br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        assert_eq!((r.status, r.body.as_str()), (202, ""));
        let r = respond(&c, Some("secret"), "POST", "/mcp", &h, b"{nope");
        assert!(r.body.contains("-32700"));
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn batch_answers_each_request() {
        let (c, d) = ctx("batch", ON);
        let h = hdrs(vec![("authorization", "Bearer secret".into())]);
        let r = respond(&c, Some("secret"), "POST", "/mcp", &h, br#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/initialized"},{"jsonrpc":"2.0","id":2,"method":"ping"}]"#);
        let v: Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn host_check() {
        assert!(host_ok("127.0.0.1:27183") && host_ok("localhost") && host_ok("LOCALHOST:1"));
        assert!(!host_ok("evil.com") && !host_ok("127.0.0.1.evil.com:80") && !host_ok("[::1]:80"));
    }

    #[test]
    fn real_socket_round_trip() {
        let (c, d) = ctx("sock", ON);
        let tok = token::load_or_create(&c.config_dir).unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let c2 = c.clone();
        let t = std::thread::spawn(move || { let (s, _) = l.accept().unwrap(); serve_connection(s, &c2); });
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let body = String::from_utf8_lossy(PING).to_string();
        write!(s, "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n{}", port, tok, body.len(), body).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        t.join().unwrap();
        assert!(out.starts_with("HTTP/1.1 200 OK"), "{}", out);
        assert!(out.contains("\"result\":{}"), "{}", out);
        let _ = fs::remove_dir_all(d);
    }
}
