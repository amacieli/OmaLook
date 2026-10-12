# PLAN — Compose (write new mail): System-font mode and HTML mode

Status: PLAN ONLY. Nothing implemented. Written 2026-10-07.
Scope: new message, reply, reply-all, forward, drafts, send, attachments. Graph + Gmail.

## 1. Findings (what exists today)

- UI is **Quickshell + QML** (`qml/shell.qml`, Qt 6.11, `qs.Commons`/`qs.Ui` kit). The Rust
  daemon is a **separate process** the QML talks to over **localhost HTTP + XMLHttpRequest**
  (`src/main.rs` routes: `/messages/read`, `/settings/senders/*`, ...). cxx-qt is not in the
  runtime path.
- `src/qt_bridge/compose_bridge.rs` is a **stub**: recipient validation + a `send()` with
  `// TODO: Call graph API`. Reuse its validation/tests; its cxx-qt shape is not usable.
- `EmailProvider` trait (`src/providers/mod.rs`) is **read-only** (fetch, set_message_read). No send,
  draft, reply or attachment API on either provider.
- Scopes are already sufficient: Graph `Mail.ReadWrite` + `Mail.Send` (also in the read-only
  fallback tier); Gmail `gmail.modify` covers `messages.send` and drafts. **No re-consent needed.**
- Reading side already has the system/HTML split: `message_rendering = html|system|system_sender`
  (`models.rs`, `settings.rs`), `Fmt.sanitizeHtml`, `Fmt.textToHtml` (`qml/mail/format.js`),
  and a Qt `Text.RichText` HTML pane. Compose should mirror that vocabulary.
- Qt's `TextEdit` can display/edit rich text but **QML cannot apply formatting to a selection**
  (no QTextCursor API). Qt's own editor example solves this with a small C++ `DocumentHandler`.
  QtWebEngine 6.11.2 is installed, but it must be initialised before the QGuiApplication, which
  Quickshell owns, so it is likely unusable here (verify in the spike).

## 2. Mode definitions

