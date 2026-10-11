# Keyboard bindings

Press `?` in the app for every binding, grouped by where it works, with each command's id.
`:` / Ctrl-P opens the command list.

## Rebinding: `[keys]` in `~/.config/omarchylook/settings.toml`

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

## Presets

`outlook` keeps the vim keys and adds: Ctrl-R reply, Ctrl-Shift-R reply all, Ctrl-F forward, Delete delete, Backspace archive,
Ctrl-Q read/unread, Ctrl-Shift-V move, Ctrl-Z undo, F9 sync, Ctrl-1..4 Mail / Calendar / People / Tasks.

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
| `A` | Open the attachment list: `Enter` saves the highlighted file (the list stays open, saved files get a check mark), the first row saves all; type to filter, Up/Down to move, `Esc` closes |
| `S` | Save every attachment of the selected message |

Files are saved individually, never zipped, into your Downloads folder (XDG user dirs, else `~/Downloads`), flat, with no
subfolder. A name that already exists becomes `name (1).ext`; nothing is overwritten. The status bar says where they went.
Change the folder with `[attachments] download_dir = "~/Documents/mail"` in `settings.toml`. There is no preview: open a saved
file with your desktop's default application. Commands: `mail.attachments`, `mail.attachments.saveall` (also `:attachments`,
`:saveattachments`). Gmail accounts are not supported yet.
