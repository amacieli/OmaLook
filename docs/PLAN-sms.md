# PLAN: SMS search, send, new thread, read/delete (KDE Connect)

Status: IMPLEMENTED 2026-10-15 (delete scrapped by owner; read sync = notification trial, see docs/SMS.md). Baseline: read-only SMS cache + UI already exists (docs/SMS.md).

## What KDE Connect can and cannot do (verified by D-Bus introspection, kdeconnect 26.08, 2026-10-15)
| Need | API | Verdict |
|---|---|---|
| Send new thread / to numbers | `conversations.sendWithoutConversation(av addrs, s text, av atts)`; addrs must be `(s)` structs | Works (verified in notes). |
| Reply in thread | `conversations.replyToConversation(x tid, s text, av atts)` | Exists, UNTESTED. Spike first; fall back to sendWithoutConversation to the thread's addresses. |
| Mark read on phone | **none** on `conversations` or `sms` interfaces | Not possible directly. |
| Delete on phone | **none** | Not possible. |
| Phone notifications | `notifications/<n>`: `dismiss()`, `sendReply(s)`; `notifications.sendAction(id, name)` | Possible indirect route for read sync (see M5). |

## Milestones (each ends with cargo test + live check, committed separately)

**M1  Search (local, FTS5).** `sms_fts` (body) + address/name matching.
- Backend: `GET /sms/threads?q=` returns threads where body matches (FTS), or address digits match, or contact name matches (sms_names reverse lookup). Result row carries a match snippet.
- UI: `/` in SMS view opens the same search prompt as mail; results replace the thread list; Esc clears. Search matches highlighted snippet shown as preview line.
- Tests: FTS index kept in sync by upsert/delete triggers; number normalisation (+1-704..., 704...); name match.

**M2  Send in an existing thread.** 
- Backend: `POST /sms/send {device, thread, text}` -> `replyToConversation` (spike decides vs sendWithoutConversation). Insert optimistic row (direction 2 "queued", temp uid), reconcile when the real `type=2` echo arrives (match thread+body+time window); mark `failed` after timeout (~2 min) with a visible retry. No auto-resend (echo lags 10-60 s; avoids duplicates).
- UI: composer line at the bottom of the conversation pane; `i`/Enter on the conversation pane focuses it (text field owns its keys), Ctrl+Enter or Enter sends (Shift+Enter newline; I'll pick Enter=send, confirm below), Esc leaves the field keeping the draft.
- Guard: device unreachable -> error toast, nothing queued.

**M3  New thread.** `n` / `c` in SMS view opens a recipients row (chips like mail compose): contact name autocomplete from sms_names vCards + raw numbers; at least one required; Enter/Tab/comma converts to chip. Body below; Ctrl+Enter sends via `sendWithoutConversation` (multi-recipient = group MMS thread). Existing thread with the same single number is opened instead of duplicated. New thread appears after phone echo (placeholder row until then).

**M4  z / x on a whole thread** (matches mail semantics).
- `z` toggles read/unread for the whole thread (all messages) locally, immediately. Opening a thread no longer auto-marks read (today it does after 0.7 s) -- behaviour change, per your "you have to press z".
- `x` deletes the thread after the same confirm dialog as mail (Cancel preselected).
- LIMITATION (see Q1): the phone cannot be told to delete. Local delete needs a tombstone table so the next full sync does not resurrect the thread (new message after deletion revives it, only messages newer than the tombstone show).

**M5  Read sync back via KDE Connect (best effort).** No read API exists. Spike: on `z`-read, find the matching notification (`isConversation`, title/text match) and call `dismiss()` / `sendAction` and see whether the phone's SMS app (Google Messages?) marks the thread read. Ship only if the spike proves it; otherwise leave read state local and say so in UI/docs. Also keep the existing rule: phone-read turns local read, local read never reverts from stale phone unread.

## Order and size
M1, M2, M4 first (no phone-side unknowns), then M3, then M5 spike. Rough touch list: `src/db/sms.rs`, `src/sms_daemon.rs` (send + echo reconcile), `src/main.rs` routes, `qml/views/SmsView.qml`, `SmsThreadList.qml`, `SmsConversation.qml` (composer), new `SmsComposeNew.qml`, `qml/keys/Commands.js` + `AppCommands.qml` (sms-scoped keys), help overlay entries, docs/SMS.md, tests (Rust unit + D-Bus fake where possible). No send test to a real number without your OK on a number to use.

## Open questions
1. Delete: accept local-only delete with tombstones (phone keeps the messages)? Or drop `x` until a phone-side route exists?
2. Read sync: OK to ship only if the notification-dismiss spike works, else local-only?
3. Composer: Enter sends or Ctrl+Enter (Enter = newline)? Default proposed: Ctrl+Enter, same as mail.
4. Which number should I use for the live send test (your own number via another line, or a family member)?
5. Opening a thread should stop auto-marking read -- confirm.
