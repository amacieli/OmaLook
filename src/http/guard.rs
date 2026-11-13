//! Who may talk to the UI's loopback port (roadmap R1.3, SEC-001 browser and DNS-rebinding vectors).
//!
//! The only legitimate client is the OmaLook UI (`qml/state/Backend.qml`, QML `XMLHttpRequest`), which
//! sends no `Origin` and no `Sec-Fetch-*` headers and always addresses `127.0.0.1:<port>` (checked in a
//! sandboxed run of the whole UI, 2026-11-12). Browsers and browser engines (including the WebKit render
//! helper) add `Origin` to cross-site requests and `Sec-Fetch-*` to every request, and a DNS-rebinding page
//! sends its own host name. All of those are refused before any route runs. Local processes that send
//! none of these are stopped by the session token (R1.4), not here.

use super::request::Request;

/// Why a request was refused. Every refusal is answered `403 Forbidden` with no body.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// An `Origin` header, whatever its value (`null` included).
    Origin,
    /// A `Sec-Fetch-*` header: the request was made by a browser.
    FetchMetadata,
    /// No `Host`, more than one, or not exactly `127.0.0.1:<port>`.
    Host,
}

impl Refusal {
    pub fn status(&self) -> &'static str {
        "403 Forbidden"
    }
}

/// The `Host` value the UI sends to a backend on `port`.
pub fn expected_host(port: u16) -> String {
    format!("127.0.0.1:{}", port)
}

/// Check one request against the rules above.
pub fn check(req: &Request, port: u16) -> Result<(), Refusal> {
    let named = |n: &'static str| req.headers.iter().filter(move |(k, _)| k.eq_ignore_ascii_case(n));
    if named("origin").next().is_some() {
        return Err(Refusal::Origin);
    }
    if req.headers.iter().any(|(k, _)| k.len() >= 10 && k.as_bytes()[..10].eq_ignore_ascii_case(b"sec-fetch-")) {
        return Err(Refusal::FetchMetadata);
    }
    let hosts: Vec<&str> = named("host").map(|(_, v)| v.as_str()).collect();
    if hosts.len() != 1 || hosts[0] != expected_host(port) {
        return Err(Refusal::Host);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::request::{read_request, Limits};
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    fn parse(raw: &str) -> Request {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        c.write_all(raw.as_bytes()).unwrap();
        let (mut s, _) = l.accept().unwrap();
        let lim = Limits { read_timeout: Duration::from_millis(300), deadline: Duration::from_millis(600), ..Limits::default() };
        read_request(&mut s, &lim).unwrap()
    }

    // The exact header set the UI's QML XMLHttpRequest sends (sandbox capture, R1.1/R1.3).
    const UI_GET: &str = "GET /accounts HTTP/1.1\r\nHost: 127.0.0.1:27182\r\nConnection: Keep-Alive\r\nAccept-Encoding: gzip, deflate\r\nAccept-Language: en-US,*\r\nUser-Agent: Mozilla/5.0\r\n\r\n";
    const UI_POST: &str = "POST /settings/signature?account=a HTTP/1.1\r\nHost: 127.0.0.1:27182\r\nContent-Type: text/plain;charset=UTF-8\r\nContent-Length: 2\r\nConnection: Keep-Alive\r\nAccept-Encoding: gzip, deflate\r\nAccept-Language: en-US,*\r\nUser-Agent: Mozilla/5.0\r\n\r\n{}";

    #[test]
    fn the_ui_gets_through() {
        assert_eq!(check(&parse(UI_GET), 27182), Ok(()));
        assert_eq!(check(&parse(UI_POST), 27182), Ok(()));
        // a scratch instance on its own port
        assert_eq!(check(&parse(&UI_GET.replace("27182", "27999")), 27999), Ok(()));
    }

    #[test]
    fn any_origin_is_refused() {
        for o in ["https://evil.example", "null", "http://127.0.0.1:27182", "file://", ""] {
            let raw = UI_POST.replace("Connection:", &format!("Origin: {}\r\nConnection:", o));
            assert_eq!(check(&parse(&raw), 27182), Err(Refusal::Origin), "origin {:?}", o);
        }
        let raw = UI_GET.replace("Connection:", "oRiGiN: https://evil.example\r\nConnection:");
        assert_eq!(check(&parse(&raw), 27182), Err(Refusal::Origin));
    }

    #[test]
    fn browser_fetch_metadata_is_refused() {
        // a no-cors GET from a web page (<img src>, fetch mode no-cors) carries no Origin but these
        for h in ["Sec-Fetch-Site: cross-site", "Sec-Fetch-Mode: no-cors", "Sec-Fetch-Dest: image", "sec-fetch-user: ?1"] {
            let raw = UI_GET.replace("Connection:", &format!("{}\r\nConnection:", h));
            assert_eq!(check(&parse(&raw), 27182), Err(Refusal::FetchMetadata), "{}", h);
        }
    }

    #[test]
    fn host_must_be_exactly_loopback_and_port() {
        for host in ["attacker.example", "attacker.example:27182", "localhost:27182", "127.0.0.1", "127.0.0.1:27183", "[::1]:27182", "127.0.0.2:27182", "127.0.0.1:27182.evil"] {
            let raw = UI_GET.replace("Host: 127.0.0.1:27182", &format!("Host: {}", host));
            assert_eq!(check(&parse(&raw), 27182), Err(Refusal::Host), "host {:?}", host);
        }
        // the header value is trimmed by the parser, so a trailing space is the same host
        assert_eq!(check(&parse(&UI_GET.replace("Host: 127.0.0.1:27182", "Host: 127.0.0.1:27182 ")), 27182), Ok(()));
        // missing, or sent twice
        assert_eq!(check(&parse(&UI_GET.replace("Host: 127.0.0.1:27182\r\n", "")), 27182), Err(Refusal::Host));
        assert_eq!(check(&parse(&UI_GET.replace("Host: 127.0.0.1:27182\r\n", "Host: 127.0.0.1:27182\r\nHost: evil\r\n")), 27182), Err(Refusal::Host));
        // right host, wrong port for this instance
        assert_eq!(check(&parse(UI_GET), 27999), Err(Refusal::Host));
    }
}
