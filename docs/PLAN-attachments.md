# Plan: reading attachments (list, view, save)

Status: BUILT for Graph 2026-10-11 (Gmail later). Decisions (Adam): `A` picker, `S` save all (no collisions in the registry); status-bar
line only after saving, naming the folder; inline images hidden and not counted as attachments; collisions auto-renamed `name (1).ext`;
flat into Downloads.

Deviations from the plan below, made while building:
- Picker is the existing command palette (`pick:` + new `keepOpen` rows), not a new popup: Enter saves, check marks, type-to-filter. No Space-marking
  (the "Save all" row covers it) and no `i` inline toggle (inline stays hidden).
- Save is one request that returns when done (no job/status polling). The attachments are cached in `attachment_cache` (JSON + `has_real`).
- Graph rejects `contentId` in `$select` (400), so only `isInline` is used on the wire; `cid:` matching against the cached HTML body remains as a second check.
- The paperclip fix lives in `attachment_cache.has_real`, which the list query prefers over `messages.has_attachments` (a later sync cannot undo it).
  The list row updates the next time the list reloads after the message was opened.
Scope: received mail only, Graph + Gmail. Sending attachments stays in `PLAN-compose.md`.
Non-goals (per Adam): no previews, no archive/zip. Opening a file is the OS's job.

## What exists today
- `messages.has_attachments` is synced (Graph `hasAttachments`; Gmail heuristic: top-level `multipart/mixed`).
- List row already shows a cyan paperclip (`MessageList.qml:170`). Reading pane shows a dim "attachments" label (`MessagePreview.qml:400`), not interactive.
- No attachment metadata, no download path, no endpoints. Body is fetched on open via `fetch_message_body` (Graph + Gmail), served by `GET /messages/body`.
- Keyboard model: command registry in `qml/keys/AppCommands.qml` (scopes, `when`, `pick:` palette pickers, `ex:` palette names); `t` (category picker) is the precedent for a keyboard popup on a message.

## Design

### 1. Data: fetch lazily, like the body
Do NOT add attachment listing to the sync path (cost on every message). On message open, fetch the list once and cache.
- New table `attachments(message_id, att_id, name, size, content_type, is_inline, kind)`, rows replaced per message.
- Graph: `GET /me/messages/{id}/attachments?$select=id,name,size,contentType,isInline`. Kinds: `file` (downloadable), `item` (attached email/event) and `reference` (OneDrive link) shown but flagged. v1 saves `file` only; `item` saved as `.eml` via `/$value` if it works, else shown as unsupported.
- Gmail: walk `payload.parts` recursively for parts with a `filename` and `body.attachmentId` (`format=full`, same call the body already makes, so zero extra request).
- Inline images (`cid:` signature logos, embedded pictures) are hidden by default; the count and the list ignore them. A key in the picker toggles "show inline".
- Fix the heuristic: after the first open, correct `has_attachments` from the real non-inline count, so the paperclip stops lying (signature logos on Graph, `multipart/related` and forwarded mail on Gmail).

### 2. Download: daemon does it, UI only asks
- `GET /messages/attachments?id=` -> `[{att_id,name,size,type,inline,kind}]`
- `POST /messages/attachments/save?id=&att=ID` (one) or `&all=1` (every non-inline file) -> `{saved:[paths], failed:[{name,error}]}`
- Graph: `/attachments/{id}/$value` (raw bytes, no base64 bloat). Gmail: `messages/{id}/attachments/{attId}` (base64url, decode).
- Stream to a `.part` file, rename on success; big files do not sit in memory. Long saves return a job id polled with `GET /messages/attachments/status` (same pattern as `/compose/status`) so the UI never blocks.
- Destination: `dirs::download_dir()` (reads `~/.config/user-dirs.dirs`), fallback `~/Downloads`; override `[attachments] download_dir` in `settings.toml`. `dirs` is already a dependency.
- Safety: strip path separators and control chars from names (no `../` tricks), cap length, fall back to `attachment` for empty names. Never overwrite: collision -> `name (1).ext`. Files saved 0644. Missing directory is created. Disk-full and expired-token errors become readable toasts, partial files cleaned up.
- "All" = individual files, one result list, per-file failures do not abort the rest.

