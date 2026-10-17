//! Auth Bridge module - Microsoft Graph OAuth2 Device Flow with cxx-qt integration
//! 
//! Provides secure token storage via OS keyring and QML signal/slot integration
//! for device code flow authentication.

use std::sync::{Arc, Mutex};
use std::thread;
use crate::auth::AuthManager;
use crate::keyring_mgr;
use log::{info, debug, warn};

/// AuthBridge - Rust-Qt bridge for Microsoft Graph Device Flow authentication
/// 
/// This struct exposes authentication operations to QML and manages
/// the full lifecycle of OAuth tokens with secure keyring storage.
pub struct AuthBridge {
    auth_manager: Arc<Mutex<AuthManager>>,
    is_logging_in: bool,
    current_user_code: String,
    current_verification_uri: String,
}

impl Default for AuthBridge {
    fn default() -> Self {
        AuthBridge {
            auth_manager: Arc::new(Mutex::new(AuthManager::new())),
            is_logging_in: false,
            current_user_code: String::new(),
            current_verification_uri: String::new(),
        }
    }
}

impl AuthBridge {
    /// Create new auth bridge and check for existing cached tokens
    pub fn new() -> Self {
        let bridge = Self::default();
        
        // Check if user is already authenticated on startup
        match bridge.auth_manager.lock() {
            Ok(auth) => {
                if auth.is_authenticated() {
                    info!("AuthBridge::new() - User is authenticated (token cached in keyring)");
                } else {
                    info!("AuthBridge::new() - No cached token found");
                }
            }
            Err(e) => {
                warn!("AuthBridge::new() - Failed to lock auth manager: {}", e);
            }
        }
        
        bridge
    }

    /// Check if user is currently authenticated
    /// 
    /// Returns true if a valid refresh token exists in the OS keyring.
    /// This is called on app startup to determine whether to show login or main UI.
    pub fn is_authenticated(&self) -> bool {
        match self.auth_manager.lock() {
            Ok(auth) => auth.is_authenticated(),
            Err(e) => {
                warn!("is_authenticated() - Failed to lock auth manager: {}", e);
                false
            }
        }
    }

    /// Initiate Microsoft Device Flow authentication
    /// 
    /// This spawns a background thread to:
    /// 1. Request device code from Microsoft
    /// 2. Poll for token completion
    /// 3. Write status to state files that QML watches
    /// 
    /// The QML layer polls for device_code.json and reacts to changes.
    /// Since this uses static qmlscene (not cxx-qt FFI bridges), state is
    /// communicated via JSON files in the config directory.
    pub fn login(&mut self) -> Result<(), String> {
        info!("AuthBridge::login() - Device Flow initiated");
        
        if self.is_logging_in {
            return Err("Login already in progress".to_string());
        }
        
        self.is_logging_in = true;
        
        let auth_manager = Arc::clone(&self.auth_manager);
        
        // Get config directory (use standard XDG config path)
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from(".config"))
            .join("omarchy");
        
