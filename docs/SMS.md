# SMS cache (KDE Connect)

**Source:** local `kdeconnectd` over the session bus (`org.kde.kdeconnect.device.conversations`, zbus blocking API). Every paired, reachable device whose SMS plugin answers is mirrored.

**Tables** (`src/db/sms.rs`, in `messages.db`): `sms_messages` (PK device, mms, uid; `is_read` = shown state, `phone_read` = phone's flag), `sms_threads` (`history_complete`, phone's reported count), `sms_devices` (name, `full_synced_at`).

**Read state:** local and authoritative. KDE Connect has no mark-read call. A sync can turn a message read when the *phone's* flag changes 0 -> 1, but never overrides what you set here (a refresh that repeats the phone's old flag changes nothing, so `z` unread sticks). `z` reads the whole thread; marking unread flips only the newest received message (a badge of 1), like mail.
Best-effort phone sync: on read, `sms_daemon::sync_read_to_phone` finds the SMS app's notification for that contact and sends its "Mark as read" action over `notifications.sendAction`. **UNVERIFIED**: dismissing the notification does not mark the thread read (tested with Textra, 2026-10-15) and no live Textra notification was available to test `sendAction`; the action names tried are "Mark as read" / "Mark as Read" / "Mark read". Results are logged (`SMS read-sync:`). Read state on the phone is otherwise unchanged.

**Daemon** (`src/sms_daemon.rs`, thread `sms`, started in `main.rs` after the scheduler):
1. Discover devices every 30 s (`daemon.devices(true,true)`; no kdeconnectd -> retry). While a paired device is not reachable, each pass (at most once a minute) also calls `daemon.forceOnNetworkChange()`, the same call as the KDE Connect app's refresh button, so a phone that dropped off the network is looked for again.
2. Full pull per device: `requestAllConversationThreads`, wait for the thread count to settle, then `requestConversation(thread, 0, 100000)` per thread and wait for `conversationLoaded`. Failed threads retry (3 attempts). ~11 min for 960 threads / 11k messages.
3. Update path: after the pull, `conversationCreated/Updated/Removed` signals are upserted live; thread heads are re-requested every 10 min as a safety net. The UI serial (`GET /sync/serial` -> `"sms"`) bumps once per batch of real changes.

**Search:** `sms_fts` (FTS5, external content over `sms_messages.body`, kept by triggers; built on first run). `GET /sms/threads?q=` keeps threads whose contact name or phone digits contain the query or whose messages contain every word (word-prefix); rows carry `match` (name|number|text) and a `snippet`.

**Sending:** `POST /sms/send` (JSON `{device?, thread?, to?, text}`) -> `sms_daemon::send_text`: `replyToConversation(thread, text, [])` (verified live) or, for new conversations, `sendWithoutConversation([(s)...], text, [])`; `to` reuses the existing thread with exactly those recipients. The text is first stored as a *pending* row (negative uid, direction 6 queued). The phone's echo (a normal sent message, usually within seconds) replaces it (`sms_reconcile_pending`: same body, time window, same thread or recipients). A refused send, or no echo within 2 min (`sms-outbox` ticker), turns the row into direction 5 = **Send failure** (red bubble, warning triangle, `R` retries; a late echo still heals it). A brand-new conversation lives under a stand-in negative thread id (`pending_thread_id`) until the phone reports the real one; `known_threads` ignores negative ids so the sync never asks the phone for them. `POST /sms/retry?thread=|uid=&device=`. `GET /sms/contacts?q=` for recipient suggestions (phone vCards).

**Routes:** `GET /sms/threads?device=&limit=&q=`, `GET /sms/messages?thread=&device=&before=&limit=`, `GET /sms/unread`, `POST /sms/read?thread=&read=true|false[&uid=&device=]`, `POST /sms/sync` (forces a new full pass).

**Gotchas**
* `conversationLoaded` is ALSO emitted per thread head by `requestAllConversationThreads` (count 1). Only trust it after your own `requestConversation`; otherwise every thread is marked complete with one message.
* `requestConversation` start/end are int32 indexes, not timestamps.
* Big threads stream for minutes: the per-thread timeout is idle time.
* kdeconnectd/phone report counts a few messages higher than they emit for ~58 threads (likely SMS/MMS uid collisions inside kdeconnectd); not recoverable over D-Bus.
* MMS attachments: KDE Connect lists metadata only. Images (`image/*`) are fetched on demand (OL-032): `GET /sms/attachment?device=&part=&uid=` -> `{state: ready|pending|failed[, path|error]}`, never blocking; the first call sends `requestAttachmentFile(part_id, unique_id)`, `attachmentReceived(path, name)` is copied to `~/.cache/omalook/sms-attachments/<device>/<unique id>` and the conversation shows it inline (`SmsImage.qml`). Other attachment types stay a one-line summary. Not yet verified against a real phone.

## UI (`qml/views/SmsView.qml`, `SmsThreadList.qml`, `SmsConversation.qml`, `sms.js`)
Two framed panes like mail: senders list (¹, `focusPane` "msg") and conversation (², "reader"). Contact names come from the phone's synced vCards (`src/sms_names.rs`, matched on last 10 digits); unknown senders show the number.
* Conversation: oldest at top, newest at bottom; sent/queued/failed on the right, received on the left; day dividers; 100-message pages, older pages load on reaching the top (`gg` or scrolling).
* Opening a thread does not read it: press `z`.
* Refresh is driven by the `sms` field of `/sync/serial` (`AppState.smsRev`); a 30 s timer is the fallback.
* Keys: j/k, PageUp/Down, gg/G in the list; l/Enter or 2 opens the conversation; h/Esc/1 back.
* Deleting texts is not supported (KDE Connect has no delete call); `x` says so.
* `/` search, `z` read/unread (opening a thread no longer reads it), `r`/`i` reply (composer under the history: Ctrl+Enter send, Esc leave, drafts kept per thread), `c`/Ctrl+N new conversation (recipient chips: names from the phone or numbers; Enter/Tab/comma add, Backspace pulls back, Ctrl+Enter send, Esc discard with confirm), `R` retry failed texts.