### 3. UI (non-intrusive at any count)
- **List:** keep the paperclip; correct after fix above. No count in the list row.
- **Reading pane:** one summary line replacing the dead label: `📎 3 attachments · A` (name and size inline when there is exactly one: `📎 report.pdf (2.1 MB) · A`). One line regardless of count, so 40 attachments cost the same space as 1. Clickable too, but keyboard is primary.
- **Picker (`A`):** popup in the same style as the category picker. Rows: name, size, type glyph. Footer shows destination folder and the key hints. Scrolls, type-to-filter for large lists.
  - `j/k`, Up/Down, `g g`/`G`: move
  - `Enter`: save the highlighted file
  - `Space`: mark/unmark; `Enter` with marks saves the marked set
  - `a`: save all (marked ignored)
  - `i`: show/hide inline images
  - `Esc`: close, focus returns to where it was
  - Saved rows get a check mark and the picker stays open, so you can save several in one visit.
- **Feedback:** toast "Saved 3 files to ~/Downloads" (or "2 saved, 1 failed: name: reason"), via the existing toast component.
- **Quick path without opening the picker:** `S` saves all attachments of the selected message (palette: `:save-attachments`); `:attachments` opens the picker.

### 4. Keys and docs
Free in the mail scope today: `A`, `S`, `o`, `d`, `D`. Proposed: `A` picker, `S` save all. Both `when: message has attachments`, so they cost nothing elsewhere. Register in `AppCommands.qml` (so they appear in `?` help and are rebindable via `[keys]`): `mail.attachments`, `mail.attachments.saveall`. Update `docs/KEYS.md`, `TRACKER.md`.

## Phases (each ends with a clean `cargo build --release` + tests, then check-in)
1. **Metadata + truth.** `attachments` table, Graph/Gmail list parsers, endpoint `GET /messages/attachments`, `has_attachments` correction. Unit tests with captured-shape JSON fixtures (nested multipart, inline cid, no-filename part, Graph item/reference attachment).
2. **Download.** Providers' fetch-bytes, filename sanitiser + collision logic (pure functions, tested first), `.part` streaming, save endpoint + job status, settings override. Tests: traversal names, collisions, empty names, big file does not buffer, per-file failure isolation.
3. **UI.** Summary line, picker popup, toast, focus return. Offscreen QML smoke test plus keyboard-only walkthrough.
4. **Keys + docs + live test.** Commands, help, KEYS.md. Real-account check on one Graph and one Gmail message with: 1 file, many files, inline-only logo, a large file, a forwarded email.

## Risks / open questions
- Gmail scope: need to confirm the current token scope allows `attachments.get` (readonly covers it; will verify before phase 2).
- Graph `item` attachments (attached emails) and `reference` attachments (cloud links): v1 shows them, saves `.eml` for items if `/$value` behaves, otherwise marks unsupported. Acceptable?
- Messages not in the local cache while offline: save fails with a clear message; no offline caching of file bytes.
- The tree has other uncommitted work (`build.sh`, `omarchylook`, `packaging/`); commit only attachment files, on a feature branch `feature/attachments`.

## Decisions needed from Adam
1. Keys `A` (picker) and `S` (save all) OK?
2. After saving, offer "open" (xdg-open on the file / reveal folder), or strictly save only? Default proposed: save only, with the path in the toast.
3. Hide inline images by default (toggle with `i`)? Proposed yes.
4. Name collisions: auto-rename `name (1).ext` (proposed) vs. ask.
5. Per-message subfolder in Downloads, or flat? Proposed flat (you said "individual files").
