# OmaLook — outstanding issues and plans

Single index of open work. Add new items at the top of the relevant section; move finished ones to **Done** with the commit hash.
IDs are stable (never reused). Status: `open` | `investigating` | `planned` | `in progress` | `done`.

## Issues

### OL-023 — External-recipient warning banner for mail and calendar invitations (open, not started)
- **Wanted (user, 2026-10-25):** for work/school accounts, or any account on a custom email domain, show a warning when people **outside the organization** are (a) invited to a calendar entry or (b) emailed (compose, reply, reply all, forward).
- **Behaviour asked for:** a notification banner only. It cannot be clicked away or dismissed; it stays while any external recipient is present and goes when the last one is removed. It does not block sending or ask for confirmation.
- **To do (design before code):** decide what counts as "inside" (the account's own email domain; whether Graph can also list the tenant's verified domains, and with which permission, unverified); how consumer domains (gmail.com, outlook.com, etc.) are told apart from custom domains, so consumer accounts get no banner; which recipient fields count in mail (To, Cc, Bcc) and calendar (Required, Optional); wording, including naming the external addresses; where it sits in `ComposeView` and in the event editor (`docs/PLAN-calendar-events.md` 5.3, attendees 5.5). Add the calendar half to that plan's phase 3 (attendees) when the user schedules it.
- **Not started; no code written.**

### OL-022 — New mail takes the first signed-in account instead of the viewed or last-used one (open, not started)
- **Seen:** 2026-10-25 while planning OL-021 (user asked whether mail has the same gap; confirmed by reading the code). `AppState.openCompose()` (`qml/state/AppState.qml`) sets a new message's account to the first signed-in account; the folder or account being viewed and the last account used are ignored. Replies and forwards use the message's own account (correct). The From dropdown in `ComposeHeader.qml` is the only workaround.
- **Proposed:** viewed account when the mail view is filtered to one, else last used (`last_account.mail` in `view_state.json`, written when a message is actually sent), else the first signed-in account. Same mechanism as the calendar's `last_account.calendar` in `docs/PLAN-calendar-events.md` 5.1.

### OL-021 — Create calendar entries: personal and meetings, attendees, free-time finder, online meetings (planned, not started)
- **Wanted (user, 2026-10-25):** keyboard-friendly way to create a calendar entry: personal (no attendees) or a meeting with required and optional attendees, find free time, and add Teams, Zoom or other online meeting providers. Microsoft Graph first, designed for other providers.
- **Plan:** `docs/PLAN-calendar-events.md` (findings, data model, Graph mapping, keys, finder algorithm, meeting providers, phases 0-7, open questions in section 13). Revised twice 2026-10-25 with the user's answers (work/school Graph first, account switcher, meeting accounts in Settings -> Accounts, recurrence in v1 with a 12-month default series end, undo window on every calendar write, last-used account, Zoom as a stub until the OAuth app exists). Phase 0 Graph spike done 2026-10-25 (findings in plan section 15; vendor items and attendee-on-series check still open). Waiting for the user to choose the next phases (proposed start: phase 0 spike, phase 1 read side and cursor, which also covers the join action of OL-010).

### OL-020 — One toggle for how a message is shown: HTML / system, this message / this sender (open, not started)
- **Wanted (user, 2026-10-24):** streamline the rendering choices — "view as HTML for this message", "for this sender", "HTML" and "system" — into one toggle-able button or setting that can be driven from the keyboard.
- **To do (design first, no code yet):** list what exists today (the per-message view-as-HTML control, the per-sender "always load images"/rendering choice, the global `message_rendering` setting with `html` | `system` | `system_sender`) and how they interact; propose a single control (e.g. one key that cycles message → sender → default, or one picker) and a clear indicator of which level is in force; get the user's approval before building.

### OL-019 — Recipients (To/Cc/Bcc) blank on some messages (fixed 2026-10-24, needs `./build.sh --live`)
- **Seen:** 2026-10-24: today's "Holiday party" from Erika showed no recipients; Graph has To = Jennifer Jackson, Gary Nusbaum, Adam. 78 other cached rows had the same (no conversation id).
- **Cause:** `GraphEmailProvider::parse_message` marked every change-feed item `meta: true`. A read/move update can carry only the changed fields, so storing it blanked the cached recipients and flagged the row complete.
- **Fix:** an item counts as complete only with a `conversationId`; Graph got `fetch_message_meta`; the daemon refetches stragglers one by one once the folder walks finish; a one-time flag (`sparse_meta_v1`) re-queues the 78 broken rows. Unit test `sparse_change_feed_item_is_not_complete`.
- **Verify:** after deploy, "Holiday party" shows the three recipients; log line "N messages were stored without their details".

