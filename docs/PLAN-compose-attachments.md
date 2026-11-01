# PLAN — Attaching files to outgoing mail (Graph + Gmail): pickers, compact display, limits

Status: PLAN ONLY. No code written. Written 2026-11-01. Tracker item: OL-035.
Scope: files attached while composing (new, reply, reply-all, forward) on Graph and Gmail accounts.
Not in scope: *received* attachments (`PLAN-attachments.md`, built), inline `cid:` images (PLAN-compose.md phase F), drafts
storage itself (PLAN-compose.md phase B2; this plan only says what attachments need from it).
Non-goals, carried over from the reading side: no previews, no automatic zipping, no cloud-link ("share a OneDrive/Drive link instead")
fallback in v1.

## 1. Where things stand (read from the code 2026-11-01)

| Area | State today |
|---|---|
| Picker | `qml/compose/AttachBrowser.qml`, opened by Ctrl+Shift+A or the Attach button. Tab ticks, Enter attaches the ticked set (or the highlighted file), type-to-filter, leading `/` or `~` jumps to a path, Ctrl+H toggles hidden files, folders cannot be attached, starts in the last folder used. Listing from `GET /fs/list` (`src/fs_browse.rs`, capped at 5000 entries). No bookmarks, no recents. |
| Display | Chips in a `Flow` (`ComposeView.qml` ~321-360): name, size, ×. Ctrl+Shift+Backspace removes the last. No count cap, no scroll, names elided with no tooltip. The chip bar sits above the editor, so 20 files push the editor down. |
| Send path | `POST /compose/send` → `compose.rs::validate_attachments` (exists, regular file, readable, total ≤ 150 MB) → `outbox` row (path list in JSON) → per-account worker → provider. Only the **path** travels; the file is re-read when the message actually goes out. |
| Graph | Works (built 2026-10-11, live send still to be run by hand): draft + `/attachments` base64 under 3 MB, `createUploadSession` in 3.75 MB chunks above; draft deleted on failure. Reply/forward use `createReply*`/`createForward`. |
| Gmail | `send_message` exists, but `compose.rs::build_mime` only emits `text/plain` or `multipart/alternative` — **no `multipart/mixed`, no attachment part**. The UI (`ComposeView.qml`) and the daemon (`compose_send_response`) both refuse attachments on Gmail accounts. |
| Forward | Graph `createForward` copies the original's attachments (not verified against the API docs) but the UI never shows them: the user cannot see or remove them. Gmail forward builds a fresh message with no originals. So the two providers silently differ. |
| Drafts | None yet. Attachments live only in the open compose view; Undo restores them (`AppState.qml` ~1041), closing loses them. |
| Drag-drop, paste, clipboard files | None. |
| Docs | `docs/KEYS.md` documents none of the compose attach bindings (against the AGENTS.md rule); `qml/compose/modes.js:21` still says attachments "arrive with the send backend". |

Gaps found beyond "Gmail is missing":
1. **Send-time re-check is weaker than the plan promised.** `outbox.rs` never re-validates the paths. A file deleted or replaced
   during the undo window fails with a raw I/O error from the provider, or worse, sends a changed file.
2. **Graph reads files under 3 MB fully into memory** (fine), but a Gmail implementation must not do the same for 20 MB.
3. **No per-account limit.** 150 MB is Graph's; Gmail's is far smaller and counted *after* base64 (section 5).

## 2. Direction: what the rest of OmaLook suggests

Read from `AGENTS.md`, `PLAN-compose.md`, `PLAN-attachments.md`, `PLAN-calendar-events.md` and `KEYS.md`:

- **The daemon does the work, the UI asks.** Picker output is paths; the daemon owns reading, MIME and uploads.
- **Everything is a registered command** (`AppCommands.qml`): rebindable, in `?` help, in which-key, reachable from the `:` palette.
- **One compact line, never a growing block** (`📎 3 attachments · A to choose, S to save all` on the reading side). Compose should
  mirror that line, so reading and writing look like the same feature.
