//! Online-meeting providers for calendar events (docs/PLAN-calendar-events.md section 7, OL-021 track B / OL-010).
//!
//! A provider trait and a registry. Pure: no network, nothing here calls a vendor.
//!
//! | provider | id sent by the editor | kind | what lands in the event |
//! |---|---|---|---|
//! | Microsoft Teams | `teamsForBusiness` | native (Graph) | `isOnlineMeeting` + `onlineMeetingProvider`; Graph makes the link, read back from `onlineMeeting.joinUrl` |
//! | Google Meet | `googleMeet` | native (Google) | `conferenceData.createRequest` (`google_meet.rs`); link read back from `hangoutLink` |
//! | Personal link | `personalLink` | static | the configured URL in `location` and in the body |
//! | Jitsi | `jitsi` | generated | `https://<server>/<random slug>` in `location` and in the body |
//! | Zoom, Webex | `zoom`, `webex` | external (stub) | nothing: "needs setup" (no OAuth app registered yet, decision 11) |
//!
//! The route calls [`prepare_request`] on the editor's JSON *before* it is queued, so the link a dry run shows is the link
//! that is saved (a Jitsi slug is chosen once, never again on a retry). Native providers leave `online_provider` in the
//! request for the Graph / Google body builders; every other kind is turned into text and `online_provider` is dropped.
//!
//! `online_provider` on a request: absent or empty = leave the event's meeting as it is (an edit keeps what is there);
//! `"none"` = no meeting (an edit removes a link block this module wrote); a provider id = that provider.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::path::Path;

use crate::errors::{OmarchyError, Result};
use crate::google_meet;

pub const TEAMS: &str = "teamsForBusiness";
pub const GOOGLE_MEET: &str = google_meet::PROVIDER;
pub const PERSONAL: &str = "personalLink";
pub const JITSI: &str = "jitsi";
pub const ZOOM: &str = "zoom";
pub const WEBEX: &str = "webex";
/// The request value for "no online meeting".
pub const NONE: &str = "none";
pub const DEFAULT_JITSI_SERVER: &str = "meet.jit.si";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Made by the calendar service itself.
    Native,
    /// A fixed URL stored once.
    Static,
    /// A URL that needs no account.
    Generated,
    /// The vendor's API makes it first (needs a meeting account).
    External,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Ok,
    /// Usable once the user sets something up; the text says what.
    NeedsSetup(String),
    /// Cannot be used for this calendar account (or is switched off).
    NotAvailable(String),
}

impl Availability {
    pub fn is_ok(&self) -> bool {
        matches!(self, Availability::Ok)
    }
    pub fn reason(&self) -> &str {
        match self {
            Availability::Ok => "",
            Availability::NeedsSetup(r) | Availability::NotAvailable(r) => r,
        }
    }
}

/// `[meetings]` in settings.toml.
#[derive(Debug, Clone, PartialEq)]
pub struct MeetingConfig {
    pub teams_enabled: bool,
    pub meet_enabled: bool,
    pub jitsi_enabled: bool,
    pub personal_enabled: bool,
    /// Label shown in the picker and the body block for the personal link ("My Zoom room").
    pub personal_label: String,
    /// An `https://` URL, or empty when none is set.
    pub personal_link: String,
    /// Host (and optional port) of the Jitsi server; no scheme.
    pub jitsi_server: String,
    /// Calendar account id -> provider id (or `"none"`) preselected in the editor.
    pub default_provider: BTreeMap<String, String>,
}

impl Default for MeetingConfig {
    fn default() -> Self {
        MeetingConfig {
            teams_enabled: true,
            meet_enabled: true,
            jitsi_enabled: true,
            personal_enabled: true,
            personal_label: String::new(),
            personal_link: String::new(),
            jitsi_server: DEFAULT_JITSI_SERVER.to_string(),
            default_provider: BTreeMap::new(),
        }
    }
}

impl MeetingConfig {
    pub fn personal_label_or_default(&self) -> &str {
        let l = self.personal_label.trim();
        if l.is_empty() { "Personal meeting link" } else { l }
    }
}

/// What a provider is asked about: the calendar account's kind (`exchange` | `gmail` | ..., the id's prefix) and the settings.
pub struct Context<'a> {
    pub account_provider: &'a str,
    pub config: &'a MeetingConfig,
}

