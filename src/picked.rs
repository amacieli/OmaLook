//! Picked-file allow-list (security review 2026-11-12, SEC-002 / OL-057, roadmap R1.5).
//!
//! A request may name a local file only if the UI registered it first in this backend run
//! (`POST /files/register`, sent when the user picks, pastes or drops a file, or types a path in
//! the contact photo / import prompt), or if the file is one of our own spooled copies (outgoing
//! or draft spool). Every route that reads a file named in a request checks here first; a
//! request that names any other file is refused before the file is opened.
//!
//! The registry lives as long as the backend process, which is also the life of the per-launch
//! session token (`http::session`). Paths are stored canonical (symlinks and `..` resolved), and
//! a check canonicalizes the path it is given, so a link to a registered file is the same file
//! and a link out of the spool is not in the spool. Each entry carries the purpose it was picked
//! for: a file picked as a contacts import cannot be attached, and so on.
//!
//! Residual (accepted, review section 4.2 Option A): a same-uid process holding the session token
//! can register a path itself. This module removes the one-request exfiltration (a single
//! `/compose/send` naming `~/.ssh/id_ed25519`), keeps future agent/MCP write paths from reading
//! arbitrary files, and stops stored payloads from pointing outside the spool. R3.2 replaces it
//! with server-issued `FileHandle`s.

use crate::compose::OutgoingAttachment;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Most paths remembered per backend run; the oldest registration is dropped beyond this.
pub const MAX_ENTRIES: usize = 4096;
/// Most paths in one `POST /files/register`.
pub const MAX_PER_REQUEST: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Purpose {
    /// Compose / draft attachments, inline pictures, a pasted picture.
    Attach,
    /// `POST /contacts/photo/set`.
    Photo,
    /// `POST /contacts/import`.
    Import,
}

impl Purpose {
    pub fn parse(s: &str) -> Option<Purpose> {
        match s {
            "attach" => Some(Purpose::Attach),
            "photo" => Some(Purpose::Photo),
            "import" => Some(Purpose::Import),
            _ => None,
        }
    }
}

/// One registered file, as answered to the UI.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Picked {
    pub path: String,
    pub name: String,
    pub size: u64,
}

/// The allow-list itself. `GLOBAL` is the one the routes use; tests make their own.
#[derive(Default)]
pub struct Registry {
    entries: HashMap<(PathBuf, Purpose), ()>,
    order: VecDeque<(PathBuf, Purpose)>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Register one path the user picked. `~` and `~/x` are expanded; the path must then be
    /// absolute and name an existing regular file (after following links). Returns the
    /// canonical path, which is what the UI must send from now on.
    pub fn register(&mut self, purpose: Purpose, raw: &str) -> Result<Picked, String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err("no file named".into());
        }
        if raw.contains('\0') {
            return Err("not a valid path".into());
        }
        let expanded = crate::fs_browse::expand(raw);
        if !expanded.is_absolute() {
            return Err("the path must be absolute".into());
        }
        let canon = std::fs::canonicalize(&expanded).map_err(|_| "that file is not there".to_string())?;
        let meta = std::fs::metadata(&canon).map_err(|_| "that file cannot be read".to_string())?;
        if !meta.is_file() {
            return Err("that is not a file".into());
        }
        let name = canon.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let key = (canon.clone(), purpose);
        if self.entries.insert(key.clone(), ()).is_none() {
            self.order.push_back(key);
            while self.order.len() > MAX_ENTRIES {
                if let Some(old) = self.order.pop_front() {
                    self.entries.remove(&old);
                }
            }
        }
        Ok(Picked { path: canon.to_string_lossy().into_owned(), name, size: meta.len() })
    }

    /// Was this file registered for `purpose`? The path is canonicalized first; a path that does
    /// not resolve is not allowed.
    pub fn allowed(&self, purpose: Purpose, path: &Path) -> bool {
        match std::fs::canonicalize(path) {
            Ok(c) => self.entries.contains_key(&(c, purpose)),
            Err(_) => false,
        }
    }
}