- **Undo window and idempotent outbox** are the safety net for anything that leaves the machine. Attachments must survive Undo and retries unchanged.
- **Capabilities per account**, shown as the reason when something is greyed (calendar plan, principle 4).
- **Text fields own their keys**; compose chords therefore have to be Ctrl-based.
- **Terminal-tool precedent** (mutt/neomutt, aerc, ranger/lf/yazi):
  - mutt: attach menu is a *list with index, size, name* that is always visible; tag-by-pattern for multi-select
    (`<tag-pattern>`); `abort_noattach` + regex catches "forgot the attachment" at send time.
    <https://neomutt.org/guide/advancedusage.html>, <https://neomutt.org/guide/reference.html>
  - aerc: the picker is an **external command with an output contract**: `file-picker-cmd`, `%f` = temp file the picker writes paths to, otherwise one path per line on stdout; examples `fzf --multi`, `ranger --choose-files=%f`.
    <https://man.archlinux.org/man/aerc-config.5.en>
  - ranger `--choosefiles`, lf `-selection-path`: file managers double as multi-file choosers.
    <https://manpages.ubuntu.com/manpages/bionic/man1/ranger.1.html>, <https://github.com/gokcehan/lf/blob/master/doc.md>
  What to take: pattern tagging, an always-visible but compact list, forgot-attachment regex, picker-as-plugin. What to leave: mutt's per-attachment
  MIME-type/description editing UI (YAGNI), and a full-screen TUI as the *only* picker (OmaLook is a GUI under Quickshell).

## 3. Design

### 3.1 The attachment model (one shape for everything)

```
AttachmentDraft { id, name (as sent), source_path, size, mtime, mime, origin, state }
origin: picked | dropped | clipboard | forwarded-original
state : ok | missing | changed | too-big | blocked(reason) | uploading | sent | failed(reason)
```
UI and daemon both speak this shape. Provider limits and warnings are properties computed from it, not special cases.

### 3.2 Snapshot at queue time (fixes the "path only" weakness)

When Send is pressed (or a draft is saved), the daemon:
1. re-validates every file (exists, regular, readable, size/mtime recorded),
2. copies each into a **spool** `~/.cache/omalook/outgoing/<outbox id>/`   (reflink where the filesystem offers it, otherwise a plain copy; never a hard link, because an edit in place would change the sent bytes),
3. puts the spool paths in the outbox row.

Effects: the file can be deleted, moved or edited during the undo window and across retries without changing what is sent; Undo restores
the original `source_path` chips; the spool is deleted when the outbox row reaches `sent` or `cancelled` (and swept at startup for orphans).
Limit: spool counts against disk, so refuse above the account limit before copying, and show a clear "disk full" error. The existing
150 MB Graph cap bounds the worst case. (Alternative considered: keep references and re-check mtime/size at send, fail on change. Cheaper, but
a failed send after the user has moved on is the worse failure mode. Open question Q1.)

### 3.3 Provider rules (a small capability table, shown in the UI)

| | Graph | Gmail |
|---|---|---|
| Total limit | 150 MB (existing constant; reconfirm) | **Hard 35 MiB (36,700,160 B) encoded on the upload endpoints (measured, phase 0)**, about 25 MB of raw files; soft warning above ~25 MB encoded. Not the 25 MB the docs and UI quote: that is not enforced at the API |
| Small file | draft + `/attachments` (base64) < 3 MB | part inside the MIME |
| Large file | `createUploadSession`, 3.75 MB chunks | still one RFC 822 message; use the upload endpoint (`/upload/gmail/v1/users/me/messages/send`, media, resumable above ~5 MB) and **not** the JSON `raw` field: `raw` has no API size limit but is ~2x slower and accepts messages that then bounce (phase 0) |
| Memory | streamed per chunk | **stream-encode MIME to a temp file** (chunked base64 + header folding), then upload from the file; never hold 20 MB × 1.37 in RAM |
| Filenames | JSON `name` | RFC 2231/5987 for non-ASCII names (`filename*=UTF-8''…`), plus an ASCII fallback; tested |
| Content type | from extension | same helper (`mime_guess`-style table, `application/octet-stream` fallback) shared by both providers |

The limit shown in the UI follows the **From** account: switching From re-evaluates every chip (turns one red with "too big for Gmail: 24 MB limit")
instead of failing at send.

If the total exceeds the limit: Send is blocked with the exact numbers and the biggest file named. No automatic link/zip/split in v1 (non-goal);
a "share a link instead" feature can be proposed later, on top of OneDrive/Drive scopes OmaLook does not request today.

### 3.4 Forwarding: make the originals visible and consistent

