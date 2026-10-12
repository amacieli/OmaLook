# Search and filtering

Backlog item 1 (mutt-parity). Built 2026-10-12 on `feature/search-filter`. Keys and prompts are in `docs/KEYS.md`.

## What it does
`/` (or `:search <pattern>`) searches the open view: one folder, or an "all accounts" folder. Results replace the list; `Esc`
(in the list), `:clear` or switching folder puts the plain folder back. A pattern is keyword terms, all ANDed, `-` negates:

    from:stripe is:unread after:30d "exact phrase" -newsletter cat:"Saint Lucia"

| Term | Matches |
|---|---|
| `word`, `"a phrase"`, `wor*` | subject, sender name, cached preview (FTS5, stemmed) or the sender address |
| `from:` `to:` `cc:` `bcc:` `subject:` | substring of that field, any case |
| `body:` | the cached body preview (FTS5) |
| `fullbody:` | the whole message text, asked of the mail server (see below) |
| `cat:` / `category:` | category name, substring, ignoring case and spaces |
| `is:unread` `is:read` `is:important` | read state, importance |
| `has:attachment` | a real (non-inline) attachment |
| `after:` `before:` `date:` | received date: `2026-10-01`, `today`, `yesterday`, relative `7d` `2w` `3m` `1y` (`date:` takes a day only) |

A space after the colon is fine (`cat: lucia`). An unknown field (`foo:bar`) is an error; quote text that really contains a colon.
`is:flagged` does not exist: the cache has no flag column.

`:filter` (palette only) builds terms from the selected message (sender, subject, each category, unread, attachment) and toggles
them into the current pattern, so they stack. Recent patterns are kept (50, `search_history.json` in the config dir) and shown
when the prompt is empty; with a search active, `/` `Enter` clears it.

## How it is built
- `src/search.rs`: tokenizer, parser, SQL builder (every value is a bound parameter), history store, provider query builders. Unit tests include
  a real-schema fixture (triggers + FTS).
- `GET /messages?q=` (`src/main.rs`, `messages_response`, run on its own thread). Bad pattern: 400 `{"error": ...}`.
  `GET`/`POST /search/history`.
- `qml/keys/Search.js` (pattern editing, node tests in `tests/search.test.js`), palette pickers in `qml/keys/AppCommands.qml`,
  `searchQuery` / `applySearch` / `clearSearch` in `AppState.qml`, match count in `BottomStatusBar.qml`.

## `fullbody:` (provider search)
The cache holds previews only (about 100 of 60,000 messages have a full body), so `fullbody:` asks the provider which messages match
and turns the ids into `m.id IN (...)` (`NOT IN` for `-fullbody:`), ANDed with everything else. Messages not in the cache are left out.
- **Exchange:** Graph `$search` with `body:word` (multi-word = every word, not an exact phrase). Graph returns only the newest few
  hundred hits per query with no next page, so `GraphEmailProvider::search_message_ids` searches in rounds (`received<=<day of the
  oldest hit>`) until a round comes back short.
- **Gmail:** `q=` (a multi-word value is a quoted phrase). Gmail cannot restrict to the body; subject and sender hits count too.
- Cap 1,000 ids per account (newest); `X-Search-Truncated: 1` makes the status bar say "(newest only)". `-fullbody:` is refused above the cap.
- Results are cached 5 minutes per (account, folder, term). A provider error is a 400 with the reason; the list is left alone.

## Measured (2026-10-12, 60,083-row cache, folder = Inbox - all accounts, 200 rows)
Release build: p50 15-22 ms, worst 28 ms over eight typical patterns (target < 50 ms). Debug build: worst 65 ms.
`fullbody:` first call: 0.8-4 s (network), then cached.

## Not verified
- Gmail `fullbody:` against live Gmail (no stored Gmail sign-in at the time).
- The QML flows in the live `/opt/omalook` install (`./build.sh --live` not run).

## Not done / ideas
Saved searches; remote results for mail not yet synced; `is:flagged` (needs a flag column).
