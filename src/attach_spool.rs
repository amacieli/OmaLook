//! Outgoing attachment spool (docs/PLAN-compose-attachments.md, section 3.2).
//!
//! When Send is pressed the daemon copies every attached file into
//! `<cache>/omalook/outgoing/<outbox id>/<n>/<name>` and queues the *copies*. The user can then
//! delete, move or edit the originals during the undo window, and across retries, without
//! changing what is sent. The undo path never sees spool paths: the UI keeps its own chips with
//! the original paths, so Undo restores them as they were.
//!
//! * Copies are real copies (`std::io::copy` between files, which the kernel turns into a
//!   reflink on filesystems that have them), never hard links: an edit in place of a hard-linked
//!   original would change the bytes that get sent.
//! * A file that changes while it is being copied is refused rather than sent half-written.
//! * The spool of a message is removed when its outbox row reaches `sent`, `cancelled` or
//!   `failed`; `sweep` removes whatever a crash left behind.

use crate::compose::OutgoingAttachment;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A spool directory younger than this is never swept: it may belong to a send that is still
/// copying its files and has not queued its outbox row yet.
pub const SWEEP_MIN_AGE: Duration = Duration::from_secs(3600);

/// `<cache dir>/omalook/outgoing`.
pub fn default_root() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("omalook")
        .join("outgoing")
}

fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Directory holding the copies of one outbox message.
pub fn spool_dir(root: &Path, outbox_id: &str) -> PathBuf {
    root.join(outbox_id)
}

fn is_disk_full(e: &io::Error) -> bool {
    e.raw_os_error() == Some(28) // ENOSPC
}

fn mtime_of(m: &fs::Metadata) -> Option<SystemTime> {
    m.modified().ok()
}

/// Copy one file, refusing it if it changes while being read.
fn copy_one(src: &Path, dest: &Path, name: &str) -> Result<u64, String> {
    let mut input = fs::File::open(src).map_err(|_| format!("{} cannot be read", name))?;
    let before = input.metadata().map_err(|_| format!("{} cannot be read", name))?;
    if !before.is_file() {
        return Err(format!("{} is not a file", name));
    }
    let mut output = fs::File::create(dest).map_err(|e| {
        if is_disk_full(&e) {
            format!("not enough disk space to prepare {} for sending", name)
        } else {
            format!("cannot prepare {} for sending: {}", name, e)
        }
    })?;
    let copied = io::copy(&mut input, &mut output).map_err(|e| {
        if is_disk_full(&e) {
            format!("not enough disk space to prepare {} for sending", name)
        } else {
            format!("cannot copy {}: {}", name, e)
        }
    })?;
    output.sync_all().ok();
    // The source must be the same size and have the same modification time as when we started,
    // and we must have copied exactly that many bytes.
    let after = fs::metadata(src).map_err(|_| format!("{} is no longer there", name))?;
    if copied != before.len() || after.len() != before.len() || mtime_of(&after) != mtime_of(&before) {
        return Err(format!("{} changed while it was being prepared for sending; try again", name));
    }
    Ok(copied)
}

