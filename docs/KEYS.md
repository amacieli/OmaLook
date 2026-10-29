# Keyboard bindings

Press `?` in the app for every binding, grouped by where it works, with each command's id.
`:` / Ctrl-P opens the command list.

## Rebinding: `[keys]` in `~/.config/omalook/settings.toml`

The file is re-read every couple of seconds, so edits apply without a restart. Problems (unknown ids,
bad keys, two commands on one key) are listed at the top of the `?` help overlay.

```toml
[keys]
preset = "outlook"                      # optional: adds a family of extra bindings (see below)
"mail.archive" = "e"                    # REPLACES the command's keys
"mail.reply"   = ["r", "C-r"]           # several keys for one command
"move.down"    = ["j", "Down", "n"]
"go.mail"      = "g m"                  # a chord: press g, then m
"item.delete"  = []                     # unbind
```

- Key names: a single character (`j`, `G`, `/`, `?`), `Esc Tab S-Tab Enter Space Up Down Left Right PgUp PgDown Home End Delete Backspace F1`–`F12`,
  and `C-x` (Ctrl), `C-S-x` (Ctrl+Shift) for letters and digits. `Ctrl-r`, `Shift-Tab`, `PageUp` are accepted too.
- A binding for a command id replaces all of that command's default keys. A preset only adds keys.
- Keys apply in the command's own scope (Everywhere, a pane, a module). The same key may mean different things in
  different scopes; two commands on one key in the SAME scope, or a key that is also the start of a chord there, is reported as a problem.
- Plain text input fields (search, compose, palette) always get their keys; bindings only apply when no text field has focus.

## Resizing the message list

`Ctrl-Right` / `Ctrl-Left`, or the vim chords `Ctrl-w >` / `Ctrl-w <` (commands `pane.widen`, `pane.narrow`; the chords take a count: `Ctrl-w 5 >` moves the divider five characters) move the divider between the message list and the
reading pane one character at a time; the other pane gives or takes the same width. They do nothing outside the mail view,
and will do nothing when the reading pane is switched off (`showReaderPane`). The width is kept across launches as
`[ui] list_width_offset` in `settings.toml`: characters relative to the default list width (0 = default, negative = narrower;
you can edit it by hand). Both panes keep a minimum width, so the offset stops growing at those limits.

## New calendar event

In the calendar view `c` or `n` opens the editor over the grid (commands `cal.new`). The account is the filtered one, else the account of the
event under the cursor, else the one pinned in Settings -> Calendar, else the one used last (`last_calendar_account` in `view_state.json`).
Only Microsoft work/school accounts can create events so far. While the editor is open the app's global keys are off and the fields own the keyboard:

| Key | Does |
|---|---|
| `Tab` / `Shift-Tab` | Next / previous field |
| `Ctrl-Enter` | Save: the event is queued and waits the undo window (`[mail] send_delay_secs`), then goes to the server |
| `Esc` | Close; once edited it asks, and a second `Esc` discards |
| `Up` / `Down` on When | Start +-15 minutes; `Shift` +-1 day; `Alt` length +-15 minutes |
| `Left` / `Right` / `Space` on Account or Show as | Change the choice |

When takes typed text (`tomorrow 3pm`, `fri 10-10:30`, `+30m`, `all day mon`; `qml/calendar/when.js`) and always shows the resolved time and any overlap with
existing events; Repeat takes `weekly`, `every 2 weeks on tue,thu`, `monthly 2nd tue`, `daily for 10`, `until 29 dec` (`qml/calendar/recurrence.js`) and shows the sentence and the next dates.
After a save a bar reads "Event queued (personal). Ctrl-Z undo" (`cal.undo`); `Ctrl-Z` takes it back and reopens the editor with everything typed. A queued event is drawn dimmed with a clock mark,
a failed one with a cross and the server's reason (`Ctrl-Z` reopens it too).

## Calendar sidebar width

In the calendar view `Ctrl-Right` / `Ctrl-Left` (or `Ctrl-w >` / `Ctrl-w <`, with a count; commands `cal.pane.widen`, `cal.pane.narrow`) move the divider between the sidebar (mini month, calendars, agenda) and the
calendar one character at a time. Kept across launches as `[ui] calendar_pane_offset` in `settings.toml` (characters relative to the default; 0 = default). The sidebar and the calendar keep a minimum width.

