//! Data models for OmaLook

use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

/// Device Flow OAuth response from Microsoft
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeviceFlowResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: i64,
    #[serde(default)]
    pub interval: i64,
}

/// Token response from Microsoft
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: i64,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub scope: String,
}

/// Cached token in keyring
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CachedToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Unix seconds at which `access_token` expires. Lets the next launch reuse a still-valid
    /// access token instead of refreshing. Absent in entries saved by older versions (and
    /// then derived from the token's JWT `exp` claim where the token is a JWT).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}

/// One page of a provider change feed (Graph `messages/delta`).
#[derive(Debug, Clone, Default)]
pub struct DeltaPage {
    /// New or changed messages.
    pub upserts: Vec<EmailMessage>,
    /// Ids that left the folder (deleted, or moved out).
    pub removed: Vec<String>,
    /// Link to resume the walk from after this page (set on every page but the last).
    pub next_link: Option<String>,
}

/// How a delta walk ended.
#[derive(Debug, Clone, PartialEq)]
pub enum DeltaEnd {
    /// Walk finished: store this link and use it next time.
    Done(String),
    /// The provider rejected the stored link (expired / malformed): start over from scratch.
    Reset,
}

/// Mail folder from Graph API / SQLite cache
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MailFolder {
    pub id: String,
    pub display_name: String,
    pub parent_folder_id: Option<String>,
    pub unread_item_count: Option<i32>,
    pub total_item_count: Option<i32>,
    #[serde(rename = "wellKnownName", default)]
    pub well_known_name: Option<String>,
}

/// Mail message from Graph API
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Message {
    pub id: String,
    pub subject: String,
    pub from: Option<Recipient>,
    pub to_recipients: Option<Vec<Recipient>>,
    pub body: Option<ItemBody>,
    pub received_date_time: Option<String>,
    pub sent_date_time: Option<String>,
    pub is_read: Option<bool>,
    pub has_attachments: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Recipient {
    pub email_address: Option<EmailAddress>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EmailAddress {
    pub address: String,
    pub name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ItemBody {
    pub content_type: Option<String>,
    pub content: String,
}

/// Email message for daemon storage
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct EmailMessage {
    pub id: String,
    pub from: String,
    pub subject: String,
    pub received: String,
    pub body: String,
    pub folder_id: Option<String>,
    pub is_read: bool,
    /// True when the provider filled in the detail fields below (even if some are empty).
    /// Rows stored without it are repopulated by the one-time backfill.
    pub meta: bool,
    pub from_name: Option<String>,
    /// Recipients as the list shows them: `Name <addr>, Name <addr>` (names optional).
    pub to_text: Option<String>,
    pub cc_text: Option<String>,
    pub bcc_text: Option<String>,
    /// Exchange: category names. Gmail: user-label names (tags). `None` = not fetched.
    pub categories: Option<Vec<String>>,
    /// When the sender sent it (ISO 8601); `received` is when the server got it.
    pub sent_at: Option<String>,
    /// "low" | "normal" | "high".
    pub importance: Option<String>,
    pub has_attachments: Option<bool>,
    /// Thread / conversation id, kept for threading later.
    pub conversation_id: Option<String>,
}

/// A category (Exchange) or user label (Gmail) of one account, with its colour.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct CategoryDef {
    pub name: String,
    /// `#rrggbb` for display.
    pub color: String,
    /// The provider's own colour value (Exchange `presetN`), when it has one.
    #[serde(default)]
    pub provider_color: Option<String>,
}

/// Colours offered when creating a category: a display colour plus what each provider needs.
/// Gmail only accepts a fixed palette of label colours; Exchange only its 25 presets.
pub struct PaletteColor {
    pub key: &'static str,
    pub label: &'static str,
    pub hex: &'static str,
    pub graph_preset: &'static str,
    /// Gmail label background (from Gmail's allowed set) and matching text colour.
    pub gmail_bg: &'static str,
    pub gmail_fg: &'static str,
}

pub const CATEGORY_PALETTE: &[PaletteColor] = &[
    PaletteColor { key: "red",     label: "Red",     hex: "#e74856", graph_preset: "preset0",  gmail_bg: "#fb4c2f", gmail_fg: "#ffffff" },
    PaletteColor { key: "orange",  label: "Orange",  hex: "#f7630c", graph_preset: "preset1",  gmail_bg: "#ffad47", gmail_fg: "#000000" },
    PaletteColor { key: "brown",   label: "Brown",   hex: "#8e562e", graph_preset: "preset2",  gmail_bg: "#a46a21", gmail_fg: "#ffffff" },
    PaletteColor { key: "yellow",  label: "Yellow",  hex: "#fce100", graph_preset: "preset3",  gmail_bg: "#fad165", gmail_fg: "#000000" },
    PaletteColor { key: "green",   label: "Green",   hex: "#10893e", graph_preset: "preset4",  gmail_bg: "#16a766", gmail_fg: "#ffffff" },
    PaletteColor { key: "teal",    label: "Teal",    hex: "#00b7c3", graph_preset: "preset5",  gmail_bg: "#2da2bb", gmail_fg: "#ffffff" },
    PaletteColor { key: "blue",    label: "Blue",    hex: "#0078d4", graph_preset: "preset7",  gmail_bg: "#4986e7", gmail_fg: "#ffffff" },
    PaletteColor { key: "purple",  label: "Purple",  hex: "#8764b8", graph_preset: "preset8",  gmail_bg: "#b99aff", gmail_fg: "#000000" },
    PaletteColor { key: "cranberry", label: "Cranberry", hex: "#c30052", graph_preset: "preset9", gmail_bg: "#f691b3", gmail_fg: "#000000" },
    PaletteColor { key: "steel",   label: "Steel",   hex: "#6b8ea5", graph_preset: "preset10", gmail_bg: "#a2b0a4", gmail_fg: "#000000" },
    PaletteColor { key: "gray",    label: "Gray",    hex: "#8a8886", graph_preset: "preset12", gmail_bg: "#999999", gmail_fg: "#ffffff" },
    PaletteColor { key: "black",   label: "Black",   hex: "#3b3a39", graph_preset: "preset14", gmail_bg: "#434343", gmail_fg: "#ffffff" },
];

pub fn palette_color(key: &str) -> Option<&'static PaletteColor> {
    CATEGORY_PALETTE.iter().find(|c| c.key == key)
}

/// Display colour of an Exchange colour preset (`preset0`…`preset24`); unknown / `none` -> gray.
pub fn graph_preset_hex(preset: &str) -> &'static str {
    match preset {
        "preset0" => "#e74856", "preset1" => "#f7630c", "preset2" => "#8e562e", "preset3" => "#fce100",
        "preset4" => "#10893e", "preset5" => "#00b7c3", "preset6" => "#847545", "preset7" => "#0078d4",
        "preset8" => "#8764b8", "preset9" => "#c30052", "preset10" => "#6b8ea5", "preset11" => "#4f6bed",
        "preset12" => "#8a8886", "preset13" => "#69797e", "preset14" => "#3b3a39", "preset15" => "#a4262c",
        "preset16" => "#ca5010", "preset17" => "#603a1d", "preset18" => "#c19c00", "preset19" => "#0b6a0b",
        "preset20" => "#038387", "preset21" => "#4d5d3c", "preset22" => "#003966", "preset23" => "#5c2e91",
        "preset24" => "#8a1050",
        _ => "#8a8886",
    }
}