        // Spawn background thread to handle the auth flow
        // This prevents blocking the QML event loop
        thread::spawn(move || {
            match auth_manager.lock() {
                Ok(mut auth) => {
                    // Trigger device flow and write device code to file
                    match auth.trigger_device_flow(&config_dir) {
                        Ok(true) => {
                            info!("Device Flow login successful - token cached");
                            // Write success state for QML to read
                            let state = serde_json::json!({
                                "status": "authenticated",
                                "is_authenticated": true,
                                "message": "Authentication successful"
                            });
                            let state_file = config_dir.join("auth_state.json");
                            if let Ok(json_str) = serde_json::to_string_pretty(&state) {
                                let _ = std::fs::write(&state_file, json_str);
                            }
                        }
                        Ok(false) => {
                            warn!("Device Flow login timed out or was denied");
                            let state = serde_json::json!({
                                "status": "timeout",
                                "is_authenticated": false,
                                "message": "Authentication timed out or was denied"
                            });
                            let state_file = config_dir.join("auth_state.json");
                            if let Ok(json_str) = serde_json::to_string_pretty(&state) {
                                let _ = std::fs::write(&state_file, json_str);
                            }
                        }
                        Err(e) => {
                            warn!("Device Flow login error: {}", e);
                            let state = serde_json::json!({
                                "status": "error",
                                "is_authenticated": false,
                                "message": format!("Authentication failed: {}", e)
                            });
                            let state_file = config_dir.join("auth_state.json");
                            if let Ok(json_str) = serde_json::to_string_pretty(&state) {
                                let _ = std::fs::write(&state_file, json_str);
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("login() - Failed to lock auth manager: {}", e);
                }
            }
        });
        
        Ok(())
    }

    /// Logout and revoke the current token
    /// 
    /// This clears all cached tokens from the OS keyring and the local auth state.
    /// After calling this, the user will need to authenticate again.
    pub fn logout(&mut self) -> Result<(), String> {
        info!("AuthBridge::logout() - Revoking token and clearing cache");
        
        match self.auth_manager.lock() {
            Ok(mut auth) => {
                match auth.logout() {
                    Ok(()) => {
                        info!("Logout successful - token revoked and keyring cleared");
                        self.is_logging_in = false;
                        self.current_user_code.clear();
                        self.current_verification_uri.clear();
                        Ok(())
                    }
                    Err(e) => {
                        warn!("Logout failed: {}", e);
                        Err(format!("Logout failed: {}", e))
                    }
                }
            }
            Err(e) => {
                warn!("logout() - Failed to lock auth manager: {}", e);
                Err(format!("Failed to acquire auth lock: {}", e))
            }
        }
    }

    /// Get the current device code (for QML to read from file)
    /// 
    /// This allows QML to poll for device code without needing file I/O access.
    pub fn get_device_code(&self) -> Option<String> {
        // Try to read device_code.json from standard config directory
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from(".config"))
            .join("omarchy");

        let device_code_file = config_dir.join("device_code.json");

        match std::fs::read_to_string(&device_code_file) {
            Ok(content) => {
                match serde_json::from_str::<serde_json::Value>(&content) {
                    Ok(_) => {
                        // Return the full JSON as a string so QML can parse it
                        Some(content)
                    }
                    Err(_) => None,
                }
            }
            Err(_) => None,
        }
    }

    /// Clear the device code file (called after auth completes)
    pub fn clear_device_code(&self) -> Result<(), String> {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from(".config"))
            .join("omarchy");

        let device_code_file = config_dir.join("device_code.json");
        match std::fs::remove_file(&device_code_file) {
            Ok(()) => Ok(()),
            Err(e) => Err(format!("Failed to clear device code file: {}", e)),
        }
    }
    /// 
    /// Attempts to copy to clipboard via multiple backends:
    /// 1. xclip (X11)
    /// 2. pbcopy (macOS)
    /// 3. wl-copy (Wayland)
    /// 4. xsel (fallback X11)
    pub fn copy_to_clipboard(&self, text: &str) -> Result<(), String> {
        use std::process::Command;
        
        info!("AuthBridge::copy_to_clipboard() - Copying {} bytes", text.len());
        
        // Try xclip first (most common on Linux with X11)
        if let Ok(mut child) = Command::new("xclip")
            .arg("-selection")
            .arg("clipboard")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                if stdin.write_all(text.as_bytes()).is_ok() {
                    if child.wait().is_ok() {
                        debug!("Successfully copied to clipboard via xclip");
                        return Ok(());
                    }
                }
            }
        }
        
        // Try wl-copy (Wayland)
        if let Ok(mut child) = Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                if stdin.write_all(text.as_bytes()).is_ok() {
                    if child.wait().is_ok() {
                        debug!("Successfully copied to clipboard via wl-copy");
                        return Ok(());
                    }
                }
            }
        }
        
        // Try xsel (fallback X11)
        if let Ok(mut child) = Command::new("xsel")
            .arg("-b")
            .arg("-i")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                if stdin.write_all(text.as_bytes()).is_ok() {
                    if child.wait().is_ok() {
                        debug!("Successfully copied to clipboard via xsel");
                        return Ok(());
                    }
                }
            }
        }
        
        warn!("No clipboard backend available (tried xclip, wl-copy, xsel)");
        Err("Clipboard not available".to_string())
    }

    /// Get current user code (for testing)
    pub fn get_user_code(&self) -> String {
        self.current_user_code.clone()
    }

    /// Set device code (for testing/mocking)
    pub fn set_device_code(&mut self, user_code: String, verification_uri: String) {
        self.current_user_code = user_code;
        self.current_verification_uri = verification_uri;
    }

    /// Check if login is in progress
    pub fn is_logging_in(&self) -> bool {
        self.is_logging_in
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_bridge_default() {
        let bridge = AuthBridge::default();
        assert_eq!(bridge.get_user_code(), "");
        assert!(!bridge.is_logging_in());
    }

    #[test]
    fn test_auth_bridge_set_device_code() {
        let mut bridge = AuthBridge::new();
        bridge.set_device_code("ABC123".to_string(), "https://microsoft.com/devicelogin".to_string());
        assert_eq!(bridge.get_user_code(), "ABC123");
        assert!(!bridge.is_logging_in());
    }

    #[test]
    fn test_copy_to_clipboard() {
        // copy_to_clipboard has been seen to block forever (OL-005), so run it on its own thread
        // and fail the test after 5 s instead of hanging the whole `cargo test` run.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let bridge = AuthBridge::new();
            // Success needs a clipboard backend; only a hang or panic is a failure here.
            let _ = tx.send(bridge.copy_to_clipboard("test"));
        });
        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => panic!("copy_to_clipboard still running after 5 s (OL-005)"),
            Err(e) => panic!("copy_to_clipboard thread died: {e}"),
        }
    }
}
