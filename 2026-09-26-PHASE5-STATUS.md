# Phase 5: Email Inbox & Search — Checkpoint Status
**Date**: 2026-09-26  
**Phase**: 5 — Email Inbox & Search  
**Status**: 🔄 IN PROGRESS — Backend complete, QML inbox display not yet built

---

## Session Summary (2026-09-26 evening)

This session resolved the authentication chain and achieved first successful email sync
from Microsoft Graph API into the local SQLite database. The backend is now fully
operational end-to-end.

---

## What Was Accomplished Today

### ✅ Microsoft Graph API — 403 Root Cause Diagnosed and Fixed

The email daemon was permanently returning `403 Forbidden` on every inbox fetch. Root
cause chain (in order of discovery):

1. **Wrong client ID**: App used `04b07795-8ddb-461a-bbee-02f9e1bf7b46` (Microsoft's
   Azure CLI app), which has no `Mail.Read` permission. This always produces 403 on
   `/me/mailFolders/inbox/messages`.

2. **Wrong scope format**: `https://graph.microsoft.com/.default` expands to the
   app's pre-consented roles — for any third-party app this never includes Mail scopes.

3. **Stale refresh token**: After fixing the client ID in source, the keyring still
   held a refresh token issued for the old Azure CLI app. Silent refresh silently
   exchanged it for a new access token — but for the wrong app. Token `appid` field
   confirmed: `04b07795-...` still in use even after code change.

4. **Device Flow 401**: New Azure app registration had "Allow public client flows"
   disabled by default. Device Flow is a public client flow; endpoint returned
   `AADSTS70002: client must be marked as mobile`.

5. **ureq 2.x error handling bug**: Non-2xx responses from `send_form()` return
   `Err(ureq::Error::Status(code, response))`, not `Ok(response)`. The existing
   error handler used `.map_err(|e| ...)` which swallowed the response body, making
   all error diagnostics useless. Added proper match arms to log full error body.

6. **Stale `auth_state.json`**: Failed auth runs write error JSON to this file.
   On restart, QML's `authStatePoller` (runs every 500ms) reads the file and floods
   the log with the old error indefinitely. Fixed by clearing this file on login
   trigger and on manual `rm`.

7. **Symlink pointed to debug binary**: `./omalook` → `target/debug/omalook`
   (set at project init). Source changes only applied to release build. Fixed:
   `ln -sf target/release/omalook omalook`.

### ✅ Azure App Registration — omalook (permanent)

One-time developer registration covering all future users:

| Field | Value |
|---|---|
| **App name** | omalook |
| **Client ID** | `9c277d6f-edb2-4f82-bda5-901b4c11c457` |
| **Account types** | Multi-tenant + personal Microsoft accounts |
| **Public client flows** | Enabled (required for Device Flow) |
| **Delegated permissions** | Mail.Read, Mail.ReadWrite, Mail.Send, User.Read, offline_access |
| **User burden** | One-time consent screen on first login only; zero Azure interaction |

Scopes in `src/auth.rs`:
```
https://graph.microsoft.com/Mail.Read
https://graph.microsoft.com/Mail.ReadWrite
https://graph.microsoft.com/Mail.Send
https://graph.microsoft.com/User.Read
offline_access
```

### ✅ First Successful Email Sync

```
[INFO] Fetched 10 emails from Graph API inbox
[INFO] Successfully synced 10 new emails
```

Confirmed emails in DB:
- Anthropic receipts, Amazon delivery notifications, NC-300 Attendance Bot, etc.
- DB: `~/.config/omalook/messages.db` — table `messages`, 10 rows
- FTS5 index: `messages_fts` with Porter stemmer — ready for search

### ✅ Graph API Error Instrumentation Added

`src/providers/graph.rs` now logs the full error body and token prefix on any non-2xx
response, making future auth/permission debugging straightforward.

---

## Current State

