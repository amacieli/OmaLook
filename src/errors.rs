//! Error types for OmaLook

use thiserror::Error;

#[derive(Error, Debug)]
pub enum OmarchyError {
    #[error("Authentication error: {0}")]
    AuthError(String),
    
    #[error("Token error: {0}")]
    TokenError(String),
    
    #[error("HTTP error: {0}")]
    HttpError(String),
    
    #[error("Database error: {0}")]
    DatabaseError(#[from] rusqlite::Error),
    
    #[error("Keyring error: {0}")]
    KeyringError(String),
    
    #[error("Settings error: {0}")]
    SettingsError(String),
    
    #[error("Invalid device flow response: {0}")]
    InvalidDeviceFlow(String),
    
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, OmarchyError>;
