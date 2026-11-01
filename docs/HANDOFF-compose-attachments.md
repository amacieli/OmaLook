# Handoff — compose attachments (OL-035), written 2026-11-01

Read `docs/PLAN-compose-attachments.md` first (design, phase results, findings). This file is only what is left and what to be careful about.

## Where it stands

| Phase | State |
|---|---|
| 0 Spikes | Done. Gmail/Graph/clipboard/picker answered live. **Open: does a QML `DropArea` receive file URLs under Quickshell** (needs a real drag from Nautilus; only matters for phase 5). |
| 1 Spool + limits (backend) | Done. 372 lib tests. |
| 2 Gmail attachments | Done and verified live (bytes compared, Gmail and Outlook side). |
| 3 Strip, list, forward chips | **Built, partly verified.** Details below. |
| 4 to 7 | Not started. |

Commands that must stay green: `cargo test --lib -- --skip test_copy_to_clipboard` (372), `node tests/commands.test.js`, `node tests/search.test.js`, `node tests/attach.test.js`, `/usr/lib/qt6/bin/qmllint` (0 errors on the compose files). On the Surface machine `~/.cargo/env` does not exist; `/usr/bin/cargo` works.

## Phase 3: what was built

- `qml/compose/attach.js` (pure layout and limits maths, node-tested), `AttachStrip.qml` (one constant-height line, absent with 0 files, 3 chips or 2 chips + `+N`, meter text, `!` chip when over the limit), `AttachList.qml` (popup, 6 rows, j/k/x/a/Esc), wiring in `ComposeView.qml`, `MailView.qml`, `AppState.qml` (`attachLimits`, `composeOriginals`, `composeAction`), palette twins in `AppCommands.qml` (`compose.attach`, `compose.attachments`, `compose.detach`, `compose.detachall`; `:attachments` of the reading pane is now `when !composing`).
- Backend: `GET /compose/originals?id=` (Graph from the cached list, Gmail via `format=full`; inline pictures left out), `OutgoingMessage.forwarded_keep`, Graph prunes un-kept originals from the forward draft (`attachments::unkept_draft_attachments`, matched by name+size) and **fails the send** if a delete fails, Gmail forward fetches kept originals by MIME part id.
- `docs/KEYS.md` documents the new keys.

## Phase 3: verified on screen (scratch instance, fake accounts, screenshots)

Strip height identical with 1, 2, 3 and 41 files and the editor text does not move; absent with 0. Forward showed the 3 `↪` originals, inline logo excluded. 14 MB x 2 on a Gmail account: `27 MB of 26 MB` in red and a red `!` chip. List panel: `Ctrl+Shift+L` opens, `j` moves, `x` removes, `Esc` closes and focus returns to the editor. Gmail forward with an unreachable account: "Could not read the original's attachments; they will not be forwarded."

## Phase 3: NOT verified, do these first on the desktop

1. **Send with the attachments over the limit** shows "The attachments are too big for Gmail (…)" and sends nothing.
2. **Palette twins while composing**: `:` is typed into the editor, so check whether any chord reaches the palette from the compose pane. If none does, the four `compose.*` commands are unreachable and either need a chord or should be dropped.
3. **`Ctrl+Shift+Backspace` from the To field did nothing** (it works from the editor; `RecipientField` probably consumes Backspace with any modifiers). Same check for `Ctrl+Shift+A` and `Ctrl+Shift+L` from To, Cc, Subject.
4. **The From dropdown could not be reached with Tab/Shift+Tab**; the From-switch re-check of the chips is therefore only tested via a message that opened from a Gmail account. Check by mouse, then decide whether the dropdown needs a key.
5. Tooltips on chips, the `+N` label, the window narrower than 640 logical px (hint hides), the chips sharing width (changed after most screenshots; the 3-chip case was seen working).
6. **Real accounts**: forward of a message with attachments on Graph (do the un-ticked ones really disappear from the sent copy? delete route could 503 on shared mailboxes) and on Gmail (`list_attachments`, kept originals). Only the matcher and the part walker are unit-tested.
7. `app.log` for QML "binding loop" warnings on `AttachStrip` (chip width depends on the row width).
8. Keyboard-only walkthrough of every strip and list action (the plan's "done when").

## Known gaps / decisions still open

- `forwarded_keep` unset: Graph keeps all originals, Gmail sends none. The UI always sets it for forwards now; other callers keep the old behaviour.
- Missing/changed file states in the strip are not computed live (only at Send, from the daemon). The plan listed more states than the strip shows.
- Plan section 8 questions still open for Adam: Q3 (names on chips, built as "up to 3"), Q4 (Ctrl+Shift+L is free in `KEYS.md`; Hyprland binds not checked on the main PC), Q5 (external picker worth building), Q6 (forgot-attachment check and sensitive-path tripwire on by default), Q7 (forward as .eml).
- Not checked: Gmail messages of 25-35 MB through `send_message`, a dropped upload chunk, resume after a 308.
- Gmail still has no attachment listing on the reading side (`attachment_parts` is the start of it).

## Next phases (see the plan for detail)

4 picker upgrade (cross-folder ticks, `t` pattern tag, totals footer, bookmarks/recents, sort), 5 other ways in (clipboard paste: read `text/uri-list` first, then `x-special/gnome-copied-files`; drag-drop only if the spike passes; external picker via `xdg-terminal-exec` + temp file), 6 guard rails, 7 drafts (waits for PLAN-compose B2).

## Cautions learned the hard way

- **Graph `DELETE /me/messages/{id}` is a hard delete into Recoverable Items**, not Deleted Items. A test with a loose `contains()` filter deleted 10 real DMARC reports; they were restored with `POST /me/mailFolders/recoverableitemsdeletions/messages/{id}/move` (`destinationId: inbox`). Delete only by exact subject after asserting the match count. Written into the `microsoft-graph-live-probing` skill.
- **Do not drive the desktop with `wtype`.** Focus kept slipping to the user's terminal and keystrokes landed there. If a scratch run is needed: the app needs `auth_state.json` (`{"is_authenticated": true, "login_serial": 1}`) in the scratch config or it shows no accounts; Hyprland here is Lua-configured (`hyprctl dispatch 'hl.dsp.focus({ window = "address:0x…" })'`); the window address changes on every launch.
- Gmail honours the **first** of `filename` / `filename*`; the `*` form goes first (already done).
- Google Cloud no longer offers "Download JSON" for an existing client; see OL-038 (make Gmail sign-in like Outlook's).

## Left uncommitted in the tree (the user's own work, not part of OL-035)

`qml/AppShell.qml`, `qml/views/PeopleView.qml`, `src/calendar_daemon.rs`, `src/contacts_daemon.rs`, `src/sync_state.rs`, and the OL-036/OL-037 hunks of `src/main.rs`, `qml/state/AppState.qml` and `docs/TRACKER.md` (the `/sync/all` work). Nothing was pushed before this commit.
