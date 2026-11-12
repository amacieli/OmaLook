//! Cache of rendered HTML mail (OL-028): the PNG and link boxes that `omalook-render` makes for one message at one
//! width and scale. A separate store from the mail cache and from the system-font view, so switching a sender back to
//! the system font loses nothing. Files live in `~/.cache/omalook/render/` as `<key>.png` + `<key>.json`.
//!
//! Size: the whole store is capped (default 512 MB, user decision 2026-11-12; `[render] cache_mb` changes it). When a new render does not fit,
//! entries of the OLDEST messages (by received date) go first; a render of a message older than everything that would
//! have to go is simply not kept, so old mail never pushes out newer mail.
//!
//! The key is the message id, a hash of the body, the width, the scale, whether remote images are on, and the helper
//! version, so a changed body, window width or helper never serves a stale picture.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The default size of the whole cache (`[render] cache_mb` in settings.toml changes it).
pub const CAP_BYTES: u64 = 512 * 1024 * 1024;

/// A cache over the default folder, sized by the setting as it is right now.
pub fn open_configured(config_dir: &Path) -> RenderCache {
    let mb = crate::settings::read_render_cache_mb(&config_dir.join("settings.toml"));
    RenderCache::new(RenderCache::default_dir(), mb * 1024 * 1024)
}
/// Bump when `omalook-render` changes what it draws.
pub const HELPER_VERSION: &str = "1";
const RENDER_TIMEOUT: Duration = Duration::from_secs(25);

fn fnv(data: &[u8], mut h: u64) -> u64 {
    for b in data { h ^= *b as u64; h = h.wrapping_mul(0x100000001b3); }
    h
}

/// Two FNV-1a runs with different seeds: 128 bits, plenty for a cache key (not a security boundary).
fn digest(parts: &[&[u8]]) -> String {
    let (mut a, mut b) = (0xcbf29ce484222325u64, 0x84222325cbf29ce4u64);
    for p in parts {
        a = fnv(p, a); a = fnv(&[0xff], a);
        b = fnv(p, b); b = fnv(&[0xfe], b);
    }
    format!("{:016x}{:016x}", a, b)
}

pub fn key(message_id: &str, body: &str, width: u32, scale: f64, images: bool) -> String {
    let body_hash = digest(&[body.as_bytes()]);
    digest(&[message_id.as_bytes(), body_hash.as_bytes(), width.to_string().as_bytes(),
             format!("{:.2}", scale).as_bytes(), &[images as u8], HELPER_VERSION.as_bytes()])
}

#[derive(Debug, Clone)]
struct Entry { key: String, received_at: String, bytes: u64 }

pub struct RenderCache { dir: PathBuf, cap: u64 }

impl RenderCache {
    pub fn new(dir: PathBuf, cap: u64) -> Self { RenderCache { dir, cap } }

    /// `~/.cache/omalook/render`.
    pub fn default_dir() -> PathBuf {
        dirs::cache_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join("omalook").join("render")
    }

    fn png(&self, key: &str) -> PathBuf { self.dir.join(format!("{key}.png")) }
    fn meta(&self, key: &str) -> PathBuf { self.dir.join(format!("{key}.json")) }

    /// The PNG and the stored JSON (helper fields plus `received_at`) when both files are there.
    pub fn lookup(&self, key: &str) -> Option<(PathBuf, String)> {
        let png = self.png(key);
        let meta = std::fs::read_to_string(self.meta(key)).ok()?;
        if !png.is_file() { return None; }
        Some((png, meta))
    }

