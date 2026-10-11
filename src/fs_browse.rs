//! Directory listing for the compose pane's file picker (`GET /fs/list`). The QML cannot read the
//! filesystem itself, so the daemon lists one directory at a time: names, kind, size, mtime.
//! Pure logic plus `read_dir`, tested against a temp directory.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// Entries returned for one directory; a bigger one is cut (and flagged) rather than freezing the UI.
pub const MAX_ENTRIES: usize = 5000;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    /// Seconds since the epoch, 0 when unknown.
    pub mtime: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Listing {
    /// Absolute path of the directory listed.
    pub path: String,
    /// Absolute path of its parent; none at `/`.
    pub parent: Option<String>,
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

/// `~`, `~/x` and empty (home) are expanded; anything else is taken as given.
pub fn expand(path: &str) -> PathBuf {
    let p = path.trim();
    let home = || dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    if p.is_empty() || p == "~" {
        home()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(p)
    }
}

/// List `path` (directories first, then files, each by case-insensitive name). Dot-files only
/// when `hidden`. Entries that cannot be inspected (broken links, no permission) are skipped.
/// A path that is a file lists the directory holding it.
pub fn list(path: &str, hidden: bool) -> Result<Listing, String> {
    let mut dir = expand(path);
    if dir.is_file() {
        dir = dir.parent().map(Path::to_path_buf).unwrap_or(dir);
    }
    let dir = std::fs::canonicalize(&dir).map_err(|_| format!("{} does not exist", dir.display()))?;
    if !dir.is_dir() {
        return Err(format!("{} is not a folder", dir.display()));
    }
    let rd = std::fs::read_dir(&dir).map_err(|_| format!("cannot open {}", dir.display()))?;
    let mut entries: Vec<Entry> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !hidden && name.starts_with('.') {
                return None;
            }
            let meta = std::fs::metadata(e.path()).ok()?; // follows links
            let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
            Some(Entry { name, dir: meta.is_dir(), size: if meta.is_dir() { 0 } else { meta.len() }, mtime })
        })
        .collect();
    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())).then_with(|| a.name.cmp(&b.name)));
    let truncated = entries.len() > MAX_ENTRIES;
    entries.truncate(MAX_ENTRIES);
    Ok(Listing {
        parent: dir.parent().map(|p| p.to_string_lossy().into_owned()),
        path: dir.to_string_lossy().into_owned(),
        entries,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("olook-fs-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn folders_first_then_files_by_name_and_dotfiles_only_on_request() {
        let d = tmp("sort");
        std::fs::create_dir(d.join("zeta")).unwrap();
        std::fs::create_dir(d.join("Alpha")).unwrap();
        std::fs::write(d.join("b.txt"), b"12345").unwrap();
        std::fs::write(d.join("A.txt"), b"").unwrap();
        std::fs::write(d.join(".secret"), b"x").unwrap();
        let l = list(d.to_str().unwrap(), false).unwrap();
        let names: Vec<_> = l.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "zeta", "A.txt", "b.txt"]);
        assert_eq!(l.entries[3].size, 5);
        assert!(l.entries[0].dir && !l.entries[2].dir);
        assert!(l.parent.is_some() && !l.truncated);
        let all = list(d.to_str().unwrap(), true).unwrap();
        assert!(all.entries.iter().any(|e| e.name == ".secret"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_file_path_lists_its_folder_and_a_missing_one_is_an_error() {
        let d = tmp("file");
        std::fs::write(d.join("x.bin"), b"1").unwrap();
        let l = list(d.join("x.bin").to_str().unwrap(), false).unwrap();
        assert_eq!(l.path, std::fs::canonicalize(&d).unwrap().to_string_lossy());
        assert!(list(d.join("nope").to_str().unwrap(), false).is_err());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn tilde_expands_and_broken_links_are_skipped() {
        assert_eq!(expand("~"), dirs::home_dir().unwrap());
        assert_eq!(expand("~/a"), dirs::home_dir().unwrap().join("a"));
        assert_eq!(expand("/etc"), PathBuf::from("/etc"));
        let d = tmp("link");
        std::os::unix::fs::symlink(d.join("gone"), d.join("dangling")).unwrap();
        std::fs::write(d.join("ok"), b"1").unwrap();
        let l = list(d.to_str().unwrap(), false).unwrap();
        assert_eq!(l.entries.len(), 1);
        std::fs::remove_dir_all(&d).ok();
    }
}