static GLOBAL: Mutex<Option<Registry>> = Mutex::new(None);

fn with_global<T>(f: impl FnOnce(&mut Registry) -> T) -> T {
    let mut g = GLOBAL.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(Registry::new))
}

/// `Registry::register` on the process-wide registry.
pub fn register(purpose: Purpose, raw: &str) -> Result<Picked, String> {
    with_global(|r| r.register(purpose, raw))
}

/// `Registry::allowed` on the process-wide registry.
pub fn allowed(purpose: Purpose, path: &Path) -> bool {
    with_global(|r| r.allowed(purpose, path))
}

/// Our spool roots: outgoing copies (`attach_spool::default_root`) and draft copies
/// (`draft_sync::default_root`).
pub fn spool_roots() -> Vec<PathBuf> {
    vec![crate::attach_spool::default_root(), crate::draft_sync::default_root()]
}

/// Is `path` a regular file inside one of `roots` once links are resolved? Roots that do not
/// exist hold nothing.
pub fn in_spool(path: &Path, roots: &[PathBuf]) -> bool {
    let Ok(c) = std::fs::canonicalize(path) else { return false };
    if !std::fs::metadata(&c).map(|m| m.is_file()).unwrap_or(false) {
        return false;
    }
    roots.iter().filter_map(|r| std::fs::canonicalize(r).ok()).any(|r| c.starts_with(&r))
}

/// The refusal text for a file the user did not pick in this run. Names only the file's display
/// name, never the path (it goes back to the UI and may be logged).
fn not_picked(a: &OutgoingAttachment) -> String {
    format!("{} was not picked in this session; attach it again", a.display_name())
}

/// Check every file an outgoing message or draft would read, before anything is opened:
///
/// * `path` must be a spooled copy (inside `roots`) or registered for `Purpose::Attach`.
/// * a non-empty `source_path` (the original behind a spooled copy, which `snapshot_draft`
///   re-reads when the original changed) must be registered, or equal the `source_path` of one
///   of `trusted_sources` (the attachments already stored for this draft by an earlier save,
///   which passed this check then).
///
/// A `path` that no longer exists is allowed only when it is listed in `trusted_sources` by its
/// own `path` (a spooled copy that has gone: the draft keeps it for the '!' chip and never reads it).
pub fn check_attachments(
    reg: &Registry,
    attachments: &[OutgoingAttachment],
    roots: &[PathBuf],
    trusted: &[OutgoingAttachment],
) -> Result<(), String> {
    for a in attachments {
        let path = Path::new(&a.path);
        let path_ok = in_spool(path, roots)
            || reg.allowed(Purpose::Attach, path)
            || (!path.exists() && trusted.iter().any(|t| t.path == a.path));
        if !path_ok {
            return Err(not_picked(a));
        }
        if !a.source_path.is_empty() {
            let src_ok = reg.allowed(Purpose::Attach, Path::new(&a.source_path))
                || trusted.iter().any(|t| !t.source_path.is_empty() && t.source_path == a.source_path);
            if !src_ok {
                return Err(not_picked(a));
            }
        }
    }
    Ok(())
}

/// For `POST /compose/send`: `attach_spool::snapshot` reads only `path` (it records `source_path` as
/// metadata and never opens it), so only `path` is checked: a spooled copy or a registered file. A
/// draft resumed after a restart is sent from its spooled copies, whose originals were picked in an
/// earlier run.
pub fn check_send_attachments(reg: &Registry, attachments: &[OutgoingAttachment], roots: &[PathBuf]) -> Result<(), String> {
    for a in attachments {
        let path = Path::new(&a.path);
        if !(in_spool(path, roots) || reg.allowed(Purpose::Attach, path)) {
            return Err(not_picked(a));
        }
    }
    Ok(())
}

/// `check_send_attachments` against the process-wide registry and the real spool roots.
pub fn check_send(attachments: &[OutgoingAttachment]) -> Result<(), String> {
    let roots = spool_roots();
    with_global(|r| check_send_attachments(r, attachments, &roots))
}

