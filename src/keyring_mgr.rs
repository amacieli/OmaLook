//! Keyring management (Linux SecretService)

use crate::errors::{OmarchyError, Result};
use log::{debug, warn};

const SERVICE_NAME: &str = "omalook";
const ACCOUNT_NAME: &str = "auth_cache";

/// Get cached token from keyring
pub fn get_cached_token() -> Result<Option<String>> {
    match keyring::Entry::new(SERVICE_NAME, ACCOUNT_NAME) {
        Ok(entry) => {
            match entry.get_password() {
                Ok(password) => {
                    debug!("Retrieved cached token from keyring");
                    Ok(Some(password))
                }
                Err(keyring::error::Error::NoEntry) => {
                    debug!("No cached token in keyring");
                    Ok(None)
                }
                Err(e) => {
                    warn!("Failed to retrieve token from keyring: {}", e);
                    Ok(None)
                }
            }
        }
        Err(e) => {
            warn!("Could not initialize keyring entry: {}", e);
            Ok(None)
        }
    }
}

/// Cache token in keyring
pub fn cache_token(token_json: &str) -> Result<()> {
    match keyring::Entry::new(SERVICE_NAME, ACCOUNT_NAME) {
        Ok(entry) => {
            entry.set_password(token_json)
                .map_err(|e| OmarchyError::KeyringError(e.to_string()))?;
            debug!("Cached token in keyring");
            Ok(())
        }
        Err(e) => {
            warn!("Could not initialize keyring entry: {}", e);
            Err(OmarchyError::KeyringError(e.to_string()))
        }
    }
}

/// Clear cached token from keyring
pub fn clear_cache() -> Result<()> {
    match keyring::Entry::new(SERVICE_NAME, ACCOUNT_NAME) {
        Ok(entry) => {
            match entry.delete_password() {
                Ok(_) => {
                    debug!("Cleared cached token from keyring");
                    Ok(())
                }
                Err(keyring::error::Error::NoEntry) => {
                    debug!("No token to clear from keyring");
                    Ok(())
                }
                Err(e) => {
                    warn!("Failed to clear keyring: {}", e);
                    Err(OmarchyError::KeyringError(e.to_string()))
                }
            }
        }
        Err(e) => {
            warn!("Could not initialize keyring entry: {}", e);
            Err(OmarchyError::KeyringError(e.to_string()))
        }
    }
}