### What Works
- ✅ Device Flow authentication (multi-tenant, personal accounts)
- ✅ Silent token refresh on restart
- ✅ Graph API inbox fetch (10 emails per poll, every 120 seconds)
- ✅ SQLite persistence (`messages` table + FTS5 index with Porter stemmer)
- ✅ Release binary: **12MB** (target: <15MB ✅)
- ✅ Memory at rest: ~33MB RSS (target: <100MB ✅)
- ✅ QML auth flow: Device Flow modal, login/logout, state polling
- ✅ HTTP trigger server (localhost:27182) for QML→Rust IPC

### What Is Not Yet Built (Phase 5 Remaining)
- ❌ QML inbox list view (messages not displayed in UI yet)
- ❌ Message detail view
- ❌ Full-text search UI (FTS5 backend exists, no frontend)
- ❌ Mark read/unread action
- ❌ Delete message action
- ❌ Sender/subject filtering UI

---

## Database Schema (current)

```sql
CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    subject TEXT NOT NULL,
    from_email TEXT NOT NULL,
    from_name TEXT,
    body TEXT NOT NULL,
    received_at DATETIME NOT NULL,
    is_read BOOLEAN DEFAULT 0,
    cached_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- FTS5 virtual table with Porter stemmer
CREATE VIRTUAL TABLE messages_fts USING fts5(
    id UNINDEXED,
    subject,
    from_email UNINDEXED,
    from_name,
    body,
    tokenize = 'porter'
);
-- Triggers keep messages_fts in sync with messages on INSERT/UPDATE/DELETE
```

---

## Key Files Changed This Session

| File | Change |
|---|---|
| `src/auth.rs` | New client ID (`9c277d6f-...`), explicit Graph scopes, `error` log import, ureq non-2xx error handling with body logging |
| `src/providers/graph.rs` | Full error body + token prefix logged on 403/non-2xx |
| `src/main.rs` | HTTP login trigger clears stale `auth_state.json` before starting Device Flow |
| `omalook` (symlink) | Fixed: now points to `target/release/omalook` |

---

## Performance Baselines (measured)

| Metric | Measured | Target | Status |
|---|---|---|---|
| Release binary size | 12MB | <15MB | ✅ |
| Memory at rest (RSS) | ~33MB | <100MB | ✅ |
| Email fetch latency | ~1s (10 emails) | — | ✅ |
| Startup time | <1s | <1s | ✅ |
| Search latency | not yet tested (no UI) | <50ms | ⏳ |

---

## Next Session: Phase 5 Remaining Work

1. **QML inbox list view** — `ListView` of messages from `messages.db` via XHR/HTTP bridge
2. **Message detail view** — expand on click, show full body
3. **Search bar** — wire to FTS5 `messages_fts` via a new `/search?q=` HTTP endpoint
4. **Mark read/unread** — POST to HTTP trigger → UPDATE `is_read` in DB
5. **Delete** — POST to HTTP trigger → DELETE from DB + messages_fts

### HTTP Endpoints to Add (for QML→Rust bridge)
```
GET  /messages?limit=50&offset=0    → inbox list JSON
GET  /messages/:id                  → single message JSON  
GET  /search?q=<term>               → FTS5 search results JSON
POST /messages/:id/read             → mark read
POST /messages/:id/unread           → mark unread
DELETE /messages/:id                → delete message
```

---

## Troubleshooting Reference

**Clear auth state (when stuck)**:
```bash
uv run --with keyring python3 -c "import keyring; keyring.delete_password('omalook', 'auth_cache')"
rm -f ~/.config/omalook/auth_state.json ~/.config/omalook/device_code.json
```

**Verify token scopes after auth**:
```bash
uv run --with keyring python3 -c "
import keyring, json, base64
val = keyring.get_password('omalook', 'auth_cache')
data = json.loads(val)
token = data['access_token']
parts = token.split('.')
payload = parts[1] + '=' * (4 - len(parts[1]) % 4)
d = json.loads(base64.urlsafe_b64decode(payload))
print('scp:', d.get('scp'))
print('appid:', d.get('appid'))
"
```
Expected: `appid: 9c277d6f-edb2-4f82-bda5-901b4c11c457`, `scp` includes `Mail.Read`.

**Query inbox DB**:
```bash
sqlite3 ~/.config/omalook/messages.db "SELECT subject, from_email, received_at FROM messages ORDER BY received_at DESC LIMIT 10;"
```
