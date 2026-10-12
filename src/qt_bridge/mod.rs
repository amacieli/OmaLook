/// Qt Bridge module - Phase 2 integration between Rust backend and QML frontend
/// 
/// This module provides bridging layers for:
/// - AuthBridge: Microsoft Device Flow authentication
/// - SettingsBridge: Font/UI settings exposed to QML
/// - ComposeBridge: Email composition and send

pub mod auth_bridge;
pub mod settings_bridge;
pub mod compose_bridge;

pub use auth_bridge::AuthBridge;
pub use settings_bridge::SettingsBridge;
pub use compose_bridge::ComposeBridge;

// Re-export for convenience in main.rs
pub mod prelude {
    pub use super::{AuthBridge, ComposeBridge, SettingsBridge};
}
