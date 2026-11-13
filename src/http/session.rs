//! The per-launch session token for the UI port (roadmap R1.4, SEC-001 local-process vector).
//!
//! Each backend start makes a new 256-bit token, writes it to `$XDG_RUNTIME_DIR/omalook/ui-<port>.token`
//! (directory 0700, file 0600, written to a temp file and renamed) and hands it to the UI it launches in
//! `OMALOOK_TOKEN`; a later launch that only opens a window reads it from the file. `http::guard` refuses
//! every request without `Authorization: Bearer <token>`. One file per port, so a scratch instance on
//! `OMALOOK_PORT` has its own. Same-uid processes can still read the file — they can read the keyring
//! and the database too, so that is the accepted residual (review §4.2 Option A).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Environment variable the launcher sets for the UI process.
pub const ENV: &str = "OMALOOK_TOKEN";

/// Where the token for `port` lives: `$XDG_RUNTIME_DIR/omalook/`, or `<config dir>/run/` when there is
/// no runtime dir (no login session).
pub fn token_path(config_dir: &Path, port: u16) -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|d| d.is_absolute() && d.is_dir())
        .map(|d| d.join("omalook"))
        .unwrap_or_else(|| config_dir.join("run"));
    base.join(format!("ui-{}.token", port))
}

fn random_hex() -> std::io::Result<String> {
    let mut buf = [0u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{:02x}", b)).collect())
}

/// Make a new token for this backend start and write it (replacing any earlier one for the port).
pub fn create(config_dir: &Path, port: u16) -> std::io::Result<String> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let path = token_path(config_dir, port);
    let dir = path.parent().expect("token path has a directory");
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    let token = random_hex()?;
    let tmp = path.with_extension("token.tmp");
    let _ = fs::remove_file(&tmp);
    let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
    f.write_all(token.as_bytes())?;
    f.sync_all()?;
    fs::rename(&tmp, &path)?;
    Ok(token)
}

/// The token of the backend running on `port`, if its file is there.
pub fn read(config_dir: &Path, port: u16) -> Option<String> {
    let t = fs::read_to_string(token_path(config_dir, port)).ok()?;
    let t = t.trim();
    (t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit())).then(|| t.to_string())
}

/// The `Authorization` header value for `token`.
pub fn bearer(token: &str) -> String {
    format!("Bearer {}", token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn create_writes_a_private_file_and_read_finds_it() {
        // No XDG_RUNTIME_DIR games here (env is process-wide): use a config dir and the runtime dir the
        // test process has, whichever token_path picks; both must be private.
        let cfg = std::env::temp_dir().join(format!("omalook-session-{}", std::process::id()));
        let _ = fs::remove_dir_all(&cfg);
        let port = 40000 + (std::process::id() % 20000) as u16;
        let t1 = create(&cfg, port).unwrap();
        assert_eq!(t1.len(), 64);
        let p = token_path(&cfg, port);
        assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(p.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(read(&cfg, port).as_deref(), Some(t1.as_str()));
        // every start makes a new one
        let t2 = create(&cfg, port).unwrap();
        assert_ne!(t1, t2);
        assert_eq!(read(&cfg, port).as_deref(), Some(t2.as_str()));
        // one file per port
        assert_ne!(token_path(&cfg, port), token_path(&cfg, port + 1));
        assert_eq!(read(&cfg, port + 1), None);
        let _ = fs::remove_file(&p);
        let _ = fs::remove_dir_all(&cfg);
    }

    #[test]
    fn read_rejects_junk() {
        let cfg = std::env::temp_dir().join(format!("omalook-session-junk-{}", std::process::id()));
        let port = 39000 + (std::process::id() % 1000) as u16;
        let p = token_path(&cfg, port);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "short").unwrap();
        assert_eq!(read(&cfg, port), None);
        fs::write(&p, "z".repeat(64)).unwrap();
        assert_eq!(read(&cfg, port), None);
        let _ = fs::remove_file(&p);
        let _ = fs::remove_dir_all(&cfg);
    }
}