/// Stable fallback colour for a category the provider gave no colour for.
pub fn fallback_category_color(name: &str) -> &'static str {
    let h = name.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    CATEGORY_PALETTE[(h as usize) % CATEGORY_PALETTE.len()].hex
}

#[cfg(test)]
mod category_palette_tests {
    use super::*;

    #[test]
    fn palette_is_consistent() {
        let mut keys: Vec<_> = CATEGORY_PALETTE.iter().map(|c| c.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), CATEGORY_PALETTE.len(), "palette keys unique");
        for c in CATEGORY_PALETTE {
            assert!(c.hex.starts_with('#') && c.hex.len() == 7);
            assert_eq!(graph_preset_hex(c.graph_preset), c.hex, "{} display colour matches its Exchange preset", c.key);
            assert!(c.gmail_bg.starts_with('#') && (c.gmail_fg == "#ffffff" || c.gmail_fg == "#000000"));
        }
        assert_eq!(graph_preset_hex("none"), "#8a8886");
        assert_eq!(fallback_category_color("x"), fallback_category_color("x"));
    }
}

/// Calendar event for daemon storage (start/end are local wall-clock
/// "YYYY-MM-DDTHH:MM:SS" strings in `time_zone`)
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct CalendarEvent {
    pub id: String,
    pub subject: String,
    pub body: String,
    pub start: String,
    pub end: String,
    pub is_all_day: bool,
    pub time_zone: String,
    /// Graph event type: singleInstance | occurrence | exception | seriesMaster
    /// (empty is treated as singleInstance). Series masters are stored but never displayed:
    /// their occurrences are.
    pub event_type: String,
    pub series_master_id: Option<String>,
    // ---- detail fields (docs/PLAN-calendar-events.md section 3); all empty when the provider has none
    pub location: String,
    pub organizer_name: String,
    pub organizer_email: String,
    pub is_organizer: bool,
    /// JSON array of `{name, email, role: required|optional|resource, response}`; "" = none
    pub attendees_json: String,
    /// Own response: none | organizer | accepted | tentativelyAccepted | declined | notResponded ("" = unknown)
    pub my_response: String,
    /// free | tentative | busy | oof | workingElsewhere ("" = unknown)
    pub show_as: String,
    /// teamsForBusiness | skypeForBusiness | googleMeet ... ("" = none)
    pub online_provider: String,
    /// Structured join link from the provider (Graph `onlineMeeting.joinUrl`, Google `hangoutLink`)
    pub online_url: String,
    pub reminder_minutes: Option<i64>,
    pub web_link: String,
    pub sensitivity: String,
    /// Graph `recurrence` object as JSON (series masters only); "" = none
    pub recurrence_json: String,
    pub calendar_id: String,
}

