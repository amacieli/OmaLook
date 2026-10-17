//! MCP permission model: master switch → per-domain level → per-tool mode. Most restrictive wins.
//!
//! Everything here fails closed: an unknown or malformed value means `off`, a settings file that
//! does not parse means MCP is disabled, and nothing is ever "allowed by default" that the
//! class table below does not say is allowed.
//!
//! Layers (see notes/DESIGN-mcp-server.md §4):
//!   1. `[mcp] enabled` — and only while `[sync] run_in_background` is on (MCP lives in the daemon).
//!   2. `[mcp.domains]` level per domain: off | read | write.
//!   3. `[mcp.tools]` mode per write tool: off | ask | allow. Absent = the tool's class default.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Domain { Mail, Calendar, Contacts, Sms, Accounts }

impl Domain {
    pub const ALL: [Domain; 5] = [Domain::Mail, Domain::Calendar, Domain::Contacts, Domain::Sms, Domain::Accounts];
    pub fn key(self) -> &'static str {
        match self { Domain::Mail => "mail", Domain::Calendar => "calendar", Domain::Contacts => "contacts", Domain::Sms => "sms", Domain::Accounts => "accounts" }
    }
}

/// How much of a domain MCP may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level { Off, Read, Write }

impl Level {
    /// Unknown text is `Off`.
    pub fn parse(s: &str) -> Level {
        match s.trim().to_ascii_lowercase().as_str() { "read" => Level::Read, "write" => Level::Write, _ => Level::Off }
    }
    pub fn key(self) -> &'static str { match self { Level::Off => "off", Level::Read => "read", Level::Write => "write" } }
}

/// What happens when a write tool is called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode { Off, Ask, Allow }

impl Mode {
    /// `None` for unknown text (the caller falls back to the class default, which is never looser
    /// than what the user wrote last time because an invalid value is simply ignored).
    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() { "off" => Some(Mode::Off), "ask" => Some(Mode::Ask), "allow" => Some(Mode::Allow), _ => None }
    }
    pub fn key(self) -> &'static str { match self { Mode::Off => "off", Mode::Ask => "ask", Mode::Allow => "allow" } }
}

/// Risk class of a tool; sets its default mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Read only.
    R,
    /// Reversible or local state: mark read, categories, archive (undo window), drafts.
    W1,
    /// Outbound or hard to take back: send, RSVP, write a file.
    W2,
    /// Destructive: permanent delete.
    W3,
}

impl Class {
    pub fn is_write(self) -> bool { self != Class::R }
    pub fn default_mode(self) -> Mode {
        match self { Class::R | Class::W1 => Mode::Allow, Class::W2 => Mode::Ask, Class::W3 => Mode::Off }
    }
}

/// `[mcp]` in settings.toml, as written. Values stay strings so a typo never fails the whole file;
/// `Policy::new` interprets them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct McpSettings {
    pub enabled: bool,
    pub ask_timeout_secs: u32,
    pub send_rate_per_hour: u32,
    /// any | contacts | thread — who a send may go to (enforced when the send tools exist).
    pub recipients: String,
    /// Account ids MCP may touch. Empty = none.
    pub accounts: Vec<String>,
    /// domain → off | read | write
    pub domains: BTreeMap<String, String>,
    /// tool name → off | ask | allow
    pub tools: BTreeMap<String, String>,
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            ask_timeout_secs: 60,
            send_rate_per_hour: 10,
            recipients: "any".into(),
            accounts: Vec::new(),
            domains: BTreeMap::new(),
            tools: BTreeMap::new(),
        }
    }
}

impl McpSettings {
    pub fn is_default(&self) -> bool { *self == McpSettings::default() }

    /// Clamp numbers and normalise text so what is stored is always usable.
    pub fn sanitized(mut self) -> Self {
        self.ask_timeout_secs = self.ask_timeout_secs.clamp(5, 600);
        self.send_rate_per_hour = self.send_rate_per_hour.min(1000);
        if !matches!(self.recipients.as_str(), "any" | "contacts" | "thread") { self.recipients = "any".into(); }
        self.accounts.sort();
        self.accounts.dedup();
        // Drop keys we do not know so the file does not collect junk; values are kept as typed
        // only if they parse, otherwise removed (= class/off default).
        self.domains.retain(|k, v| Domain::ALL.iter().any(|d| d.key() == k) && matches!(v.as_str(), "off" | "read" | "write"));
        self.tools.retain(|_, v| Mode::parse(v).is_some());
        self
    }
}

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
    PRESETS.iter().copied().find(|p| with_preset(s, p).map_or(false, |x| x.domains == normalized_domains(s) && s.tools.is_empty())).unwrap_or("custom")
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