- Forward shows the original's attachments as ordinary chips tagged `↪ original`, individually removable. Reply and reply-all carry none.
- Graph: keep `createForward` but, before send, delete from the server draft the originals the user removed (`DELETE …/attachments/{id}`).
  Alternative if that proves fragile: build the forward as a new draft and re-attach only the kept originals using the existing reading-side download code.
- Gmail: the daemon fetches the kept originals through the existing reading-side code (`src/attachments.rs`, `attachments.get`) into the spool and builds them into the MIME.
- Originals count toward the limit and appear in the meter, so the user sees *why* a forward is too big.
- Optional (phase 6): `:forward-eml`, forward the message as an `.eml` attachment (mutt's `mime_forward`); both providers can build it from the raw message.

### 3.5 Display: one line, fixed height, any number of files

Replace the wrapping chip bar by a **single strip** between the header and the editor, mirroring the reading-pane line:

```
📎 3 files · 4.2 MB of 18 MB   report.pdf  budget.xlsx  +1        Ctrl+Shift+A add · Ctrl+Shift+L list
```
Rules:
- Zero attachments: the strip does not exist (no wasted row, no empty label).
- 1 to 3 files: names as small chips on the line, elided in the middle, full name in a tooltip, × on each. 4 or more: `+N` overflow, no chips.
- The **meter** is text, not a bar: `4.2 MB of 18 MB`; turns the warning colour above 80 %, the error colour above 100 % (theme keys only, per the colour work in `PLAN-colours.md`; no hard-wired colours).
- A glyph per chip for state (`!` missing/changed, `↪` forwarded original, `…` uploading), with the reason in the tooltip.
- Strip height is constant, so the editor never jumps, whether there are 1 or 40 files.

**List panel** (Ctrl+Shift+L, or click the strip): a bounded popup anchored to the strip, at most ~6 rows tall, scrolling, same style as the
existing attachment picker/palette so it is not new UI. Rows: index, name, size, origin glyph, state. Keys inside: `j/k`/arrows move,
`x` or `Delete` remove, `a` add more (opens the picker), `R` rename-as-sent (phase 6), `Esc` returns focus to the editor. Palette twins:
`:attach`, `:attachments`, `:detach` (last), `:detach-all`.

### 3.6 Picking: built-in first, three ways in

1. **Built-in browser** (`AttachBrowser.qml`), upgraded, stays the default because it works inside Quickshell with no terminal:
   - **Selection persists across folders** (tick in one folder, navigate, tick more, Enter attaches the whole set). This is the missing piece for "many files in one go".
   - **Tag by pattern** (mutt's `T`): `t` opens a one-line prompt, `*.pdf`, `2026-*.png`; ticks every matching file in the folder. `*` / `Ctrl+A` ticks all files, `u` clears.
   - Footer shows `5 ticked · 12.4 MB of 18 MB`, row tint if a file alone is too big, dim + reason for folders and unreadable files.
   - **Bookmarks and recents**: `[attachments] bookmarks = ["~/Documents", "~/Pictures"]` in `settings.toml`, `Ctrl+B` lists them with the recent folders and recent files (last 20 attached, stored in the cache DB). Starts in the last folder used (as today).
   - Sort by name/date/size (`s` cycles), newest first is the common attach case.
2. **External picker command** (aerc contract): `[attachments] file_picker_cmd = "yazi --chooser-file=%f"`. The daemon launches it in the user's terminal
   (`xdg-terminal-exec`, which Omarchy ships, else `$TERMINAL`), waits, then reads the paths from the `%f` temp file or stdout (one per line). It returns the same
   `[{path,name,size}]` the built-in browser does. If not configured, the command simply does not appear. Exact flags per tool are documented in
   `KEYS.md`/settings help rather than hard-coded; yazi's chooser flag must be confirmed on a real install.
3. **Files from outside the picker**:
   - **Paste**: Ctrl+V in the compose pane when the clipboard holds files (`text/uri-list`, `x-special/gnome-copied-files`) or a plain `file://`/absolute path list attaches them; otherwise paste behaves as text. Spike on Wayland first (`wl-paste --list-types`).
   - **Drag and drop** from a file manager onto the compose pane (QML `DropArea` accepting URLs). Spike inside Quickshell first; ship only if it works reliably.
   - **From the command line / other apps** (later): `omalook --attach file…` or an MCP/HTTP "compose with attachments" entry so `xdg-open mailto:` style handoffs and file-manager "Send to" can pre-fill a compose. Out of v1; recorded so the model (3.1) leaves room.

### 3.7 Forgot-attachment check and send-time guard rails

- **Forgot attachment** (neomutt's `abort_noattach`): on send with zero attachments, scan the text the user typed (quoted reply text excluded) for the regex in
  `[attachments] forgot_regex` (default `\b(attach(ed|ment|ments)?|enclosed|see the file)\b`, case-insensitive). On a match, a confirm dialog
  (existing `ConfirmDialog`): **Attach… / Send anyway / Cancel**. `[attachments] forgot_check = false` turns it off.
- **Sensitive-path tripwire** (cheap, optional): attaching anything under `~/.ssh`, `~/.gnupg`, `~/.aws`, or named `*.pem`, `*.key`, `.env`, asks once. The list is a
  setting; OmaLook is a keyboard tool on a developer's machine, and the mistake costs a lot.
- **Empty or zero-byte file**: warn in the strip, allow.
- **Duplicate name**: two chips named `scan.pdf` are allowed by both providers; show `scan.pdf` and `scan.pdf (2)` in the strip only, and send under the real names unless the user renames (phase 6).
- Everything above runs *before* the outbox accepts the message, so the undo-send window stays a clean "I changed my mind" and not a place to catch errors.

### 3.8 Drafts (dependency on PLAN-compose.md B2)

When drafts exist, an attachment is persisted by **spool path + original path + size/mtime** in the draft row; the spool is keyed by draft id and kept until the draft is sent or discarded.
Provider drafts: Graph pushes files with `/attachments` on save (large via upload session, same code as send); Gmail drafts carry the files in the MIME that `drafts.update` rewrites,
so push is debounced harder and skipped over ~10 MB until close/send. On resume, a missing spool file shows a `!` chip, never silently drops. No work in this plan until B2 lands; the model above is built so B2 only adds persistence.

## 4. Files likely to change

Rust: `src/compose.rs` (MIME `multipart/mixed`, limits table, forgot-regex helper), new `src/attach_spool.rs` (snapshot, sweep), `src/outbox.rs` (re-validate, spool lifecycle),
`src/providers/gmail.rs` (stream MIME, upload endpoint), `src/providers/graph.rs` (forward originals removal), `src/attachments.rs` (reuse original download for forward),
`src/fs_browse.rs` (recents, bookmarks, sort, glob), `src/settings.rs` + `src/models.rs` (`[attachments]` keys), `src/main.rs` (routes), `src/db.rs` (recent files, optional).
QML: `qml/compose/AttachBrowser.qml`, new `qml/compose/AttachStrip.qml` and `AttachList.qml`, `ComposeView.qml` (swap chip bar), `qml/keys/AppCommands.qml`, `qml/compose/modes.js` (stale text), `qml/settings/pages/MailPage.qml`.
Docs: `docs/KEYS.md` (compose attach section: it is missing today), `docs/TRACKER.md`, `docs/PLAN-compose.md` (mark Gmail attachments done when it is).

New routes (sketch): `GET /attach/limits?account=`, `POST /attach/validate` (paths → `AttachmentDraft[]` with state), `GET /attach/recents`, `POST /attach/pick-external`. Compose send/cancel unchanged on the wire except for the extra `forwarded_keep` ids.

## 5. Phases (each ends with a clean `cargo build --release`, tests, check-in; commits on `main` per AGENTS.md)

### Phase 0 — Spikes (1 session, results recorded here)
- Gmail: largest message that `messages.send` accepts through `raw` vs `uploadType=media`/`resumable`; real encoded limit (25 MB) behaviour and error text.
- Graph: confirm `createForward` copies attachments, and that `DELETE …/attachments/{id}` on the draft works before send.
- Wayland: what a file manager puts on the clipboard (`wl-paste --list-types`); does a QML `DropArea` receive file URLs inside Quickshell?
- `xdg-terminal-exec` launching a picker from the daemon and returning via a temp file.
- Done when: the four answers are written in this section; any "no" removes its item from phase 5.

**Phase 0 results (2026-11-01; four of five spikes answered, drag-and-drop still needs a hand drag)**

Gmail and Graph were run live from a throwaway integration test (`tests/spike_gmail.rs`, not committed) against `adam.macielinski@gmail.com` and the Exchange account. Message sizes below are the encoded RFC 822 size, attachment bytes incompressible. Every spike message was moved to Trash afterwards (Gmail), or removed (Graph forward draft).

| Spike | Result |
|---|---|
| Gmail: size limits by send method | **`uploadType=media` and `resumable`: hard limit 36,700,160 bytes (35 MiB) of message.** Over it: `413`, `"Media is too large. Limit: 36700160"`, `FAILED_PRECONDITION`. Accepted at 10, 20, 24, 26, 30 MB (resumable also 24, 26). **The JSON `raw` field has no such limit at the API:** accepted at 4, 10, 20, 24, 26, 34, 40 and 60 MB (a 60 MB request took 19 s). |
| Gmail: is 25 MB enforced? | **Not at the API.** A 24 MB and a 27 MB (28.3 MB encoded) message sent to the Exchange account both returned 200 and **arrived** (both in the cache with the attachment flag, ~1 min). No bounce. The only bounces seen were 40 MB and 60 MB messages sent to self (arrive minutes later in Inbox as a "Mail Delivery Subsystem … Your message was not sent" message, subject `Re: <original>`); 34 MB to self went through. So the real ceilings are: 35 MiB for the upload endpoints, something between 34 and 40 MB for delivery, plus whatever the **recipient** server accepts (Exchange Online took 28 MB). Not tested: an external recipient with a 25 MB cap. |
| Gmail: speed | 24 MB: media 3.8 s, resumable 5.0 s, raw 7.6 s. Raw JSON is slowest (base64 twice, ~1.33x extra). |
| Graph: does `createForward` copy attachments, can they be deleted from the draft? | **Yes to both** on a personal (non-shared) mailbox, with a 24 MB attachment: `createForward` returned 201 with `hasAttachments=true`, the draft listed the inherited `spike.bin`, `DELETE …/attachments/{id}` returned 204 and the list was then empty; deleting the draft returned 204. The 503 reported for shared mailboxes (Microsoft Q&A 5844120) did not occur here. So keep the planned "delete un-kept originals" route (3.4) for personal mailboxes and keep the rebuild-draft route as the fallback if a delete fails. |
| External picker via `xdg-terminal-exec` + temp file | **Yes.** It blocks until the command exits (3 s sleep: returned after 3 s, exit 0) and a temp file written by the picker is readable afterwards. Terminal is foot; it prints a harmless `xdg-toplevel-icon` warning on stderr. fzf is installed; yazi, ranger and lf are not, so yazi's chooser flag is still unconfirmed. |
| Clipboard file types from a file manager | **Yes.** Nautilus, after Ctrl+A / Ctrl+C on two files, offers: `x-special/gnome-copied-files` (`copy\nfile:///…\nfile:///…`), `text/uri-list` (`file:///…\r\n` per file), `text/plain;charset=utf-8`, `application/vnd.portal.filetransfer`, `application/vnd.portal.files`. Read `text/uri-list` first (percent-encoded URIs, CRLF), fall back to `gnome-copied-files` (first line is `copy` or `cut`). Other file managers not checked. |
| QML `DropArea` inside Quickshell | **Not run.** Needs a real drag from Nautilus onto a scratch OmaLook window; a script cannot do it reliably. Ships only if it works (phase 5). |

What changes in the plan:
- **3.3 Gmail limit:** do not hard-code "25 MB including base64". Use the **upload endpoint** (`media`, or `resumable` above ~5 MB per Google's guide) with a **hard cap of 35 MiB encoded** (a 413 otherwise), show a **soft warning above ~25 MB** ("some recipients' servers may reject this"), and never use the JSON `raw` field for messages with attachments (no size gain, 2x slower, and it accepts messages that then bounce). Raw files ≈ encoded x 0.73, so the hard cap is about 25 MB of raw files and the warning starts near 18 MB.
- **3.4 Forward on Graph:** the delete route works; the fallback stays.
- **Q2 (open question 2)** now reads: block above 35 MiB, warn above 25 MB. Needs Adam's agreement.
- **Cleanup:** the Gmail test messages are in Trash. Graph `DELETE` on a message is a hard delete into Recoverable Items (not Deleted Items), so the Exchange test copies are there. In phase 2 a test with a too-loose filter also deleted 10 real "[Preview] Report Domain" messages the same way; all 10 were moved back to the Inbox from Recoverable Items and the count was checked (29 before and after).

### Phase 1 — Truth and safety (backend only, no visible change)

**Status: done 2026-11-01** (361 lib tests pass; not yet run through the live HTTP path: see TRACKER OL-035). Built as `src/attach_spool.rs`, limits table in `src/compose.rs` (`attachment_limits`, `check_attachments`), worker lifecycle in `src/outbox.rs`, `/attach/limits` in `src/main.rs`. Deviations from the text below: the spool is also removed when a send ends `failed` (nothing can retry a failed row, so keeping it would only leak disk); a spool younger than 1 hour is never swept (it may still be copying); the Gmail refusal stays until phase 2; the disk-full test exercises the write-failure path and the ENOSPC mapping separately because a full disk cannot be faked portably.
- Spool snapshot (3.2), send-time re-validation, orphan sweep; unit tests: file deleted, replaced, grown during the undo window; Undo restores source paths; disk-full; sweep removes orphans.
- Capability table per provider (3.3) and `GET /attach/limits`.
- Document the existing compose attach keys in `KEYS.md`; fix the stale `modes.js` text.
- Done when: a Graph message sent with a file deleted mid-window still delivers the original bytes, and a replaced file does not leak into the send.

### Phase 2 — Gmail attachments (parity with Graph)

**Status: done 2026-11-01** except the UI listing of a forward's originals (phase 3). Built: `compose::write_mime` (multipart/mixed around plain or alternative, files base64-streamed in 57 x 2048-byte chunks, RFC 2231 names), `GoogleApi::upload_message` (resumable upload from a temp file, 8 MiB chunks, resumes from Google's reported offset), `GmailProvider::send_message` (files go through the upload path, no files keep the old `raw` path), `GmailProvider::fetch_originals` + `attachment_parts` (forward originals by MIME part id, `OutgoingMessage.forwarded_keep`), both refusals removed (daemon and `ComposeView.qml`).

Live results against the real Gmail account (sent to self, read back through the API, bytes compared; this run: 1 file, 5 files including an empty and a 1-byte file and one crossing the 57 x 2048 chunk edge, a 15 MB file in 4.9-6.4 s, a non-ASCII name, an over-limit refusal, a threaded reply with a file, a forward with 2 kept originals plus a new file) all matched. Sent to the Exchange mailbox: name `für-outlook.bin` and all 200,000 bytes matched.

Findings that changed the code:
- **Gmail uses the first of `filename` / `filename*`.** With the ASCII fallback first the API returned `Pr_fung.pdf`; with `filename*=UTF-8''...` first (or alone, or as an encoded word) it returned the real name. Headers now put the `*` form first, for `name` and `filename`.
- **Gmail has no attachment listing on the reading side yet** (the message list says "Gmail accounts are not supported yet"). The plan assumed reading-side code to reuse for forwarding originals; there was none, so `fetch_originals` reads `format=full` itself. Phase 3's forward chips for Gmail need a list of a message's attachments (`attachment_parts` is the start of it).
- A forward on Gmail with `forwarded_keep: None` still sends no originals (Graph still keeps all); phase 3 makes both follow the user's chips.
- Not run: a Gmail message at 25-35 MB through `send_message`; network drop mid-upload; resume after a 308 (the logic is written but only the all-in-order path was exercised live).

- `build_mime`: `multipart/mixed` wrapping `alternative`/`plain`; streamed base64 (76-column lines), RFC 2231 filenames, content-type helper; golden tests (one file, many files, non-ASCII name, large file does not buffer, HTML + attachment, reply with threading headers + attachment).
- Gmail upload path for large messages; limit enforcement from the table; remove both refusals (UI and `compose_send_response`).
- Forward originals on Gmail via the spool (3.4).
- Done when: a live Gmail send to self with 1 file, 5 files, a 15 MB file and a non-ASCII filename arrives intact in Gmail web and in Outlook web; an over-limit send is blocked with numbers.

### Phase 3 — Compact strip and list panel

**Status 2026-11-01: built, partly verified.** See `docs/HANDOFF-compose-attachments.md` for what was seen on screen, what was not (Send refusal, palette twins, keyboard-only walkthrough, live forward on Graph and Gmail) and the known gaps. Rules chosen: up to 3 files get a chip each; with 4 or more, two chips and `+N`; no strip at all with 0 files.
- `AttachStrip.qml`, `AttachList.qml`, commands `compose.attach`, `compose.attachments`, `compose.detach`, `compose.detach-all` (+ palette names), state glyphs, meter by From account, From-switch re-evaluation.
- Forwarded originals appear as `↪` chips; Graph removal of un-kept originals.
- Offscreen QML smoke test with 0, 1, 3, 4 and 40 files (constant strip height, editor does not move), keyboard-only walkthrough.
- Done when: composing with 40 attachments looks the same height as with 4, and every strip/list action works without the mouse.

### Phase 4 — Picker upgrade (built-in)
- Cross-folder persistent selection, pattern tag, tick-all/clear, totals footer and over-limit tint, bookmarks and recents, sort cycling. Tests (node) for the glob matcher and selection model; manual run with 200-entry folders and a 5000-entry folder.
- Done when: 12 files from two folders are attached in one Enter, and `t *.pdf` ticks exactly the PDFs.

### Phase 5 — Other ways in
- Clipboard file paste, drag-and-drop (only if the phase-0 spike passed), external picker command with the aerc output contract. Each is independently shippable and behind its own check.
- Done when: a file copied in the file manager pastes as a chip; a yazi (or fzf) picker configured in `settings.toml` returns multiple files.

### Phase 6 — Guard rails and extras
- Forgot-attachment check, sensitive-path tripwire, rename-as-sent, `:forward-eml`, duplicate-name display.
- Done when: "see attached" with no file prompts; `.ssh/id_ed25519` asks; the regex is changeable and can be turned off.

### Phase 7 — Drafts integration (starts only after PLAN-compose.md B2)
- Persist attachments with drafts (3.8), provider draft attachment push, resume with `!` chips.

## 6. Testing and verification
- `cargo test --lib -- --skip test_copy_to_clipboard`: spool lifecycle, limits per provider, MIME goldens, filename encoding, forgot-regex cases (quoted text ignored), tripwire list.
- `node tests/…`: glob matcher, picker selection model, strip layout rules (which of chips/`+N`/count shows for N files).
- Offscreen QML smoke for strip/list; keyboard-only walkthrough recorded in the phase check-in.
- Live matrix (self-sent, opened in the web client): {Graph, Gmail} × {new, reply, forward-with-originals} × {1 small, many small, one large, non-ASCII name, file deleted during undo window}.
- Failure drills: network drop mid-upload (retry, no duplicate, no double spool), expired token (existing reauth modal), disk full while spooling.

## 7. Risks
- Gmail's real size ceiling and error behaviour are the biggest unknown (phase 0). If `raw` fails at a size lower than 25 MB, the upload endpoint is mandatory, not optional.
- Spool disk use and cleanup bugs (orphans). Mitigation: sweep at startup, hard cap, tests.
- Drag-drop and clipboard files may not work reliably under Quickshell; the plan survives their loss.
- External picker needs a terminal that can run it; failure to launch must surface a readable message and leave the built-in browser available.
- Forwarded originals depend on the exact Graph copy behaviour; fallback is the rebuild-draft path.
- The working tree holds unrelated uncommitted work (`scripts/`): stage selectively, never `git add -A` (AGENTS.md).

## 8. Open questions for Adam
1. **Snapshot vs reference** (3.2): spool copy at send (recommended; costs disk briefly) or keep references and fail on change?
2. **Gmail limit stance:** block above the account limit (recommended), or allow Send and let the provider reject?
3. **Strip auto-chips:** names shown for up to 3 files (proposed); would you rather see only the count always?
4. **Keys:** keep Ctrl+Shift+A (attach), Ctrl+Shift+Backspace (remove last); add Ctrl+Shift+L (list). Free in compose? (to be checked against `KEYS.md` / Hyprland binds at build time)
5. **External picker:** worth building (phase 5), or is the improved built-in browser enough?
6. **Sensitive-path tripwire** and **forgot-attachment check**: both on by default (proposed)?
7. **Forward-as-.eml** (mutt style): wanted at all?

## 9. Answers to questions in section 8.
1. 
2. block above the account limit - whatever the email provider, be it gmail, graph, or any other provider we add in the future.
3. let's list for up to 3, for now, and we can adjust later if needed.
4. proposed key binds are OK, but need to be aligned with my answers in the other PLAN document.
5. i prefer a built-in picker, along the style of superfile, but maybe simpler, as the goal is just to pick files, not replicate all superfile functionality.
6. no need for the sensitive path or forgot attachment tripwires at this time.
7. i don't understand.


