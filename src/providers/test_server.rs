//! Test-only fake HTTP server on 127.0.0.1:0 (std only, one thread).
//!
//! Each accepted connection gets the next scripted `Reply`; once the script is used up every
//! further request gets 599 (not retried by any provider), so a test that sends one request too
//! many fails fast instead of hanging. Every request is recorded (method, path, headers) so
//! tests can assert on what was sent. Connections are closed after each reply.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Reply {
    pub fn new(status: u16, body: &str) -> Self {
        Self { status, headers: Vec::new(), body: body.to_string() }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub path: String,
    /// Header names are lower-cased.
    pub headers: Vec<(String, String)>,
}

impl Seen {
    /// Case-insensitive header lookup.
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers.iter().find(|(k, _)| *k == name).map(|(_, v)| v.as_str())
    }
}

pub struct FakeServer {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeServer {
    pub fn start(script: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
        let addr = listener.local_addr().expect("local addr");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen_t, stop_t) = (Arc::clone(&seen), Arc::clone(&stop));
        let thread = std::thread::spawn(move || {
            let mut script = script.into_iter();
            for stream in listener.incoming() {
                if stop_t.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let Some(request) = read_request(&mut stream) else { continue };
                seen_t.lock().unwrap_or_else(|p| p.into_inner()).push(request);
                let reply = script.next().unwrap_or_else(|| Reply::new(599, "unscripted request"));
                let _ = write_reply(&mut stream, &reply);
            }
        });
        Self { addr, seen, stop, thread: Some(thread) }
    }

    /// `http://127.0.0.1:<port>`, for the provider's test base override.
    pub fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Requests received so far, in order.
    pub fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocked accept() so the thread sees the stop flag.
        let _ = TcpStream::connect(self.addr);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Option<Seen> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        reader.read_line(&mut h).ok()?;
        let h = h.trim_end_matches(['\r', '\n']);
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    // Drain the body so the client's write completes before we reply.
    let len: usize = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).ok()?;
    Some(Seen { method, path, headers })
}

fn write_reply(stream: &mut TcpStream, reply: &Reply) -> std::io::Result<()> {
    let mut out = format!("HTTP/1.1 {} {}\r\n", reply.status, reason(reply.status));
    for (k, v) in &reply.headers {
        out.push_str(&format!("{}: {}\r\n", k, v));
    }
    out.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reply.body.len()));
    out.push_str(&reply.body);
    stream.write_all(out.as_bytes())?;
    stream.flush()
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        410 => "Gone",
        412 => "Precondition Failed",
        429 => "Too Many Requests",
        503 => "Service Unavailable",
        _ => "Status",
    }
}