/// A link a non-native provider produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingLink {
    pub label: String,
    pub url: String,
}

pub trait OnlineProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn kind(&self) -> Kind;
    fn available(&self, cx: &Context) -> Availability;
    /// The link for a Static / Generated provider; `None` for a native one (the service makes it).
    /// `seed` is entropy for a generated URL (see [`random_slug`]).
    fn link(&self, _cx: &Context, _seed: &str) -> Option<MeetingLink> {
        None
    }
    /// The picker row's label (the personal link shows the user's own name for it).
    fn display_label(&self, _cx: &Context) -> String {
        self.label().to_string()
    }
}

struct Teams;
struct Meet;
struct Personal;
struct Jitsi;
struct Stub {
    id: &'static str,
    label: &'static str,
}

impl OnlineProvider for Teams {
    fn id(&self) -> &'static str { TEAMS }
    fn label(&self) -> &'static str { "Microsoft Teams" }
    fn kind(&self) -> Kind { Kind::Native }
    fn available(&self, cx: &Context) -> Availability {
        if cx.account_provider != "exchange" {
            return Availability::NotAvailable("needs a work or school Microsoft account".into());
        }
        if !cx.config.teams_enabled {
            return Availability::NotAvailable("switched off in Settings".into());
        }
        Availability::Ok
    }
}

impl OnlineProvider for Meet {
    fn id(&self) -> &'static str { GOOGLE_MEET }
    fn label(&self) -> &'static str { "Google Meet" }
    fn kind(&self) -> Kind { Kind::Native }
    fn available(&self, cx: &Context) -> Availability {
        if cx.account_provider != "gmail" {
            return Availability::NotAvailable("needs a Google account".into());
        }
        if !cx.config.meet_enabled {
            return Availability::NotAvailable("switched off in Settings".into());
        }
        Availability::Ok
    }
}

impl OnlineProvider for Personal {
    fn id(&self) -> &'static str { PERSONAL }
    fn label(&self) -> &'static str { "Personal meeting link" }
    fn kind(&self) -> Kind { Kind::Static }
    fn available(&self, cx: &Context) -> Availability {
        if !cx.config.personal_enabled {
            return Availability::NotAvailable("switched off in Settings".into());
        }
        if !is_https_url(&cx.config.personal_link) {
            return Availability::NeedsSetup("set your link in Settings -> Accounts".into());
        }
        Availability::Ok
    }
    fn link(&self, cx: &Context, _seed: &str) -> Option<MeetingLink> {
        Some(MeetingLink { label: self.display_label(cx), url: cx.config.personal_link.trim().to_string() })
    }
    fn display_label(&self, cx: &Context) -> String {
        cx.config.personal_label_or_default().to_string()
    }
}

impl OnlineProvider for Jitsi {
    fn id(&self) -> &'static str { JITSI }
    fn label(&self) -> &'static str { "Jitsi" }
    fn kind(&self) -> Kind { Kind::Generated }
    fn available(&self, cx: &Context) -> Availability {
        if !cx.config.jitsi_enabled {
            return Availability::NotAvailable("switched off in Settings".into());
        }
        if clean_server(&cx.config.jitsi_server).is_none() {
            return Availability::NeedsSetup("the Jitsi server in Settings is not a host name".into());
        }
        Availability::Ok
    }
    fn link(&self, cx: &Context, seed: &str) -> Option<MeetingLink> {
        let server = clean_server(&cx.config.jitsi_server)?;
        Some(MeetingLink { label: "Jitsi Meeting".into(), url: format!("https://{}/{}", server, random_slug(seed)) })
    }
}

impl OnlineProvider for Stub {
    fn id(&self) -> &'static str { self.id }
    fn label(&self) -> &'static str { self.label }
    fn kind(&self) -> Kind { Kind::External }
    fn available(&self, _cx: &Context) -> Availability {
        Availability::NeedsSetup(format!("{} sign-in is not built yet", self.label))
    }
}

/// Every provider, in picker order. Zoom and Webex are stubs: no OAuth app exists, so they answer "needs setup"
/// and make no network call and keep nothing in the keyring (decision 11).
pub fn registry() -> Vec<Box<dyn OnlineProvider>> {
    vec![
        Box::new(Teams),
        Box::new(Meet),
        Box::new(Personal),
        Box::new(Jitsi),
        Box::new(Stub { id: ZOOM, label: "Zoom" }),
        Box::new(Stub { id: WEBEX, label: "Webex" }),
    ]
}

