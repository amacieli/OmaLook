# Omarchy-Look: QML Outlook Clone

A native Linux desktop email/calendar/contacts/tasks client built in **Qt/QML**, styled to match the Omarchy aesthetic (dark, minimal, keyboard-first). Connects exclusively to **Microsoft 365 / Exchange Online** via the **Microsoft Graph REST API**.

## Features (Roadmap)

- ✓ Phase 1: Device Flow authentication (no Azure app registration needed for end users)
- ⏳ Phase 2: App shell with navigation
- ⏳ Phase 3: Mail module (read-only)
- ⏳ Phase 4: Mail compose & actions
- ⏳ Phase 5: Categories
- ⏳ Phase 6: Calendar module
- ⏳ Phase 7: Contacts module
- ⏳ Phase 8: Tasks module
- ⏳ Phase 9: Polish & packaging

## Project Structure

```
omalook/
├── main.py                  # Entry point
├── pyproject.toml          # Dependencies
├── qml/                    # QML UI layer
│   └── App.qml            # Main window
├── backend/               # Python backend
│   ├── auth.py            # Device Flow authentication
│   ├── graph_client.py    # Graph API wrapper
│   ├── bridges.py         # QML ↔ Python interface
│   └── __init__.py
└── README.md              # This file
```

## Requirements

- Python 3.10+
- Qt 6.x (via PySide6)
- Microsoft 365 account

## Setup

1. **Clone and set up environment**
   ```bash
   cd /mnt/ai/projects/omalook
   ./setup.sh
   ```

2. **Run** (that's it — no Azure app registration needed!)
   ```bash
   python main.py
   ```

3. **Login**
   - You'll see a device code on screen
   - Open the URL in any browser on any device
   - Enter the code
   - Authenticate with your Microsoft 365 account
   - Done — token is cached locally

## Documentation

Internal planning docs (design, phased roadmap, initial spec) are in `/mnt/ai/projects/omalook-internal/` to keep this repo clean.

## Authentication Flow

**No Azure app registration required for end users.** The app uses Microsoft's public client ID (the same one used by Outlook desktop and Teams).

1. App launches and checks for cached token
2. If none found, shows login screen
3. User clicks "Sign in"
4. **Device Flow** appears:
   - User sees a code on screen
   - Opens URL on any device (phone, tablet, browser)
   - Enters code and authenticates with Microsoft 365
5. Access + refresh tokens cached in OS keyring (SecretService/KWallet on Linux)
6. App displays user's name in top bar
7. On relaunch, tokens refresh silently from cache

This is how **Thunderbird**, **New Outlook desktop**, and other native mail clients work — seamless for end users.

## Tech Stack

| Layer | Technology |
|-------|-----------|
| UI framework | Qt 6 / QML |
| Backend | Python 3.10+ with PySide6 |
| Auth | Device Flow OAuth (httpx) |
| API client | httpx (async) |
| Cache | SQLite (aiosqlite) |
| Build | pyproject.toml + venv |

## Keyboard Shortcuts (Phase 2+)

- `1`–`4` — Switch modules (Mail, Calendar, Contacts, Tasks)
- `Ctrl+\` — Toggle nav rail collapse
- `Ctrl+R` — Manual sync
- `?` — Show shortcuts overlay

## Development Notes

- **No Electron.** This is a real, native Qt application.
- **No Azure setup for users.** Uses Microsoft's public client ID.
- **Offline-capable.** All data cached locally; syncs on reconnect.
- **Graph-only.** No IMAP/SMTP — all via Microsoft Graph REST API.
- **Keyboard-first.** Full keybinding coverage; mouse optional.
- **Omarchy aesthetic.** Dark background (#1a1a1a), Inter/JetBrains Mono, rounded cards.

## References

- [Microsoft Device Flow auth](https://learn.microsoft.com/azure/active-directory/develop/v2-oauth2-device-code)
- [Microsoft Graph overview](https://learn.microsoft.com/graph/overview)
- [PySide6 docs](https://doc.qt.io/qtforpython-6/)
- [Qt QML docs](https://doc.qt.io/qt-6/qtqml-index.html)

## License

MIT
