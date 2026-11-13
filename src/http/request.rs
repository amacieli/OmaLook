//! Reading one HTTP/1.1 request from the UI's loopback connection (roadmap R1.2, SEC-004).
//!
//! The old server read a single 1 KiB chunk with no timeout on its only accept thread: one idle
//! connection froze every route, a lying `Content-Length` stalled them for seconds, and request
//! bodies over ~1 KiB (long signatures) were cut. `read_request` reads the whole head and body
//! with per-read timeouts, an overall deadline and size caps, and hands back the exact bytes so the
//! existing route chain can keep working on them unchanged until the typed router (R3.1).

use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// Size and time limits for one request.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Request line + headers.
    pub max_head: usize,
    /// Body, unless `body_limit` raises it for a route.
    pub max_body: usize,
    /// Longest wait for any single read.
    pub read_timeout: Duration,
    /// Longest time the whole request may take to arrive (stops a client that trickles bytes).
    pub deadline: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_head: 64 * 1024,
            max_body: 1024 * 1024,
            read_timeout: Duration::from_secs(5),
            deadline: Duration::from_secs(10),
        }
    }
}

/// A request as received: method, target (path + query), parsed headers and the raw bytes
/// (head + body, nothing after the body).
#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub raw: Vec<u8>,
    head_len: usize,
}

impl Request {
    /// Path without the query string.
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or("")
    }
    /// First value of a header, name compared without case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    /// The request line as sent ("GET /x?y HTTP/1.1").
    pub fn first_line(&self) -> String {
        let head = String::from_utf8_lossy(&self.raw[..self.head_len]);
        head.lines().next().unwrap_or("").to_string()
    }
}

/// Why a request could not be read; `status()` is the answer to send (if the socket still works).
#[derive(Debug, PartialEq, Eq)]
pub enum ReadError {
    /// Nothing arrived at all (closed, or idle until the timeout): just drop the connection.
    Empty,
    /// The client stopped sending, or took longer than the deadline.
    Timeout,
    /// Connection closed in the middle of the request.
    Closed,
    BadRequest(&'static str),
    HeadTooLarge,
    BodyTooLarge,
    /// `Transfer-Encoding` (chunked) bodies are not accepted; the UI always sends `Content-Length`.
    LengthRequired,
}

impl ReadError {
    pub fn status(&self) -> Option<&'static str> {
        match self {
            ReadError::Empty | ReadError::Closed => None,
            ReadError::Timeout => Some("408 Request Timeout"),
            ReadError::BadRequest(_) => Some("400 Bad Request"),
            ReadError::HeadTooLarge => Some("431 Request Header Fields Too Large"),
            ReadError::BodyTooLarge => Some("413 Content Too Large"),
            ReadError::LengthRequired => Some("411 Length Required"),
        }
    }
}

/// Body cap for one route. Mail bodies (compose send, draft save) may carry up to
/// `compose::MAX_BODY_BYTES` plus the JSON around them; everything else gets `limits.max_body`.
/// The routes still apply their own, smaller caps when they parse the body.
pub fn body_limit(method: &str, path: &str, limits: &Limits) -> usize {
    match (method, path) {
        ("POST", "/compose/send") | ("POST", "/drafts/save") => {
            limits.max_body.max(crate::compose::MAX_BODY_BYTES + 64 * 1024)
        }
        _ => limits.max_body,
    }
}

fn is_timeout(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)
}

/// A stream whose read timeout can be set (TcpStream; tests use it too).
pub trait TimedStream: Read + Write {
    fn set_timeout(&mut self, d: Duration) -> std::io::Result<()>;
}

impl TimedStream for std::net::TcpStream {
    fn set_timeout(&mut self, d: Duration) -> std::io::Result<()> {
        self.set_read_timeout(Some(d))
    }
}