    fn entries(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return out };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") { continue; }
            let Some(key) = p.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
            let meta_len = e.metadata().map(|m| m.len()).unwrap_or(0);
            let png_len = std::fs::metadata(self.png(&key)).map(|m| m.len()).unwrap_or(0);
            let received_at = std::fs::read_to_string(&p).ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .and_then(|v| v["received_at"].as_str().map(str::to_string))
                .unwrap_or_default();       // unknown date sorts first: goes first
            out.push(Entry { key, received_at, bytes: meta_len + png_len });
        }
        out
    }

    fn remove(&self, key: &str) {
        let _ = std::fs::remove_file(self.png(key));
        let _ = std::fs::remove_file(self.meta(key));
    }

    /// Keep a finished render. `helper_json` is the helper's stdout line. Returns false when it was not kept because
    /// it is older than everything that would have had to go to make room.
    pub fn store(&self, key: &str, message_id: &str, received_at: &str, png: &Path, helper_json: &str) -> std::io::Result<bool> {
        std::fs::create_dir_all(&self.dir)?;
        let mut meta: serde_json::Value = serde_json::from_str(helper_json.trim())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        meta["received_at"] = received_at.into();
        meta["id"] = message_id.into();
        // write beside, then rename: a reader never sees half a file
        let tmp_png = self.dir.join(format!("{key}.png.tmp"));
        std::fs::copy(png, &tmp_png)?;
        let meta_text = serde_json::to_string(&meta).unwrap_or_default();
        let tmp_meta = self.dir.join(format!("{key}.json.tmp"));
        std::fs::File::create(&tmp_meta)?.write_all(meta_text.as_bytes())?;
        std::fs::rename(&tmp_png, self.png(key))?;
        std::fs::rename(&tmp_meta, self.meta(key))?;
        Ok(self.evict(key))
    }

    /// Oldest messages first until the store fits. False when `keep` itself had to go.
    fn evict(&self, keep: &str) -> bool {
        let mut all = self.entries();
        let mut total: u64 = all.iter().map(|e| e.bytes).sum();
        if total <= self.cap { return true; }
        all.sort_by(|a, b| a.received_at.cmp(&b.received_at).then_with(|| a.key.cmp(&b.key)));
        for e in all {
            if total <= self.cap { break; }
            self.remove(&e.key);
            total = total.saturating_sub(e.bytes);
            if e.key == keep { return false; }
        }
        true
    }

    /// Bring the store down to its cap now (after the cap was lowered): oldest messages first.
    pub fn trim(&self) -> u64 { self.evict(""); self.total_bytes() }

    pub fn total_bytes(&self) -> u64 { self.entries().iter().map(|e| e.bytes).sum() }
}