pub fn find(id: &str) -> Option<Box<dyn OnlineProvider>> {
    registry().into_iter().find(|p| p.id() == id)
}

/// The picker rows for one calendar account: `[{id, label, kind, available, reason, default}]`, "None" first. At most one
/// row has `default: true` (the configured default for this account, when it is still usable; else None).
pub fn options(account_id: &str, config: &MeetingConfig) -> Value {
    let cx = Context { account_provider: crate::accounts::provider_of(account_id), config };
    let wanted = config.default_provider.get(account_id).map(String::as_str).unwrap_or(NONE);
    let mut rows = vec![json!({ "id": NONE, "label": "None", "kind": "none", "available": true, "reason": "", "default": false })];
    for p in registry() {
        let a = p.available(&cx);
        rows.push(json!({
            "id": p.id(),
            "label": p.display_label(&cx),
            "kind": match p.kind() { Kind::Native => "native", Kind::Static => "static", Kind::Generated => "generated", Kind::External => "external" },
            "available": a.is_ok(),
            "reason": a.reason(),
            "default": wanted == p.id() && a.is_ok(),
        }));
    }
    let any = rows.iter().any(|r| r["default"] == true);
    rows[0]["default"] = json!(!any);
    Value::Array(rows)
}

#[derive(Debug, Clone, PartialEq)]
pub enum MeetingError {
    Unknown(String),
    NeedsSetup { label: String, reason: String },
    NotAvailable { label: String, reason: String },
}

impl std::fmt::Display for MeetingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown(p) => write!(f, "unknown online meeting provider: {p}"),
            Self::NeedsSetup { label, reason } => write!(f, "{label} needs setup: {reason}"),
            Self::NotAvailable { label, reason } => write!(f, "{label} is not available here: {reason}"),
        }
    }
}

// ── link text: insertion and removal ──

/// `https://` and a host, no spaces: the only kind of link this module writes anywhere.
pub fn is_https_url(s: &str) -> bool {
    let s = s.trim();
    match s.strip_prefix("https://") {
        Some(rest) => !rest.is_empty() && !rest.starts_with('/') && !s.contains(char::is_whitespace),
        None => false,
    }
}

/// A Jitsi server as a bare `host[:port]`: a pasted `https://host/` is accepted, anything odd is refused.
fn clean_server(s: &str) -> Option<String> {
    let s = s.trim().trim_start_matches("https://").trim_end_matches('/');
    let ok = !s.is_empty()
        && !s.contains(['/', ' ', '?', '#', '@'])
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'));
    ok.then(|| s.to_string())
}

/// 12 lowercase letters/digits chosen at random (per-process OS-seeded hashing plus the seed and the clock), e.g.
/// `omalook-k3x9q2m7ab4z`. Unguessable enough for a room name; the caller stores it, so it is never made again.
pub fn random_slug(seed: &str) -> String {
    const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
    let mut out = String::from("omalook-");
    let mut n = 0u64;
    let mut bits = 0u32;
    for i in 0..12u32 {
        if bits < 5 {
            let mut h = RandomState::new().build_hasher();
            h.write(seed.as_bytes());
            h.write_u32(i);
            h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
            n = h.finish();
            bits = 64;
        }
        out.push(ALPHABET[(n & 31) as usize] as char);
        n >>= 5;
        bits -= 5;
    }
    out
}

/// The block put at the top of the body: `Join <label>`, the URL, then a blank line before the user's notes.
pub fn link_block(link: &MeetingLink) -> String {
    format!("Join {}\n{}\n\n", link.label.trim(), link.url)
}

/// Body with `link` as its first block. An earlier block is replaced, never duplicated.
pub fn insert_block(body: &str, link: &MeetingLink) -> String {
    let (rest, _) = strip_block(body);
    format!("{}{}", link_block(link), rest)
}

/// Remove the leading `Join <label>` + `https://` block this module writes; returns the rest and the URL it held.
/// A body whose first line is "Join ..." but is not followed by a URL line is left alone.
pub fn strip_block(body: &str) -> (String, Option<String>) {
    let mut lines = body.splitn(3, '\n');
    let (first, second) = (lines.next().unwrap_or(""), lines.next().unwrap_or(""));
    if !first.starts_with("Join ") || !is_https_url(second.trim_end_matches('\r')) {
        return (body.to_string(), None);
    }
    let url = second.trim().to_string();
    let rest = lines.next().unwrap_or("");
    let rest = rest.strip_prefix('\n').or_else(|| rest.strip_prefix("\r\n")).unwrap_or(rest);
    (rest.to_string(), Some(url))
}