### OL-018 — Make hyperlinks clickable in both system mode and HTML mode (done 2026-10-24: mouse + keyboard)
- **Done:** system view (rich text, underlined, hand cursor), HTML view (WebKit link boxes + Qt fallback cursor); keyboard: Tab/]/[ , Enter/o, y, f hints, see docs/KEYS.md "Links in a message". Also fixed the white window: Qt 6.12 has its own `Color` type that shadowed the Omarchy kit's, and Omarchy 4.0.6 no longer loads the theme into `Color` by itself (shell.qml now does; all QML uses `Commons.Color`).
- **Gotchas:** Qt paints a span's background over a following pre-wrap newline (use `<br>`); Text has no `positionToRectangle` (link positions come from `linkAt` probing); `linkColor` is ignored for rich-text anchors (colour a span inside); QML replace callbacks may get "" for unmatched groups.
- **Original note:**
- **Wanted (user, 2026-10-23):** links in mail bodies must be clickable in both the system-font view and the HTML view. Extends OL-003 (system view only) to the HTML view as well.
- **To do:** scope with the user first (see OL-003 open question: bare URLs only, not link text that differs from its target). For the system view, follow OL-003. For the HTML view, check how `tools/render/omalook-render.c` already reports link boxes and whether bare-text URLs that the HTML does not wrap in `<a>` are missed; add them to the link list. Only http(s) and mailto. Verify with a message containing both kinds of link in each mode.

### OL-017 — Refresh mail button only re-reads the cache; add an F5 shortcut for it (in progress)
- **Wanted (user, 2026-10-20):** map a keyboard shortcut to the message list's "refresh emails" button, and make the button fetch new mail from the server immediately.
- **Cause (confirmed by reading the code):** `MailView.qml` wired the button to `app.loadMessages()`, which only re-reads `messages.db`. Nothing woke the daemon, so new mail waited for the 2-minute poll. F9 "sync" had the same gap.
- **Done in working tree (not committed):** `sync_state::request_mail_sync()` + `POST /mail/sync` wake the mail daemon loop (`src/email_daemon.rs`) for an immediate message sync; `AppState.syncMail()` used by the button, F9 and the new `mail.refresh` command on **F5**; `docs/KEYS.md` updated.
- **To verify:** `cargo test` for the new wake (sync_state), build, deploy and check that new mail appears without waiting for the poll interval.

### OL-016 — Duplicate mail: two messages from Brayden Wells on 2026-10-19 that are really one (open, not started)
- **Seen:** 2026-10-19 (user report). OmaLook shows two emails from Brayden Wells dated October 19; the user believes the mailbox holds only one.
- **Likely cause (unconfirmed):** the same message stored twice, most likely from the delta sync (`src/email_daemon.rs`, `src/providers/`) or the SQLite cache (`src/db/`), for example a re-delivered delta page or an id mismatch between the provider's id and the cache key. A copy in a second folder (e.g. Inbox and a category or folder view) is also possible.
- **To do:** compare the two rows' provider message ids and internet message ids (`Message-ID`) in the cache; check the provider (Graph/Gmail) directly to confirm there is one message; if the ids match, find where the duplicate is inserted and make the upsert idempotent; if they differ, check whether the provider really holds two; add a test for the dedupe key; purge the duplicate row from the cache and confirm it does not come back after a re-sync.

### OL-015 — Reply All: no keyboard shortcut, and Cc/Bcc recipients are dropped (fixed 2026-10-24, awaiting live check)
- **Fixed:** Reply All read a non-existent `m.to_recipients`; it now builds To = sender, Cc = original To + Cc minus own addresses and duplicates, Bcc = original Bcc (`Fmt.replyAllRecipients`, `qml/mail/format.js`). Replying to your own sent mail goes to its original recipients. The compose recipient parser no longer splits `"Doe, Bob" <b@y.org>` at the comma. `R` was already bound (`mail.replyall`; Ctrl-Shift-R in the outlook preset). Checked with node only; **not yet seen in the app**.
- **Seen:** 2026-10-19 (user report). Two faults in the mail reply flow:
  1. There is no keyboard binding for Reply All.
  2. Clicking the Reply All button does not carry over the Cc and Bcc recipients; they are missing from the compose window.
- **To do:** find the Reply All action in the mail/compose QML (`qml/mail/`, `qml/compose/`) and its keybinding in the keys model; add a key and document it in `docs/KEYS.md`; check how the reply recipient list is built (original To/Cc/Bcc, minus the user's own address) and fix the Cc/Bcc copy; verify with a message that has Cc and Bcc on both the button and the key, and confirm the recipients in the compose window before sending.

### OL-014 — Remember the last selected view per area (done 2026-10-24, needs `./build.sh --live`)
- **Wanted (user, 2026-10-17):** remember the view the user last selected in each area, so it is restored when the app opens again.
- **Done:** the module (Settings is not restored: the last other module is), the mail folder (an \"all accounts\" row too), the calendar view (day / work week / week / month), the people view and sort, and the task filter. Stored in `~/.config/omalook/view_state.json` (backend `GET`/`POST /settings/view_state`, `src/view_state.rs`; a flat JSON object, separate from `settings.toml`). `AppState.qml`: `loadViewState()` on first backend answer, saves 300 ms after a change, and not before the stored state was read. A folder that no longer exists or an unknown value is ignored and the default stays.
- **Checked on a scratch instance (own config dir, port 27999):** calendar/week, people favorites + last name and task filter `done` came back; saving after `g t`, `g p` and `f` rewrote the file; a stored \"Sent - all accounts\" folder was reopened; a bogus view/folder/mode fell back to Mail / Inbox. Unit test `view_state::tests::round_trip_and_refusals`.
- **Not covered:** the selected message, scroll position, calendar date and search are not remembered (only the views).

### OL-013 — Status indicator in the bottom-right corner: daemon, MCP server, KDE Connect (open, not started)
- **Wanted (user, 2026-10-17):** a coded "online" indicator in the bottom-right corner of the window. One letter per service: `D`/`d` = daemon, `M`/`m` = MCP server, `S`/`s` = KDE Connect. Capital = online, lower = offline.
- **Colours:** fixed (not theme-adjustable) green for online and red for offline, in colour-blind-friendly shades. Do not take them from the Omarchy theme.
- **To do:** find the existing health signals for the daemon (backend HTTP on 27182), the MCP listener (27183) and `kdeconnectd` (reuse the SMS daemon's check; see OL-009 for the parse issue); add the indicator to the shell chrome in QML; verify each letter flips case when its service is stopped and started.

### OL-012 — Distribution lists (open, not started)
- **Wanted (user, 2026-10-17):** add distribution lists. No design details recorded yet; scope to be discussed before planning.

### OL-011 — Per-element colours: choose theme colour and intensity for each interface element (planned, not started)
- **Wanted (user, 2026-10-17):** name every element of the OmaLook interface (box edges, titles, subject lines, dates, …) and let the user pick a theme colour and an intensity for each, with one dropdown per choice and a live preview; keyboard-driven with vim-style chords.
- **Plan:** `docs/PLAN-colours.md` (element catalogue, intensity model, `[colours]` in settings.toml, Settings → Colours screen, key chords, phases 0-5; decisions settled 2026-10-17 in §9: live apply, theme keys only, one shared border colour (focused `accent`, idle `accent@-2`), no per-theme overrides, per-sender Auto, blend intensity).
- **Next:** phase 0 (inventory of hard-wired colours, no visible change).

### OL-009 — SMS settings say "KDE Connect is installed, but no phone is paired" when SMS works (open)
- **Seen:** 2026-10-16 (user report). The SMS settings page reports the unpaired state, but SMS is working, including inside OmaLook.
- **Likely cause (from reading the code, unconfirmed):** `qml/settings/pages/SmsPage.qml` decides "paired" by parsing `kdeconnect-cli -l` lines for the `paired` flag (`parseDevices`). The check is either not matching the real output format or is reading a device list that differs from the one the SMS daemon (`src/sms_daemon.rs`, talks to `kdeconnectd`) uses.
- **To do:** run `kdeconnect-cli -l` by hand and compare its output with the regex in `parseDevices`; confirm which device the SMS daemon actually uses; fix the parse or the source of truth so the page matches reality; re-check with the phone paired and with it unpaired.

### OL-010 — Online meeting providers: set up and detect several (open, not started)
- **Wanted (user, 2026-10-16):** support more than one online meeting provider, with each one configurable: Microsoft Teams (work/school account), Microsoft Teams (personal/consumer account), Zoom, and other common ones (Google Meet, Webex, and similar).
- **To do:** decide how each provider is set up (Teams work/school via the existing Graph account; Teams personal and Zoom need their own sign-in or OAuth app, to be checked); detect the meeting link in calendar events and invitations per provider; per-provider settings page with status (configured / not configured); join action per event. Check which of these already work with the existing calendar code before building.

### OL-006 — Harden the UI backend port 27182: no auth, CORS `*` (planned, not started)
- **Seen:** 2026-10-16 while designing MCP. `start_http_trigger_server` (`src/main.rs`) answers every route on `127.0.0.1:27182` without authentication and with `Access-Control-Allow-Origin: *`. Any local process, or any web page open in a browser, can call `/messages/action` (archive/trash/delete), `/compose/send`, `/sms/send`, `/accounts/remove`, `/settings/*`.
- **Why it matters for MCP:** the MCP listener (port 27183, bearer token, Origin/Host checks) enforces permissions, but they are moot while this port lets anything do the same actions directly.
- **Sketch (not decided):** per-launch token handed to the QML UI and required on every route; drop the CORS header; check `Host`/`Origin`; later the single-instance guard on the port.
- **Decision (2026-10-16):** do not tackle yet. Fold into OL-007.

### OL-007 — Whole-app security analysis (planned, not started)
- **Scope (user, 2026-10-16):** a separate pass over every security issue in OmaLook, not just MCP. Starting list: OL-006; unauthenticated routes; the `/fs/list` file browser; attachment save paths; token/keyring handling; the render helper's isolation; what is written to logs and `perf.log`; the MCP surface once built.

### OL-008 — OS notification integration (planned, not started)
- **Wanted (user, 2026-10-16):** hook into the desktop notification system (freedesktop `org.freedesktop.Notifications` over D-Bus, `zbus` is already a dependency): new mail/SMS/invite alerts, and MCP "ask" approval prompts as notifications with actions when the OmaLook window is closed or the daemon runs in the background.
- **Until then:** MCP approval prompts are in-app only (keyboard-driven toast). See `notes/DESIGN-mcp-server.md` §4.3.

### OL-005 — `test_copy_to_clipboard` hangs; find the root cause (open)
- **Seen:** every full `cargo test --lib` run for weeks blocks forever at 0% CPU on `qt_bridge::auth_bridge::tests::test_copy_to_clipboard` (re-confirmed 2026-10-13); until now it was skipped with `--skip`. Predates all recent work.
- **Stopgap (2026-10-13):** the test runs the call on a thread and panics after 5 s, so the suite finishes; the test now **fails** (red) until this is fixed. It does not fix the hang.
- **Evidence:** the hung run leaves an orphaned `wl-copy` process (no xclip on this machine) waiting on stdin, consistent with the suspect below. `pkill -x wl-copy` clears them.
- **Prime suspect (unconfirmed, from reading the code):** `AuthBridge::copy_to_clipboard` (`src/qt_bridge/auth_bridge.rs`) calls `child.wait()` on `xclip` while still holding the child's `stdin` pipe, so xclip never sees EOF and never exits. Likely a real bug in the app's copy-to-clipboard too, not only the test. Same pattern may exist in the `wl-copy` branch.
- **To do:** confirm (run it under `strace -f` or run `xclip`/`wl-copy` by hand); `drop(stdin)` before `wait()`; check what `xclip` does under Wayland/XWayland; then remove the stopgap or keep it as a guard, and re-test the copy button in the UI.

### OL-001 — UI hangs at ~100% CPU on one thread (open)
- **Seen:** 2026-10-07, `quickshell -p /opt/omalook/qml`, PID 3868852. Window stopped responding; one thread pinned at ~97% CPU
  from startup onward (4:09 CPU in 4:15 elapsed); had to be killed by hand.
- **Likely trigger (user report):** rapid scrolling from one message across many others in very quick succession
  (each selection starts a body fetch + rich-text layout in the preview pane).
- **Evidence so far:** log (`/run/user/1000/quickshell/by-pid/<pid>/log.log`) shows
  `MessagePreview.qml:338 Connection refused` (the `Text.RichText` body), a `QIODevice::read (QSslSocket): device not open`
  warning, and 55x `QFont::setPixelSize: Pixel size <= 0 (0)`. No stack trace: `ptrace_scope=1` blocks `eu-stack`/`gdb` attach.
  The Rust daemon was alive when the user ran `ps` (0:01 CPU); it died from the user's `pkill`, so it is **not** the cause.
- **Hypotheses (unconfirmed):**
  1. Body requests not cancelled/debounced on fast selection change, so stale responses pile up and re-lay-out the HTML.
  2. `Text.RichText` layout loop on a particular message (large/nested table, image with 0 size, remote `<img>` fetch retries).
  3. Binding loop from the zero-pixel-size font warnings.
- **To do:** reproduce on a debug run (`QT_LOGGING_RULES`, perf timeline in `perf.rs`); capture the offending message id;
  debounce body fetch (~120 ms) and ignore stale responses by message id; run with `sudo sysctl kernel.yama.ptrace_scope=0`
  once to get a stack if it recurs. Find the source of the `Pixel size <= 0` warnings.
- **Note:** to kill it, use the PID of `quickshell -p /opt/omalook/qml`, never `pkill quickshell` (that is also the Omarchy desktop shell).

### OL-004 — quickshell SIGSEGV opening HTML mail with remote images (fixed 2026-10-10)
- **Seen:** crash report `~/.cache/quickshell/crashes/6v922t8npmt` (pid 3468927): SIGSEGV in `QUrl::resolved` <- `QTextDocument::resource` <- `QQuickText::resourceRequestFinished`, right after `Connection refused` / `QSslSocket: device not open` from the HTML `Text` in `MessagePreview.qml`.
- **Cause:** Qt's `Text.RichText` fetches remote `<img>` (and `background=`) itself; the completion callback crashes when the request fails or the text has changed meanwhile. Marketing mail with tracking pixels triggers it.
- **Fix:** `Fmt.sanitizeHtml` never passes Qt a remote URL any more: allowed remote images become `[alt]` links opened in the browser; `background`/`srcset`/`poster` attributes are stripped. data: images still render inline.

### OL-002 — HTML rendering is not correct (fixed 2026-10-10 with a WebKitGTK helper)
- **Cause:** Qt `Text.RichText` has no `border-radius`, `box-shadow`, inline-block layout or `<style>` support, so styled mail (Stripe receipts, newsletters) came out as flat boxes with collapsed columns.
- **Fix:** `tools/render/omalook-render.c` (WebKitGTK 4.1, ~200 lines of C) renders the HTML to a PNG plus link boxes in its own process; `MessagePreview.qml` shows the PNG with a click area per link. A GTK widget cannot be embedded in a Quickshell window on Wayland, so a separate process is the embedding. Crash/hang/timeout (20 s) of the helper only means that message falls back to the Qt renderer; 3 failures disable the helper for the session.
- **Isolation:** ephemeral WebKit data (no cookies/cache), page JS blocked (CSP `script-src 'none'` + only our own link query runs), navigation and popups denied, no remote loads unless `--images` (message/sender allowed).
- **Build/ship:** `build.sh` compiles it to `bin/` when `webkit2gtk-4.1`, gtk3 and gcc exist (else a warning and Qt fallback); `--live` installs it to `/opt/omalook/bin`. Runtime needs `libwebkit2gtk-4.1` (Arch: `webkit2gtk-4.1`).
- **Known limits:** no text selection in the WebKit view (use the System view to copy text); blocked remote images show as WebKit's empty image boxes; re-renders on every selection/resize (no cache yet).

### OL-003 — Make hyperlinks clickable in "system font" view (open)
- **Seen:** 2026-10-07, user request. In system-font (plain) rendering, URLs in the body are not clickable.
- **To do:** in `Fmt.textToHtml`/the system-font path, linkify `http(s)://`, `www.` and `mailto:` text (escape first, then wrap
  in `<a href>`), keep `onLinkActivated: Qt.openUrlExternally`. Only http(s) and mailto schemes; show the real target on hover.
  Open question: all links or only "some" (user said "some hyperlinks")? Clarify which (e.g. only URLs that appear as bare text,
  not link-text-differs-from-target which is a phishing signal).

## Plans

| Plan | File | Status |
|---|---|---|
| New calendar entries: personal + meetings, attendees, free-time finder, Teams/Zoom/other (OL-021) | `docs/PLAN-calendar-events.md` | planned (plan written 2026-10-25, decisions recorded; user to choose next phases) |
| Compose: system-font / HTML modes, drafts, send | `docs/PLAN-compose.md` | planned; Phase D spike passed 2026-10-07; UI first draft + Phase A (send, outbox, undo delay) built 2026-10-08 |
| Per-element colours: theme colour + intensity per interface element, preview, vim chords (OL-011) | `docs/PLAN-colours.md` | planned (plan written 2026-10-17; decisions settled) |
| Mail list views / field chooser | `notes/PLAN-mail-list-views.md` (gitignored `notes/`) | not started |
| Keyboard model: vim nav, `g` modules, palette, mail actions | `docs/PLAN-keyboard.md` | planned (plan written 2026-10-09; decisions Q1-Q6 settled) |
| Mail details (sender name, To/Cc/Bcc, categories, sent, importance, attachments) + categories key `t` | `docs/PLAN-mail-fields.md` | built 2026-10-09 (branch `feature/mail-fields`) |
| Compose attachments: in-app file browser, chips, Graph small + upload-session (Gmail refused) | `docs/PLAN-compose.md` | built 2026-10-11; live Graph send confirmed working (small file); large-file upload session and reply/forward with attachments still unchecked |
| Search and filtering: `/` prompt, keyword patterns, quick filters, history, `fullbody:` via Graph / Gmail | `docs/PLAN-search.md` | built 2026-10-12 (branch `feature/search-filter`); Gmail `fullbody:` not yet tried live |
| Received-mail attachments: list, picker (`A`), save one / all (`S`) into Downloads (Graph only) | `docs/PLAN-attachments.md` | built 2026-10-11 (merged to main) |

## Backlog — mutt-parity gaps (compared 2026-10-11; tackle in this order)

Where OmaLook already leads mutt: native Graph + OAuth, calendar and invitations, contacts, SMS, HTML mail via the
WebKit helper, undo-send, rebindable keys with which-key and a palette. Mutt's edge is mail handling:

1. [x] **Search and filtering**: done 2026-10-12, see `docs/PLAN-search.md` (the `l` limit view was dropped on purpose).
2. [ ] **Threading**: thread view, collapse/expand, thread-wide actions.
3. [ ] **Archive / delete / move**: queued today but never checked against Graph or Gmail. Verify live, then fix what breaks.
4. [ ] **Bulk operations**: act on all marked messages, mark by pattern.
5. [ ] **Drafts** (PLAN-compose.md Phase B2): save, resume, sync to the provider, crash recovery.
6. [ ] **Gmail attachments** (send): MIME multipart in `build_mime`; the UI currently refuses them for Gmail.
7. [ ] **Compose extras**: signatures, external `$EDITOR`, forward-as-attachment, bounce, reply-to-list, aliases.
8. [ ] **Security**: GPG / S/MIME sign and encrypt.
9. [ ] **Offline**: test and harden use without a network (SQLite cache exists, untested offline).
10. [ ] **Automation**: per-account / per-folder hooks and macros.
11. [ ] Unchecked live paths for compose attachments: file of 3 MB or more (upload session), reply and forward with attachments.
12. [ ] Inline images (cid) in HTML compose.

## Done
- Backlog 1, search and filtering (2026-10-12): `9f07565` pattern engine, `99bb981` prompt and list, `78db1be` cat fix, `6a7d613` quick filters + history, `7e13369` `fullbody:` via provider. Also fixed in this branch: the project rename had changed the keyring service name, which hid every saved sign-in.
