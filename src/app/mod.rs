//! Backend-side application state that the UI refers to by opaque ids (R3.2, OL-087).
//!
//! `files` holds the `FileHandle` registry: a request names a user file by a handle the backend
//! issued, never by a filesystem path. It replaces the R1.5 allow-list (`picked.rs`, OL-057).

pub mod calendar;
pub mod contacts;
pub mod files;
pub mod mail;
pub mod policy;
pub mod sms;

use serde::Serialize;

/// Longest preview / event body / SMS body a read returns, in characters.
pub const MAX_TEXT: usize = 2000;
/// Preview length inside list results (the single-message read gives the full stored preview).
pub const LIST_PREVIEW: usize = 300;
/// Answer for a message, event or contact that does not exist or is not visible to the caller.
pub const NOT_FOUND: &str = "not found";

/// Keep at most `n` characters, marking a cut.
pub fn clip(text: &str, n: usize) -> String {
    let mut it = text.chars();
    let head: String = it.by_ref().take(n).collect();
    if it.next().is_some() { format!("{}…", head) } else { head }
}

/// One page of a list read. Callers fetch `limit + 1` rows; `of` trims the extra and says where
/// the next page starts (`offset + limit`).
#[derive(Debug, Serialize)]
pub struct Page<T> {
    pub count: usize,
    pub results: Vec<T>,
    pub next_cursor: Option<String>,
}

impl<T> Page<T> {
    pub fn of(mut rows: Vec<T>, limit: i64, offset: i64) -> Self {
        let more = rows.len() as i64 > limit;
        rows.truncate(limit as usize);
        Page { count: rows.len(), results: rows, next_cursor: if more { Some((offset + limit).to_string()) } else { None } }
    }
}

/// `prefix` followed by `nbytes` of the OS RNG as lowercase hex (`fh-…`, `fp-…`). Fails only when
/// `/dev/urandom` cannot be read; callers refuse the request then rather than make a guessable id.
pub fn random_id(prefix: &str, nbytes: usize) -> Result<String, String> {
    use std::io::Read;
    let mut buf = vec![0u8; nbytes];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .map_err(|_| "no random source for a new id".to_string())?;
    let mut out = String::with_capacity(prefix.len() + nbytes * 2);
    out.push_str(prefix);
    for b in buf {
        out.push_str(&format!("{:02x}", b));
    }
    Ok(out)
}