/// What the policy says about one tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Not advertised and not callable (reported to the client as an unknown tool).
    Hidden,
    /// Needs the user's approval first.
    Ask,
    Allow,
}

/// The effective policy: a snapshot built from settings.toml. Cheap to rebuild per request, so
/// a change in Settings applies to the next call.
#[derive(Debug, Clone)]
pub struct Policy {
    active: bool,
    settings: McpSettings,
}

impl Policy {
    /// `run_in_background` gates everything: MCP is a feature of the background daemon, so with
    /// that setting off MCP is off whatever `[mcp]` says.
    pub fn new(settings: McpSettings, run_in_background: bool) -> Policy {
        Policy { active: settings.enabled && run_in_background, settings }
    }

    /// A policy that denies everything.
    pub fn locked() -> Policy { Policy { active: false, settings: McpSettings::default() } }

    pub fn active(&self) -> bool { self.active }
    pub fn settings(&self) -> &McpSettings { &self.settings }

    pub fn level(&self, domain: Domain) -> Level {
        if !self.active { return Level::Off; }
        let lv = self.settings.domains.get(domain.key()).map(|s| Level::parse(s)).unwrap_or(Level::Off);
        // Accounts has no write tools; a stray "write" must not mean more than read.
        if domain == Domain::Accounts { lv.min(Level::Read) } else { lv }
    }

    /// Mode of a tool, ignoring the domain level.
    fn mode_of(&self, name: &str, class: Class) -> Mode {
        if !class.is_write() { return Mode::Allow; }
        self.settings.tools.get(name).and_then(|s| Mode::parse(s)).unwrap_or_else(|| class.default_mode())
    }

    pub fn decide(&self, name: &str, domain: Domain, class: Class) -> Decision {
        let level = self.level(domain);
        if level == Level::Off { return Decision::Hidden; }
        if class.is_write() && level < Level::Write { return Decision::Hidden; }
        match self.mode_of(name, class) {
            Mode::Off => Decision::Hidden,
            Mode::Ask => Decision::Ask,
            Mode::Allow => Decision::Allow,
        }
    }

