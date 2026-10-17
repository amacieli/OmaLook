# omalook Component Reference Guide

**Generated:** 2026-09-26  
**Project:** omalook (Omarchy-style Outlook clone with Rust backend + Qt QML frontend)  
**Location:** /mnt/ai/projects/omalook/

---

## Quick Reference Table

| Component | Version | Type | Docs | Notes |
|-----------|---------|------|------|-------|
| Rust | 1.98.1 | Toolchain | [rust-lang.org](https://doc.rust-lang.org/) | Edition 2021 |
| Cargo | 1.98.1 | Package Mgr | [cargo](https://doc.rust-lang.org/cargo/) | Dependency mgmt |
| Qt Base | 6.11.2 | Framework | [doc.qt.io](https://doc.qt.io/qt-6/) | Core framework |
| Qt Declarative | 6.11.2 | QML Runtime | [QML Docs](https://doc.qt.io/qt-6/qmlmodule.html) | UI layer |
| CMake | 4.4.3 | Build System | [cmake.org](https://cmake.org/) | Qt integration |
| SQLite | 3.53.4 | Database | [sqlite.org](https://www.sqlite.org/) | Bundled with rusqlite |
| cxx-qt | 0.10 | Rust↔Qt FFI | [cxx-qt.github.io](https://cxx-qt.github.io/) | Bridge layer |
| cxx | 1.0 | Rust↔C++ FFI | [cxx.rs](https://cxx.rs/) | Underlying FFI |
| ureq | 2.9 | HTTP Client | [docs.rs](https://docs.rs/ureq/latest/ureq/) | Graph API calls |
| oauth2 | 4.4 | OAuth2 | [docs.rs](https://docs.rs/oauth2/latest/oauth2/) | Device Flow auth |
| rusqlite | 0.30 | SQLite Bindings | [docs.rs](https://docs.rs/rusqlite/latest/rusqlite/) | Database access |
| zbus | 3.15 | D-Bus (KDE Connect SMS) | [docs.rs](https://docs.rs/zbus/3.15.2/zbus/) | Blocking API; already in tree via keyring |
| keyring | 2.1 | Keyring | [docs.rs](https://docs.rs/keyring/latest/keyring/) | SecretService (Linux) |
| Microsoft Graph | v1.0 | External API | [learn.microsoft.com](https://learn.microsoft.com/en-us/graph/) | Mail, user data |

---

## Rust Ecosystem

### Rust Toolchain

**Version:** 1.98.1  
**Release Date:** 2026-09-01

**Documentation Links:**
- [Official Docs](https://doc.rust-lang.org/)
- [Standard Library API](https://doc.rust-lang.org/std/)

**Notes:** Edition 2021 used in project

### Cargo

**Version:** 1.98.1  
**Release Date:** 2026-08-05

**Documentation Links:**
- [Cargo Guide](https://doc.rust-lang.org/cargo/)
- [Crates.io Registry](https://docs.rs/)

**Notes:** Package and dependency manager

---

## Qt Framework

### Qt Base

**Version:** 6.11.2  
**System Package:** Arch Linux (qt6-base 6.11.2-3)

**Documentation Links:**
- [Qt 6 Documentation](https://doc.qt.io/qt-6/)
- [Class Reference](https://doc.qt.io/qt-6/classes.html)
- [QML Modules](https://doc.qt.io/qt-6/qmlmodule.html)
- [Porting Guide](https://doc.qt.io/qt-6/topics-porting.html)

**Notes:** Core Qt framework, QML support via qt6-declarative 6.11.2-1

### Qt Declarative (QML)

**Version:** 6.11.2  
**System Package:** qt6-declarative 6.11.2-1

**Documentation Links:**
- [Qt Quick Overview](https://doc.qt.io/qt-6/qtquick-index.html)
- [QML Index](https://doc.qt.io/qt-6/qml-index.html)
- [QML Language Reference](https://doc.qt.io/qt-6/qmlreference.html)

**Notes:** QML language and Qt Quick controls, enables QML-based UI

### Qt NetworkAuth

**Version:** 6.11.2  
**System Package:** qt6-networkauth 6.11.2-1

**Documentation Links:**
- [Module Documentation](https://doc.qt.io/qt-6/qtnetworkauth-index.html)
- [OAuth2 Authorization](https://doc.qt.io/qt-6/qtoauth2authorizationserver.html)

**Notes:** OAuth2 support for Qt (not currently used; using oauth2 crate instead)

---

## Build Tools

### CMake

**Version:** 4.4.3

**Documentation Links:**
- [CMake Documentation](https://cmake.org/cmake/help/v3.24/)
- [CMake Reference](https://cmake.org/cmake/help/v3.24/manual/cmake.1.html)
- [Qt with CMake Guide](https://cmake.org/cmake/help/latest/guide/using-qt-with-cmake.html)

**Notes:** Cross-platform build system; Qt 6.11.2 uses CMake for integration

---

## Rust Crate Dependencies

### cxx-qt

**Version:** 0.10  
**Crates.io:** https://crates.io/crates/cxx-qt  
**GitHub:** https://github.com/cxx-qt/cxx-qt

**Documentation Links:**
- [cxx-qt Book](https://cxx-qt.github.io/)
- [API Documentation](https://docs.rs/cxx-qt/latest/cxx_qt/)

**Notes:** Safe Rust↔C++ FFI with Qt bindings; enables QML bridge layer. **CRITICAL for Phase 2.**

### cxx

**Version:** 1.0  
**Crates.io:** https://crates.io/crates/cxx  
**GitHub:** https://github.com/dtolnay/cxx

**Documentation Links:**
- [cxx.rs](https://cxx.rs/)
- [API Documentation](https://docs.rs/cxx/latest/cxx/)

**Notes:** Safe FFI bridge between Rust and C++; underlying transport for cxx-qt

### cxx-qt-build

**Version:** 0.10  
**Crates.io:** https://crates.io/crates/cxx-qt-build

**Documentation Links:**
- [cxx-qt Book](https://cxx-qt.github.io/)
- [API Documentation](https://docs.rs/cxx-qt-build/latest/cxx_qt_build/)

**Notes:** Build-time support for cxx-qt; configures Qt integration in build.rs

### cxx-build

**Version:** 1.0  
**Crates.io:** https://crates.io/crates/cxx-build

**Documentation Links:**
- [cxx.rs](https://cxx.rs/)
- [API Documentation](https://docs.rs/cxx-build/latest/cxx_build/)

**Notes:** Build script support for cxx FFI layer

### ureq

**Version:** 2.9  
**Crates.io:** https://crates.io/crates/ureq  
**GitHub:** https://github.com/algesten/ureq  
**Features:** json

**Documentation Links:**
- [Main Documentation](https://docs.rs/ureq/latest/ureq/)
- [API Reference (2.9.0)](https://docs.rs/ureq/2.9.0/ureq/)

**Notes:** Synchronous HTTP client; used for Microsoft Graph API calls

### oauth2

**Version:** 4.4  
**Crates.io:** https://crates.io/crates/oauth2  
**GitHub:** https://github.com/ramosbugs/oauth2-rs

**Documentation Links:**
- [Main Documentation](https://docs.rs/oauth2/latest/oauth2/)
- [API Reference (4.4.0)](https://docs.rs/oauth2/4.4.0/oauth2/)

**Notes:** OAuth2 Device Flow implementation for Microsoft Graph authentication

### rusqlite

**Version:** 0.30  
**Crates.io:** https://crates.io/crates/rusqlite  
**GitHub:** https://github.com/rusqlite/rusqlite  
**Features:** bundled, chrono

**Documentation Links:**
- [Main Documentation](https://docs.rs/rusqlite/latest/rusqlite/)
- [API Reference (0.30.0)](https://docs.rs/rusqlite/0.30.0/rusqlite/)

**Notes:** SQLite bindings; bundled feature includes SQLite 3.53.4 build

### chrono

**Version:** 0.4  
**Crates.io:** https://crates.io/crates/chrono  
**GitHub:** https://github.com/chronotope/chrono  
**Features:** serde

**Documentation Links:**
- [Main Documentation](https://docs.rs/chrono/latest/chrono/)
- [API Reference (0.4.0)](https://docs.rs/chrono/0.4.0/chrono/)

**Notes:** Date/time handling; integrates with rusqlite for timestamp storage

### serde & serde_json

**Version:** 1.0  
**Crates.io:** https://crates.io/crates/serde  
**GitHub:** https://github.com/serde-rs/serde

**Documentation Links:**
- [Serde Overview](https://serde.rs/)
- [Serde API](https://docs.rs/serde/latest/serde/)
- [JSON API](https://docs.rs/serde_json/latest/serde_json/)

**Notes:** Serialization framework; JSON parsing for Graph API responses

### zbus
**Docs:** https://docs.rs/zbus/3.15.2/zbus/ · **Crates.io:** https://crates.io/crates/zbus  
**Notes:** Used by `sms_daemon.rs` (blocking `Connection`/`MessageIterator`) to talk to `kdeconnectd` on the session bus. SMS design, schema and routes: `docs/SMS.md`.

### keyring

**Version:** 2.1  
**Crates.io:** https://crates.io/crates/keyring  
**GitHub:** https://github.com/hwchen/keyring-rs

**Documentation Links:**
- [Main Documentation](https://docs.rs/keyring/latest/keyring/)
- [API Reference (2.1.0)](https://docs.rs/keyring/2.1.0/keyring/)

**Notes:** Cross-platform keyring storage; Linux uses SecretService (D-Bus)

### notify

**Version:** 6.1  
**Crates.io:** https://crates.io/crates/notify  
**GitHub:** https://github.com/notify-rs/notify

**Documentation Links:**
- [Main Documentation](https://docs.rs/notify/latest/notify/)
- [API Reference (6.1.0)](https://docs.rs/notify/6.1.0/notify/)

**Notes:** File system watcher; monitors settings.toml for live reload

### log & env_logger

**Version:** 0.4 & 0.11  
**GitHub:** https://github.com/rust-lang/log

**Documentation Links:**
- [Log Crate](https://docs.rs/log/latest/log/)
- [Env Logger API](https://docs.rs/env_logger/latest/env_logger/)

**Notes:** Logging infrastructure; RUST_LOG env var controls output

### anyhow & thiserror

**Version:** 1.0

**Documentation Links:**
- [Anyhow API](https://docs.rs/anyhow/latest/anyhow/)
- [Thiserror API](https://docs.rs/thiserror/latest/thiserror/)

**Notes:** Error handling; anyhow for Result wrapping, thiserror for custom errors

### toml

**Version:** 0.8  
**Crates.io:** https://crates.io/crates/toml  
**GitHub:** https://github.com/toml-rs/toml

**Documentation Links:**
- [Main Documentation](https://docs.rs/toml/latest/toml/)
- [API Reference (0.8.0)](https://docs.rs/toml/0.8.0/toml/)

**Notes:** TOML parsing for settings.toml configuration files

### url & urlencoding

**Version:** 2.5 & 2.1

**Documentation Links:**
- [URL API](https://docs.rs/url/latest/url/)
- [URLEncoding API](https://docs.rs/urlencoding/latest/urlencoding/)

**Notes:** URL parsing and encoding for OAuth2 and Graph API requests

---

## External APIs

### Microsoft Graph API

**Version:** v1.0 (latest)  
**API Endpoint:** https://graph.microsoft.com/v1.0

**Documentation Links:**
- [Graph Overview](https://learn.microsoft.com/en-us/graph/)
- [API Reference](https://learn.microsoft.com/en-us/graph/api/overview)
- [Authentication Concepts](https://learn.microsoft.com/en-us/graph/auth/auth-concepts)
- [Message Resource](https://learn.microsoft.com/en-us/graph/api/resources/message?view=graph-rest-1.0)
- [mailFolder Resource](https://learn.microsoft.com/en-us/graph/api/resources/mailfolder?view=graph-rest-1.0)
- [List mailFolders](https://learn.microsoft.com/en-us/graph/api/user-list-mailfolders?view=graph-rest-1.0)
- [List messages in folder](https://learn.microsoft.com/en-us/graph/api/mailfolder-list-messages?view=graph-rest-1.0)
- [User Resource](https://learn.microsoft.com/en-us/graph/api/resources/user)
- [User OAuth2 Flow](https://learn.microsoft.com/en-us/graph/auth-v2-user)
- [Device Code Flow (CRITICAL)](https://learn.microsoft.com/en-us/graph/auth-oauth-device-code)
- [OData Query Parameters](https://learn.microsoft.com/en-us/graph/query-parameters)

**mailFolder resource — official v1.0 properties** (from docs, last updated 2024-08-23):

| Property | Type | Selectable | Description |
|---|---|---|---|
| id | String | ✅ | Unique identifier |
| displayName | String | ✅ | Display name |
| parentFolderId | String | ✅ | Parent folder ID |
| childFolderCount | Int32 | ✅ | Number of child folders |
| totalItemCount | Int32 | ✅ | Total items in folder |
| unreadItemCount | Int32 | ✅ | Unread item count |
| isHidden | Boolean | ✅ | Whether folder is hidden |

> ⚠️ **CRITICAL**: `wellKnownName` is **NOT** a v1.0 property. It exists in beta only and is unreliable. Using it in `$select` causes a 400 Bad Request. Well-known names (inbox, drafts, sentitems, etc.) are **URL path segments only** — use them in the URL like `/me/mailFolders/inbox`, not as a field to read back from the API. We infer them locally from `displayName` via `infer_well_known_name()` in `graph.rs`.

**Well-known folder names** (use as URL path segments, not API fields):

| Name | Description |
|---|---|
| `inbox` | Inbox |
| `drafts` | Drafts |
| `sentitems` | Sent Items |
| `deleteditems` | Deleted Items |
| `junkemail` | Junk Email |
| `archive` | Archive |
| `outbox` | Outbox |
| `clutter` | Clutter (low-priority) |
| `conversationhistory` | Skype IM history |
| `msgfolderroot` | Root of all mail folders |
| `recoverableitemsdeletions` | Soft-deleted items |

**message resource — official v1.0 properties used in omalook:**

| Property | Type | Notes |
|---|---|---|
| id | String | Unique; changes on folder move unless `Prefer: IdType="ImmutableId"` |
| subject | String | |
| from | recipient | Object: `{ emailAddress: { name, address } }` |
| receivedDateTime | DateTimeOffset | ISO 8601 UTC |
| bodyPreview | String | First 255 chars, plain text |
| isRead | Boolean | |
| parentFolderId | String | ID of containing folder |
| isDraft | Boolean | |
| hasAttachments | Boolean | Does not include inline attachments |
| importance | String | `low`, `normal`, `high` |
| conversationId | String | |

**Valid `$select` fields for messages:** `id,subject,from,receivedDateTime,bodyPreview,isRead,parentFolderId,isDraft,hasAttachments,importance,conversationId`

**Valid `$select` fields for mailFolders:** `id,displayName,parentFolderId,childFolderCount,totalItemCount,unreadItemCount,isHidden`

**Endpoint Examples:**
- List folders: `GET /me/mailFolders?$top=100&$select=id,displayName,parentFolderId,unreadItemCount,totalItemCount`
- List messages in folder: `GET /me/mailFolders/{id}/messages?$top=10&$select=id,subject,from,receivedDateTime,bodyPreview,isRead,parentFolderId`
- List messages (well-known): `GET /me/mailFolders/inbox/messages?$top=10`
- Include hidden folders: `GET /me/mailFolders?includeHiddenFolders=true`
- Send message: `POST /me/sendMail`
- Get user profile: `GET /me`

**Notes:**
- Graph API v1.0 is the stable channel; avoid beta for production features
- `$select` reduces response payload — always use it; invalid fields cause 400 Bad Request
- `wellKnownName` was a beta-only field, never in v1.0; do not use it in `$select`
- List mailFolders only returns **direct children of root** by default; use `/childFolders` to recurse
- Default response excludes hidden folders; add `?includeHiddenFolders=true` to include them
- `parentFolderId` on messages maps to `id` on the containing mailFolder

### Azure AD

**Version:** v1.0 (Microsoft identity platform)

**Documentation Links:**
- [Azure AD Development](https://learn.microsoft.com/en-us/azure/active-directory/develop/)
- [OAuth2/OpenID Connect](https://learn.microsoft.com/en-us/azure/active-directory/develop/identity-protocols-openid-connect-code)
- [Device Code Flow Spec](https://learn.microsoft.com/en-us/azure/active-directory/develop/v2-oauth2-device-code)

**Notes:** OAuth2 provider for Microsoft Graph; Device Code Flow for user authentication

---

## Database

### SQLite

**Version:** 3.53.4  
**Release Date:** 2026-07-24

**Documentation Links:**
- [SQLite Home](https://www.sqlite.org/)
- [C API Reference](https://www.sqlite.org/cintro.html)
- [FTS5 Full-Text Search](https://www.sqlite.org/fts5.html)
- [Best Practices](https://www.sqlite.org/bestpractice.html)

**Features Used:**
- FTS5 for full-text search on email bodies
- JSON1 for flexible settings storage
- Common Table Expressions (CTEs) for complex queries

**Notes:** Bundled with rusqlite; FTS5 support for mail search

---

## System Libraries

### Linux SecretService

**Version:** Latest (freedesktop.org spec)

**Documentation Links:**
- [SecretService Specification](https://specifications.freedesktop.org/secret-service/latest/)
- [D-Bus API](https://specifications.freedesktop.org/secret-service/latest/Secret.Service.html)

**Notes:** D-Bus-based secret storage used by keyring crate on Linux for OAuth2 token storage

### OpenSSL

**Version:** 3.x (system default)

**Documentation Links:**
- [OpenSSL Documentation](https://www.openssl.org/docs/)
- [libssl Reference](https://www.openssl.org/docs/man3.0/man3/ssl.html)

**Notes:** TLS/SSL for HTTPS connections in ureq HTTP client

---

## Usage Guide by Phase

### Phase 1: Backend Infrastructure (COMPLETE ✅)
- **Rust 1.98.1:** Core business logic, models, database
- **SQLite 3.53.4:** Email cache, user settings
- **oauth2 4.4:** Microsoft Graph authentication (Device Code Flow)
- **ureq 2.9:** HTTP requests to Graph API
- **keyring 2.1:** Token storage in SecretService
- **serde/serde_json 1.0:** JSON parsing from Graph API
- **rusqlite 0.30:** Database access with FTS5

### Phase 2: Qt Integration (IN PROGRESS 🔄)
- **Qt Base 6.11.2:** Core Qt framework
- **Qt Declarative 6.11.2:** QML UI framework
- **CMake 4.4.3:** Build configuration for Qt 6
- **cxx-qt 0.10:** Rust↔C++ bridge layer for QML context objects
- **cxx-qt-build 0.10 + cxx-build 1.0:** Compile-time macro support

### Phase 3: QML UI & Finishing (PLANNED 📋)
- **Qt Declarative 6.11.2:** Full UI implementation
- **notify 6.1:** File system watcher for settings hot-reload
- **log/env_logger 0.4/0.11:** Runtime diagnostics

---

## API Documentation Cheat Sheet

### Microsoft Graph Authentication (Device Code Flow)
1. **Device Code Flow Spec:** https://learn.microsoft.com/en-us/graph/auth-oauth-device-code
2. **Azure AD Device Code:** https://learn.microsoft.com/en-us/azure/active-directory/develop/v2-oauth2-device-code
3. **Rust oauth2 crate:** https://docs.rs/oauth2/latest/oauth2/

**Quick Flow:**
```
1. Request device code: POST /devicecode
2. Display code to user
3. Poll token endpoint while user authenticates
4. Receive access token (store in keyring)
5. Use token with ureq for Graph API calls
```

### Microsoft Graph Mail API
1. **Message Resource:** https://learn.microsoft.com/en-us/graph/api/resources/message
2. **List Messages:** https://learn.microsoft.com/en-us/graph/api/user-list-messages
3. **Send Mail:** https://learn.microsoft.com/en-us/graph/api/user-sendmail

**Common Endpoints:**
- `GET /me/mailFolders/inbox/messages` — List inbox
- `GET /me/messages/{id}` — Get message details
- `POST /me/sendMail` — Send message
- `GET /me` — Get user profile

### Qt QML & C++ Integration
1. **Qt Quick Overview:** https://doc.qt.io/qt-6/qtquick-index.html
2. **QML Language Reference:** https://doc.qt.io/qt-6/qmlreference.html
3. **cxx-qt Book (REQUIRED for Phase 2):** https://cxx-qt.github.io/
4. **cxx.rs FFI:** https://cxx.rs/

### SQLite Full-Text Search
1. **FTS5 Documentation:** https://www.sqlite.org/fts5.html
2. **rusqlite docs:** https://docs.rs/rusqlite/latest/rusqlite/
3. **SQLite Best Practices:** https://www.sqlite.org/bestpractice.html

### Linux SecretService (Keyring)
1. **SecretService Spec:** https://specifications.freedesktop.org/secret-service/latest/
2. **Keyring crate:** https://docs.rs/keyring/latest/keyring/
3. **D-Bus Specification:** https://dbus.freedesktop.org/doc/dbus-daemon.1.html

---

## Version Compatibility Matrix

| Layer | Component | Version | Compatibility | Status | Verified |
|-------|-----------|---------|----------------|--------|----------|
| Language | Rust | 1.98.1 | Edition 2021 | ✅ Active | 2026-09-26 |
| Package Mgr | Cargo | 1.98.1 | Latest | ✅ Active | 2026-09-26 |
| UI Framework | Qt | 6.11.2 | All modules compatible | ✅ Verified | 2026-09-26 |
| UI Language | QML | Qt 6.11.2 | Qt6 standard | ✅ Verified | 2026-09-26 |
| FFI Bridge | cxx-qt | 0.10 | Qt 6.11.2 ↔ Rust 1.98 | ✅ Verified | 2026-09-26 |
| FFI Transport | cxx | 1.0 | Rust 1.98 | ✅ Verified | 2026-09-26 |
| Build System | CMake | 4.4.3 | Qt 6 standard | ✅ Verified | 2026-09-26 |
| Database | SQLite | 3.53.4 | FTS5 support | ✅ Bundled | 2026-09-26 |
| DB Bindings | rusqlite | 0.30 | SQLite 3.53.4 | ✅ Verified | 2026-09-26 |
| Auth | oauth2 | 4.4 | Device Code Flow | ✅ Verified | Phase 1 |
| HTTP | ureq | 2.9 | JSON + TLS | ✅ Verified | Phase 1 |
| Secrets | keyring | 2.1 | SecretService (Linux) | ✅ System ready | Phase 1 |
| Graph API | Microsoft Graph | v1.0 | Latest | ✅ Available | Phase 1+ |

---

## How to Use This Guide

1. **Before implementing a feature:**
   - Check the version and docs link in this file
   - Read the relevant documentation link
   - Verify compatibility matrix status

2. **API lookup:**
   - Use the "API Documentation Cheat Sheet" section
   - Look up endpoint specifications in the docs

3. **Troubleshooting:**
   - Cross-reference version numbers with GitHub issues
   - Check the crates.io page for known issues
   - Search the official documentation

4. **Updating dependencies:**
   - Check compatibility matrix before bumping versions
   - Test with `cargo check` and `cargo build`
   - Review CHANGELOG.md on GitHub for breaking changes

### Example: Adding a new Microsoft Graph endpoint
1. Consult: [Microsoft Graph API Reference](https://learn.microsoft.com/en-us/graph/api/overview)
2. Review: [ureq 2.9 docs](https://docs.rs/ureq/2.9.0/ureq/) for HTTP patterns
3. Verify: Compatibility with oauth2 4.4 (already verified)
4. Test: `cargo check` before committing

### Example: Creating a new QML component
1. Consult: [QML Language Reference](https://doc.qt.io/qt-6/qmlreference.html)
2. Review: [cxx-qt Book](https://cxx-qt.github.io/) for Rust↔QML binding patterns
3. Check: Qt 6.11.2 API docs for Qt Quick controls
4. Test: `cargo check` and manual QML validation

---

## Quick Dependency Check Commands

```bash
# Check Rust version
rustc --version
cargo --version

# Check Qt version (system)
qmake --version
pacman -Q | grep qt6

# Check CMake version
cmake --version

# Check SQLite version
sqlite3 --version

# List all Rust dependencies with versions
cargo tree --depth=1

# Check dependency updates (if outdated installed)
cargo outdated

# Generate full dependency tree
cargo tree
```

---

## Useful Resources by Topic

### Microsoft Graph Basics
- [Graph Overview](https://learn.microsoft.com/en-us/graph/)
- [Getting Started Guide](https://learn.microsoft.com/en-us/graph/get-started)
- [Authentication Patterns](https://learn.microsoft.com/en-us/graph/auth/auth-concepts)
- [REST API Guidelines](https://github.com/microsoft/api-guidelines/blob/vNext/Guidelines.md)

### Rust + Qt Development
- **[cxx-qt Book (REQUIRED for Phase 2)](https://cxx-qt.github.io/)** — Start here for macro syntax
- [Qt Documentation](https://doc.qt.io/qt-6/)
- [Rust FFI Guide](https://doc.rust-lang.org/nomicon/ffi.html)
- [cxx.rs Examples](https://github.com/dtolnay/cxx/tree/master/examples)

### Database Design
- [SQLite Best Practices](https://www.sqlite.org/bestpractice.html)
- [FTS5 Full-Text Search](https://www.sqlite.org/fts5.html)
- [rusqlite Examples](https://github.com/rusqlite/rusqlite/tree/master/examples)
- [SQLite JSON1 Functions](https://www.sqlite.org/json1.html)

### System Integration
- [D-Bus Specification](https://dbus.freedesktop.org/doc/)
- [SecretService Specification](https://specifications.freedesktop.org/secret-service/latest/)
- [Freedesktop Standards](https://specifications.freedesktop.org/)

### OAuth2 & Authentication
- [OAuth 2.0 RFC 6749](https://tools.ietf.org/html/rfc6749)
- [Device Code Flow RFC 8628](https://tools.ietf.org/html/rfc8628)
- [OpenID Connect Spec](https://openid.net/connect/)

---

## Last Updated

- **Date:** 2026-09-26
- **Phase:** 2 (Scaffolding Complete)
- **Git Commit:** Phase 2 completion
- **Next Review:** Before Phase 2 continuation (cxx-qt macro implementation)

**Maintenance Notes:**
- This file should be updated whenever a major dependency is added, upgraded, or removed
- Compatibility matrix should be verified before upgrading versions
- API documentation links should be checked annually for rot
- Phase/feature columns should be updated as work progresses

---

## Contributing to This Guide

When you update a dependency:
1. Update version number in this file
2. Add release date and compatibility notes
3. Test with `cargo check` and document results
4. Update the compatibility matrix status
5. Commit changes with the dependency upgrade

Example commit message:
```
Upgrade oauth2 from 4.4 to 4.5

- Updated COMPONENT_REFERENCE.md with new version
- Verified compatibility with ureq 2.9 and Rust 1.98.1
- All tests passing
- No breaking changes in oauth2 API
```