## Links in a message (reading pane)

In the reading pane, in both the system-font and the HTML view (needs the pane focused, and at least one link):

| Key | Command | Action |
|-----|---------|--------|
| `Tab` / `]` | `reader.link.next` | next link (the first press picks the first link on screen); wraps |
| `Shift-Tab` / `[` | `reader.link.prev` | previous link |
| `Enter` / `o` | `reader.link.open` | open the current link in the browser |
| `y` | `reader.link.copy` | copy the current link's address (`wl-copy`) |
| `Esc` | `reader.link.clear` | drop the selection |
| `f` | `reader.link.hints` | label every link (`a s d f g h j k l`, two letters when there are more than 9); type a label to open it, `Backspace` undoes a letter, `Esc` cancels |

The current link is shown in inverse video (system view) or as an outlined box (HTML view), and the status bar shows
`link 3/8 · <address>`. These keys apply only while there is a link to act on, so `Tab` (next pane), `Enter`, `y` (accept
invitation), `f` (forward) and `Esc` keep their usual meaning in a message without links or with no link selected; with links,
use `h` / `Left` to leave the reading pane and the `Forward` button or `Ctrl-F` (outlook preset) to forward.
Links in the system view are underlined with a hand cursor; the mouse works as before. In the HTML view's Qt fallback
renderer (used only when the WebKit helper fails) there are no keyboard links.

## How a message is shown: `H` (OL-020)

`H` (command `mail.render`, `:render`) steps the open message through the levels; the header chip (also clickable) shows the view and
the level that decides it, e.g. `HTML · sender`, `System · message`, `System · default`.

| Setting (Settings -> Mail) | Cycle on `H` |
|---|---|
| Always System with per-sender override (`system_sender`) | HTML · sender -> System · message -> System · default (the sender's always-HTML rule is removed) -> HTML · message -> HTML · sender |
| Always HTML / Always System | default <-> message |

A message-level choice lasts until the app quits (storing it is OL-022). A sender-level choice is the always-HTML flag in Settings -> Senders.
While a reply or forward of the open message is inside its undo window, `H` and the chip are off (a notice says why); other messages are not affected.
The "This message has HTML formatting" bar is now just a hint pointing at `H` (its dropdown and View button are gone). Rebind with `"mail.render"` in `[keys]`.

## Presets

`outlook` keeps the vim keys and adds: Ctrl-R reply, Ctrl-Shift-R reply all, Ctrl-F forward, Delete delete, Backspace archive,
Ctrl-Q read/unread, Ctrl-Shift-V move, Ctrl-Z undo, F9 sync, F5 refresh mail from the server (same as the Refresh button in the message list; `:refreshmail`), Ctrl-1..4 Mail / Calendar / People / Tasks.

## Meeting invitations (Microsoft accounts)

Any calendar message shows a banner at the top of the reading pane: a meeting invitation (with time, place and
organizer), a cancellation, or someone's answer to your own meeting. On an invitation from someone else the banner
also shows your current answer, and these keys apply:

| Key | Action |
|-----|--------|
| `y` | Accept |
| `m` | Tentatively accept |
| `n` | Decline |

Every answer also marks the invitation read, and updates your calendar (accept and tentative add the event, decline removes it); the calendar view
reloads. The reply goes to the organizer silently (no comment). You can change your answer later with the same keys;
the banner re-checks the server after a few minutes, so an answer given in Outlook or on a phone shows up too.
Not answerable: your own meetings, cancelled meetings, and messages that are only someone's reply.

The keys are active only on an answerable invitation, so `m` still means move to folder on ordinary mail. Commands:
`invite.accept`, `invite.tentative`, `invite.decline` (also `:accept`, `:tentative`, `:decline`). Responding needs the
`Calendars.ReadWrite` permission; without it the banner says so instead of failing silently. Gmail accounts are not
supported yet.

## Attachments (Microsoft accounts)

A message with real attachments shows one line under its header: `📎 report.pdf (2.1 MB) · A to save`, or
`📎 3 attachments · A to choose, S to save all`. Pictures that are part of the HTML body (signature logos, embedded
images) are not attachments and are never listed.

| Key | Action |
|-----|--------|
| `/` | Search the open folder (or "all accounts" folder): `from:stripe is:unread after:7d "exact phrase" -word`. Enter runs it, `Esc` in the list clears it; also `:search <pattern>`, `:clear`. Switching folder clears the search. With nothing typed the list offers, in order: clear / edit the current search (so `/` `Enter` clears it), your recent patterns (so with no search active `/` `Enter` repeats the last one), then the syntax |
| `fullbody:word` | (a search term, not a key) Searches the whole message text at the mail server instead of the cached preview, for the open folder: Exchange via Graph `$search`, Gmail via `q=`. Takes a few seconds the first time (results are reused for 5 minutes). At most the newest 1000 matches per account are used; the status bar then says "(newest only)". Gmail cannot restrict to the body, so subject and sender hits count too. `-fullbody:word` works unless it matches over 1000 messages. Needs the account signed in |
| `:filter` | Quick filters from the selected message: from this sender, same subject, each of its categories, unread only, has attachment. Each row toggles its term in the current search (a check mark = already in it), so they stack: `from:bob is:unread`. Palette only, no key |
| `A` | Open the attachment list: `Enter` saves the highlighted file (the list stays open, saved files get a check mark), the first row saves all; type to filter, Up/Down to move, `Esc` closes |
| `S` | Save every attachment of the selected message |

Files are saved individually, never zipped, into your Downloads folder (XDG user dirs, else `~/Downloads`), flat, with no
subfolder. A name that already exists becomes `name (1).ext`; nothing is overwritten. The status bar says where they went.
Change the folder with `[attachments] download_dir = "~/Documents/mail"` in `settings.toml`. There is no preview: open a saved
file with your desktop's default application. Commands: `mail.attachments`, `mail.attachments.saveall` (also `:attachments`,
`:saveattachments`). Gmail accounts are not supported yet.

