//! Private permissions for OmaLook's config and cache directories (R2.6 / HARD-1).
//!
//! `set_private_umask` runs first thing in `main`, so every file and directory the backend creates
//! afterwards is owner-only. `harden_permissions` then tightens what already exists from earlier
//! runs (0755 dirs, 0644 databases and tokens). Only tightening is done: symlinks are never
//! followed or chmod'ed, and subdirectories are not walked.
//!
//! `umask` is process-wide, so the quickshell child started by the backend inherits it. That is
//! acceptable: it only narrows what the child creates.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

extern "C" {
    // POSIX umask(2). Declared directly so the crate needs no libc dependency.
    fn umask(mask: u32) -> u32;
}

/// Set the process umask to 077 (new files 0600, new dirs 0700).
pub fn set_private_umask() {
    // SAFETY: umask only swaps the process file-mode creation mask; it has no memory effects.
    unsafe {
        umask(0o077);
    }
}

/// Create (if missing) and tighten `config_dir` and `cache_dir`, then tighten every regular file
/// directly inside `config_dir`: dirs to 0700, files to 0600. Symlinks are skipped. Returns the
/// first error; the other entries are still processed.
pub fn harden_permissions(config_dir: &Path, cache_dir: &Path) -> io::Result<()> {
    let mut first_err: Option<io::Error> = None;
    let mut note = |r: io::Result<()>| {
        if let Err(e) = r {
            first_err.get_or_insert(e);
        }
    };

    for dir in [config_dir, cache_dir] {
        note(fs::create_dir_all(dir));
        note(set_mode_if_real_dir(dir, 0o700));
    }

    // A config dir that is itself a symlink is left alone, files included.
    let config_is_real_dir = fs::symlink_metadata(config_dir).map(|m| m.file_type().is_dir()).unwrap_or(false);
    if config_is_real_dir {
        match fs::read_dir(config_dir) {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(entry) => {
                            let path = entry.path();
                            // symlink_metadata: a link is judged by itself, never by its target.
                            if let Ok(meta) = fs::symlink_metadata(&path) {
                                if meta.file_type().is_file() {
                                    note(set_mode(&path, 0o600));
                                }
                            }
                        }
                        Err(e) => note(Err(e)),
                    }
                }
            }
            Err(e) => note(Err(e)),
        }
    }

    match first_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn set_mode_if_real_dir(path: &Path, mode: u32) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_dir() {
        set_mode(path, mode)
    } else {
        Ok(())
    }
}

fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.permissions().mode() & 0o777 == mode {
        return Ok(());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn mode_of(p: &Path) -> u32 {
        fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("omalook-fsperm-{}-{}-{}", name, std::process::id(), nanos));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn tightens_dirs_and_files_and_skips_symlinks() {
        let root = scratch("harden");
        let config = root.join("config");
        let cache = root.join("cache");
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(&cache).unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&cache, fs::Permissions::from_mode(0o755)).unwrap();

        let db = config.join("messages.db");
        fs::write(&db, b"x").unwrap();
        fs::set_permissions(&db, fs::Permissions::from_mode(0o644)).unwrap();
        let wal = config.join("messages.db-wal");
        fs::write(&wal, b"x").unwrap();
        fs::set_permissions(&wal, fs::Permissions::from_mode(0o644)).unwrap();

        // A symlink inside the config dir pointing at an outside file: neither may change.
        let outside = root.join("outside.txt");
        fs::write(&outside, b"y").unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o644)).unwrap();
        let link = config.join("mcp.token");
        symlink(&outside, &link).unwrap();

        harden_permissions(&config, &cache).unwrap();

        assert_eq!(mode_of(&config), 0o700);
        assert_eq!(mode_of(&cache), 0o700);
        assert_eq!(mode_of(&db), 0o600);
        assert_eq!(mode_of(&wal), 0o600);
        assert_eq!(mode_of(&outside), 0o644, "symlink target must not be chmod'ed");
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "symlink must stay a symlink");

        // Idempotent.
        harden_permissions(&config, &cache).unwrap();
        assert_eq!(mode_of(&db), 0o600);

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn creates_missing_dirs_private() {
        let root = scratch("create");
        let config = root.join("c");
        let cache = root.join("k");
        harden_permissions(&config, &cache).unwrap();
        assert_eq!(mode_of(&config), 0o700);
        assert_eq!(mode_of(&cache), 0o700);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn symlinked_config_dir_is_not_followed() {
        let root = scratch("linkdir");
        let real = root.join("real");
        fs::create_dir_all(&real).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o755)).unwrap();
        let config = root.join("config-link");
        symlink(&real, &config).unwrap();
        let cache = root.join("cache");
        let _ = harden_permissions(&config, &cache);
        assert_eq!(mode_of(&real), 0o755, "dir behind a symlink must not be chmod'ed");
        fs::remove_dir_all(&root).unwrap();
    }
}