/// Phone number with its type ("mobile", "home", "work")
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ContactPhone {
    pub kind: String,
    pub number: String,
}

/// Postal address, already flattened to one line ("street, city, state zip, country")
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ContactAddress {
    pub kind: String,
    pub text: String,
}

/// Contact for daemon storage
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct Contact {
    pub id: String,
    pub display_name: String,
    pub given_name: String,
    pub surname: String,
    pub company: String,
    pub job_title: String,
    pub emails: Vec<String>,
    pub phones: Vec<ContactPhone>,
    pub addresses: Vec<ContactAddress>,
    pub folder_id: Option<String>,
    pub created_at: String,
    pub modified_at: String,
}

/// A contact folder ("contact list" in the UI)
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ContactFolder {
    pub id: String,
    pub display_name: String,
    pub parent_folder_id: Option<String>,
}

/// Contact plus the view-only fields the People view needs
#[derive(Debug, Serialize, Clone)]
pub struct ContactRow {
    #[serde(flatten)]
    pub contact: Contact,
    pub folder_name: String,
    pub is_favorite: bool,
    /// Which account the contact belongs to (opaque id, and that account's email address)
    pub account_id: String,
    pub account_email: String,
}

/// A configured mail/calendar account (see `accounts.rs` for the id scheme)
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub id: String,
    pub provider: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    /// Non-secret provider settings as JSON (e.g. IMAP host/port). Secrets live in the keyring.
    pub config: String,
    /// False after "Log out": sync is stopped but tokens and cached data are kept.
    pub enabled: bool,
}

/// Cached mail entry in SQLite
#[derive(Debug, Clone)]
pub struct CachedMessage {
    pub id: String,
    pub subject: String,
    pub from_email: String,
    pub from_name: Option<String>,
    pub body: String,
    pub received_at: DateTime<Utc>,
    pub is_read: bool,
    pub cached_at: DateTime<Utc>,
}

/// Settings structure (mirrors TOML config)
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Settings {
    pub font: FontSettings,
    pub color: ColorSettings,
    pub ui: UiSettings,
    pub sync: SyncSettings,
    /// Absent in settings files written before this section existed.
    #[serde(default)]
    pub calendar: CalendarSettings,
    /// Absent in settings files written before this section existed.
    #[serde(default)]
    pub mail: MailSettings,
    /// Keyboard bindings (`[keys]`): kept in the struct so rewrites of settings.toml by other
    /// settings (sidebar, folder pane, send delay…) never drop what the user wrote there.
    #[serde(default)]
    pub keys: KeySettings,
    /// MCP server permissions (`[mcp]`). Kept in the struct so other settings' rewrites keep it.
    #[serde(default, skip_serializing_if = "crate::mcp::policy::McpSettings::is_default")]
    pub mcp: crate::mcp::policy::McpSettings,
}

/// `[keys]` in settings.toml. `preset` adds a family of extra bindings ("outlook"); every other
/// entry rebinds one command: `"mail.archive" = "e"`, `"move.down" = ["j", "Down"]`, `"x" = []`
/// to unbind. The command ids are the ones the `?` help overlay shows. The UI interprets these;
/// the backend only stores and serves them.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct KeySettings {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preset: String,
    #[serde(flatten)]
    pub bindings: std::collections::BTreeMap<String, KeyBinding>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum KeyBinding {
    One(String),
    Many(Vec<String>),
}

