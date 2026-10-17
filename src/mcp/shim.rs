//! `omalook mcp`: the stdio side of MCP, for clients that spawn a command.
//!
//! Holds no policy and no state. Each JSON-RPC line on stdin is POSTed to the running daemon's
//! MCP port with the bearer token from `mcp.token`; the answer is written to stdout. If the daemon
//! is not running (or MCP is off) every request gets a JSON-RPC error saying so. Never starts
//! the daemon. Logs go to stderr only: stdout is the protocol.

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::Path;
use std::time::Duration;

const DOWN: &str = "OmaLook is not running with MCP enabled. Start OmaLook, turn on Settings → General → Keep running in background, and enable Settings → MCP.";

fn rpc_error(id: Value, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": message } })
}

/// Forward one line; the lines to print (zero for a notification).
pub fn forward(config_dir: &Path, port: u16, line: &str) -> Option<String> {
    let parsed: Option<Value> = serde_json::from_str(line).ok();
    let id_of = |v: &Value| v.get("id").cloned();
    let down = |p: &Option<Value>| -> Option<String> {
        // Notifications (no id) cannot be answered; a batch is answered once.
        let id = match p { Some(Value::Object(_)) => id_of(p.as_ref().unwrap()), Some(Value::Array(_)) => Some(Value::Null), _ => Some(Value::Null) };
        id.map(|id| rpc_error(id, DOWN).to_string())
    };
    let Some(token) = super::token::load_or_create(config_dir) else { return down(&parsed); };
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(2)).timeout(Duration::from_secs(120)).build();
    let resp = agent
        .post(&format!("http://127.0.0.1:{}/mcp", port))
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_string(line);
    match resp {
        Ok(r) if r.status() == 202 => None,
        Ok(r) => r.into_string().ok().filter(|s| !s.is_empty()),
        Err(ureq::Error::Status(code, _)) => {
            let msg = match code {
                401 => "OmaLook refused the MCP token. It was regenerated; restart this MCP client.".to_string(),
                503 => DOWN.to_string(),
                c => format!("OmaLook MCP returned HTTP {}", c),
            };
            parsed.as_ref().and_then(|p| p.get("id").cloned()).map(|id| rpc_error(id, &msg).to_string())
        }
        Err(_) => down(&parsed),
    }
}

/// Run until stdin closes.
pub fn run(config_dir: &Path) {
    let port = super::mcp_port();
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() { continue; }
        if let Some(reply) = forward(config_dir, port, &line) {
            if writeln!(out, "{}", reply).is_err() || out.flush().is_err() { break; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_down_answers_requests_but_not_notifications() {
        let d = std::env::temp_dir().join(format!("omalook-mcp-shim-{}", std::process::id()));
        // Port 1 is never listening.
        let r = forward(&d, 1, r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#).unwrap();
        let v: Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["id"], 7);
        assert!(v["error"]["message"].as_str().unwrap().contains("not running"));
        assert!(forward(&d, 1, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        let _ = std::fs::remove_dir_all(d);
    }
}