    /// Whether MCP may touch this account. Empty list = none.
    pub fn account_allowed(&self, account_id: &str) -> bool {
        self.active && self.settings.accounts.iter().any(|a| a == account_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pol(domains: &[(&str, &str)], tools: &[(&str, &str)], enabled: bool, bg: bool) -> Policy {
        let mut s = McpSettings { enabled, ..Default::default() };
        for (k, v) in domains { s.domains.insert(k.to_string(), v.to_string()); }
        for (k, v) in tools { s.tools.insert(k.to_string(), v.to_string()); }
        Policy::new(s, bg)
    }

    #[test]
    fn default_settings_deny_everything() {
        let p = Policy::new(McpSettings::default(), true);
        assert!(!p.active());
        for d in Domain::ALL {
            assert_eq!(p.decide("x", d, Class::R), Decision::Hidden);
            assert_eq!(p.decide("x", d, Class::W1), Decision::Hidden);
        }
    }

    #[test]
    fn master_switch_off_beats_domain_levels() {
        let p = pol(&[("mail", "write")], &[], false, true);
        assert_eq!(p.decide("mail_list_messages", Domain::Mail, Class::R), Decision::Hidden);
    }

    #[test]
    fn mcp_needs_keep_running_in_background() {
        let p = pol(&[("mail", "read")], &[], true, false);
        assert!(!p.active());
        assert_eq!(p.decide("mail_list_messages", Domain::Mail, Class::R), Decision::Hidden);
        let p = pol(&[("mail", "read")], &[], true, true);
        assert_eq!(p.decide("mail_list_messages", Domain::Mail, Class::R), Decision::Allow);
    }

    #[test]
    fn read_level_hides_write_tools() {
        let p = pol(&[("mail", "read")], &[("mail_send", "allow")], true, true);
        assert_eq!(p.decide("mail_list_messages", Domain::Mail, Class::R), Decision::Allow);
        assert_eq!(p.decide("mail_mark_read", Domain::Mail, Class::W1), Decision::Hidden);
        assert_eq!(p.decide("mail_send", Domain::Mail, Class::W2), Decision::Hidden, "a tool override cannot exceed the domain level");
    }

    #[test]
    fn write_level_uses_class_defaults() {
        let p = pol(&[("mail", "write")], &[], true, true);
        assert_eq!(p.decide("mail_mark_read", Domain::Mail, Class::W1), Decision::Allow);
        assert_eq!(p.decide("mail_send", Domain::Mail, Class::W2), Decision::Ask);
        assert_eq!(p.decide("mail_delete_permanent", Domain::Mail, Class::W3), Decision::Hidden);
    }

    #[test]
    fn tool_overrides_apply_in_both_directions() {
        let p = pol(&[("mail", "write")], &[("mail_send", "allow"), ("mail_mark_read", "off"), ("mail_delete_permanent", "ask")], true, true);
        assert_eq!(p.decide("mail_send", Domain::Mail, Class::W2), Decision::Allow);
        assert_eq!(p.decide("mail_mark_read", Domain::Mail, Class::W1), Decision::Hidden);
        assert_eq!(p.decide("mail_delete_permanent", Domain::Mail, Class::W3), Decision::Ask);
    }

    #[test]
    fn overrides_never_change_read_tools() {
        let p = pol(&[("mail", "read")], &[("mail_list_messages", "off")], true, true);
        assert_eq!(p.decide("mail_list_messages", Domain::Mail, Class::R), Decision::Allow);
    }

    #[test]
    fn garbage_values_fail_closed() {
        let p = pol(&[("mail", "WRITE!!"), ("sms", "")], &[("mail_send", "yes")], true, true);
        assert_eq!(p.level(Domain::Mail), Level::Off);
        assert_eq!(p.level(Domain::Sms), Level::Off);
        // A bad tool value is ignored → class default (ask for send), never "allow".
        let p = pol(&[("mail", "write")], &[("mail_send", "yes")], true, true);
        assert_eq!(p.decide("mail_send", Domain::Mail, Class::W2), Decision::Ask);
    }

    #[test]
    fn accounts_domain_cannot_write() {
        let p = pol(&[("accounts", "write")], &[], true, true);
        assert_eq!(p.level(Domain::Accounts), Level::Read);
        assert_eq!(p.decide("accounts_list", Domain::Accounts, Class::R), Decision::Allow);
        assert_eq!(p.decide("accounts_remove", Domain::Accounts, Class::W3), Decision::Hidden);
    }

    #[test]
    fn account_scope_defaults_to_none() {
        let mut s = McpSettings { enabled: true, ..Default::default() };
        assert!(!Policy::new(s.clone(), true).account_allowed("graph-1"));
        s.accounts = vec!["graph-1".into()];
        let p = Policy::new(s.clone(), true);
        assert!(p.account_allowed("graph-1"));
        assert!(!p.account_allowed("gmail-2"));
        assert!(!Policy::new(s, false).account_allowed("graph-1"), "inactive policy allows no account");
    }

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

    #[test]
    fn sanitize_drops_junk_and_clamps() {
        let mut s = McpSettings::default();
        s.ask_timeout_secs = 0;
        s.recipients = "everyone".into();
        s.domains.insert("mail".into(), "read".into());
        s.domains.insert("nonsense".into(), "write".into());
        s.domains.insert("sms".into(), "banana".into());
        s.tools.insert("mail_send".into(), "ask".into());
        s.tools.insert("mail_x".into(), "maybe".into());
        s.accounts = vec!["b".into(), "a".into(), "a".into()];
        let s = s.sanitized();
        assert_eq!(s.ask_timeout_secs, 5);
        assert_eq!(s.recipients, "any");
        assert_eq!(s.domains.keys().collect::<Vec<_>>(), vec!["mail"]);
        assert_eq!(s.tools.keys().collect::<Vec<_>>(), vec!["mail_send"]);
        assert_eq!(s.accounts, vec!["a", "b"]);
    }
}
