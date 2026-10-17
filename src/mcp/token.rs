//! The MCP bearer token: `<config dir>/mcp.token`, mode 0600, 256 random bits as hex.
//! Created on first use; regenerating it cuts off every client that has the old one.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub fn token_path(config_dir: &Path) -> PathBuf { config_dir.join("mcp.token") }

fn random_hex() -> std::io::Result<String> {
    let mut buf = [0u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{:02x}", b)).collect())
}

fn write_private(path: &Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("token.tmp");
    let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(content.as_bytes())?;
    f.sync_all()?;
    fs::rename(&tmp, path)
}

/// The existing token, or a fresh one written first. `None` only if the file cannot be made.
pub fn load_or_create(config_dir: &Path) -> Option<String> {
    let path = token_path(config_dir);
    if let Ok(s) = fs::read_to_string(&path) {
        let s = s.trim().to_string();
        if s.len() >= 32 { return Some(s); }
    }
    regenerate(config_dir)
}

/// Replace the token. Returns the new one.
pub fn regenerate(config_dir: &Path) -> Option<String> {
    let t = random_hex().ok()?;
    fs::create_dir_all(config_dir).ok()?;
    write_private(&token_path(config_dir), &t).ok()?;
    Some(t)
}

/// Compare without stopping at the first difference.
pub fn matches(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().min(b.len()) { diff |= (a[i] ^ b[i]) as usize; }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn dir(n: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("omalook-mcp-token-{}-{}", n, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn created_once_private_and_stable() {
        let d = dir("a");
        let t1 = load_or_create(&d).unwrap();
        assert_eq!(t1.len(), 64);
        assert_eq!(fs::metadata(token_path(&d)).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(load_or_create(&d).unwrap(), t1);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn regenerate_changes_it() {
        let d = dir("b");
        let t1 = load_or_create(&d).unwrap();
        let t2 = regenerate(&d).unwrap();
        assert_ne!(t1, t2);
        assert_eq!(load_or_create(&d).unwrap(), t2);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn compare() {
        assert!(matches("abc", "abc"));
        assert!(!matches("abc", "abd"));
        assert!(!matches("abc", "abcd"));
        assert!(!matches("", "a"));
    }
}
