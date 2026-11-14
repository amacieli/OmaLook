//! FileHandle registry (roadmap R3.2, OL-087; supersedes the R1.5 allow-list OL-057 in `picked.rs`).
//!
//! The UI never sends a filesystem path for a user file. It registers the file it was given
//! (`POST /files/register`, purpose attach|photo|import) and gets back an opaque handle
//! `fh-` + 32 hex chars from the OS RNG. Later requests carry the handle; the route resolves it
//! to the path here, and refuses anything else. The backend itself hands the UI handles for files
//! it already trusts (our spool copies, the attachments of a stored draft) with `mint_for_stored`,
//! so a resumed draft round-trips with handles only.
//!
//! `register` refuses files under the secret roots (the omalook config dir, the runtime token dir,
//! keyrings, `~/.ssh`, `~/.gnupg`, `/proc`, `/sys`). `/fs/list` is scoped to a picker: the UI opens
//! `POST /files/picker` when its folder browser opens and passes the `fp-` id on every listing.
//!
//! Residual (review §4.2 Option A, unchanged): a same-uid process holding the session token can
//! register or list any file outside the secret roots. Handles stop a request from naming a path;
//! they do not stop a process that already has the token.

use crate::app::random_id;
use crate::compose::OutgoingAttachment;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Most handles remembered per backend run; the oldest is dropped beyond this.
pub const MAX_ENTRIES: usize = 4096;
/// Most files in one `POST /files/register`.
pub const MAX_PER_REQUEST: usize = 256;
/// Most live pickers; the oldest is dropped beyond this.
pub const MAX_PICKERS: usize = 16;
/// A picker expires this long after its last `/fs/list`.
pub const PICKER_IDLE: Duration = Duration::from_secs(30 * 60);

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

/// One registered file, as answered to the UI. No path: the UI gets the handle only.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Registered {
    pub handle: String,
    pub name: String,
    pub size: u64,
}

#[allow(dead_code)] // name and size are kept with the handle; the UI gets them at register time.
struct Entry {
    purpose: Purpose,
    path: PathBuf,
    name: String,
    size: u64,
}

/// The handle table. `GLOBAL` is the one the routes use; tests make their own and inject roots.
pub struct Registry {
    by_handle: HashMap<String, Entry>,
    by_key: HashMap<(Purpose, PathBuf), String>,
    order: VecDeque<String>,
    secrets: Vec<PathBuf>,
}

impl Default for Registry {
    fn default() -> Self {
        Registry::with_secret_roots(secret_roots())
    }
}

impl Registry {
    pub fn new() -> Registry {
        Registry::default()
    }

    pub fn with_secret_roots(secrets: Vec<PathBuf>) -> Registry {
        Registry { by_handle: HashMap::new(), by_key: HashMap::new(), order: VecDeque::new(), secrets }
    }

    /// Register one path the user picked. `~` and `~/x` are expanded; the path must then be
    /// absolute, name an existing regular file after links are followed, and not lie under a
    /// secret root. Registering the same file for the same purpose again returns the same handle.
    pub fn register(&mut self, purpose: Purpose, raw: &str) -> Result<Registered, String> {
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
        if under_any(&canon, &self.secrets) {
            return Err("that file cannot be attached from here".into());
        }
        let name = canon.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let handle = self.insert(purpose, canon, name.clone(), meta.len())?;
        Ok(Registered { handle, name, size: meta.len() })
    }

    /// The handle for a file the backend already trusts (a spool copy, or an attachment stored
    /// with a draft). No picker check: the caller decides what is trusted. The file need not
    /// exist (a spool copy that has gone keeps its chip, and its handle still resolves).
    pub fn mint_for_stored(&mut self, purpose: Purpose, path: &Path, size: u64) -> Result<String, String> {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.insert(purpose, path.to_path_buf(), name, size)
    }

    /// The path behind `handle`, for `purpose` only. Unknown, forged or wrong-purpose handles give None.
    pub fn resolve(&self, purpose: Purpose, handle: &str) -> Option<PathBuf> {
        self.by_handle.get(handle).filter(|e| e.purpose == purpose).map(|e| e.path.clone())
    }

