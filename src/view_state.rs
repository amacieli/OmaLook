//! Where the user last was (module, mail folder, calendar / people / task view), kept between
//! runs in `view_state.json` next to `settings.toml`. The UI owns the content (a flat JSON
//! object of short strings); this file only stores it, so a new view needs no backend change.
//! It is not part of `settings.toml`: the app rewrites it on every navigation and a hand-edited
//! settings file must not be touched by that.

use std::path::Path;

/// Largest body accepted: a handful of short strings, never more.
const MAX_BYTES: usize = 4096;

/// The stored object as JSON text; `{}` when nothing is stored or the file is damaged.
pub fn load(path: &Path) -> String {
    std::fs::read_to_string(path)
        .ok()
        .filter(|s| is_flat_object(s))
        .unwrap_or_else(|| "{}".to_string())
}

/// Store `body` if it is a flat JSON object of strings/numbers/booleans. False = refused.
pub fn save(path: &Path, body: &str) -> bool {
    if body.len() > MAX_BYTES || !is_flat_object(body) { return false; }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).and_then(|_| std::fs::rename(&tmp, path)).is_ok()
}

fn is_flat_object(text: &str) -> bool {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(serde_json::Value::Object(m)) => m.values().all(|v| v.is_string() || v.is_number() || v.is_boolean()),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_refusals() {
        let dir = std::env::temp_dir().join(format!("omalook-vs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("view_state.json");
        assert_eq!(load(&p), "{}");                                   // nothing stored yet
        assert!(save(&p, r#"{"view":"calendar","calendar_mode":"week"}"#));
        assert_eq!(load(&p), r#"{"view":"calendar","calendar_mode":"week"}"#);
        assert!(!save(&p, "[1,2]"));                                  // not an object
        assert!(!save(&p, r#"{"a":{"b":1}}"#));                       // not flat
        assert!(!save(&p, "not json"));
        assert!(!save(&p, &format!(r#"{{"a":"{}"}}"#, "x".repeat(5000))));
        assert_eq!(load(&p), r#"{"view":"calendar","calendar_mode":"week"}"#); // refusals kept the old state
        std::fs::write(&p, "garbage").unwrap();
        assert_eq!(load(&p), "{}");                                   // damaged file reads as empty
        let _ = std::fs::remove_dir_all(&dir);
    }
}
