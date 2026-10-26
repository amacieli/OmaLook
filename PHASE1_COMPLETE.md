# OmaLook Phase 1: Rust Scaffolding

**Status**: ✅ Complete  
**Date**: September 26, 2026  
**Goal**: Fully refactored Python code to Rust modules + build infrastructure

## What's Built

### Build Infrastructure

#### `build.sh` (2.75 KB)
- Full Rust build pipeline
- `--release` flag for optimized builds
- `--clean` flag for clean rebuilds
- `--dev-setup` for environment validation
- Outputs colored build logs with binary info

**Usage**:
```bash
./build.sh                    # Debug build
./build.sh --release          # Release build (optimized, stripped)
./build.sh --clean --release  # Clean release build
./build.sh --dev-setup        # Check dependencies (Qt 6, Rust)
```

#### `run.sh` (3.11 KB)
- Executes built binary
- `--release` flag to run release build
- `--dev` flag for development mode (file watching)
- `--qml-dir <path>` to override QML directory
- **Hot-reload support**: Watches QML files with `watchexec` or `inotifywait`
- Sets `QML_DIR` and `RUST_LOG` env vars for binary

**Usage**:
```bash
./run.sh                              # Run debug binary
./run.sh --release                    # Run release binary
./run.sh --dev                        # Dev mode (watches QML)
./run.sh --qml-dir /custom/path       # Custom QML directory
./run.sh --release --dev              # Release + dev mode
```

#### `package.sh` (3.83 KB)
- **Phase 6 placeholder** with full documentation
- Supported formats: `appimage`, `aur`, `pacman`, `deb`, `rpm`
- Prints implementation roadmap for each format
- Will be implemented when ready for distribution

**Usage**:
```bash
./package.sh appimage   # Show AppImage build steps
./package.sh deb        # Show Debian build steps
./package.sh all        # List all formats
```

### Cargo Configuration

#### `Cargo.toml` (1.22 KB)
- **Binary name**: `omalook`
- **Edition**: 2021
- **Dependencies**:
  - **HTTP**: `ureq` (sync/blocking, no Tokio)
  - **OAuth2**: `oauth2` crate
  - **Keyring**: `keyring` (Linux SecretService)
  - **SQLite**: `rusqlite` with bundled SQLite + FTS5
  - **Qt**: `cxx-qt` (Qt 6.7) + `cxx` FFI
  - **Config**: `toml` (TOML parsing)
  - **File watching**: `notify` (inotify backend)
  - **Serialization**: `serde` + `serde_json`
  - **Logging**: `log` + `env_logger`
  - **Errors**: `thiserror` + `anyhow`
  - **URLs**: `urlencoding`
  - **Dates**: `chrono`

- **Release profile**: 
  - LTO enabled
  - Single codegen unit
  - Symbols stripped
  - Optimized for size/speed

### Rust Modules (6 files, 500+ LOC)

#### `src/lib.rs`
- Public module exports
- `init_logging()` function for log setup

#### `src/main.rs`
- Application entry point
- XDG config directory support (`~/.config/omalook`)
- Database and settings initialization
- Auth state check (demonstrates backend readiness)

#### `src/models.rs` (4.2 KB)
Data structures mirroring Graph API responses:
- `DeviceFlowResponse`, `TokenResponse`, `CachedToken`
- `Message`, `Recipient`, `EmailAddress`, `ItemBody`
- `CachedMessage` (SQLite schema)
- **Settings structure** with all TOML fields:
  - `FontSettings` (family, base_size, scale_factor)
  - `ColorSettings` (8 colors: bg_dark, accent_purple, etc.)
  - `UiSettings` (window size, animations)
  - `SyncSettings` (poll_interval_secs, auto_sync, cache_retention_days)

#### `src/errors.rs` (809 B)
Custom error types using `thiserror`:
- `AuthError`, `TokenError`, `HttpError`, `DatabaseError`, `KeyringError`, `SettingsError`
- `Result<T>` type alias for ergonomics

#### `src/keyring_mgr.rs` (2.4 KB)
Token caching via Linux SecretService:
- `get_cached_token()` → `Option<String>`
- `cache_token(json)` → writes to keyring
- `clear_cache()` → logout cleanup
- Graceful fallback if keyring unavailable

#### `src/auth.rs` (10.7 KB) — **Refactored from `backend/auth.py`**
Device Flow OAuth2 authentication:
- `AuthManager::new()` → `AuthManager`
- `login(callback)` → Device Flow UI + polling
- `get_token()` → Cached or refreshed access token
- `is_authenticated()` → Check cached tokens
- `logout()` → Clear tokens
- **Key methods**:
  - `acquire_device_code()` → Request user code
  - `poll_for_token()` → Poll until success/timeout/denial
  - `acquire_token_silent()` → Refresh from refresh_token
  - Token caching via keyring

#### `src/graph.rs` (5.6 KB) — **Refactored from `backend/graph_client.py`**
Microsoft Graph API client (sync, no async):
- `GraphClient::new(token)` → Client
- `get_messages(limit)` → Recent inbox messages
- `get_message(id)` → Full message body
- `mark_read(id)` → Mark as read
- `search_messages(query)` → FTS-ready search
- `send_mail(to, subject, body)` → Send email

#### `src/db.rs` (8.0 KB) — **Refactored from cached mail logic**
SQLite with FTS5 local cache:
- `Database::open(path)` → Connection + schema
- **Schema**:
  - `messages` table (id, subject, from_email, from_name, body, received_at, is_read, cached_at)
  - `messages_fts` virtual table (FTS5 with Porter tokenizer)
  - Triggers auto-sync FTS on INSERT/UPDATE/DELETE