/// Location with the join URL added: a location that is empty becomes the URL; any other keeps its text and gets
/// `; <url>` (the structured field still carries the link, so Outlook and phones show a Join button).
pub fn location_with(location: &str, url: &str) -> String {
    let loc = location.trim();
    if loc.is_empty() { url.to_string() } else if loc.contains(url) { loc.to_string() } else { format!("{loc}; {url}") }
}

/// Inverse of [`location_with`] for the URL a block held.
pub fn location_without(location: &str, url: &str) -> String {
    let loc = location.trim();
    if loc == url {
        return String::new();
    }
    loc.strip_suffix(url).map(|r| r.trim_end().trim_end_matches(';').trim_end().to_string()).unwrap_or_else(|| loc.to_string())
}

// ── applying a provider choice to a request ──

/// What an event body, location and provider become after the choice.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub body: String,
    pub location: String,
    /// Left for the Graph / Google builders; `None` for every non-native kind and for "none".
    pub online_provider: Option<String>,
    /// The URL written into the text, for the answer ("Jitsi link: ...").
    pub url: Option<String>,
}

/// Apply `choice` (see the module doc for absent / "none" / id) to a body and location.
pub fn prepare(choice: Option<&str>, body: &str, location: &str, cx: &Context, seed: &str) -> std::result::Result<Prepared, MeetingError> {
    let choice = choice.map(str::trim).filter(|c| !c.is_empty());
    let unchanged = || Prepared { body: body.to_string(), location: location.to_string(), online_provider: None, url: None };
    let Some(choice) = choice else { return Ok(unchanged()) };
    // Anything this module wrote earlier goes first, so a changed choice replaces the link and "none" removes it.
    let (bare_body, old_url) = strip_block(body);
    let bare_loc = old_url.as_deref().map(|u| location_without(location, u)).unwrap_or_else(|| location.to_string());
    if choice == NONE {
        return Ok(Prepared { body: bare_body, location: bare_loc, online_provider: None, url: None });
    }
    let p = find(choice).ok_or_else(|| MeetingError::Unknown(choice.to_string()))?;
    match p.available(cx) {
        Availability::Ok => {}
        Availability::NeedsSetup(reason) => return Err(MeetingError::NeedsSetup { label: p.label().into(), reason }),
        Availability::NotAvailable(reason) => return Err(MeetingError::NotAvailable { label: p.label().into(), reason }),
    }
    match p.kind() {
        Kind::Native => Ok(Prepared { body: bare_body, location: bare_loc, online_provider: Some(p.id().to_string()), url: None }),
        Kind::External => Err(MeetingError::NeedsSetup { label: p.label().into(), reason: "not built yet".into() }),
        Kind::Static | Kind::Generated => {
            let link = p.link(cx, seed).ok_or_else(|| MeetingError::NeedsSetup { label: p.label().into(), reason: "no link".into() })?;
            Ok(Prepared {
                body: insert_block(&bare_body, &link),
                location: location_with(&bare_loc, &link.url),
                online_provider: None,
                url: Some(link.url),
            })
        }
    }
}

/// The route's step: rewrite `body`, `location` and `online_provider` of the editor's JSON in place. Returns the URL it
/// wrote (static / generated kinds) for the answer.
pub fn prepare_request(req: &mut Value, account_id: &str, config: &MeetingConfig, seed: &str) -> std::result::Result<Option<String>, MeetingError> {
    let cx = Context { account_provider: crate::accounts::provider_of(account_id), config };
    let choice = req["online_provider"].as_str().map(String::from);
    let p = prepare(choice.as_deref(), req["body"].as_str().unwrap_or(""), req["location"].as_str().unwrap_or(""), &cx, seed)?;
    if choice.as_deref().map(str::trim).filter(|c| !c.is_empty()).is_none() {
        return Ok(None);
    }
    req["body"] = json!(p.body);
    req["location"] = json!(p.location);
    match &p.online_provider {
        Some(id) => req["online_provider"] = json!(id),
        None => {
            if let Some(o) = req.as_object_mut() {
                o.remove("online_provider");
            }
        }
    }
    Ok(p.url)
}

