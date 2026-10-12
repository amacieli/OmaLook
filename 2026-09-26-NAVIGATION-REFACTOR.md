# Navigation Refactor: Settings Panel & Account Management

**Date:** September 26, 2026  
**Status:** ✅ COMPLETE

## Summary

Refactored navigation flow to move account management into the Settings panel. Users now go straight to the main app view, with Settings accessible via a left sidebar link. The Graph API authentication flow is now triggered from a button in the Settings menu.

## Changes Made

### 1. AppShell.qml (Navigation Bar)
- **Updated** `currentView` property to include "settings" option
- **Enabled** Settings nav item (was previously disabled with greyed-out styling)
  - Now highlights in purple when active
  - Click to switch to Settings view
- **Removed** Logout button entirely
  - Account management flows through Settings instead
- **Added** Settings view to content Loader
  - Integrates SettingsPanel component when Settings is active

### 2. SettingsPanel.qml (Settings Screen)
- **Added** `authBridge` property (required, passed from AppShell)
- **Added** "Account Settings" section with Graph API authentication button
  - Label: "🔐 Re-authenticate with Graph API"
  - Green hover effect
  - **Wired** to call `authBridge.start_device_code_flow()`
  - Clicking this button triggers the device code flow for Graph auth

### 3. main.qml (Test Entry Point)
- **Fixed** initial auth state: `isAuthenticated: true`
- Now launches directly to main app view (no login screen)

### 4. App.qml (Proper Entry Point)
- **Fixed** initial auth state: `isAuthenticated: true`
- Ready for C++ integration when Qt app replaces qmlscene launcher

## User Flow (New)

1. **Launch** → Main Mail view (AppShell)
2. **Left Sidebar** → Click "[S] Settings" to open Settings panel
3. **Settings Panel** → Click "🔐 Re-authenticate with Graph API" button
4. **Graph Auth** → Device code flow initiated via `authBridge.start_device_code_flow()`
5. **Back to Mail** → Click "[M] Mail" to return to main view

## Intent

This structure establishes account management as a **preferences/configuration task** rather than a login/authentication task. Users manage mail accounts (Graph, and future providers) via the Settings menu, not a login screen. The Settings panel is the unified hub for:
- Font configuration
- UI preferences
- Sync settings
- **Account/mail service authentication**

## Build Status

✅ Build successful (no new errors)
- Binary: `/mnt/ai/projects/omalook/target/debug/omalook`
- Size: 45M
- All QML files compile without syntax errors

## Next Steps

1. **Wire the Graph flow** in the backend: ensure `authBridge.start_device_code_flow()` is callable from QML
2. **Test the full flow**: launch `./run.sh`, navigate to Settings, click Graph button, verify device code display
3. **Add more mail providers** to Account Settings section (IMAP, Office 365, etc.)
4. **Account list view**: Show currently authenticated accounts and allow removing them
