//! Settings manager with TOML config and file watching

use crate::errors::{OmarchyError, Result};
use crate::models::{CalendarSettings, MailSettings, Settings};
use log::{debug, info, warn};
use std::path::{Path, PathBuf};
use std::fs;
use std::sync::{Arc, Mutex};

pub struct SettingsManager {
    path: PathBuf,
    settings: Arc<Mutex<Settings>>,
    _watcher: Option<notify::RecommendedWatcher>,
}

impl SettingsManager {
    /// Load or create settings from TOML file
    pub fn open(path: &str) -> Result<Self> {
        let path = PathBuf::from(path);
        debug!("Loading settings from: {}", path.display());
        
        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        
        // Load or create settings
        let settings = if path.exists() {
            let content = fs::read_to_string(&path)?;
            toml::from_str(&content)
                .map_err(|e| OmarchyError::SettingsError(e.to_string()))?
        } else {
            let default = Settings::default();
            let content = toml::to_string_pretty(&default)
                .map_err(|e| OmarchyError::SettingsError(e.to_string()))?;
            fs::write(&path, content)?;
            default
        };
        
        info!("Settings loaded from: {}", path.display());
        
        let settings = Arc::new(Mutex::new(settings));
        
        // Set up file watcher (optional, non-fatal if it fails)
        let watcher = Self::setup_watcher(&path, Arc::clone(&settings)).ok();
        
        Ok(Self {
            path,
            settings,
            _watcher: watcher,
        })
    }
    
    /// Set up file watcher for settings changes
    fn setup_watcher(
        path: &Path,
        settings: Arc<Mutex<Settings>>,
    ) -> Result<notify::RecommendedWatcher> {
        use notify::{Watcher, RecursiveMode, Result as NotifyResult};
        
        let path = path.to_path_buf();
        let path_clone = path.clone();
        let path_for_read = path.clone();
        
        let mut watcher = notify::recommended_watcher(move |res: NotifyResult<_>| {
            if let Ok(event) = res {
                // Reload settings on file change
                if let notify::Event { ref paths, .. } = event {
                    if paths.iter().any(|p| p == &path_clone) {
                        match fs::read_to_string(&path_for_read) {
                            Ok(content) => {
                                match toml::from_str::<Settings>(&content) {
                                    Ok(new_settings) => {
                                        if let Ok(mut s) = settings.lock() {
                                            *s = new_settings;
                                            info!("Settings reloaded from file");
                                        }
                                    }
                                    Err(e) => {
                                        warn!("Failed to parse settings file: {}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("Failed to read settings file: {}", e);
                            }
                        }
                    }
                }
            }
        })
        .map_err(|e| OmarchyError::SettingsError(e.to_string()))?;
        
        if let Some(parent) = path.parent() {
            watcher.watch(parent, RecursiveMode::NonRecursive)
                .map_err(|e| OmarchyError::SettingsError(e.to_string()))?;
        }
        
        debug!("File watcher set up for settings");
        Ok(watcher)
    }
    
    /// Get current settings (read-only)
    pub fn get(&self) -> Result<Settings> {
        self.settings.lock()
            .map(|guard| guard.clone())
            .map_err(|_| OmarchyError::SettingsError("Settings lock poisoned".to_string()))
    }
    
    /// Update settings and write to disk
    pub fn set(&self, new_settings: Settings) -> Result<()> {
        let content = toml::to_string_pretty(&new_settings)
            .map_err(|e| OmarchyError::SettingsError(e.to_string()))?;
        
        fs::write(&self.path, &content)?;
        debug!("Settings written to: {}", self.path.display());
        
        self.settings.lock()
            .map(|mut guard| *guard = new_settings)
            .map_err(|_| OmarchyError::SettingsError("Settings lock poisoned".to_string()))
    }
    
    /// Update a specific setting field
    pub fn update_sync_interval(&self, seconds: i32) -> Result<()> {
        let mut settings = self.get()?;
        settings.sync.poll_interval_secs = seconds.max(10); // Enforce 10s minimum
        self.set(settings)
    }
    
    /// Get sync interval (with validation)
    pub fn get_sync_interval(&self) -> Result<i32> {
        let settings = self.get()?;
        Ok(settings.sync.poll_interval_secs.max(10))
    }
}

/// Calendar settings from a settings.toml, defaulting when the file is missing, unreadable or
/// has no [calendar] section. Always within the supported range.
pub fn read_calendar_settings(path: &Path) -> CalendarSettings {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str::<Settings>(&s).ok())
        .map(|s| s.calendar)
        .unwrap_or_default()
        .sanitized()
}

/// Default and limits of the mail poll interval (`[sync] poll_interval_secs`).
pub const DEFAULT_POLL_SECS: u64 = 60;
pub const MIN_POLL_SECS: u64 = 10;
pub const MAX_POLL_SECS: u64 = 3600;

/// Mail poll interval from a settings.toml, in seconds: the default when the file is missing,
/// unreadable or has no usable value, otherwise clamped to 10..=3600. Cheap enough to call
/// every cycle, so an edit takes effect without restarting the app.
pub fn read_poll_interval(path: &Path) -> u64 {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str::<toml::Value>(&s).ok())
        .and_then(|v| v.get("sync")?.get("poll_interval_secs")?.as_integer())
        .map(|n| (n.max(0) as u64).clamp(MIN_POLL_SECS, MAX_POLL_SECS))
        .unwrap_or(DEFAULT_POLL_SECS)
}

/// Seconds a sent message waits before it goes out (`[mail] send_delay_secs`): the default (3)
/// when the file is missing, unreadable or has no usable value, otherwise clamped to 0..=60.
/// Read on every send, so a change applies to the next message without a restart.
pub fn read_send_delay(path: &Path) -> u64 {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str::<toml::Value>(&s).ok())
        .and_then(|v| v.get("mail")?.get("send_delay_secs")?.as_integer())
        .map(|n| n.clamp(0, MailSettings::MAX_SEND_DELAY_SECS as i64) as u64)
        .unwrap_or(MailSettings::DEFAULT_SEND_DELAY_SECS as u64)
}

