//! Typed router for the UI's backend (roadmap R3.1, HARD-4).
//!
//! A route is `(method, path pattern) -> handler`, matched on the parsed request (exact path, or
//! a declared prefix), never by searching the request line for a substring. The guard
//! (`http::guard`: Origin, fetch metadata, Host, session token) has already run before any
//! route is looked up, so every route here is refused without the token.
//!
//! Handlers get a [`Ctx`] (config dir + the parsed request, body already read with its cap) and
//! return a [`Response`]; they never touch the socket.

use super::request::Request;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Deferred work run after the response is written.
pub type AfterSend = Box<dyn FnOnce() + Send>;

/// How a route's path is matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    /// The path (query string removed) equals this.
    Exact(&'static str),
    /// The path starts with this. Use only for routes that carry an id in the path.
    Prefix(&'static str),
}

/// Whether a route runs under the dispatch lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lock {
    /// One at a time with every other locked route (read-modify-write of settings files etc.).
    Exclusive,
    /// Read-only route that may run beside anything else (polls, provider reads, cache scans).
    None,
}

pub type Handler = fn(&Ctx) -> Response;

#[derive(Clone, Copy)]
pub struct Route {
    pub method: &'static str,
    pub pattern: Pattern,
    pub lock: Lock,
    pub handler: Handler,
}

impl Route {
    pub const fn new(method: &'static str, pattern: Pattern, lock: Lock, handler: Handler) -> Route {
        Route { method, pattern, lock, handler }
    }
    fn matches_path(&self, path: &str) -> bool {
        match self.pattern {
            Pattern::Exact(p) => path == p,
            Pattern::Prefix(p) => path.starts_with(p),
        }
    }
    /// The pattern's text (for logs and tests).
    pub fn pattern_str(&self) -> &'static str {
        match self.pattern {
            Pattern::Exact(p) | Pattern::Prefix(p) => p,
        }
    }
}

/// What a request matched.
pub enum Found<'r> {
    Route(&'r Route),
    /// The path is known but not for this method.
    WrongMethod,
    NotFound,
}

pub struct Router {
    routes: Vec<Route>,
}

impl Router {
    pub fn new(routes: Vec<Route>) -> Router {
        Router { routes }
    }
    pub fn routes(&self) -> &[Route] {
        &self.routes
    }
    /// Exact matches win over prefixes; among prefixes the longest wins.
    pub fn find(&self, method: &str, path: &str) -> Found<'_> {
        let mut best: Option<&Route> = None;
        let mut path_known = false;
        for r in &self.routes {
            if !r.matches_path(path) {
                continue;
            }
            path_known = true;
            if r.method != method {
                continue;
            }
            best = match (best, r.pattern) {
                (_, Pattern::Exact(_)) => return Found::Route(r),
                (None, _) => Some(r),
                (Some(b), Pattern::Prefix(p)) if p.len() > b.pattern_str().len() => Some(r),
                (b, _) => b,
            };
        }
        match best {
            Some(r) => Found::Route(r),
            None if path_known => Found::WrongMethod,
            None => Found::NotFound,
        }
    }
}

/// The application's route table, built once.
pub fn table() -> &'static Router {
    static TABLE: OnceLock<Router> = OnceLock::new();
    TABLE.get_or_init(|| Router::new(super::routes::all()))
}

/// Routes run one at a time unless marked `Lock::None` (see `Lock`). A handler that panicked
/// earlier leaves the lock poisoned; the data it guards is the files and DB, not the `()`.
static DISPATCH: Mutex<()> = Mutex::new(());

pub fn lock_for(route: &Route) -> Option<MutexGuard<'static, ()>> {
    match route.lock {
        Lock::None => None,
        Lock::Exclusive => Some(DISPATCH.lock().unwrap_or_else(|e| e.into_inner())),
    }
}

/// The legacy chain in `main.rs` shares the same lock while routes move over.
pub fn exclusive() -> MutexGuard<'static, ()> {
    DISPATCH.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a handler sees.
pub struct Ctx<'a> {
    pub config_dir: &'a PathBuf,
    pub req: &'a Request,
}

impl<'a> Ctx<'a> {
    pub fn new(config_dir: &'a PathBuf, req: &'a Request) -> Ctx<'a> {
        Ctx { config_dir, req }
    }
    /// `?key=` from the query string, percent-decoded; empty counts as absent.
    pub fn query(&self, key: &str) -> Option<String> {
        query_value(&self.req.target, key)
    }
    pub fn path(&self) -> &str {
        self.req.path()
    }
    /// Path of `messages.db` as a string (what `Database::open` takes).
    pub fn db_path(&self) -> String {
        self.config_dir.join("messages.db").to_string_lossy().into_owned()
    }
    /// The body as UTF-8, refused above `limit` bytes. (The reader already capped the whole body
    /// per route; this is the route's own, smaller cap.)
    pub fn body_text(&self, limit: usize) -> Result<String, String> {
        if self.req.body.len() > limit {
            return Err("message too large".into());
        }
        String::from_utf8(self.req.body.clone()).map_err(|_| "message is not valid UTF-8".into())
    }
    /// The body parsed as JSON; an empty or unparsable body is `Null`.
    pub fn json_body(&self, limit: usize) -> Result<serde_json::Value, String> {
        let text = self.body_text(limit)?;
        Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
    }
}