    fn insert(&mut self, purpose: Purpose, path: PathBuf, name: String, size: u64) -> Result<String, String> {
        let key = (purpose, path.clone());
        if let Some(h) = self.by_key.get(&key) {
            return Ok(h.clone());
        }
        let handle = loop {
            let h = random_id("fh-", 16)?;
            if !self.by_handle.contains_key(&h) {
                break h;
            }
        };
        self.by_key.insert(key, handle.clone());
        self.by_handle.insert(handle.clone(), Entry { purpose, path, name, size });
        self.order.push_back(handle.clone());
        while self.order.len() > MAX_ENTRIES {
            if let Some(old) = self.order.pop_front() {
                if let Some(e) = self.by_handle.remove(&old) {
                    self.by_key.remove(&(e.purpose, e.path));
                }
            }
        }
        Ok(handle)
    }
}

/// Directories no UI file pick may come from, and no folder the picker may list. Some may not
/// exist on this machine; those hold nothing to refuse.
pub fn secret_roots() -> Vec<PathBuf> {
    let mut v = vec![crate::google_auth::default_config_dir()];
    if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|d| d.is_absolute()) {
        v.push(rt.join("omalook"));
    }
    if let Some(home) = dirs::home_dir() {
        v.push(home.join(".local/share/keyrings"));
        v.push(home.join(".ssh"));
        v.push(home.join(".gnupg"));
    }
    v.push(PathBuf::from("/proc"));
    v.push(PathBuf::from("/sys"));
    v
}

/// Is `path` (already canonical) equal to or inside one of `roots` (canonicalized here)?
pub fn under_any(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().filter_map(|r| std::fs::canonicalize(r).ok()).any(|r| path.starts_with(&r))
}

/// `/fs/list` refuses any directory under a secret root.
pub fn in_secret_root(path: &Path) -> bool {
    under_any(path, &secret_roots())
}

/// Open pickers, keyed by id. A picker lives for `PICKER_IDLE` after its last use.
#[derive(Default)]
pub struct Pickers {
    by_id: HashMap<String, (Purpose, Instant)>,
    order: VecDeque<String>,
}

impl Pickers {
    pub fn open_at(&mut self, purpose: Purpose, now: Instant) -> Result<String, String> {
        self.prune(now);
        let id = loop {
            let id = random_id("fp-", 16)?;
            if !self.by_id.contains_key(&id) {
                break id;
            }
        };
        self.by_id.insert(id.clone(), (purpose, now));
        self.order.push_back(id.clone());
        while self.order.len() > MAX_PICKERS {
            if let Some(old) = self.order.pop_front() {
                self.by_id.remove(&old);
            }
        }
        Ok(id)
    }

    /// The picker's purpose if it is live; using it renews its idle timer. An expired picker is
    /// removed and gives None.
    pub fn touch_at(&mut self, id: &str, now: Instant) -> Option<Purpose> {
        let live = match self.by_id.get(id) {
            Some((_, last)) => now.duration_since(*last) < PICKER_IDLE,
            None => false,
        };
        if !live {
            self.by_id.remove(id);
            return None;
        }
        let entry = self.by_id.get_mut(id)?;
        entry.1 = now;
        Some(entry.0)
    }

    fn prune(&mut self, now: Instant) {
        let by_id = &mut self.by_id;
        self.order.retain(|id| match by_id.get(id) {
            Some((_, last)) if now.duration_since(*last) < PICKER_IDLE => true,
            _ => {
                by_id.remove(id);
                false
            }
        });
    }
}

static GLOBAL: Mutex<Option<Registry>> = Mutex::new(None);
static PICKERS: Mutex<Option<Pickers>> = Mutex::new(None);

fn with_global<T>(f: impl FnOnce(&mut Registry) -> T) -> T {
    let mut g = GLOBAL.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(Registry::new))
}