/// Persist the send delay (clamped) into settings.toml, leaving every other setting as it
/// was. Refuses to touch a file that does not parse. Returns what was stored.
pub fn write_send_delay(path: &Path, secs: i32) -> Result<u64> {
    let mut settings: Settings = match fs::read_to_string(path) {
        Ok(content) => toml::from_str(&content).map_err(|e| OmarchyError::SettingsError(e.to_string()))?,
        Err(_) => Settings::default(),
    };
    settings.mail = MailSettings { send_delay_secs: secs }.sanitized();
    let content = toml::to_string_pretty(&settings).map_err(|e| OmarchyError::SettingsError(e.to_string()))?;
    fs::write(path, content)?;
    Ok(settings.mail.send_delay_secs as u64)
}

/// Persist calendar settings (clamped) into settings.toml, leaving every other setting as it
/// was. Refuses to touch a file that does not parse. Returns what was stored.
pub fn write_calendar_settings(path: &Path, wanted: &CalendarSettings) -> Result<CalendarSettings> {
    let mut settings: Settings = match fs::read_to_string(path) {
        Ok(content) => toml::from_str(&content).map_err(|e| OmarchyError::SettingsError(e.to_string()))?,
        Err(_) => Settings::default(),
    };
    settings.calendar = wanted.sanitized();
    let content = toml::to_string_pretty(&settings).map_err(|e| OmarchyError::SettingsError(e.to_string()))?;
    fs::write(path, content)?;
    Ok(settings.calendar)
}

/// The accepted values of the message-rendering setting, or None for anything else.
pub fn normalize_message_rendering(value: &str) -> Option<&'static str> {
    match value {
        "html" => Some("html"),
        "system" => Some("system"),
        "system_sender" => Some("system_sender"),
        _ => None,
    }
}

/// Persist the message-rendering mode into settings.toml, leaving every other setting as it
/// was. Refuses an unknown value and a file that does not parse. Returns what was stored.
pub fn write_message_rendering(path: &Path, value: &str) -> Result<String> {
    let mode = normalize_message_rendering(value)
        .ok_or_else(|| OmarchyError::SettingsError(format!("unknown message rendering mode: {}", value)))?;
    let mut settings: Settings = match fs::read_to_string(path) {
        Ok(content) => toml::from_str(&content).map_err(|e| OmarchyError::SettingsError(e.to_string()))?,
        Err(_) => Settings::default(),
    };
    settings.ui.message_rendering = mode.to_string();
    let content = toml::to_string_pretty(&settings).map_err(|e| OmarchyError::SettingsError(e.to_string()))?;
    fs::write(path, content)?;
    Ok(settings.ui.message_rendering)
}

