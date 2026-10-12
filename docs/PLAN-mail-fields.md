# Mail details and categories

Built 2026-10-09 on `feature/mail-fields` (branched from `feature/keyboard`). Decisions: Adam, same day.

## Fields now stored per message
| Field | Exchange (Graph) | Gmail |
|---|---|---|
| sender name (`from_name`) | `from.emailAddress.name` | display name in `From` |
| To / Cc / Bcc (`to_text`…) | `toRecipients`… as `Name <addr>, …` | `To` / `Cc` / `Bcc` headers |
| categories | `categories` (names) | the message's user labels, as names ("tags") |
| sent time (`sent_at`) | `sentDateTime` | `Date` header |
| importance | `importance` | `Importance` / `X-Priority` header |
| has attachments | `hasAttachments` | top-level `multipart/mixed` (heuristic) |
| conversation id | `conversationId` | `threadId` (kept for threading later) |

Not taken (suggested, declined or left out): flag status, "sender" (on-behalf-of), message body (the body is still fetched on open).
`messages.meta_ok` = 0 marks rows stored before this; 1 once their details are in.

## How new messages get them
Both providers' fetches now ask for the extra fields; Graph's change feed carries them too, so category changes made in
Outlook arrive as ordinary feed changes. Rows written by new code are born with `meta_ok = 1`.

## One-time repopulate of existing messages
* **Exchange:** the stored feed links bake in the old field list, so on first start with the new build each account's feeds are
  reset once (`account_flags.fields_v1`) and the normal walk (500 per page, newest first, resumable) rewrites every row with its
  details. ~60k messages took ~10 minutes of background work. Pushes (read flags, archive/delete/move, categories) are serviced
  between pages, and the UI is told at most every 30 s while it runs.
* **Gmail:** no feed, so details are fetched message by message (10 at a time, throttled), a ~60 s slice per sync cycle,
  everything, newest first. A message that is gone from the server (404) is marked done.
* Progress: `GET /messages/meta_progress`; the bottom bar shows "filling in message details · N to go".
* Back up first: `~/.config/omalook/backup/messages-before-fields.db` was taken before the first run.

## Categories
* Key **`t`** (message list / reading pane; applies to the marked messages, else the cursor row) opens a picker: existing
  categories with colour swatches (✓ all / – some of the targets carry it), type to filter, Enter toggles. A name that matches
  nothing offers **+ New category "x"…**, then a colour picker (12 colours). `:tag` / `:category` do the same.
* The picker says category (Exchange), tag (Gmail) or label (other providers) and lists that account's own set.
* Exchange: names sync as the message's `categories`; the account's master list gives the colours, new categories are created
  there with an Exchange colour preset. Gmail: a tag is a user label (add/remove label; new tag = new label with an allowed
  Gmail colour). Changes show at once and are pushed by the daemon (retry on transient errors; a refusal puts the old value back).
* Scope: Exchange reads/writes the master list with `MailboxSettings.ReadWrite`. Token tiers are now plus → full → base; an
  account that lacks the new permission (or an app registration that does not list it) falls back without losing anything, and
  the picker shows a sign-in hint with no colour step. On Adam's account the list was readable without re-signing in.
* Not built: renaming / recolouring / deleting categories, a categories page in Settings, search by category.

## Backend surface
`GET /messages` rows gain `from_name, to_text, cc_text, bcc_text, sent_at, importance, has_attachments, conversation_id,
cats` (JSON text `[["name","#rrggbb"],…]`). `GET /categories?account=`, `POST /categories/create?account=&name=&color=`,
`POST /messages/category?ids=&op=add|remove&name=`, `GET /messages/meta_progress`.

## Known limits
* The full-text index no longer re-indexes `from_name` on update (it did a full scan per row); names filled in by this
  repopulate are not in the FTS table. Rebuild it when search is built.
* Creating a category on the provider (Graph `masterCategories`, Gmail `labels.create`) is covered by unit tests of the
  request shapes only; it was not exercised against a live mailbox to avoid leaving test categories behind.
* `test_copy_to_clipboard` in the lib tests hangs without a clipboard (pre-existing); run tests with `--skip test_copy_to_clipboard`.
