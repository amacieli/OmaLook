//! Backend-side application state that the UI refers to by opaque ids (R3.2, OL-087).
//!
//! `files` holds the `FileHandle` registry: a request names a user file by a handle the backend
//! issued, never by a filesystem path. It replaces the R1.5 allow-list (`picked.rs`, OL-057).

pub mod files;

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