#[cfg(test)]
mod message_rendering_tests {
    use super::*;

    #[test]
    fn modes_default_validate_and_persist() {
        assert_eq!(Settings::default().ui.message_rendering, "system_sender");
        assert_eq!(normalize_message_rendering("html"), Some("html"));
        assert_eq!(normalize_message_rendering("Always HTML"), None);

        let p = std::env::temp_dir().join(format!("omalook-render-{}.toml", std::process::id()));
        let _ = fs::remove_file(&p);
        let mut base = Settings::default();
        base.ui.sidebar_expanded = false;
        fs::write(&p, toml::to_string_pretty(&base).unwrap()).unwrap();
        assert_eq!(write_message_rendering(&p, "system").unwrap(), "system");
        assert!(write_message_rendering(&p, "bogus").is_err());
        let back: Settings = toml::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(back.ui.message_rendering, "system");
        assert!(!back.ui.sidebar_expanded, "other settings untouched");

        // a settings file from before this setting existed still loads, with the default
        let old = fs::read_to_string(&p).unwrap().lines().filter(|l| !l.starts_with("message_rendering")).collect::<Vec<_>>().join("\n");
        let legacy: Settings = toml::from_str(&old).unwrap();
        assert_eq!(legacy.ui.message_rendering, "system_sender");
        let _ = fs::remove_file(&p);
    }
}

#[cfg(test)]
mod calendar_settings_tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("omalook-settings-{}-{}.toml", name, std::process::id()));
        let _ = fs::remove_file(&p);
        p
    }

    #[test]
    fn defaults_are_five_years_back_and_ten_ahead() {
        assert_eq!(read_calendar_settings(&temp("missing")), CalendarSettings { recurrence_years_back: 5, recurrence_years_ahead: 10 });
    }

    #[test]
    fn a_settings_file_from_before_this_section_still_loads() {
        let p = temp("legacy");
        let mut old = toml::to_string_pretty(&Settings::default()).unwrap();
        old = old.split("[calendar]").next().unwrap().to_string();       // simulate the old file
        assert!(!old.contains("calendar"));
        fs::write(&p, old).unwrap();
        assert!(toml::from_str::<Settings>(&fs::read_to_string(&p).unwrap()).is_ok(), "must not break app start-up");
        assert_eq!(read_calendar_settings(&p).recurrence_years_ahead, 10);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn write_round_trips_clamps_and_preserves_other_settings() {
        let p = temp("write");
        let mut base = Settings::default();
        base.ui.sidebar_expanded = false;
        base.font.family = "Custom Font".into();
        fs::write(&p, toml::to_string_pretty(&base).unwrap()).unwrap();

        let stored = write_calendar_settings(&p, &CalendarSettings { recurrence_years_back: 7, recurrence_years_ahead: 12 }).unwrap();
        assert_eq!((stored.recurrence_years_back, stored.recurrence_years_ahead), (7, 12));
        assert_eq!(read_calendar_settings(&p), stored);
        let back: Settings = toml::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        assert!(!back.ui.sidebar_expanded && back.font.family == "Custom Font", "other settings untouched");

        let clamped = write_calendar_settings(&p, &CalendarSettings { recurrence_years_back: -3, recurrence_years_ahead: 99 }).unwrap();
        assert_eq!((clamped.recurrence_years_back, clamped.recurrence_years_ahead), (0, 30));
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn an_unparseable_settings_file_is_not_overwritten() {
        let p = temp("broken");
        fs::write(&p, "this is = not [valid toml").unwrap();
        assert!(write_calendar_settings(&p, &CalendarSettings::default()).is_err());
        assert_eq!(fs::read_to_string(&p).unwrap(), "this is = not [valid toml");
        let _ = fs::remove_file(&p);
    }
}

#[cfg(test)]
mod poll_interval_tests {
    use super::*;

    fn file(tag: &str, content: Option<&str>) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("omalook-poll-{}-{}.toml", tag, std::process::id()));
        match content {
            Some(c) => fs::write(&p, c).unwrap(),
            None => { let _ = fs::remove_file(&p); }
        }
        p
    }

    #[test]
    fn reads_the_configured_value() {
        assert_eq!(read_poll_interval(&file("v", Some("[sync]\npoll_interval_secs = 45\nauto_sync = true\n"))), 45);
    }

    #[test]
    fn missing_file_or_section_gives_the_default() {
        assert_eq!(read_poll_interval(&file("none", None)), DEFAULT_POLL_SECS);
        assert_eq!(read_poll_interval(&file("nosec", Some("[ui]\nwindow_width = 1\n"))), DEFAULT_POLL_SECS);
        assert_eq!(read_poll_interval(&file("bad", Some("not toml ["))), DEFAULT_POLL_SECS);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        assert_eq!(read_poll_interval(&file("lo", Some("[sync]\npoll_interval_secs = 1\n"))), MIN_POLL_SECS);
        assert_eq!(read_poll_interval(&file("neg", Some("[sync]\npoll_interval_secs = -5\n"))), MIN_POLL_SECS);
        assert_eq!(read_poll_interval(&file("hi", Some("[sync]\npoll_interval_secs = 999999\n"))), MAX_POLL_SECS);
    }

    #[test]
    fn the_real_settings_file_shape_parses() {
        // the section as written by SettingsManager (all keys present, plus other sections)
        let toml = "[font]\nfamily = \"monospace\"\n\n[sync]\npoll_interval_secs = 60\nauto_sync = true\ncache_retention_days = 30\n";
        assert_eq!(read_poll_interval(&file("real", Some(toml))), 60);
    }
}