/// Value of `key` in the query part of `target` (a request target or a whole request line).
pub fn query_value(target: &str, key: &str) -> Option<String> {
    let needle = format!("{}=", key);
    target
        .split_once('?')
        .and_then(|(_, q)| {
            q.split([' ', '\r', '\n']).next().and_then(|q| {
                q.split('&').find_map(|kv| kv.strip_prefix(needle.as_str()).map(url_decode))
            })
        })
        .filter(|v| !v.is_empty())
}

/// Percent-decode a query value (`%3D` -> `=`, `+` -> space).
pub fn url_decode(s: &str) -> String {
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

/// A complete answer, plus optional work to run after it is sent (still under the route's lock),
/// for routes whose client must not wait (sign-out, account removal).
pub struct Response {
    bytes: Vec<u8>,
    after: Option<Box<dyn FnOnce() + Send>>,
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        410 => "Gone",
        412 => "Precondition Failed",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

/// Header values must not split the head (HARD-5).
fn clean(v: &str) -> String {
    v.chars().filter(|c| !c.is_control()).collect()
}

impl Response {
    /// `status` with a body of `content_type` and extra headers.
    pub fn new(status: u16, content_type: &str, headers: &[(&str, &str)], body: Vec<u8>) -> Response {
        let mut head = format!("HTTP/1.1 {} {}\r\n", status, reason(status));
        if !content_type.is_empty() {
            head.push_str(&format!("Content-Type: {}\r\n", clean(content_type)));
        }
        for (k, v) in headers {
            head.push_str(&format!("{}: {}\r\n", clean(k), clean(v)));
        }
        head.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(&body);
        Response { bytes, after: None }
    }
    /// 200 with a JSON body.
    pub fn json(body: impl Into<String>) -> Response {
        Response::json_status(200, body)
    }
    pub fn json_status(status: u16, body: impl Into<String>) -> Response {
        Response::new(status, "application/json", &[], body.into().into_bytes())
    }
    /// `{"error": msg}` with `status`.
    pub fn json_error(status: u16, msg: &str) -> Response {
        Response::json_status(status, serde_json::json!({ "error": msg }).to_string())
    }
    pub fn text(status: u16, body: impl Into<String>) -> Response {
        Response::new(status, "text/plain; charset=utf-8", &[], body.into().into_bytes())
    }
    /// Status line only, empty body.
    pub fn empty(status: u16) -> Response {
        Response::new(status, "", &[], Vec::new())
    }
    /// An answer a moved handler already formatted as a whole HTTP response. For handlers moved
    /// as they were; prefer the typed constructors in new code.
    pub fn preformatted(http: impl Into<Vec<u8>>) -> Response {
        Response { bytes: http.into(), after: None }
    }
    /// Run `f` after the answer has been written and the connection closed.
    pub fn then(mut self, f: impl FnOnce() + Send + 'static) -> Response {
        self.after = Some(Box::new(f));
        self
    }
    /// Hand over the bytes and the deferred work.
    pub fn into_parts(self) -> (Vec<u8>, Option<AfterSend>) {
        (self.bytes, self.after)
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    /// The status code (for tests and logs).
    pub fn status(&self) -> u16 {
        std::str::from_utf8(&self.bytes[..self.bytes.len().min(16)])
            .ok()
            .and_then(|s| s.split(' ').nth(1))
            .and_then(|c| c.parse().ok())
            .unwrap_or(0)
    }
    /// The body (for tests).
    pub fn body(&self) -> &[u8] {
        match self.bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            Some(i) => &self.bytes[i + 4..],
            None => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(_: &Ctx) -> Response {
        Response::json("{}")
    }

    fn sample() -> Router {
        Router::new(vec![
            Route::new("GET", Pattern::Exact("/contacts"), Lock::None, h),
            Route::new("GET", Pattern::Exact("/contacts/get"), Lock::None, h),
            Route::new("GET", Pattern::Prefix("/sms/"), Lock::Exclusive, h),
            Route::new("GET", Pattern::Prefix("/sms/threads/"), Lock::None, h),
            Route::new("POST", Pattern::Exact("/contacts"), Lock::Exclusive, h),
        ])
    }

    fn pat(f: Found) -> Option<&'static str> {
        match f {
            Found::Route(r) => Some(r.pattern_str()),
            _ => None,
        }
    }

    #[test]
    fn exact_paths_do_not_bleed_into_longer_ones() {
        let r = sample();
        assert_eq!(pat(r.find("GET", "/contacts")), Some("/contacts"));
        assert_eq!(pat(r.find("GET", "/contacts/get")), Some("/contacts/get"));
        assert!(matches!(r.find("GET", "/contacts/getx"), Found::NotFound));
        assert!(matches!(r.find("GET", "/contactsx"), Found::NotFound));
        assert!(matches!(r.find("GET", "/x/contacts"), Found::NotFound));
    }

    #[test]
    fn method_must_match() {
        let r = sample();
        assert!(matches!(r.find("DELETE", "/contacts"), Found::WrongMethod));
        assert!(matches!(r.find("POST", "/contacts/get"), Found::WrongMethod));
        assert!(matches!(r.find("POST", "/contacts"), Found::Route(Route { lock: Lock::Exclusive, .. })));
    }

    #[test]
    fn longest_prefix_wins() {
        let r = sample();
        assert_eq!(pat(r.find("GET", "/sms/threads/7")), Some("/sms/threads/"));
        assert_eq!(pat(r.find("GET", "/sms/messages")), Some("/sms/"));
    }

    #[test]
    fn query_values_are_decoded_and_empty_is_absent() {
        assert_eq!(query_value("/x?a=1&b=h%C3%A9+llo", "b").as_deref(), Some("hé llo"));
        assert_eq!(query_value("GET /x?a=1&b=2 HTTP/1.1", "b").as_deref(), Some("2"));
        assert_eq!(query_value("/x?a=&b=2", "a"), None);
        assert_eq!(query_value("/x?ab=1", "b"), None);
        assert_eq!(query_value("/x", "a"), None);
    }

    #[test]
    fn responses_are_well_formed() {
        let r = Response::json_error(404, "no such thing");
        assert_eq!(r.status(), 404);
        let text = String::from_utf8(r.as_bytes().to_vec()).unwrap();
        assert!(text.starts_with("HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\n"));
        assert!(text.contains(&format!("Content-Length: {}\r\n\r\n", r.body().len())));
        let bad = Response::new(200, "text/plain", &[("X-Name", "a\r\nSet-Cookie: x")], b"ok".to_vec());
        let text = String::from_utf8(bad.as_bytes().to_vec()).unwrap();
        assert!(text.contains("X-Name: aSet-Cookie: x\r\n"));
        assert_eq!(text.matches("\r\n").count(), 5, "status, type, X-Name, length, blank");
    }

    #[test]
    fn the_table_has_no_duplicate_routes() {
        let routes = table().routes();
        for (i, a) in routes.iter().enumerate() {
            for b in &routes[i + 1..] {
                assert!(
                    !(a.method == b.method && a.pattern == b.pattern),
                    "duplicate route {} {}",
                    a.method,
                    a.pattern_str()
                );
            }
        }
    }

    /// Every route in the table is behind the guard: without the token it is 401, with any
    /// Origin or a wrong Host 403 (the guard runs before the router looks anything up).
    #[test]
    fn every_route_is_refused_without_token_origin_or_host() {
        use crate::http::guard::{check, Refusal};
        use crate::http::request::{read_request, Limits};
        let token = "a".repeat(64);
        let port = 27182;
        let read = |raw: String| {
            let mut s = crate::http::request::test_stream(raw.into_bytes());
            read_request(&mut s, &Limits::default()).expect("request parses")
        };
        for r in table().routes() {
            let path = match r.pattern {
                Pattern::Exact(p) => p.to_string(),
                Pattern::Prefix(p) => format!("{}x", p),
            };
            let host = "Host: 127.0.0.1:27182\r\n";
            let auth = format!("Authorization: Bearer {}\r\n", token);
            let no_token = read(format!("{} {} HTTP/1.1\r\n{}Content-Length: 0\r\n\r\n", r.method, path, host));
            assert!(matches!(check(&no_token, port, &token), Err(Refusal::Token)), "{} {}", r.method, path);
            let origin = read(format!("{} {} HTTP/1.1\r\n{}{}Origin: null\r\nContent-Length: 0\r\n\r\n", r.method, path, host, auth));
            assert!(matches!(check(&origin, port, &token), Err(Refusal::Origin)), "{} {}", r.method, path);
            let wrong_host = read(format!("{} {} HTTP/1.1\r\nHost: localhost:27182\r\n{}Content-Length: 0\r\n\r\n", r.method, path, auth));
            assert!(matches!(check(&wrong_host, port, &token), Err(Refusal::Host)), "{} {}", r.method, path);
            let ok = read(format!("{} {} HTTP/1.1\r\n{}{}Content-Length: 0\r\n\r\n", r.method, path, host, auth));
            assert!(check(&ok, port, &token).is_ok(), "{} {}", r.method, path);
            assert!(matches!(table().find(r.method, &path), Found::Route(_)), "{} {} not found", r.method, path);
        }
    }
}
