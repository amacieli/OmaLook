//! MCP settings: presets for the `[mcp]` block. The types and the decision rules live in
//! `crate::app::policy` and are re-exported here so `mcp::policy::X` paths keep working.
//!
//! Everything fails closed: an unknown or malformed value means `off`, a settings file that
//! does not parse means MCP is disabled, and nothing is ever "allowed by default" that the
//! class table below does not say is allowed.
//!
//! Layers (see notes/DESIGN-mcp-server.md §4):
//!   1. `[mcp] enabled` — and only while `[sync] run_in_background` is on (MCP lives in the daemon).
//!   2. `[mcp.domains]` level per domain: off | read | write.
//!   3. `[mcp.tools]` mode per write tool: off | ask | allow. Absent = the tool's class default.

pub use crate::app::policy::*;
use std::collections::BTreeMap;

/// The one-click permission sets on the Settings → MCP page. They set domain levels and clear
/// per-tool overrides; the master switch and the account list are left alone.
pub const PRESETS: [&str; 3] = ["locked", "read_only", "approval"];

/// `current` with a preset applied. `None` for an unknown name.
pub fn with_preset(current: &McpSettings, name: &str) -> Option<McpSettings> {
    let level = match name { "locked" => "off", "read_only" => "read", "approval" => "write", _ => return None };
    let mut s = current.clone();
    s.tools.clear();
    s.domains = Domain::ALL
        .iter()
        // Accounts has no write tools: it tops out at read.
        .map(|d| (d.key().to_string(), if *d == Domain::Accounts && level == "write" { "read" } else { level }.to_string()))
        .collect();
    Some(s)
}

/// Which preset the domain levels and tool overrides add up to, or "custom".
pub fn preset_of(s: &McpSettings) -> &'static str {
    PRESETS.iter().copied().find(|p| with_preset(s, p).is_some_and(|x| x.domains == normalized_domains(s) && s.tools.is_empty())).unwrap_or("custom")
}

/// Domain levels with every domain present (missing = off), as text.
fn normalized_domains(s: &McpSettings) -> BTreeMap<String, String> {
    Domain::ALL
        .iter()
        .map(|d| {
            let lv = s.domains.get(d.key()).map(|v| Level::parse(v)).unwrap_or(Level::Off);
            let lv = if *d == Domain::Accounts { lv.min(Level::Read) } else { lv };
            (d.key().to_string(), lv.key().to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_set_levels_and_clear_overrides_but_keep_switch_and_accounts() {
        let mut cur = McpSettings { enabled: true, accounts: vec!["a".into()], ..Default::default() };
        cur.tools.insert("mail_send".into(), "allow".into());
        let ro = with_preset(&cur, "read_only").unwrap();
        assert!(ro.enabled && ro.accounts == vec!["a".to_string()] && ro.tools.is_empty());
        assert!(Domain::ALL.iter().all(|d| ro.domains[d.key()] == "read"));
        let ap = with_preset(&cur, "approval").unwrap();
        assert_eq!(ap.domains["mail"], "write");
        assert_eq!(ap.domains["accounts"], "read", "accounts never exceeds read");
        let lk = with_preset(&cur, "locked").unwrap();
        assert!(Domain::ALL.iter().all(|d| lk.domains[d.key()] == "off"));
        assert!(with_preset(&cur, "yolo").is_none());
    }

    #[test]
    fn preset_detection_names_what_the_settings_add_up_to() {
        let base = McpSettings::default();
        assert_eq!(preset_of(&base), "locked", "a fresh install is the locked preset");
        for p in PRESETS { assert_eq!(preset_of(&with_preset(&base, p).unwrap()), p); }
        let mut custom = with_preset(&base, "approval").unwrap();
        custom.tools.insert("mail_send".into(), "off".into());
        assert_eq!(preset_of(&custom), "custom");
        let mut custom = with_preset(&base, "read_only").unwrap();
        custom.domains.insert("sms".into(), "off".into());
        assert_eq!(preset_of(&custom), "custom");
    }
}