fn with_pickers<T>(f: impl FnOnce(&mut Pickers) -> T) -> T {
    let mut g = PICKERS.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(Pickers::default))
}

/// `Registry::register` on the process-wide registry.
pub fn register(purpose: Purpose, raw: &str) -> Result<Registered, String> {
    with_global(|r| r.register(purpose, raw))
}

/// `Registry::resolve` on the process-wide registry.
pub fn resolve(purpose: Purpose, handle: &str) -> Option<PathBuf> {
    with_global(|r| r.resolve(purpose, handle))
}

/// `Registry::mint_for_stored` on the process-wide registry; the size is read from disk if it can be.
pub fn mint_for_stored(purpose: Purpose, path: &Path) -> Result<String, String> {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    with_global(|r| r.mint_for_stored(purpose, path, size))
}

/// `POST /files/picker`: a new picker id for the browser of `purpose`.
pub fn open_picker(purpose: Purpose) -> Result<String, String> {
    with_pickers(|p| p.open_at(purpose, Instant::now()))
}

/// Is `id` a live picker? Renews it when it is.
pub fn picker_live(id: &str) -> bool {
    with_pickers(|p| p.touch_at(id, Instant::now())).is_some()
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

/// The refusal text for a handle that does not resolve. Names only the file's display name.
fn not_picked(name: &str) -> String {
    let name = if name.trim().is_empty() { "a file" } else { name.trim() };
    format!("{} was not picked in this session; attach it again", name)
}

/// Rewrites the `attachments` array of a request body from handles to the paths the backend
/// holds, before the body is read as an `OutgoingMessage`. Rules:
///
/// * an attachment with a `handle` must resolve for `Purpose::Attach`, else it is refused;
/// * an attachment with a non-empty `path` and no handle is refused (requests carry no paths);
/// * any `source_path` the client sent is dropped. The original of a spooled copy is taken from
///   `stored` (the draft's own attachments, which the backend wrote), matched by spool path.
pub fn resolve_request(body: &mut Value, stored: &[OutgoingAttachment]) -> Result<(), String> {
    let Some(list) = body.get_mut("attachments") else { return Ok(()) };
    let Some(items) = list.as_array_mut() else { return Ok(()) };
    for item in items.iter_mut() {
        let Some(obj) = item.as_object_mut() else { return Err("unreadable attachment".into()) };
        let name = obj.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let handle = obj.get("handle").and_then(Value::as_str).unwrap_or("").to_string();
        let sent_path = obj.get("path").and_then(Value::as_str).unwrap_or("").to_string();
        let path = if !handle.is_empty() {
            match resolve(Purpose::Attach, &handle) {
                Some(p) => p.to_string_lossy().into_owned(),
                None => return Err(not_picked(&name)),
            }
        } else if !sent_path.is_empty() {
            return Err(not_picked(&name));
        } else {
            String::new()
        };
        let source = stored.iter().find(|s| !path.is_empty() && s.path == path && !s.source_path.is_empty()).map(|s| s.source_path.clone()).unwrap_or_default();
        obj.remove("handle");
        obj.insert("path".into(), Value::String(path));
        obj.insert("source_path".into(), Value::String(source));
    }
    Ok(())
}

/// The attachments as the UI may see them: each stored path replaced by a handle (minted here),
/// `path` and `source_path` removed. An attachment whose handle cannot be minted goes without one,
/// so a later save refuses it and asks for it again.
pub fn attachments_for_ui(atts: &[OutgoingAttachment]) -> Vec<Value> {
    atts.iter()
        .map(|a| {
            let mut v = serde_json::to_value(a).unwrap_or(Value::Null);
            if let Some(obj) = v.as_object_mut() {
                obj.remove("path");
                obj.remove("source_path");
                if !a.path.is_empty() {
                    match mint_for_stored(Purpose::Attach, Path::new(&a.path)) {
                        Ok(h) => {
                            obj.insert("handle".into(), Value::String(h));
                        }
                        Err(e) => log::warn!("attachment handle not minted: {}", e),
                    }
                }
            }
            v
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("omalook-files-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn att(path: &Path, source: &str) -> OutgoingAttachment {
        OutgoingAttachment { path: path.to_string_lossy().into_owned(), name: "f".into(), source_path: source.into(), ..Default::default() }
    }

    fn fresh() -> Registry {
        Registry::with_secret_roots(Vec::new())
    }

    #[test]
    fn handle_has_the_documented_shape() {
        let d = tmp("shape");
        let f = d.join("a.txt");
        fs::write(&f, b"abc").unwrap();
        let mut reg = fresh();
        let r = reg.register(Purpose::Attach, f.to_str().unwrap()).unwrap();
        assert!(r.handle.starts_with("fh-"));
        assert_eq!(r.handle.len(), 3 + 32);
        assert!(r.handle[3..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert_eq!((r.name.as_str(), r.size), ("a.txt", 3));
    }

    #[test]
    fn handle_round_trip_resolves_to_the_canonical_path() {
        let d = tmp("roundtrip");
        let f = d.join("picked.txt");
        fs::write(&f, b"p").unwrap();
        let mut reg = fresh();
        let r = reg.register(Purpose::Attach, f.to_str().unwrap()).unwrap();
        assert_eq!(reg.resolve(Purpose::Attach, &r.handle), Some(fs::canonicalize(&f).unwrap()));
    }

    #[test]
    fn wrong_purpose_is_refused() {
        let d = tmp("purpose");
        let f = d.join("contacts.vcf");
        fs::write(&f, b"x").unwrap();
        let mut reg = fresh();
        let h = reg.register(Purpose::Import, f.to_str().unwrap()).unwrap().handle;
        assert!(reg.resolve(Purpose::Import, &h).is_some());
        assert!(reg.resolve(Purpose::Attach, &h).is_none());
        assert!(reg.resolve(Purpose::Photo, &h).is_none());
    }

    #[test]
    fn unknown_and_forged_handles_are_refused() {
        let d = tmp("forged");
        let f = d.join("x");
        fs::write(&f, b"x").unwrap();
        let mut reg = fresh();
        reg.register(Purpose::Attach, f.to_str().unwrap()).unwrap();
        assert!(reg.resolve(Purpose::Attach, "fh-00000000000000000000000000000000").is_none());
        assert!(reg.resolve(Purpose::Attach, f.to_str().unwrap()).is_none(), "a path is not a handle");
        assert!(reg.resolve(Purpose::Attach, "").is_none());
    }

    #[test]
    fn same_file_same_purpose_gives_the_same_handle() {
        let d = tmp("same");
        let f = d.join("x");
        fs::write(&f, b"x").unwrap();
        let link = d.join("link");
        std::os::unix::fs::symlink(&f, &link).unwrap();
        let mut reg = fresh();
        let a = reg.register(Purpose::Attach, f.to_str().unwrap()).unwrap().handle;
        let b = reg.register(Purpose::Attach, f.to_str().unwrap()).unwrap().handle;
        let c = reg.register(Purpose::Attach, link.to_str().unwrap()).unwrap().handle;
        assert_eq!(a, b);
        assert_eq!(a, c, "a link to the same file is the same file");
        let p = reg.register(Purpose::Photo, f.to_str().unwrap()).unwrap().handle;
        assert_ne!(a, p, "a different purpose gets its own handle");
    }

    #[test]
    fn secret_roots_are_refused_and_others_are_not() {
        let d = tmp("secret");
        let secret = d.join("secret-dir");
        fs::create_dir_all(secret.join("nested")).unwrap();
        let inside = secret.join("nested").join("id_ed25519");
        let outside = d.join("notes.txt");
        fs::write(&inside, b"key").unwrap();
        fs::write(&outside, b"ok").unwrap();
        let mut reg = Registry::with_secret_roots(vec![secret.clone()]);
        assert!(reg.register(Purpose::Attach, inside.to_str().unwrap()).is_err());
        assert!(reg.register(Purpose::Attach, outside.to_str().unwrap()).is_ok());
        // A link from outside into the secret root is that file, so it is refused too.
        let sneaky = d.join("sneaky");
        std::os::unix::fs::symlink(&inside, &sneaky).unwrap();
        assert!(reg.register(Purpose::Attach, sneaky.to_str().unwrap()).is_err());
        assert!(under_any(&inside, std::slice::from_ref(&secret)));
        assert!(!under_any(&outside, std::slice::from_ref(&secret)));
    }

    #[test]
    fn the_real_secret_roots_cover_config_keys_and_proc() {
        let roots = secret_roots();
        assert!(roots.contains(&crate::google_auth::default_config_dir()));
        assert!(roots.contains(&PathBuf::from("/proc")));
        assert!(roots.contains(&PathBuf::from("/sys")));
        assert!(in_secret_root(Path::new("/proc/self")));
    }

    #[test]
    fn register_refuses_dirs_missing_relative_and_nul() {
        let d = tmp("refuse");
        let mut reg = fresh();
        assert!(reg.register(Purpose::Attach, d.to_str().unwrap()).is_err());
        assert!(reg.register(Purpose::Attach, d.join("nope").to_str().unwrap()).is_err());
        assert!(reg.register(Purpose::Attach, "relative/file").is_err());
        assert!(reg.register(Purpose::Attach, "").is_err());
        assert!(reg.register(Purpose::Attach, "/etc/passwd\0x").is_err());
    }

    #[test]
    fn links_resolve_and_a_link_out_of_the_spool_is_not_in_it() {
        let d = tmp("links");
        let secret = d.join("secret");
        fs::write(&secret, b"s").unwrap();
        // A link inside the spool pointing out of it is not a spooled copy.
        let spool = d.join("spool");
        fs::create_dir_all(spool.join("x")).unwrap();
        let out = spool.join("x").join("file");
        std::os::unix::fs::symlink(&secret, &out).unwrap();
        assert!(!in_spool(&out, std::slice::from_ref(&spool)));
        // `..` out of the spool is not in the spool.
        let dotdot = spool.join("x").join("..").join("..").join("secret");
        assert!(!in_spool(&dotdot, std::slice::from_ref(&spool)));
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
        let mut reg = fresh();
        let h = reg.register(Purpose::Attach, link.to_str().unwrap()).unwrap().handle;
        assert_eq!(reg.resolve(Purpose::Attach, &h), Some(fs::canonicalize(&real).unwrap()));
    }

    #[test]
    fn registry_is_bounded_and_drops_the_oldest() {
        let d = tmp("bound");
        let mut reg = fresh();
        let mut first = None;
        for i in 0..MAX_ENTRIES + 2 {
            let f = d.join(format!("f{}", i));
            fs::write(&f, b"x").unwrap();
            let h = reg.register(Purpose::Attach, f.to_str().unwrap()).unwrap().handle;
            if i == 0 {
                first = Some(h);
            }
        }
        assert_eq!(reg.by_handle.len(), MAX_ENTRIES);
        assert_eq!(reg.by_key.len(), MAX_ENTRIES);
        assert!(reg.resolve(Purpose::Attach, &first.unwrap()).is_none(), "oldest handle dropped");
    }

    #[test]
    fn stored_paths_mint_handles_that_resolve_even_when_the_file_is_gone() {
        let d = tmp("stored");
        let gone = d.join("spool").join("dr").join("0").join("file");
        let mut reg = fresh();
        let h = reg.mint_for_stored(Purpose::Attach, &gone, 7).unwrap();
        assert_eq!(reg.resolve(Purpose::Attach, &h), Some(gone.clone()));
        assert_eq!(reg.mint_for_stored(Purpose::Attach, &gone, 7).unwrap(), h, "same path, same handle");
    }

    #[test]
    fn pickers_expire_thirty_minutes_after_last_use_and_are_capped() {
        let t0 = Instant::now();
        let mut p = Pickers::default();
        let id = p.open_at(Purpose::Attach, t0).unwrap();
        assert!(id.starts_with("fp-") && id.len() == 35);
        // Used at +20 min: live, and the idle timer restarts from there.
        assert_eq!(p.touch_at(&id, t0 + Duration::from_secs(20 * 60)), Some(Purpose::Attach));
        assert_eq!(p.touch_at(&id, t0 + Duration::from_secs(45 * 60)), Some(Purpose::Attach));
        // Not used for 30 minutes: expired and gone.
        assert_eq!(p.touch_at(&id, t0 + Duration::from_secs(45 * 60) + PICKER_IDLE), None);
        assert_eq!(p.touch_at(&id, t0 + Duration::from_secs(45 * 60)), None);
        assert_eq!(p.touch_at("fp-nope", t0), None);
        // Cap: the oldest of MAX_PICKERS+1 is dropped.
        let mut ids = Vec::new();
        for _ in 0..MAX_PICKERS + 1 {
            ids.push(p.open_at(Purpose::Import, t0).unwrap());
        }
        assert_eq!(p.touch_at(&ids[0], t0), None);
        assert_eq!(p.touch_at(ids.last().unwrap(), t0), Some(Purpose::Import));
    }

    #[test]
    fn resolve_request_takes_handles_and_refuses_paths() {
        let d = tmp("wire");
        let f = d.join("doc.pdf");
        fs::write(&f, b"pdf").unwrap();
        let reg_handle = mint_for_stored(Purpose::Attach, &f).unwrap();
        let mut body = serde_json::json!({ "attachments": [ { "handle": reg_handle, "name": "doc.pdf", "source_path": "/etc/shadow" } ] });
        resolve_request(&mut body, &[]).unwrap();
        assert_eq!(body["attachments"][0]["path"], f.to_string_lossy().as_ref());
        assert!(body["attachments"][0].get("handle").is_none());
        assert_eq!(body["attachments"][0]["source_path"], "", "a client source_path is dropped");
        // A path with no handle is refused, as is a forged handle.
        let mut body = serde_json::json!({ "attachments": [ { "path": f.to_string_lossy(), "name": "doc.pdf" } ] });
        assert!(resolve_request(&mut body, &[]).unwrap_err().contains("doc.pdf was not picked"));
        let mut body = serde_json::json!({ "attachments": [ { "handle": "fh-forged", "name": "x" } ] });
        assert!(resolve_request(&mut body, &[]).is_err());
        // A handle for another purpose is refused.
        let import = with_global(|r| r.register(Purpose::Import, f.to_str().unwrap())).unwrap().handle;
        let mut body = serde_json::json!({ "attachments": [ { "handle": import, "name": "x" } ] });
        assert!(resolve_request(&mut body, &[]).is_err());
    }

    #[test]
    fn a_stored_draft_comes_back_with_handles_that_resolve() {
        let d = tmp("draftback");
        let copy = d.join("spool").join("dr").join("0").join("report.txt");
        fs::create_dir_all(copy.parent().unwrap()).unwrap();
        fs::write(&copy, b"hello").unwrap();
        let original = d.join("report.txt");
        let stored = vec![OutgoingAttachment { path: copy.to_string_lossy().into_owned(), name: "report.txt".into(), size: 5, source_path: original.to_string_lossy().into_owned(), ..Default::default() }];
        let ui = attachments_for_ui(&stored);
        assert!(ui[0].get("path").is_none() && ui[0].get("source_path").is_none(), "no path goes back to the UI");
        let handle = ui[0]["handle"].as_str().unwrap().to_string();
        // The UI sends it back: it resolves to the spool copy and the original is restored from the draft.
        let mut body = serde_json::json!({ "attachments": [ { "handle": handle, "name": "report.txt", "size": 5 } ] });
        resolve_request(&mut body, &stored).unwrap();
        assert_eq!(body["attachments"][0]["path"], copy.to_string_lossy().as_ref());
        assert_eq!(body["attachments"][0]["source_path"], original.to_string_lossy().as_ref());
    }

    #[test]
    fn spool_check_is_unchanged_for_stored_payloads() {
        let d = tmp("stored-payload");
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
}
