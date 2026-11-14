//! Launch-to-mail timing marks.
//!
//! `perf::mark("label")` prints `[PERF] +<ms since process start> [thread] label` to stderr
//! and appends the same line to `<config dir>/perf.log` (truncated at each launch). The QML
//! side posts its own marks to `POST /perf?m=<label>`, so one file holds the whole timeline
//! from process start to "emails visible". Cost per mark is a mutex + one small write.

use std::fs::File;
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static LOG: OnceLock<Mutex<Option<File>>> = OnceLock::new();

/// Call first thing in `main`: fixes t=0 and (re)creates the log file.
pub fn init(config_dir: &std::path::Path) {
    let _ = START.set(Instant::now());
    let _ = std::fs::create_dir_all(config_dir);
    let path = config_dir.join("perf.log");
    let file = File::create(&path).ok();
    if file.is_some() {
        // Owner-only even if the umask was not set (R2.6). Regular file check via symlink_metadata.
        if std::fs::symlink_metadata(&path).map(|m| m.file_type().is_file()).unwrap_or(false) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
    }
    let _ = LOG.set(Mutex::new(file));
    mark("process start");
}

/// Route name for logging: `METHOD /path` with no query string, fragment, or request target
/// beyond the path. Segments that look like ids (digits, long mixed tokens) or contain anything
/// outside `[A-Za-z0-9_.-]` (emails, encoded data) become `:id`. Anything unparseable is `?`.
/// R2.7: perf.log must never carry search terms, file paths, account ids or addresses.
pub fn route_name(request_line: &str) -> String {
    const MAX_ROUTE: usize = 120;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    let method = if !method.is_empty() && method.len() <= 10 && method.bytes().all(|b| b.is_ascii_uppercase()) {
        method
    } else {
        "?"
    };
    if !target.starts_with('/') {
        return format!("{} ?", method);
    }
    let path = target.split(['?', '#']).next().unwrap_or("");
    let mut route = String::new();
    for seg in path.split('/').skip(1) {
        route.push('/');
        if is_id_segment(seg) {
            route.push_str(":id");
        } else {
            route.push_str(seg);
        }
        if route.len() > MAX_ROUTE {
            route.truncate(MAX_ROUTE);
            route.push_str("...");
            break;
        }
    }
    if route.is_empty() {
        route.push('/');
    }
    format!("{} {}", method, route)
}

fn is_id_segment(seg: &str) -> bool {
    if seg.is_empty() {
        return false;
    }
    if !seg.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.') {
        return true;
    }
    let has_digit = seg.bytes().any(|b| b.is_ascii_digit());
    seg.bytes().all(|b| b.is_ascii_digit()) || (has_digit && seg.len() >= 12)
}

/// Milliseconds since `init` (0.0 if `init` was never called).
pub fn elapsed_ms() -> f64 {
    START.get().map(|s| s.elapsed().as_secs_f64() * 1000.0).unwrap_or(0.0)
}

pub fn mark(label: &str) {
    let thread = std::thread::current();
    let line = format!("+{:>8.1}ms [{}] {}", elapsed_ms(), thread.name().unwrap_or("main"), label);
    eprintln!("[PERF] {}", line);
    if let Some(lock) = LOG.get() {
        if let Ok(mut guard) = lock.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = writeln!(f, "{}", line);
            }
        }
    }
}

/// Short tag for an account id in perf lines (R2.7): the first 8 hex digits of its FNV-1a 64-bit hash.
/// Stable across runs and builds, so lines from one account still group together. The id never
/// reaches perf.log. Not a cryptographic hash: a 32-bit tag of a short id can be matched by guessing.
pub fn acct_tag(account_id: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in account_id.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", (h >> 32) as u32)
}

/// Time a block: marks `<label> (<n> ms)` when the returned guard drops.
pub struct Span {
    label: String,
    t: Instant,
}

pub fn span(label: impl Into<String>) -> Span {
    Span { label: label.into(), t: Instant::now() }
}

impl Drop for Span {
    fn drop(&mut self) {
        mark(&format!("{} done in {:.1}ms", self.label, self.t.elapsed().as_secs_f64() * 1000.0));
    }
}

#[cfg(test)]
mod tests {
    use super::{acct_tag, route_name};

    #[test]
    fn acct_tag_is_stable_short_and_hides_the_id() {
        let id = "gmail-7f3a9c1e2b";
        let t = acct_tag(id);
        assert_eq!(t.len(), 8);
        assert!(t.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(t, acct_tag(id), "same id, same tag");
        assert_ne!(t, acct_tag("exchange-7f3a9c1e2b"));
        assert!(!t.contains("7f3a9c"));
    }

    #[test]
    fn drops_query_string_fs_list() {
        assert_eq!(route_name("GET /fs/list?path=/home/x HTTP/1.1"), "GET /fs/list");
    }

    #[test]
    fn drops_search_terms_and_account() {
        assert_eq!(route_name("GET /messages?q=secret&account=a@b HTTP/1.1"), "GET /messages");
    }

    #[test]
    fn drops_fragment_and_keeps_plain_routes() {
        assert_eq!(route_name("GET /sync/serial#x HTTP/1.1"), "GET /sync/serial");
        assert_eq!(route_name("POST /perf?m=label HTTP/1.1"), "POST /perf");
    }

    #[test]
    fn id_like_segments_become_placeholder() {
        assert_eq!(route_name("GET /messages/12345/attachments HTTP/1.1"), "GET /messages/:id/attachments");
        assert_eq!(route_name("DELETE /x/0123456789abcdef HTTP/1.1"), "DELETE /x/:id");
        assert_eq!(route_name("GET /a/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA-1 HTTP/1.1"), "GET /a/:id");
    }

    #[test]
    fn email_or_encoded_segment_becomes_placeholder() {
        assert_eq!(route_name("GET /user/a@b.com HTTP/1.1"), "GET /user/:id");
        assert_eq!(route_name("GET /file/%2Fhome%2Fx HTTP/1.1"), "GET /file/:id");
    }

    #[test]
    fn unparseable_lines_are_marked() {
        assert_eq!(route_name(""), "? ?");
        assert_eq!(route_name("get /x HTTP/1.1"), "? /x");
        assert_eq!(route_name("GET http://h/x HTTP/1.1"), "GET ?");
        assert_eq!(route_name("GET / HTTP/1.1"), "GET /");
    }

    #[test]
    fn very_long_route_is_capped() {
        let long = format!("GET /{} HTTP/1.1", "abc/".repeat(100));
        let r = route_name(&long);
        assert!(r.len() <= 4 + 120 + 3, "{}", r.len());
        assert!(!r.contains('?'));
    }
}