/// Copy `attachments` into the spool of `outbox_id` and return them rewritten to point at the
/// copies: `path` is the copy, `source_path` the original, `name` the name the recipient sees and
/// `size` the number of bytes copied. On any failure nothing is left behind.
pub fn snapshot(root: &Path, outbox_id: &str, attachments: &[OutgoingAttachment]) -> Result<Vec<OutgoingAttachment>, String> {
    if attachments.is_empty() {
        return Ok(Vec::new());
    }
    if !safe_id(outbox_id) {
        return Err("invalid message id".into());
    }
    let dir = spool_dir(root, outbox_id);
    let result = (|| {
        let mut out = Vec::with_capacity(attachments.len());
        for (i, a) in attachments.iter().enumerate() {
            let name = a.display_name();
            let slot = dir.join(i.to_string());
            fs::create_dir_all(&slot).map_err(|e| {
                if is_disk_full(&e) {
                    format!("not enough disk space to prepare {} for sending", name)
                } else {
                    format!("cannot prepare {} for sending: {}", name, e)
                }
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                // Attachments are the user's private files: keep the copies private too.
                let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
                let _ = fs::set_permissions(&slot, fs::Permissions::from_mode(0o700));
            }
            // A one-time-use file name inside its own directory: the real name is kept in `name`.
            let dest = slot.join("file");
            let size = copy_one(Path::new(&a.path), &dest, &name)?;
            out.push(OutgoingAttachment {
                path: dest.to_string_lossy().into_owned(),
                name,
                size,
                content_type: a.content_type.clone(),
                source_path: if a.source_path.is_empty() { a.path.clone() } else { a.source_path.clone() },
            });
        }
        Ok(out)
    })();
    if result.is_err() {
        remove(root, outbox_id);
    }
    result
}

/// Check, just before a message goes out, that what is about to be sent is what was queued:
/// a spooled copy must still exist with the size recorded when it was copied; a message queued
/// without a spool (older payloads) falls back to checking the picked file itself.
pub fn verify(attachments: &[OutgoingAttachment]) -> Result<(), String> {
    for a in attachments {
        let name = a.display_name();
        let meta = fs::metadata(&a.path).map_err(|_| {
            if a.source_path.is_empty() {
                format!("{} is no longer there", name)
            } else {
                format!("the prepared copy of {} is missing", name)
            }
        })?;
        if !meta.is_file() {
            return Err(format!("{} is not a file", name));
        }
        if !a.source_path.is_empty() && meta.len() != a.size {
            return Err(format!("the prepared copy of {} is damaged", name));
        }
    }
    Ok(())
}

/// Delete the spool of one message. Missing is fine.
pub fn remove(root: &Path, outbox_id: &str) {
    if safe_id(outbox_id) {
        let _ = fs::remove_dir_all(spool_dir(root, outbox_id));
    }
}

/// Delete every spool directory that no live outbox row owns and that is older than `min_age`.
/// Returns how many were removed.
pub fn sweep(root: &Path, live: &HashSet<String>, min_age: Duration) -> usize {
    let Ok(entries) = fs::read_dir(root) else { return 0 };
    let mut removed = 0;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if live.contains(&name) {
            continue;
        }
        let old_enough = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map(|age| age >= min_age)
            .unwrap_or(false);
        if old_enough && fs::remove_dir_all(e.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("olook-spool-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn pick(dir: &Path, name: &str, bytes: &[u8]) -> OutgoingAttachment {
        let p = dir.join(name);
        fs::write(&p, bytes).unwrap();
        OutgoingAttachment { path: p.to_string_lossy().into(), name: name.into(), size: bytes.len() as u64, ..Default::default() }
    }

    #[test]
    fn snapshot_copies_the_bytes_and_remembers_the_original() {
        let (src, root) = (tmp("a-src"), tmp("a-root"));
        let a = pick(&src, "Q3 report.pdf", b"original bytes");
        let spooled = snapshot(&root, "ob-1", &[a.clone()]).unwrap();
        assert_eq!(spooled.len(), 1);
        let s = &spooled[0];
        assert_ne!(s.path, a.path);
        assert!(Path::new(&s.path).starts_with(spool_dir(&root, "ob-1")));
        assert_eq!(s.source_path, a.path, "Undo restores the original path");
        assert_eq!(s.display_name(), "Q3 report.pdf", "the recipient still sees the real name");
        assert_eq!(s.size, 14);
        assert_eq!(fs::read(&s.path).unwrap(), b"original bytes");
        assert!(verify(&spooled).is_ok());
        fs::remove_dir_all(&src).ok();
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_original_can_be_deleted_or_replaced_without_changing_what_is_sent() {
        let (src, root) = (tmp("b-src"), tmp("b-root"));
        let a = pick(&src, "a.txt", b"version one");
        let b = pick(&src, "b.txt", b"keep me");
        let spooled = snapshot(&root, "ob-2", &[a.clone(), b.clone()]).unwrap();
        fs::remove_file(&a.path).unwrap(); // deleted during the undo window
        fs::write(&b.path, b"version two, much longer").unwrap(); // replaced
        assert!(verify(&spooled).is_ok(), "the copies are untouched");
        assert_eq!(fs::read(&spooled[0].path).unwrap(), b"version one");
        assert_eq!(fs::read(&spooled[1].path).unwrap(), b"keep me");
        fs::remove_dir_all(&src).ok();
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_edit_in_place_does_not_reach_the_copy() {
        // The reason copies are not hard links.
        let (src, root) = (tmp("c-src"), tmp("c-root"));
        let a = pick(&src, "a.txt", b"before");
        let spooled = snapshot(&root, "ob-3", &[a.clone()]).unwrap();
        let mut f = fs::OpenOptions::new().write(true).truncate(true).open(&a.path).unwrap();
        io::Write::write_all(&mut f, b"after!").unwrap();
        assert_eq!(fs::read(&spooled[0].path).unwrap(), b"before");
        fs::remove_dir_all(&src).ok();
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_or_unreadable_file_is_refused_and_leaves_no_spool() {
        let (src, root) = (tmp("d-src"), tmp("d-root"));
        let ok = pick(&src, "ok.txt", b"fine");
        let gone = OutgoingAttachment { path: src.join("gone.txt").to_string_lossy().into(), ..Default::default() };
        let err = snapshot(&root, "ob-4", &[ok, gone]).unwrap_err();
        assert_eq!(err, "gone.txt cannot be read");
        assert!(!spool_dir(&root, "ob-4").exists(), "the first file's copy is cleaned up too");
        let folder = OutgoingAttachment { path: src.to_string_lossy().into(), ..Default::default() };
        assert!(snapshot(&root, "ob-5", &[folder]).unwrap_err().contains("is not a file"));
        assert!(!spool_dir(&root, "ob-5").exists());
        fs::remove_dir_all(&src).ok();
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_spool_that_cannot_be_written_is_reported_and_cleaned_up() {
        // The disk-full path: any write failure is reported by file name and leaves nothing.
        let (src, root) = (tmp("e-src"), tmp("e-root"));
        let a = pick(&src, "a.txt", b"x");
        fs::write(spool_dir(&root, "ob-6"), b"in the way").unwrap(); // a file where the directory must go
        let err = snapshot(&root, "ob-6", &[a]).unwrap_err();
        assert!(err.contains("a.txt"), "{}", err);
        assert!(is_disk_full(&io::Error::from_raw_os_error(28)));
        assert!(!is_disk_full(&io::Error::from_raw_os_error(13)));
        fs::remove_dir_all(&src).ok();
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn verify_catches_a_missing_or_damaged_copy() {
        let (src, root) = (tmp("f-src"), tmp("f-root"));
        let a = pick(&src, "a.txt", b"12345");
        let mut spooled = snapshot(&root, "ob-7", &[a]).unwrap();
        fs::write(&spooled[0].path, b"123").unwrap();
        assert_eq!(verify(&spooled).unwrap_err(), "the prepared copy of a.txt is damaged");
        fs::remove_file(&spooled[0].path).unwrap();
        assert_eq!(verify(&spooled).unwrap_err(), "the prepared copy of a.txt is missing");
        // an old payload with no spool: the file itself is checked
        spooled[0].source_path.clear();
        assert_eq!(verify(&spooled).unwrap_err(), "a.txt is no longer there");
        fs::remove_dir_all(&src).ok();
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn remove_deletes_one_message_and_ignores_bad_ids() {
        let (src, root) = (tmp("g-src"), tmp("g-root"));
        snapshot(&root, "ob-8", &[pick(&src, "a.txt", b"x")]).unwrap();
        snapshot(&root, "ob-9", &[pick(&src, "b.txt", b"y")]).unwrap();
        remove(&root, "ob-8");
        remove(&root, "../etc");
        remove(&root, "");
        assert!(!spool_dir(&root, "ob-8").exists());
        assert!(spool_dir(&root, "ob-9").exists());
        assert!(snapshot(&root, "../x", &[pick(&src, "c.txt", b"z")]).is_err(), "no path tricks in the id");
        fs::remove_dir_all(&src).ok();
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn sweep_removes_only_old_orphans() {
        let (src, root) = (tmp("h-src"), tmp("h-root"));
        for id in ["live", "orphan"] {
            snapshot(&root, id, &[pick(&src, "a.txt", b"x")]).unwrap();
        }
        let live: HashSet<String> = ["live".to_string()].into();
        assert_eq!(sweep(&root, &live, SWEEP_MIN_AGE), 0, "a fresh orphan might still be copying");
        assert_eq!(sweep(&root, &live, Duration::ZERO), 1);
        assert!(spool_dir(&root, "live").exists());
        assert!(!spool_dir(&root, "orphan").exists());
        assert_eq!(sweep(&root.join("nope"), &live, Duration::ZERO), 0, "no spool directory yet");
        fs::remove_dir_all(&src).ok();
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn nothing_to_attach_makes_no_spool() {
        let root = tmp("i-root");
        assert!(snapshot(&root, "ob-10", &[]).unwrap().is_empty());
        assert!(!spool_dir(&root, "ob-10").exists());
        fs::remove_dir_all(&root).ok();
    }
}