#[cfg(test)]
mod send_delay_tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("omalook-senddelay-{}-{}.toml", name, std::process::id()));
        let _ = fs::remove_file(&p);
        p
    }

    #[test]
    fn defaults_to_three_seconds() {
        assert_eq!(read_send_delay(&temp("missing")), 3);
        assert_eq!(Settings::default().mail.send_delay_secs, 3);
        let p = temp("nosec");
        fs::write(&p, "[ui]\nwindow_width = 1\n").unwrap();
        assert_eq!(read_send_delay(&p), 3);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn zero_is_honoured_and_values_are_clamped() {
        let p = temp("vals");
        fs::write(&p, "[mail]\nsend_delay_secs = 0\n").unwrap();
        assert_eq!(read_send_delay(&p), 0, "0 means send at once, not 'use the default'");
        fs::write(&p, "[mail]\nsend_delay_secs = -4\n").unwrap();
        assert_eq!(read_send_delay(&p), 0);
        fs::write(&p, "[mail]\nsend_delay_secs = 9999\n").unwrap();
        assert_eq!(read_send_delay(&p), 60);
        fs::write(&p, "not toml [").unwrap();
        assert_eq!(read_send_delay(&p), 3);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn write_round_trips_and_preserves_other_settings() {
        let p = temp("write");
        let mut base = Settings::default();
        base.ui.sidebar_expanded = false;
        fs::write(&p, toml::to_string_pretty(&base).unwrap()).unwrap();
        assert_eq!(write_send_delay(&p, 10).unwrap(), 10);
        assert_eq!(read_send_delay(&p), 10);
        assert_eq!(write_send_delay(&p, 500).unwrap(), 60);
        let back: Settings = toml::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        assert!(!back.ui.sidebar_expanded, "other settings untouched");
        // a file from before this section existed still loads
        let old: String = fs::read_to_string(&p).unwrap().split("[mail]").next().unwrap().to_string();
        assert!(toml::from_str::<Settings>(&old).is_ok());
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn an_unparseable_file_is_not_overwritten() {
        let p = temp("broken");
        fs::write(&p, "this is = not [valid toml").unwrap();
        assert!(write_send_delay(&p, 5).is_err());
        assert_eq!(fs::read_to_string(&p).unwrap(), "this is = not [valid toml");
        let _ = fs::remove_file(&p);
    }
}