impl KeyBinding {
    pub fn keys(&self) -> Vec<String> {
        match self {
            KeyBinding::One(k) if k.is_empty() => Vec::new(),
            KeyBinding::One(k) => vec![k.clone()],
            KeyBinding::Many(v) => v.clone(),
        }
    }
}

#[cfg(test)]
mod key_settings_tests {
    use super::*;

    #[test]
    fn keys_table_round_trips_beside_the_other_settings() {
        let mut base = Settings::default();
        base.keys.preset = "outlook".into();
        base.keys.bindings.insert("mail.archive".into(), KeyBinding::One("e".into()));
        base.keys.bindings.insert("move.down".into(), KeyBinding::Many(vec!["j".into(), "Down".into()]));
        base.keys.bindings.insert("item.delete".into(), KeyBinding::Many(vec![]));
        let text = toml::to_string_pretty(&base).unwrap();
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!(back.keys, base.keys, "{}", text);
        // a file without [keys] still loads
        let plain = toml::to_string_pretty(&Settings::default()).unwrap().replace("[keys]", "");
        assert!(toml::from_str::<Settings>(&plain).unwrap().keys.bindings.is_empty());
    }

    #[test]
    fn hand_written_keys_parse() {
        let mut s = toml::to_string_pretty(&Settings::default()).unwrap().replace("[keys]\n", "");
        s.push_str("\n[keys]\npreset = \"outlook\"\n\"go.mail\" = \"g m\"\n\"mail.move\" = [\"m\", \"C-S-v\"]\n\"item.delete\" = \"\"\n");
        let st: Settings = toml::from_str(&s).unwrap();
        assert_eq!(st.keys.preset, "outlook");
        assert_eq!(st.keys.bindings["mail.move"].keys(), vec!["m", "C-S-v"]);
        assert!(st.keys.bindings["item.delete"].keys().is_empty());
    }
}

/// Mail behaviour settings (`[mail]` in settings.toml).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MailSettings {
    /// Seconds a sent message waits (during which it can still be taken back) before it goes
    /// out. 0 = send at once. Default 3, at most `MAX_SEND_DELAY_SECS`.
    #[serde(default = "MailSettings::default_send_delay")]
    pub send_delay_secs: i32,
}

impl MailSettings {
    pub const DEFAULT_SEND_DELAY_SECS: i32 = 3;
    pub const MAX_SEND_DELAY_SECS: i32 = 60;
    fn default_send_delay() -> i32 { Self::DEFAULT_SEND_DELAY_SECS }

    pub fn sanitized(&self) -> Self {
        Self { send_delay_secs: self.send_delay_secs.clamp(0, Self::MAX_SEND_DELAY_SECS) }
    }
}

impl Default for MailSettings {
    fn default() -> Self { Self { send_delay_secs: Self::DEFAULT_SEND_DELAY_SECS } }
}

/// How far recurring meetings are expanded into individual occurrences, in whole years
/// either side of today (Settings → Calendar).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct CalendarSettings {
    #[serde(default = "CalendarSettings::default_back")]
    pub recurrence_years_back: i32,
    #[serde(default = "CalendarSettings::default_ahead")]
    pub recurrence_years_ahead: i32,
}

impl CalendarSettings {
    pub const MAX_YEARS_BACK: i32 = 20;
    pub const MAX_YEARS_AHEAD: i32 = 30;

    fn default_back() -> i32 { 5 }
    fn default_ahead() -> i32 { 10 }

    /// Clamp to the supported range (0..=20 back, 1..=30 ahead).
    pub fn sanitized(&self) -> Self {
        Self {
            recurrence_years_back: self.recurrence_years_back.clamp(0, Self::MAX_YEARS_BACK),
            recurrence_years_ahead: self.recurrence_years_ahead.clamp(1, Self::MAX_YEARS_AHEAD),
        }
    }
}

