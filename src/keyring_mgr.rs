//! Keyring management (Linux SecretService)

use crate::errors::{OmarchyError, Result};
use log::{debug, warn};

// Keyring entries are saved sign-ins: the service name is their identity and must keep its
// pre-rename value, or every account looks signed out.
const SERVICE_NAME: &str = "omarchylook";
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
            match entry.delete_credential() {
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

#[cfg(test)]
mod default_store_tests {
    /// keyring 3 has no default store unless a store feature is enabled, and then it silently uses
    /// an in-memory mock (`EntryOnly`): every saved sign-in would vanish. The Linux default must be
    /// the Secret Service store (`UntilDelete`, the same store keyring 2 used). This only builds the
    /// builder; it never opens the Secret Service or touches the user's keyring.
    #[test]
    fn default_store_is_secret_service_not_the_mock() {
        let persistence = keyring::default::default_credential_builder().persistence();
        assert!(
            matches!(persistence, keyring::credential::CredentialPersistence::UntilDelete),
            "keyring default store is not the Secret Service: check the keyring features in Cargo.toml"
        );
    }
}