- **Methods**:
  - `cache_message(msg)` → Graph API → SQLite
  - `get_message(id)` → Retrieve from cache
  - `get_recent_messages(limit)` → Inbox view
  - `search(query)` → FTS5 full-text search
  - `mark_read(id, is_read)` → Update read state
  - `cleanup_old_messages(days)` → Retention policy
  - `get_unread_count()` → Quick stat

#### `src/settings.rs` (4.8 KB) — **Refactored from `backend/settings.py`**
TOML configuration + file watching:
- `SettingsManager::open(path)` → Loads or creates config
- **File watching**: Auto-reloads on changes (via `notify`)
- **Methods**:
  - `get()` → Current `Settings` struct (read-only)
  - `set(new_settings)` → Update + write to disk
  - `update_sync_interval(seconds)` → Helper (enforces 10s min)
  - `get_sync_interval()` → Get current sync interval
- **TOML structure** (auto-created on first run):
  ```toml
  [font]
  family = "monospace"
  base_size = 10
  scale_factor = 1.0
  
  [color]
  bg_dark = "#0d0d0d"
  bg_surface = "#242424"
  border = "#333333"
  text_primary = "#e8e8e8"
  text_secondary = "#888888"
  accent_purple = "#7c6af7"
  danger_red = "#ff6b6b"
  success_green = "#51cf66"
  
  [ui]
  window_width = 1280
  window_height = 800
  use_tui_style = true
  animation_enabled = true
  
  [sync]
  poll_interval_secs = 60
  auto_sync = true
  cache_retention_days = 30
  ```

## Refactoring Summary

| Python Module | Rust Module | LOC | Purpose |
|---------------|-------------|-----|---------|
| `auth.py` (277 LOC) | `src/auth.rs` | 336 | Device Flow OAuth2 |
| `graph_client.py` (227 LOC) | `src/graph.rs` | 175 | Graph API client |
| `bridges.py` (144 LOC) | `src/models.rs` | 133 | Data models (QML ↔ Rust) |
| `settings.py` (257 LOC) | `src/settings.rs` | 150 | TOML config + watching |
| `main.py` (88 LOC) | `src/main.rs` | 76 | App initialization |
| **Database logic** (custom) | `src/db.rs` | 250 | SQLite + FTS5 cache |

**Total**: 993 LOC Python → ~1,200 LOC Rust (cleaner, type-safe, zero-cost abstractions)

## Architecture Decisions Made

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **HTTP** | `ureq` (sync) | No Tokio overhead; blocking I/O fine for polling |
| **Async** | None (sync only) | Local SQLite is fast; single Graph API call at a time |
| **Keyring** | `secretservice` (Linux) | Standard Linux secret storage (GNOME Keyring, KDE Wallet) |
| **Database** | SQLite + FTS5 | Local cache, full-text search, snappy performance |
| **Qt binding** | `cxx-qt` | Official Qt Rust bridge; full QML support |
| **Config** | TOML + file watching | Human-readable; auto-reload on change |
| **Binary** | Pure Rust (no Python) | Eliminates Python runtime dependency; smaller distributable |

## Next Steps (Phase 2 onwards)

### Phase 2: Qt/QML Integration
- Set up `cxx-qt` build script
- Create Rust ↔ QML bridges (signals/slots)
- Integrate `AuthManager` → login screen
- Implement `GraphClient` → message list

### Phase 3: Polling & Sync
- Background sync thread (1-min intervals, adjustable)
- Cache invalidation + cleanup
- Unread badge updates

### Phase 4: Search & Features
- FTS5 search integration
- Compose/send mail workflow
- Mark read/unread

### Phase 5: Testing & Polish
- Unit tests for each module
- Integration tests (mock Graph API)
- Error recovery

### Phase 6: Packaging
- `package.sh` full implementation
- AppImage, AUR, deb, rpm, pacman
- CI/CD pipeline

## Local Development Setup

### Prerequisites
```bash
# Debian/Ubuntu
sudo apt install qt6-base-dev qt6-qml-module-qtquick rustup

# Arch/Manjaro
sudo pacman -S qt6-base qt6-declarative rust

# Check installations
pkg-config --modversion Qt6Core
rustc --version
cargo --version
```

### First Build
```bash
cd omalook
./build.sh --dev-setup   # Validate dependencies
./build.sh --release      # Build optimized binary
./run.sh --release        # Run
```

### Development Workflow
```bash
./run.sh --dev            # Terminal 1: Run with hot-reload
# Edit QML files in terminal 2; restart app to reload
```

## Configuration

Settings are stored in `~/.config/omalook/settings.toml` and automatically reload on change.

To adjust sync interval:
```bash
# Edit settings.toml
nano ~/.config/omalook/settings.toml

# Change:
# [sync]
# poll_interval_secs = 120  (was 60)
```

Database is at `~/.config/omalook/omarchy.db` (SQLite).

## Performance Notes

- **Fast startup**: Rust binary, minimal dependencies
- **Snappy UI**: Local SQLite queries (ms), not network round-trips
- **Memory efficient**: No Python runtime, no garbage collection
- **Responsive sync**: Blocking HTTP + background thread (will upgrade to Tokio if needed for parallel API calls)

---

**Phase 1 Complete** ✅  
All Python code refactored to Rust. Ready for Phase 2: Qt/QML integration.