// ── settings.toml [meetings] ──

fn str_of<'a>(t: &'a toml::Value, k: &str) -> Option<&'a str> {
    t.get(k).and_then(|v| v.as_str())
}

pub fn read_config(path: &Path) -> MeetingConfig {
    let mut c = MeetingConfig::default();
    let Some(root) = std::fs::read_to_string(path).ok().and_then(|s| toml::from_str::<toml::Value>(&s).ok()) else { return c };
    let Some(m) = root.get("meetings") else { return c };
    let flag = |k: &str, d: bool| m.get(k).and_then(|v| v.as_bool()).unwrap_or(d);
    c.teams_enabled = flag("teams_enabled", true);
    c.meet_enabled = flag("meet_enabled", true);
    c.jitsi_enabled = flag("jitsi_enabled", true);
    c.personal_enabled = flag("personal_enabled", true);
    c.personal_label = str_of(m, "personal_label").unwrap_or("").to_string();
    c.personal_link = str_of(m, "personal_link").unwrap_or("").to_string();
    if let Some(s) = str_of(m, "jitsi_server").filter(|s| !s.trim().is_empty()) {
        c.jitsi_server = s.to_string();
    }
    if let Some(t) = m.get("default_provider").and_then(|v| v.as_table()) {
        c.default_provider = t.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect();
    }
    c
}

/// Clean what the settings page sent: an unusable personal link or server is refused (never stored half-valid).
pub fn normalize(c: &MeetingConfig) -> std::result::Result<MeetingConfig, String> {
    let mut c = c.clone();
    c.personal_link = c.personal_link.trim().to_string();
    c.personal_label = c.personal_label.trim().chars().take(60).collect();
    if !c.personal_link.is_empty() && !is_https_url(&c.personal_link) {
        return Err("the personal meeting link must start with https://".into());
    }
    let server = c.jitsi_server.trim();
    c.jitsi_server = if server.is_empty() {
        DEFAULT_JITSI_SERVER.to_string()
    } else {
        clean_server(server).ok_or("the Jitsi server must be a host name such as meet.jit.si")?
    };
    c.default_provider.retain(|_, v| v == NONE || find(v).is_some());
    Ok(c)
}