/// Run the helper on `body`, writing the PNG to `out`. Returns the helper's JSON line. Killed after 25 s.
pub fn run_helper(helper: &Path, body: &str, width: u32, scale: f64, images: bool, out: &Path) -> Result<String, String> {
    let mut cmd = std::process::Command::new(helper);
    cmd.arg("--width").arg(width.to_string()).arg("--scale").arg(format!("{scale}")).arg("--out").arg(out);
    if images { cmd.arg("--images"); }
    cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().map_err(|e| format!("cannot start the render helper: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let payload = body.as_bytes().to_vec();
    let writer = std::thread::spawn(move || { let _ = stdin.write_all(&payload); });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if started.elapsed() > RENDER_TIMEOUT => { let _ = child.kill(); let _ = child.wait(); return Err("render timed out".into()); }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let _ = writer.join();
    if !status.success() { return Err(format!("render helper exited with {status}")); }
    let mut text = String::new();
    if let Some(mut so) = child.stdout.take() { use std::io::Read; let _ = so.read_to_string(&mut text); }
    if text.trim().is_empty() { return Err("render helper printed nothing".into()); }
    Ok(text)
}

/// The width and scale the reading pane last used: what the background pre-render draws at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hint { pub width: u32, pub scale: f64 }

impl RenderCache {
    pub fn write_hint(&self, h: Hint) {
        if self.read_hint() == Some(h) { return; }
        let _ = std::fs::create_dir_all(&self.dir);
        let _ = std::fs::write(self.dir.join("hint"), format!("{} {}", h.width, h.scale));
    }
    pub fn read_hint(&self) -> Option<Hint> {
        let t = std::fs::read_to_string(self.dir.join("hint")).ok()?;
        let mut it = t.split_whitespace();
        Some(Hint { width: it.next()?.parse().ok()?, scale: it.next()?.parse().ok()? })
    }
}

/// Mail from senders set to "always HTML" is drawn ahead of time (newest first, a few per round) so it is ready
/// when opened. Needs the reading pane to have been used once (for the width) and the helper to exist.
pub fn prerender_round(db_path: &str, cache: &RenderCache, helper: &Path, max: usize) -> usize {
    let Some(h) = cache.read_hint() else { return 0 };
    if !helper.is_file() { return 0; }
    let Ok(conn) = rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) else { return 0 };
    let _ = conn.busy_timeout(Duration::from_secs(5));
    let Ok(mut stmt) = conn.prepare(
        "SELECT m.id, m.body_full, m.received_at, s.always_images FROM messages m
         JOIN sender_prefs s ON lower(s.email) = lower(m.from_email)
         WHERE s.always_html = 1 AND m.body_type = 'html' AND m.body_full IS NOT NULL
           AND m.received_at >= date('now', '-60 days')
         ORDER BY m.received_at DESC LIMIT 300") else { return 0 };
    let rows: Vec<(String, String, String, bool)> = match stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default(), r.get::<_, i64>(3)? != 0))) {
        Ok(it) => it.flatten().collect(),
        Err(_) => return 0,
    };
    let mut done = 0;
    for (id, body, received, images) in rows {
        if done >= max { break; }
        let k = key(&id, &body, h.width, h.scale, images);
        if cache.lookup(&k).is_some() { continue; }
        let tmp = std::env::temp_dir().join(format!("omalook-prerender-{}.png", std::process::id()));
        match run_helper(helper, &body, h.width, h.scale, images, &tmp) {
            Ok(json) => { let _ = cache.store(&k, &id, &received, &tmp, &json); done += 1; }
            Err(_) => { done += 1; }     // a message the helper refuses is not retried within this round
        }
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(format!("{}.json", tmp.display()));
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("omalook-rc-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn png(dir: &Path, name: &str, bytes: usize) -> PathBuf { let p = dir.join(name); std::fs::write(&p, vec![7u8; bytes]).unwrap(); p }
    const J: &str = r##"{"width":900,"height":100,"scale":1,"bg":"#ffffff","links":[]}"##;

    #[test]
    fn key_changes_with_every_input() {
        let k = key("m1", "<p>a</p>", 900, 2.0, false);
        assert_eq!(k, key("m1", "<p>a</p>", 900, 2.0, false));
        for other in [key("m2", "<p>a</p>", 900, 2.0, false), key("m1", "<p>b</p>", 900, 2.0, false), key("m1", "<p>a</p>", 901, 2.0, false),
                      key("m1", "<p>a</p>", 900, 1.0, false), key("m1", "<p>a</p>", 900, 2.0, true)] { assert_ne!(k, other); }
    }

    #[test]
    fn stored_render_is_found_with_its_date() {
        let d = tmp("hit");
        let c = RenderCache::new(d.join("c"), 1 << 20);
        let p = png(&d, "x.png", 100);
        assert!(c.lookup("k1").is_none());
        assert!(c.store("k1", "m1", "2026-11-01T10:00:00Z", &p, J).unwrap());
        let (f, meta) = c.lookup("k1").unwrap();
        assert!(f.is_file());
        let v: serde_json::Value = serde_json::from_str(&meta).unwrap();
        assert_eq!((v["received_at"].as_str(), v["width"].as_i64(), v["id"].as_str()), (Some("2026-11-01T10:00:00Z"), Some(900), Some("m1")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_new_render_pushes_out_the_oldest_messages_first() {
        let d = tmp("evict");
        let c = RenderCache::new(d.join("c"), 1200);
        let p = png(&d, "x.png", 400);
        // three entries of about 400 + meta bytes: only two fit in 1200
        assert!(c.store("a", "ma", "2026-01-01T00:00:00Z", &p, J).unwrap());
        assert!(c.store("b", "mb", "2026-03-01T00:00:00Z", &p, J).unwrap());
        assert!(c.store("c", "mc", "2026-06-01T00:00:00Z", &p, J).unwrap());
        assert!(c.lookup("a").is_none(), "the oldest message went");
        assert!(c.lookup("b").is_some() && c.lookup("c").is_some());
        assert!(c.total_bytes() <= 1200);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_render_of_an_older_message_does_not_push_out_newer_ones() {
        let d = tmp("older");
        let c = RenderCache::new(d.join("c"), 1200);
        let p = png(&d, "x.png", 400);
        assert!(c.store("b", "mb", "2026-03-01T00:00:00Z", &p, J).unwrap());
        assert!(c.store("c", "mc", "2026-06-01T00:00:00Z", &p, J).unwrap());
        let kept = c.store("old", "mo", "2025-01-01T00:00:00Z", &p, J).unwrap();
        assert!(!kept, "older than everything: not kept");
        assert!(c.lookup("old").is_none());
        assert!(c.lookup("b").is_some() && c.lookup("c").is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn hint_round_trips() {
        let d = tmp("hint");
        let c = RenderCache::new(d.join("c"), 10);
        assert!(c.read_hint().is_none());
        c.write_hint(Hint { width: 812, scale: 1.5 });
        assert_eq!(c.read_hint(), Some(Hint { width: 812, scale: 1.5 }));
        let _ = std::fs::remove_dir_all(&d);
    }
}