| | **System-font mode** (default) | **HTML mode** |
|---|---|---|
| Editor font | UI/system font, fixed | Per-run font family and size |
| Enabled | Bold, italic, underline, bullet and numbered lists, links, quote, undo/redo, attach | Everything in system mode + font family/size, text colour, highlight, strikethrough, sub/superscript, alignment, indent/outdent, headings, horizontal rule, inline images, tables, clear formatting, rich paste, signature |
| Disabled | Font, size, colour, highlight, alignment, images, tables (greyed, tooltip says why) | none |
| Wire format | `text/plain` if no formatting was used; otherwise HTML with **no font/colour/size declarations** (recipient's client default applies) | Full HTML (`text/html`) |
| Reply quoting | Quoted original run through the sanitiser, flattened to system style | Quoted original kept as sanitised HTML (blockquote) |

The "system" definition deliberately means "light HTML in the recipient's default font", because
true plain text cannot carry bold/italic. Open question Q1 below if you meant strict plain text.

Switching modes: system to HTML is lossless. HTML to system strips the disabled attributes, so show a
confirm (reuse `Ui/ConfirmDialog`) only when such attributes exist.
Default comes from `[mail.compose] default_format = "system" | "html"`, with a per-window toggle and
optional per-account override.

## 3. Architecture

```
QML ComposeView (TextEdit + toolbar)
   |  C++ QML plugin: DocumentHandler (formatting, toHtml/fromHtml)   <- only native piece
   |  XMLHttpRequest, JSON body
Rust daemon HTTP:  /compose/*  ->  outbox table (SQLite)  ->  provider.send_*()  ->  Graph / Gmail
```

Decision D1 (editor engine). Recommended: **TextEdit + a tiny C++ `DocumentHandler` QML plugin**
(~250 lines, from Qt's texteditor example) on the Quickshell import path (`QML_IMPORT_PATH`, set
by `run.sh`/launcher, same place that symlinks `Commons`/`Ui`). Gives native look and keyboard,
full format set via QTextCursor/QTextCharFormat, no browser. Cost: first C++ in the repo
(CMake/qmake target plus a packaging step). Approved (see section 8). Fallback was: system-mode only with
Markdown-style source-free shortcuts, HTML mode deferred. WebEngine only if the spike shows it can
load inside Quickshell.

Decision D2 (HTML output). `QTextDocument::toHtml()` emits verbose Qt-flavoured HTML
(`-qt-paragraph-type`, full DOCTYPE and head). Add a Rust post-processor `compose_html::clean()`:
extract body only, drop `-qt-*` properties, convert `<p>` margin styles to Outlook-friendly values,
base64-inline images to `cid:` attachments, and for system mode strip font-family, font-size and
colour. Tested with golden files.

## 4. Phases

### Phase A — Backend send path, Graph and Gmail — DONE 2026-10-08 (live send to self still to be run by hand)
Built: `src/compose.rs` (model, validation, HTML clean-up, Graph JSON, Gmail MIME, retry policy), `outbox` table +
`src/outbox.rs` (one worker thread per account, woken at once by `sync_state::request_outbox_run`), `send_message` on
both providers, routes `POST /compose/send`, `POST /compose/cancel`, `GET /compose/status`, `GET|POST /settings/mail`,
setting `[mail] send_delay_secs` (default 3, 0 = immediately, max 60), Settings > Mail field, `SendToast.qml` (Undo / `u`).
Part of Phase B was pulled in (outbox, undo window, retry/no-retry rules, crash handling); drafts, attachments and
`compose_html` golden tests are still open. `OMALOOK_PORT` runs a second backend beside the real one for tests.

- [ ] `models.rs`: `OutgoingMessage { account_id, to, cc, bcc, subject, body: Body{Text|Html}, attachments, in_reply_to: Option<id>, kind: New|Reply|ReplyAll|Forward }`.
- [ ] `providers/mod.rs`: add `send_message`, `create_draft`, `update_draft`, `delete_draft` with default
      "not supported" bodies (same pattern as `set_message_read`).
- [ ] `providers/graph.rs`: `POST /me/sendMail` (`saveToSentItems:true`); reply/forward via
      `/me/messages/{id}/createReply|createReplyAll|createForward` then `PATCH` then `/send` so
      threading and `conversationId` are preserved; drafts via `POST/PATCH /me/messages`.
      Attachments under 3 MB inline `fileAttachment`; larger via `createUploadSession`.
- [ ] Move `is_valid_email` / `parse_recipients` out of `compose_bridge.rs` into a `compose.rs`
      module (keep the existing tests); delete the stub bridge once routes exist.
- [ ] Tests: request-body builder unit tests (no network); one manual send to self.
- Done when: `curl -X POST localhost:PORT/compose/send -d @msg.json` delivers to Adam's own address.

### Attachments while composing (Graph) — built 2026-10-11, live send still to be run by hand
Attach (title-row button or Ctrl+Shift+A) opens `qml/compose/AttachBrowser.qml`, a keyboard-driven file browser
fed by `GET /fs/list?path=&hidden=` (`src/fs_browse.rs`). Chips show name, size and a remove button; Ctrl+Shift+Backspace removes the last.
Only the file *path* travels in `OutgoingMessage.attachments`; sizes are re-read from disk at queue time and the
file is read again when the message goes out (a file deleted inside the undo window fails the send with a clear error).
Graph: any message with files is built as a draft (`POST /me/messages`, or `createReply|createForward`), files under
3 MB go to `/attachments` as base64, larger ones through `createUploadSession` in 3.75 MB chunks, then `/send`; a failed
attempt deletes the draft. Limit 150 MB total. Gmail accounts are refused with a message (MIME attachments still open).
Not done: drafts persistence (B2), inline images (cid), Gmail.

### Phase B — Outbox and reliability
- [ ] `db.rs`: `outbox(id, account_id, payload_json, state, attempts, last_error, created_at)`.
- [ ] `scheduler.rs`: drain outbox; retry with backoff on network or 5xx; never retry 4xx (surface the error).
- [ ] Undo-send window (default 5 s, setting `[mail.compose] undo_send_secs`): message sits in
      outbox as `pending` until the timer ends. Prevents the "sent to the wrong person" class of mistake.
- [ ] Send is idempotent per outbox id, so a retry never double-sends.
- [ ] Routes: `POST /compose/send`, `POST /compose/draft`, `GET /compose/outbox`, `POST /compose/cancel?id=`.

### Phase B2 — Drafts (save, resume, sync)
Model: the **provider is the source of truth** for drafts (so they appear in Outlook web, phone, Gmail),
with a **local copy** so nothing is lost offline or on a crash.
- [ ] `db.rs`: `drafts(local_id, account_id, remote_id NULL, kind, in_reply_to, to_text, cc_text, bcc_text,
      subject, body, body_format 'system'|'html', attachments_json, dirty, updated_at, state 'editing'|'saved'|'sent')`.
      Local write on every autosave (cheap, synchronous); remote push is debounced.
- [ ] Autosave triggers: 10 s after the last keystroke, on window blur, on mode toggle, on Esc/close,
      and on app quit (flush). Status line shows `saved locally` / `saved to Drafts` / `save failed (retrying)`.
- [ ] Remote sync, Graph: first save `POST /me/messages` (stores `remote_id`), later saves `PATCH /me/messages/{id}`,
      body contentType follows the compose mode; attachments added via `/attachments`; discard `DELETE`.
      Gmail: `users.drafts.create` then `drafts.update` (full raw MIME each time), discard `drafts.delete`.
- [ ] Failure behaviour: network or 5xx leaves `dirty=1`, scheduler retries with backoff; 4xx surfaces an error but
      the local copy stays. Never delete the local copy until the remote delete or send is confirmed.
- [ ] Send path: sending a draft uses `remote_id` (Graph `POST /me/messages/{id}/send`, Gmail `drafts.send`) so no
      duplicate lands in Drafts or Sent; local row goes to `state='sent'` then is pruned.
- [ ] Resume: opening a message in the Drafts folder (Enter, or `e`) loads the draft into ComposeView in its original
      mode (HTML drafts reopen in HTML; if a Graph draft body is HTML but uses only system-mode features, offer system).
      Reply/forward drafts keep `in_reply_to` so threading survives.
- [ ] Crash recovery: on startup, any local draft with `state='editing'` and `dirty=1` prompts "Recover unsaved draft?"
      (Resume / Discard).
- [ ] Discard flow: Esc on a non-empty compose asks Save draft / Discard / Cancel; empty compose closes silently.
- [ ] Remote-edit conflict: if the provider copy changed (Graph `changeKey`/`lastModifiedDateTime`, Gmail draft `message.id`)
      since our last push, keep both: save ours as a new draft and tell the user. No silent overwrite.
- [ ] Routes: `GET /drafts`, `GET /drafts/get?id=`, `POST /drafts/save`, `POST /drafts/discard?id=`.
- [ ] Reading-pane Drafts folder already syncs via delta/label `DRAFT`; make our local `remote_id` dedupe against it so a
      draft never shows twice in the list.
- [ ] Tests: autosave debounce, offline then reconnect pushes once, PATCH vs POST selection, send-from-draft leaves no
      leftover, conflict path, crash-recovery state machine, Graph/Gmail request builders (no network).
- Done when: type, kill the app, relaunch, and the text is recoverable; a draft started here opens and edits correctly
  in Outlook web and Gmail web, and vice versa.

### Phase C — Gmail send
- [ ] `providers/gmail.rs`: build RFC 5322 (use `mail-builder` or `lettre::message`; multipart/alternative
      for HTML mode with a generated text part; `In-Reply-To`/`References` for threads; attachments).
      `POST .../messages/send {raw: base64url, threadId}`; drafts via `users.drafts`.
- [ ] Tests: MIME builder snapshot tests (headers, boundaries, UTF-8 subject encoding, non-ASCII body).

### Phase D — C++ DocumentHandler plugin (spike first, 1 day box)
**Spike result 2026-10-07: PASS.** A CMake `qt_add_qml_module` plugin (`OmaLook.Compose`, Qt 6.11.2) loads in
Quickshell 0.3.1 via `QML_IMPORT_PATH`; a `DocumentHandler` bolded a TextEdit selection and `toHtml()` returned the expected
span (verbose Qt HTML confirmed, so D2 `clean()` stays). Source: `plugin/omalook-compose/spike/`. WebEngine not tested (not needed).
- [ ] Spike: confirm Quickshell loads a third-party QML module from an import path on this machine; confirm WebEngine
      is not viable (record result here either way).
- [ ] `plugin/omalook-compose/` with `DocumentHandler` (bold, italic, underline, strike, font family/size,
      colour, highlight, align, list style, indent, insert link/image/table, clear formatting, `html` property,
      `plainText`, `hasRichAttributes`).
- [ ] `build.sh` and `package.sh` build the plugin; `run.sh` sets `QML_IMPORT_PATH`.
- Done when: a 30-line QML test window bolds a selection and returns the expected HTML.

### Phase E — Compose UI (system mode)
- [ ] `qml/compose/ComposeView.qml` as a window or overlay (see D3), `ComposeHeader.qml` (From, To/Cc/Bcc, Subject),
      `RecipientField.qml` (chips, autocomplete from the existing `/contacts` endpoint, paste-split on `,;`),
      `FormatBar.qml` (buttons bind to `DocumentHandler`; `enabled` from a mode table in one JS file so the
      matrix in section 2 lives in exactly one place).
- [ ] Entry points: `c` / Ctrl+N new (MailView), `r` reply, `a` reply-all, `f` forward; remove the hotkeys from
      the reading-pane focus trap (set `editing = true` while composing, same as `FieldChooser`).
- [ ] Keyboard: Ctrl+Enter send, Ctrl+B/I/U, Ctrl+K link, Ctrl+Shift+7/8 lists, Esc to discard-or-save-draft prompt.
- [ ] Wire autosave, status line, Drafts-folder resume and Esc save/discard prompt to Phase B2.
- Done when: new, reply, reply-all, forward send correctly from the keyboard alone in system mode.

### Phase F — HTML mode
- [ ] Enable remaining controls in `FormatBar.qml` (font, size, colour, highlight, align, indent, headings, rule, table, image).
- [ ] Inline images: paste or drop, stored as `cid:` parts (Graph `isInline`+`contentId`; Gmail multipart/related).
- [ ] Rich paste: accept clipboard `text/html`, run through `Fmt.sanitizeHtml`-equivalent whitelist, strip scripts, remote
      images, and event handlers.
- [ ] `compose_html::clean()` from D2 plus golden tests.
- [ ] Mode toggle with confirm (section 2) and the `[mail.compose]` setting in `MailPage.qml`.
- Done when: a message with every feature opens correctly in Outlook web, Gmail web and Thunderbird.

### Phase G — Polish
- [ ] Signatures (per account, in the same two formats), attachments bar with size limits (Graph 150 MB total,
      Gmail 25 MB), spell check (QTextDocument has none built in; evaluate Hunspell in the plugin, otherwise skip),
      "forgot attachment" and empty-subject warnings, Sent folder refresh after send via the existing sync-serial signal.

## 5. Files likely to change

Create: `src/compose.rs`, `src/compose_html.rs`, `plugin/omalook-compose/*`, `qml/compose/*.qml`, `qml/compose/modes.js`.
Modify: `src/models.rs`, `src/db.rs`, `src/scheduler.rs`, `src/main.rs` (routes), `src/providers/{mod,graph,gmail}.rs`,
`src/settings.rs`, `qml/mail/MailView.qml`, `qml/state/AppState.qml`, `qml/AppShell.qml`, `qml/settings/pages/MailPage.qml`,
`build.sh`, `package.sh`, `run.sh`. Remove: `src/qt_bridge/compose_bridge.rs`.

## 6. Testing and verification

- `cargo test`: recipient validation, Graph JSON builder, Gmail MIME snapshots, outbox state machine, `clean()` goldens.
- Manual matrix: {Graph, Gmail} x {new, reply, reply-all, forward} x {system, HTML} x {with/without attachment},
  sent to self, each opened in the web client to confirm threading, font and formatting.
- Failure drills: kill network mid-send (retry, no duplicate), expired token (existing reauth modal), 4xx (error shown, draft kept).
- Perf: opening compose must not stall the 60 fps message list (Phase 2 perf timeline already exists in `perf.rs`).

## 7. Risks

- First C++ in the repo: build and packaging complexity (mitigation: spike in Phase D before committing to E/F).
- Qt `toHtml()` fidelity in Outlook (mitigation: `clean()` plus real-client test matrix).
- Double-send on retry (mitigation: idempotent outbox ids, no retry on 4xx, undo window).
- Working tree already has uncommitted changes (AppShell, AppState, SettingsView, SMS files, Cargo.toml). Commit or stash,
  `git fetch`, and branch `feature/compose` from clean upstream before starting.

## 8. Decisions (settled 2026-10-07)

- Q1 System font: the editor shows the fixed UI font; on send it carries **no font, size or colour**, so the recipient's
  client default applies. Light HTML (bold, italic, underline, lists, links), not strict plain text.
  Sent as `text/plain` only when no formatting was used.
- Q2 C++ plugin: approved, provided its look, feel and theming match the Omarchy UI kit. Keep the C++ to the
  minimum (DocumentHandler only, no widgets or chrome). All visuals are QML built from `qs.Commons` / `qs.Ui`
  (Color, Style, UiText, BorderSurface, ConfirmDialog, ThemedScrollBar); no hardcoded colours or fonts, live theming
  preserved. Everything else in Rust (provider calls, outbox, drafts, HTML clean-up, MIME building).
- Q3 Layout: compose **replaces the reading pane** in place (not a window). If the reading pane is switched off, the
  message list narrows a bit to make room for the compose pane, and returns to full width on close.
  One compose at a time in v1. Update Phase E: `ComposeView` is hosted by `MailView.qml` in the preview slot, driven by
  the existing `readingPane` view-spec (off/right/bottom/auto) with a `composeWidth` used only while composing.
  The "(see D3)" window option is dropped.
- Q4 Undo-send: 5 s default (setting `[mail.compose] undo_send_secs`), changeable.
- Q5 Signatures: Phase G (not v1).
- Q6 Drafts: local save every 10 s; provider push debounced 30 s plus on close/blur/send. Revisit after hands-on use.

## 9. Still open
- None blocking. First action is the Phase D spike (does Quickshell load a QML plugin from an import path).