impl Default for CalendarSettings {
    fn default() -> Self {
        Self { recurrence_years_back: Self::default_back(), recurrence_years_ahead: Self::default_ahead() }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FontSettings {
    pub family: String,
    pub base_size: i32,
    /// Whole-UI scale (Settings -> Appearance). Missing in older files -> 1.0.
    #[serde(default = "default_scale_factor")]
    pub scale_factor: f64,
}

fn default_scale_factor() -> f64 { 1.0 }

impl FontSettings {
    /// Calculate effective font size: base_size * scale_factor
    pub fn effective_size(&self) -> i32 {
        (self.base_size as f64 * self.scale_factor).round() as i32
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ColorSettings {
    pub bg_dark: String,
    pub bg_surface: String,
    pub border: String,
    pub text_primary: String,
    pub text_secondary: String,
    pub accent_purple: String,
    pub danger_red: String,
    pub success_green: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UiSettings {
    pub window_width: i32,
    pub window_height: i32,
    pub use_tui_style: bool,
    pub animation_enabled: bool,
    /// Whether the left nav sidebar is expanded (persisted across launches)
    #[serde(default = "default_true")]
    pub sidebar_expanded: bool,
    /// How the reading pane picks HTML or system-font rendering:
    /// "html" | "system" | "system_sender" (system, unless the sender is listed
    /// with "always HTML"). See `settings::normalize_message_rendering`.
    #[serde(default = "default_message_rendering")]
    pub message_rendering: String,
    /// Show the folder pane in the mail view (off by default: folders are reached with the
    /// folder picker, `g f`).
    #[serde(default)]
    pub folder_pane: bool,
    /// Message list width, in characters, relative to its default width (negative = narrower).
    /// Changed with Ctrl-Left / Ctrl-Right in the mail view; the reading pane takes the rest.
    #[serde(default)]
    pub list_width_offset: i32,
}

fn default_message_rendering() -> String { "system_sender".to_string() }

fn default_true() -> bool { true }

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SyncSettings {
    /// Poll interval in seconds (default 60, min 10)
    pub poll_interval_secs: i32,
    /// Auto-sync on startup
    pub auto_sync: bool,
    /// Keep messages cached for N days (0 = forever)
    pub cache_retention_days: i32,
    /// Keep the backend (sync, SMS mirror, HTTP server) running after the UI window is closed.
    /// Absent in settings files written before this setting existed.
    #[serde(default)]
    pub run_in_background: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font: FontSettings {
                family: "JetBrainsMono Nerd Font".to_string(),
                base_size: 14,
                scale_factor: 1.0,
            },
            color: ColorSettings {
                bg_dark: "#0d0d0d".to_string(),
                bg_surface: "#242424".to_string(),
                border: "#333333".to_string(),
                text_primary: "#e8e8e8".to_string(),
                text_secondary: "#888888".to_string(),
                accent_purple: "#7c6af7".to_string(),
                danger_red: "#ff6b6b".to_string(),
                success_green: "#51cf66".to_string(),
            },
            ui: UiSettings {
                window_width: 1280,
                window_height: 800,
                use_tui_style: true,
                animation_enabled: true,
                sidebar_expanded: true,
                message_rendering: default_message_rendering(),
                folder_pane: false,
                list_width_offset: 0,
            },
            sync: SyncSettings {
                poll_interval_secs: 120, // 2 minute polling interval for email daemon
                auto_sync: true,
                cache_retention_days: 30,
                run_in_background: false,
            },
            calendar: CalendarSettings::default(),
            mail: MailSettings::default(),
            keys: KeySettings::default(),
            mcp: Default::default(),
        }
    }
}

/// A mailbox operation that moves a message out of the list it is in. Queued locally first
/// (the row is hidden at once), pushed to the provider after the undo window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageAction {
    /// Out of the inbox into the archive (Gmail: drop the INBOX label).
    Archive,
    /// Into Deleted Items / Trash.
    Trash,
    /// Gone for good (only from Deleted Items / Trash).
    Delete,
    /// Into another folder of the same account (provider folder id).
    Move(String),
}

impl MessageAction {
    /// The form stored in `messages.action_pending`.
    pub fn to_db(&self) -> String {
        match self {
            MessageAction::Archive => "archive".into(),
            MessageAction::Trash => "trash".into(),
            MessageAction::Delete => "delete".into(),
            MessageAction::Move(dest) => format!("move:{}", dest),
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "archive" => Some(MessageAction::Archive),
            "trash" => Some(MessageAction::Trash),
            "delete" => Some(MessageAction::Delete),
            _ => s.strip_prefix("move:").filter(|d| !d.is_empty()).map(|d| MessageAction::Move(d.to_string())),
        }
    }
}

#[cfg(test)]
mod message_action_tests {
    use super::*;

    #[test]
    fn round_trips_through_the_db_form() {
        for a in [MessageAction::Archive, MessageAction::Trash, MessageAction::Delete, MessageAction::Move("AAMk=/x:y".into())] {
            assert_eq!(MessageAction::from_db(&a.to_db()), Some(a));
        }
        assert_eq!(MessageAction::from_db("move:"), None);
        assert_eq!(MessageAction::from_db("bogus"), None);
    }
}