/// Persist `[meetings]`, leaving every other key and section as it was (edited as a generic TOML tree).
pub fn write_config(path: &Path, wanted: &MeetingConfig) -> Result<MeetingConfig> {
    let c = normalize(wanted).map_err(OmarchyError::SettingsError)?;
    let mut root: toml::Value = match std::fs::read_to_string(path) {
        Ok(content) => toml::from_str(&content).map_err(|e| OmarchyError::SettingsError(e.to_string()))?,
        Err(_) => toml::Value::Table(Default::default()),
    };
    let table = root.as_table_mut().ok_or_else(|| OmarchyError::SettingsError("settings.toml is not a table".into()))?;
    let mut m = toml::map::Map::new();
    m.insert("teams_enabled".into(), toml::Value::Boolean(c.teams_enabled));
    m.insert("meet_enabled".into(), toml::Value::Boolean(c.meet_enabled));
    m.insert("jitsi_enabled".into(), toml::Value::Boolean(c.jitsi_enabled));
    m.insert("personal_enabled".into(), toml::Value::Boolean(c.personal_enabled));
    m.insert("personal_label".into(), toml::Value::String(c.personal_label.clone()));
    m.insert("personal_link".into(), toml::Value::String(c.personal_link.clone()));
    m.insert("jitsi_server".into(), toml::Value::String(c.jitsi_server.clone()));
    let mut d = toml::map::Map::new();
    for (k, v) in &c.default_provider {
        d.insert(k.clone(), toml::Value::String(v.clone()));
    }
    m.insert("default_provider".into(), toml::Value::Table(d));
    table.insert("meetings".into(), toml::Value::Table(m));
    let content = toml::to_string_pretty(&root).map_err(|e| OmarchyError::SettingsError(e.to_string()))?;
    std::fs::write(path, content)?;
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> MeetingConfig {
        MeetingConfig { personal_link: "https://zoom.us/j/123456789".into(), personal_label: "My Zoom room".into(), ..Default::default() }
    }
    fn cx<'a>(p: &'a str, c: &'a MeetingConfig) -> Context<'a> {
        Context { account_provider: p, config: c }
    }

    #[test]
    fn native_providers_follow_the_account_kind() {
        let c = cfg();
        assert!(Teams.available(&cx("exchange", &c)).is_ok());
        assert!(!Teams.available(&cx("gmail", &c)).is_ok());
        assert!(Meet.available(&cx("gmail", &c)).is_ok());
        assert!(!Meet.available(&cx("exchange", &c)).is_ok());
        let off = MeetingConfig { teams_enabled: false, ..cfg() };
        assert_eq!(Teams.available(&cx("exchange", &off)).reason(), "switched off in Settings");
    }

    #[test]
    fn zoom_and_webex_are_stubs_that_need_setup() {
        let c = cfg();
        for id in [ZOOM, WEBEX] {
            let p = find(id).unwrap();
            assert!(matches!(p.available(&cx("exchange", &c)), Availability::NeedsSetup(_)));
            assert_eq!(p.kind(), Kind::External);
            let err = prepare(Some(id), "", "", &cx("exchange", &c), "s").unwrap_err();
            assert!(matches!(err, MeetingError::NeedsSetup { .. }), "{err}");
        }
        assert_eq!(find(ZOOM).unwrap().available(&cx("gmail", &c)).reason(), "Zoom sign-in is not built yet");
    }

    #[test]
    fn personal_link_needs_an_https_url() {
        let none = MeetingConfig::default();
        assert!(matches!(Personal.available(&cx("exchange", &none)), Availability::NeedsSetup(_)));
        let http = MeetingConfig { personal_link: "http://x.example/room".into(), ..Default::default() };
        assert!(!Personal.available(&cx("exchange", &http)).is_ok());
        assert!(Personal.available(&cx("exchange", &cfg())).is_ok());
        assert_eq!(Personal.link(&cx("exchange", &cfg()), "").unwrap().label, "My Zoom room");
    }

    #[test]
    fn jitsi_link_is_https_on_the_server_with_a_random_slug() {
        let c = cfg();
        let a = Jitsi.link(&cx("exchange", &c), "tx1").unwrap();
        let b = Jitsi.link(&cx("exchange", &c), "tx1").unwrap();
        assert!(a.url.starts_with("https://meet.jit.si/omalook-"), "{}", a.url);
        assert_eq!(a.url.len(), "https://meet.jit.si/omalook-".len() + 12);
        assert_ne!(a.url, b.url, "a slug is random, not derived from the seed");
        let own = MeetingConfig { jitsi_server: "https://jitsi.home.lan:8443/".into(), ..cfg() };
        assert!(Jitsi.link(&cx("gmail", &own), "x").unwrap().url.starts_with("https://jitsi.home.lan:8443/omalook-"));
        let bad = MeetingConfig { jitsi_server: "evil.example/../x".into(), ..cfg() };
        assert!(Jitsi.link(&cx("gmail", &bad), "x").is_none());
        assert!(matches!(Jitsi.available(&cx("gmail", &bad)), Availability::NeedsSetup(_)));
    }

    #[test]
    fn block_insert_replace_and_remove_round_trip() {
        let link = MeetingLink { label: "Jitsi Meeting".into(), url: "https://meet.jit.si/omalook-abc".into() };
        let with = insert_block("agenda\nline two", &link);
        assert_eq!(with, "Join Jitsi Meeting\nhttps://meet.jit.si/omalook-abc\n\nagenda\nline two");
        let other = MeetingLink { label: "My Zoom room".into(), url: "https://zoom.us/j/1".into() };
        assert_eq!(insert_block(&with, &other), "Join My Zoom room\nhttps://zoom.us/j/1\n\nagenda\nline two");
        let (rest, url) = strip_block(&with);
        assert_eq!((rest.as_str(), url.as_deref()), ("agenda\nline two", Some("https://meet.jit.si/omalook-abc")));
        // empty notes
        assert_eq!(strip_block(&insert_block("", &link)).0, "");
        // not our block: left alone
        assert_eq!(strip_block("Join us for lunch\nbring food").1, None);
        assert_eq!(strip_block("Join us for lunch\nbring food").0, "Join us for lunch\nbring food");
        assert_eq!(strip_block("").1, None);
    }

    #[test]
    fn location_gets_the_link_and_gives_it_back() {
        let u = "https://zoom.us/j/1";
        assert_eq!(location_with("", u), u);
        assert_eq!(location_with("Room 4", u), "Room 4; https://zoom.us/j/1");
        assert_eq!(location_with(u, u), u);
        assert_eq!(location_without(u, u), "");
        assert_eq!(location_without("Room 4; https://zoom.us/j/1", u), "Room 4");
        assert_eq!(location_without("Room 4", u), "Room 4");
    }

    #[test]
    fn prepare_static_puts_the_link_in_body_and_location_and_drops_the_provider() {
        let c = cfg();
        let p = prepare(Some(PERSONAL), "notes", "Room 4", &cx("exchange", &c), "tx").unwrap();
        assert_eq!(p.body, "Join My Zoom room\nhttps://zoom.us/j/123456789\n\nnotes");
        assert_eq!(p.location, "Room 4; https://zoom.us/j/123456789");
        assert_eq!((p.online_provider, p.url.as_deref()), (None, Some("https://zoom.us/j/123456789")));
    }

    #[test]
    fn prepare_native_keeps_the_provider_for_the_builder_and_checks_the_account() {
        let c = cfg();
        let p = prepare(Some(TEAMS), "notes", "", &cx("exchange", &c), "tx").unwrap();
        assert_eq!((p.online_provider.as_deref(), p.body.as_str(), p.url), (Some(TEAMS), "notes", None));
        let e = prepare(Some(TEAMS), "", "", &cx("gmail", &c), "tx").unwrap_err();
        assert!(matches!(e, MeetingError::NotAvailable { .. }), "{e}");
        assert!(matches!(prepare(Some("carrierPigeon"), "", "", &cx("gmail", &c), "t"), Err(MeetingError::Unknown(_))));
    }

    #[test]
    fn edit_switch_replaces_and_none_removes_the_link() {
        let c = cfg();
        let first = prepare(Some(JITSI), "notes", "", &cx("exchange", &c), "tx").unwrap();
        // switch to the personal link: the Jitsi block and location go, the new ones come
        let second = prepare(Some(PERSONAL), &first.body, &first.location, &cx("exchange", &c), "tx").unwrap();
        assert!(!second.body.contains("jit.si") && !second.location.contains("jit.si"));
        assert_eq!(second.body, "Join My Zoom room\nhttps://zoom.us/j/123456789\n\nnotes");
        assert_eq!(second.location, "https://zoom.us/j/123456789");
        // none: back to the user's own text
        let gone = prepare(Some(NONE), &second.body, &second.location, &cx("exchange", &c), "tx").unwrap();
        assert_eq!((gone.body.as_str(), gone.location.as_str(), gone.online_provider), ("notes", "", None));
        // absent: untouched
        let kept = prepare(None, &second.body, &second.location, &cx("exchange", &c), "tx").unwrap();
        assert_eq!((kept.body, kept.location), (second.body.clone(), second.location.clone()));
    }

    #[test]
    fn prepare_request_rewrites_the_json_in_place() {
        let c = cfg();
        let mut req = json!({"subject": "s", "body": "n", "location": "", "online_provider": "personalLink"});
        let url = prepare_request(&mut req, "exchange-abc123", &c, "tx").unwrap();
        assert_eq!(url.as_deref(), Some("https://zoom.us/j/123456789"));
        assert!(req.get("online_provider").is_none());
        assert_eq!(req["location"], "https://zoom.us/j/123456789");
        let mut teams = json!({"body": "n", "online_provider": "teamsForBusiness"});
        assert_eq!(prepare_request(&mut teams, "exchange-abc123", &c, "tx").unwrap(), None);
        assert_eq!(teams["online_provider"], "teamsForBusiness");
        let mut meet = json!({"body": "n", "online_provider": "googleMeet"});
        prepare_request(&mut meet, "gmail-0a1b2c", &c, "tx").unwrap();
        assert_eq!(meet["online_provider"], "googleMeet");
        let mut untouched = json!({"body": "n"});
        assert_eq!(prepare_request(&mut untouched, "gmail-0a1b2c", &c, "tx").unwrap(), None);
        assert_eq!(untouched, json!({"body": "n"}));
        let mut stub = json!({"body": "n", "online_provider": "zoom"});
        assert!(prepare_request(&mut stub, "exchange-1", &c, "tx").is_err());
    }

    #[test]
    fn the_request_builds_the_graph_and_google_bodies() {
        // end to end through the real builders: Teams asks Graph for the meeting, Meet asks Google, a link goes in text
        let c = cfg();
        let base = |p: &str| json!({"subject": "s", "start": "2026-11-16T09:00", "end": "2026-11-16T09:30", "time_zone": "UTC", "body": "n", "online_provider": p});
        let mut r = base(TEAMS);
        prepare_request(&mut r, "exchange-1", &c, "tx").unwrap();
        let g = crate::calendar_write::EventDraft::from_request(&r, "tx").unwrap().to_graph_json().unwrap();
        assert_eq!((g["isOnlineMeeting"].clone(), g["onlineMeetingProvider"].clone()), (json!(true), json!("teamsForBusiness")));
        let mut r = base(GOOGLE_MEET);
        prepare_request(&mut r, "gmail-1", &c, "tx").unwrap();
        let d = crate::calendar_write::EventDraft::from_request(&r, "tx").unwrap();
        assert!(crate::providers::google_calendar_write::create_body(&d).unwrap().get("conferenceData").is_some());
        assert!(crate::providers::google_calendar_write::create_url(&d).contains("conferenceDataVersion=1"));
        let mut r = base(PERSONAL);
        prepare_request(&mut r, "exchange-1", &c, "tx").unwrap();
        let g = crate::calendar_write::EventDraft::from_request(&r, "tx").unwrap().to_graph_json().unwrap();
        assert!(g.get("isOnlineMeeting").is_none());
        assert_eq!(g["location"]["displayName"], "https://zoom.us/j/123456789");
        assert!(g["body"]["content"].as_str().unwrap().starts_with("Join My Zoom room\nhttps://zoom.us/j/123456789\n\nn"));
        // the join detector finds the link from the cached event's location afterwards
        assert_eq!(crate::join_link::provider_of("https://zoom.us/j/123456789"), Some("zoom"));
    }

    #[test]
    fn options_list_everything_with_reasons_and_one_default() {
        let mut c = cfg();
        c.default_provider.insert("exchange-1".into(), TEAMS.into());
        c.default_provider.insert("gmail-1".into(), TEAMS.into()); // not usable on Google: falls back to None
        let rows = options("exchange-1", &c);
        let rows = rows.as_array().unwrap();
        assert_eq!(rows.len(), 7);
        assert_eq!(rows[0]["id"], NONE);
        let by = |id: &str| rows.iter().find(|r| r["id"] == id).unwrap().clone();
        assert_eq!(by(TEAMS)["default"], true);
        assert_eq!(rows[0]["default"], false);
        assert_eq!(by(GOOGLE_MEET)["available"], false);
        assert_eq!(by(ZOOM)["reason"], "Zoom sign-in is not built yet");
        assert_eq!(by(PERSONAL)["label"], "My Zoom room");
        assert_eq!(rows.iter().filter(|r| r["default"] == true).count(), 1);
        let g = options("gmail-1", &c);
        assert_eq!(g[0]["default"], true);
        assert_eq!(g.as_array().unwrap().iter().filter(|r| r["default"] == true).count(), 1);
    }

    #[test]
    fn settings_round_trip_keeps_other_sections_and_refuses_bad_input() {
        let dir = std::env::temp_dir().join(format!("omalook-meet-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("settings.toml");
        std::fs::write(&p, "[mail]\nsend_delay_secs = 9\n").unwrap();
        assert_eq!(read_config(&p), MeetingConfig::default());
        let mut c = cfg();
        c.jitsi_enabled = false;
        c.default_provider.insert("exchange-1".into(), TEAMS.into());
        c.default_provider.insert("gmail-1".into(), "bogus".into());
        let stored = write_config(&p, &c).unwrap();
        assert!(!stored.default_provider.contains_key("gmail-1"));
        assert_eq!(read_config(&p), stored);
        assert!(std::fs::read_to_string(&p).unwrap().contains("send_delay_secs = 9"));
        assert!(write_config(&p, &MeetingConfig { personal_link: "http://x".into(), ..cfg() }).is_err());
        assert!(write_config(&p, &MeetingConfig { jitsi_server: "a b".into(), ..cfg() }).is_err());
        assert_eq!(read_config(&p), stored, "a refused save changes nothing");
        std::fs::remove_dir_all(&dir).ok();
    }
}