/// Read one request. On `Expect: 100-continue` the interim answer is sent before the body.
pub fn read_request<S: TimedStream>(stream: &mut S, limits: &Limits) -> Result<Request, ReadError> {
    let start = Instant::now();
    let mut data: Vec<u8> = Vec::with_capacity(2048);
    let mut chunk = vec![0u8; 16 * 1024];

    // Each read waits at most read_timeout, and never past the overall deadline.
    let mut read_some = |stream: &mut S, data: &mut Vec<u8>| -> Result<usize, ReadError> {
        let left = limits.deadline.checked_sub(start.elapsed()).ok_or(ReadError::Timeout)?;
        let wait = left.min(limits.read_timeout).max(Duration::from_millis(1));
        stream.set_timeout(wait).map_err(|_| ReadError::Closed)?;
        match stream.read(&mut chunk) {
            Ok(n) => {
                data.extend_from_slice(&chunk[..n]);
                Ok(n)
            }
            Err(e) if is_timeout(&e) => Err(ReadError::Timeout),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => Ok(usize::MAX), // try again
            Err(_) => Err(ReadError::Closed),
        }
    };

    // ---- head
    let head_len = loop {
        if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if data.len() > limits.max_head {
            return Err(ReadError::HeadTooLarge);
        }
        match read_some(stream, &mut data) {
            Ok(0) => return Err(if data.is_empty() { ReadError::Empty } else { ReadError::Closed }),
            Ok(_) => {}
            Err(ReadError::Timeout) if data.is_empty() => return Err(ReadError::Empty),
            Err(e) => return Err(e),
        }
    };
    if head_len > limits.max_head {
        return Err(ReadError::HeadTooLarge);
    }

    let head = std::str::from_utf8(&data[..head_len - 4]).map_err(|_| ReadError::BadRequest("head is not UTF-8"))?.to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split(' ');
    let (method, target, version) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(m), Some(t), Some(v), None) if !m.is_empty() && t.starts_with('/') && v.starts_with("HTTP/1.") => (m, t, v),
        _ => return Err(ReadError::BadRequest("bad request line")),
    };
    let _ = version;
    if !method.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(ReadError::BadRequest("bad method"));
    }
    let mut headers = Vec::new();
    for line in lines {
        let (k, v) = line.split_once(':').ok_or(ReadError::BadRequest("bad header line"))?;
        if k.is_empty() || k.bytes().any(|b| b == b' ' || b == b'\t') {
            return Err(ReadError::BadRequest("bad header name"));
        }
        headers.push((k.to_string(), v.trim().to_string()));
    }
    let header = |name: &str| headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str()).collect::<Vec<_>>();

    if !header("transfer-encoding").is_empty() {
        return Err(ReadError::LengthRequired);
    }
    let lengths = header("content-length");
    let len = match lengths.as_slice() {
        [] => 0usize,
        [v] => v.parse::<usize>().map_err(|_| ReadError::BadRequest("bad Content-Length"))?,
        _ => return Err(ReadError::BadRequest("several Content-Length headers")),
    };
    if len > body_limit(method, target.split('?').next().unwrap_or(""), limits) {
        return Err(ReadError::BodyTooLarge);
    }

    // ---- body
    if len > 0 && data.len() < head_len + len && header("expect").iter().any(|v| v.eq_ignore_ascii_case("100-continue")) {
        let _ = stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
    }
    while data.len() < head_len + len {
        match read_some(stream, &mut data) {
            Ok(0) => return Err(ReadError::Closed),
            Ok(_) => {}
            Err(e) => return Err(e),
        }
    }
    data.truncate(head_len + len);

    Ok(Request {
        method: method.to_string(),
        target: target.to_string(),
        headers,
        body: data[head_len..].to_vec(),
        raw: data,
        head_len,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    fn fast() -> Limits {
        Limits { read_timeout: Duration::from_millis(200), deadline: Duration::from_millis(600), ..Limits::default() }
    }

    /// Server side of a loopback pair; `client` runs on its own thread with the other end.
    fn pair<F: FnOnce(TcpStream) + Send + 'static>(client: F) -> (TcpStream, thread::JoinHandle<()>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let h = thread::spawn(move || client(TcpStream::connect(addr).unwrap()));
        let (s, _) = l.accept().unwrap();
        (s, h)
    }

    #[test]
    fn reads_a_get_with_query() {
        let (mut s, h) = pair(|mut c| { c.write_all(b"GET /messages?limit=5 HTTP/1.1\r\nHost: 127.0.0.1:27182\r\n\r\n").unwrap(); thread::sleep(Duration::from_millis(300)); });
        let r = read_request(&mut s, &fast()).unwrap();
        assert_eq!((r.method.as_str(), r.target.as_str(), r.path()), ("GET", "/messages?limit=5", "/messages"));
        assert_eq!(r.header("host"), Some("127.0.0.1:27182"));
        assert_eq!(r.first_line(), "GET /messages?limit=5 HTTP/1.1");
        assert!(r.body.is_empty());
        h.join().unwrap();
    }

    #[test]
    fn reads_a_body_that_arrives_in_pieces_and_longer_than_1_kib() {
        let body = format!("{{\"text\":\"{}\"}}", "x".repeat(1500));
        let b2 = body.clone();
        let (mut s, h) = pair(move |mut c| {
            let head = format!("POST /settings/signature?account=a HTTP/1.1\r\nContent-Length: {}\r\n\r\n", b2.len());
            c.write_all(head.as_bytes()).unwrap();
            for part in b2.as_bytes().chunks(400) { c.write_all(part).unwrap(); thread::sleep(Duration::from_millis(20)); }
            thread::sleep(Duration::from_millis(300));
        });
        let r = read_request(&mut s, &fast()).unwrap();
        assert_eq!(r.body, body.as_bytes());
        assert!(r.raw.ends_with(body.as_bytes()));
        assert!(String::from_utf8_lossy(&r.raw).starts_with("POST /settings/signature"));
        h.join().unwrap();
    }

    #[test]
    fn idle_connection_gives_up_quietly() {
        let (mut s, h) = pair(|c| { thread::sleep(Duration::from_millis(900)); drop(c); });
        let t = Instant::now();
        assert_eq!(read_request(&mut s, &fast()).unwrap_err(), ReadError::Empty);
        assert!(t.elapsed() < Duration::from_millis(700), "took {:?}", t.elapsed());
        h.join().unwrap();
    }

    #[test]
    fn lying_content_length_times_out() {
        let (mut s, h) = pair(|mut c| { c.write_all(b"POST /x HTTP/1.1\r\nContent-Length: 100\r\n\r\nshort").unwrap(); thread::sleep(Duration::from_millis(900)); });
        let t = Instant::now();
        let e = read_request(&mut s, &fast()).unwrap_err();
        assert_eq!(e, ReadError::Timeout);
        assert_eq!(e.status(), Some("408 Request Timeout"));
        assert!(t.elapsed() < Duration::from_millis(700));
        h.join().unwrap();
    }

    #[test]
    fn trickling_client_hits_the_deadline() {
        let (mut s, h) = pair(|mut c| {
            let _ = c.write_all(b"GET /a");
            for _ in 0..12 { thread::sleep(Duration::from_millis(100)); if c.write_all(b"a").is_err() { return; } }
        });
        let t = Instant::now();
        assert_eq!(read_request(&mut s, &fast()).unwrap_err(), ReadError::Timeout);
        assert!(t.elapsed() < Duration::from_millis(900), "took {:?}", t.elapsed());
        drop(s);
        h.join().unwrap();
    }

    #[test]
    fn caps_body_and_head() {
        let (mut s, h) = pair(|mut c| { let _ = c.write_all(b"POST /settings/mail HTTP/1.1\r\nContent-Length: 2000000\r\n\r\n"); thread::sleep(Duration::from_millis(200)); });
        assert_eq!(read_request(&mut s, &fast()).unwrap_err(), ReadError::BodyTooLarge);
        h.join().unwrap();

        let (mut s, h) = pair(|mut c| {
            let _ = c.write_all(b"GET / HTTP/1.1\r\n");
            let big = format!("X-Big: {}\r\n", "a".repeat(70 * 1024));
            let _ = c.write_all(big.as_bytes());
            thread::sleep(Duration::from_millis(200));
        });
        assert_eq!(read_request(&mut s, &fast()).unwrap_err(), ReadError::HeadTooLarge);
        h.join().unwrap();
    }

    #[test]
    fn mail_routes_get_the_larger_body_cap() {
        let l = Limits::default();
        assert_eq!(body_limit("POST", "/settings/signature", &l), 1024 * 1024);
        assert!(body_limit("POST", "/compose/send", &l) > crate::compose::MAX_BODY_BYTES);
        assert!(body_limit("POST", "/drafts/save", &l) > crate::compose::MAX_BODY_BYTES);
        assert_eq!(body_limit("GET", "/compose/send", &l), 1024 * 1024);
    }

    #[test]
    fn rejects_chunked_bad_lines_and_double_lengths() {
        for (raw, want) in [
            (&b"POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n"[..], ReadError::LengthRequired),
            (&b"GET http://evil/ HTTP/1.1\r\n\r\n"[..], ReadError::BadRequest("bad request line")),
            (&b"get /x HTTP/1.1\r\n\r\n"[..], ReadError::BadRequest("bad method")),
            (&b"POST /x HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab"[..], ReadError::BadRequest("several Content-Length headers")),
            (&b"POST /x HTTP/1.1\r\nContent-Length: -1\r\n\r\n"[..], ReadError::BadRequest("bad Content-Length")),
            (&b"GET /x HTTP/1.1\r\nno colon here\r\n\r\n"[..], ReadError::BadRequest("bad header line")),
        ] {
            let bytes = raw.to_vec();
            let (mut s, h) = pair(move |mut c| { let _ = c.write_all(&bytes); thread::sleep(Duration::from_millis(100)); });
            assert_eq!(read_request(&mut s, &fast()).unwrap_err(), want);
            h.join().unwrap();
        }
    }

    #[test]
    fn answers_expect_100_continue() {
        let (mut s, h) = pair(|mut c| {
            c.write_all(b"POST /x HTTP/1.1\r\nContent-Length: 3\r\nExpect: 100-continue\r\n\r\n").unwrap();
            let mut buf = [0u8; 64];
            let n = c.read(&mut buf).unwrap();
            assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 100 Continue"));
            c.write_all(b"abc").unwrap();
            thread::sleep(Duration::from_millis(100));
        });
        assert_eq!(read_request(&mut s, &fast()).unwrap().body, b"abc");
        h.join().unwrap();
    }

    #[test]
    fn ignores_bytes_after_the_body() {
        let (mut s, h) = pair(|mut c| { c.write_all(b"POST /x HTTP/1.1\r\nContent-Length: 2\r\n\r\nokEXTRA").unwrap(); thread::sleep(Duration::from_millis(100)); });
        let r = read_request(&mut s, &fast()).unwrap();
        assert_eq!(r.body, b"ok");
        assert!(r.raw.ends_with(b"\r\n\r\nok"));
        h.join().unwrap();
    }
}