/// `check_attachments` against the process-wide registry and the real spool roots.
pub fn check_message_attachments(attachments: &[OutgoingAttachment], trusted: &[OutgoingAttachment]) -> Result<(), String> {
    let roots = spool_roots();
    with_global(|r| check_attachments(r, attachments, &roots, trusted))
}

/// For queued/stored payloads about to be sent (outbox replay, draft send): every attachment must
/// be a spooled copy. Nothing outside the spool is read from a stored row.
pub fn check_stored(attachments: &[OutgoingAttachment], roots: &[PathBuf]) -> Result<(), String> {
    for a in attachments {
        if !in_spool(Path::new(&a.path), roots) {
            return Err(format!("{} is not a prepared copy; attach it again", a.display_name()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("omalook-picked-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn att(path: &Path, source: &str) -> OutgoingAttachment {
        OutgoingAttachment { path: path.to_string_lossy().into_owned(), name: "f".into(), source_path: source.into(), ..Default::default() }
    }

    #[test]
    fn unregistered_path_is_refused_and_registered_one_passes() {
        let d = tmp("basic");
        let secret = d.join("secret");
        let picked = d.join("picked.txt");
        fs::write(&secret, b"s").unwrap();
        fs::write(&picked, b"p").unwrap();
        let mut reg = Registry::new();
        let p = reg.register(Purpose::Attach, picked.to_str().unwrap()).unwrap();
        assert_eq!(p.size, 1);
        assert_eq!(p.name, "picked.txt");
        let roots = vec![d.join("spool")];
        assert!(check_attachments(&reg, &[att(&picked, "")], &roots, &[]).is_ok());
        let e = check_attachments(&reg, &[att(&picked, ""), att(&secret, "")], &roots, &[]).unwrap_err();
        assert!(!e.contains(secret.to_str().unwrap()), "error must not echo the path");
    }

    #[test]
    fn purposes_are_separate() {
        let d = tmp("purpose");
        let f = d.join("contacts.vcf");
        fs::write(&f, b"x").unwrap();
        let mut reg = Registry::new();
        reg.register(Purpose::Import, f.to_str().unwrap()).unwrap();
        assert!(reg.allowed(Purpose::Import, &f));
        assert!(!reg.allowed(Purpose::Attach, &f));
        assert!(!reg.allowed(Purpose::Photo, &f));
    }

    #[test]
    fn links_resolve_both_ways() {
        let d = tmp("links");
        let real = d.join("real");
        let secret = d.join("secret");
        fs::write(&real, b"r").unwrap();
        fs::write(&secret, b"s").unwrap();
        let mut reg = Registry::new();
        reg.register(Purpose::Attach, real.to_str().unwrap()).unwrap();
        // A link to a registered file is that file.
        let to_real = d.join("to_real");
        std::os::unix::fs::symlink(&real, &to_real).unwrap();
        assert!(reg.allowed(Purpose::Attach, &to_real));
        // A link inside the spool pointing out of it is not a spooled copy.
        let spool = d.join("spool");
        fs::create_dir_all(spool.join("x")).unwrap();
        let out = spool.join("x").join("file");
        std::os::unix::fs::symlink(&secret, &out).unwrap();
        assert!(!in_spool(&out, &[spool.clone()]));
        // `..` out of the spool is not in the spool.
        let dotdot = spool.join("x").join("..").join("..").join("secret");
        assert!(!in_spool(&dotdot, &[spool.clone()]));
        // A real file in the spool is.
        let copy = spool.join("x").join("copy");
        fs::write(&copy, b"c").unwrap();
        assert!(in_spool(&copy, &[spool]));
    }

    #[test]
    fn registering_a_link_stores_its_target() {
        let d = tmp("reglink");
        let real = d.join("real");
        fs::write(&real, b"r").unwrap();
        let link = d.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mut reg = Registry::new();
        let p = reg.register(Purpose::Attach, link.to_str().unwrap()).unwrap();
        assert_eq!(PathBuf::from(&p.path), fs::canonicalize(&real).unwrap());
    }

    #[test]
    fn register_refuses_dirs_missing_relative_and_nul() {
        let d = tmp("refuse");
        let mut reg = Registry::new();
        assert!(reg.register(Purpose::Attach, d.to_str().unwrap()).is_err());
        assert!(reg.register(Purpose::Attach, d.join("nope").to_str().unwrap()).is_err());
        assert!(reg.register(Purpose::Attach, "relative/file").is_err());
        assert!(reg.register(Purpose::Attach, "").is_err());
        assert!(reg.register(Purpose::Attach, "/etc/passwd\0x").is_err());
    }

    #[test]
    fn source_path_must_be_picked_or_already_stored() {
        let d = tmp("source");
        let spool = d.join("spool");
        fs::create_dir_all(spool.join("dr").join("0")).unwrap();
        let copy = spool.join("dr").join("0").join("file");
        fs::write(&copy, b"c").unwrap();
        let original = d.join("original");
        let secret = d.join("secret");
        fs::write(&original, b"o").unwrap();
        fs::write(&secret, b"s").unwrap();
        let reg = Registry::new();
        let roots = vec![spool];
        // A spooled copy with a source the draft already had: fine (restored draft, new run).
        let stored = vec![att(&copy, original.to_str().unwrap())];
        assert!(check_attachments(&reg, &[att(&copy, original.to_str().unwrap())], &roots, &stored).is_ok());
        // Same copy, source swapped for a file never picked: refused (snapshot_draft would read it).
        assert!(check_attachments(&reg, &[att(&copy, secret.to_str().unwrap())], &roots, &stored).is_err());
        // No stored draft, unregistered source: refused.
        assert!(check_attachments(&reg, &[att(&copy, original.to_str().unwrap())], &roots, &[]).is_err());
    }

    #[test]
    fn send_checks_only_the_file_it_reads() {
        let d = tmp("send");
        let spool = d.join("spool");
        fs::create_dir_all(spool.join("dr").join("0")).unwrap();
        let copy = spool.join("dr").join("0").join("file");
        fs::write(&copy, b"c").unwrap();
        let secret = d.join("secret");
        fs::write(&secret, b"s").unwrap();
        let reg = Registry::new();
        let roots = vec![spool];
        // A resumed draft's spooled copy, original from an earlier run: sendable.
        assert!(check_send_attachments(&reg, &[att(&copy, secret.to_str().unwrap())], &roots).is_ok());
        // An unpicked file as the path itself: refused.
        assert!(check_send_attachments(&reg, &[att(&secret, "")], &roots).is_err());
    }

    #[test]
    fn a_gone_copy_passes_only_if_the_draft_had_it() {
        let d = tmp("gone");
        let roots = vec![d.join("spool")];
        let gone = d.join("spool").join("dr").join("0").join("file");
        let reg = Registry::new();
        let stored = vec![att(&gone, "")];
        assert!(check_attachments(&reg, &[att(&gone, "")], &roots, &stored).is_ok());
        assert!(check_attachments(&reg, &[att(&gone, "")], &roots, &[]).is_err());
    }

    #[test]
    fn stored_payloads_must_point_into_the_spool() {
        let d = tmp("stored");
        let spool = d.join("spool");
        fs::create_dir_all(&spool).unwrap();
        let copy = spool.join("file");
        let outside = d.join("outside");
        fs::write(&copy, b"c").unwrap();
        fs::write(&outside, b"o").unwrap();
        let roots = vec![spool];
        assert!(check_stored(&[att(&copy, "")], &roots).is_ok());
        assert!(check_stored(&[att(&outside, "")], &roots).is_err());
    }

    #[test]
    fn registry_is_bounded() {
        let d = tmp("bound");
        let f = d.join("f");
        fs::write(&f, b"x").unwrap();
        let mut reg = Registry::new();
        for _ in 0..3 {
            reg.register(Purpose::Attach, f.to_str().unwrap()).unwrap();
        }
        assert_eq!(reg.order.len(), 1, "re-registering does not grow the list");
    }
}