## Texts (SMS view)
| Key | Command id | Does |
|---|---|---|
| `z` | `sms.read` | mark the whole conversation read / unread (opening one does not read it) |
| `r`, `i` | `sms.reply` | write in the open conversation; `Ctrl+Enter` sends, `Esc` leaves (draft is kept) |
| `c`, `Ctrl+N` | `sms.new` | new conversation: recipients (contact names or numbers), then the text |
| `/` | `app.search` | search conversations by name, number or message text; `Esc` clears |
| `R` | `sms.retry` | send failed texts again (a failed text shows a red "Send failure") |
| `x` | `item.delete` | says deleting is not possible (KDE Connect has no delete) |

## Calendar (OL-021 phase 1)

The calendar has a keyboard cursor (the highlighted day; in the day and week grids also a 30-minute slot).

| Key | Does |
|---|---|
| `h` `l` | Previous / next day (work week skips the weekend) |
| `j` `k` | Month: next / previous week. Day and week grids: next / previous 30-minute slot (past midnight lands on the neighbouring day) |
| `Ctrl-d` `Ctrl-u` | Grids: half a screen of slots. Month: next / previous month |
| `gg` `G` | Grids: first / last slot of the day |
| `[` `]` (also `PgUp` `PgDn`) | Previous / next period (these two keep their old meaning; `j` `k` no longer mean "period") |
| `t` | Today (cursor goes to the current half hour) |
| `d` `W` `w` `m` | Day, work week, week, month view |
| `Tab` `Shift-Tab` | Next / previous event under the cursor (only when there is more than one; otherwise they cycle panes as usual) |
| `Enter` | Show / hide the detail pane of the selected event (on an empty month cell: open that day) |
| `Esc` | Close the detail pane |
| `J` | Open the selected event's join link (Teams, Zoom, Meet, Webex, Jitsi or any `https` link) in the browser |
| `y` | Copy the join link |
| `A` | Cycle the account filter: all, each account (only with two or more accounts) |
| `r` | Refresh |

With more than one account, event bars take the account's colour and the header shows the filter. The join link is the provider's own link
when there is one, else a known meeting host found in the location or body, else the first `https` URL in the location.
