# Build OmaLook Field Chooser

You are implementing a field-chooser feature for OmaLook, an Omarchy-first desktop tool. Build it as a real, keyboard-first TUI-style dialog using Python and QML. Do not invent extra product scope. Ship a working chooser that reads and writes the project config and feels like Omarchy’s existing terminal apps: monospace, pane-based, lazygit-like, no mouse required.

## Product goal

Create a floating field-chooser dialog with two panes:

- Left pane: CHOSEN fields, in export order.
- Right pane: AVAILABLE fields that are not currently chosen.

The user moves fields between panes, reorders the chosen list, filters the available list, accepts or cancels. On accept, persist the chosen field ids in order. On cancel, change nothing.

This is not a plugin. It is a standalone dialog used by OmaLook. The typeface is already chosen and pulled from system defaults. Do not hardcode a font family, font file, or font size. Use whatever font the OmaLook / Qt / Quickshell stack already inherits from the system and current Omarchy theme.

## Tech stack

- Python for config I/O, field model, filtering, reordering, accept/cancel, and process exit codes.
- QML for the dialog UI, two lists, focus, keybindings, and status bar.
- Keep the QML visual language terminal-like: box-drawing or simple rectangular panels, dim inactive pane, bright focused row, numbered chosen rows, muted secondary text for field ids. No Material cards, no rounded shopping-UI chrome, no icon-heavy toolbars, no drag-and-drop as the primary model.

If the project already has a QML window helper or Python embedding pattern, reuse it. Otherwise create a small, isolated module so this dialog can be opened from OmaLook and also launched directly for testing.

## Config

Master field list and chosen state live in:

~/.config/omalook/settings.toml

Create the directory and file if missing, with safe defaults.

The TOML must support at least:

- A master list of available fields. Each field has a stable id, a human label, and optional group and default flags.
- The current chosen field ids, stored as an ordered list. Order in that list is the export order.

Suggested shape, described here so you implement it rather than paste a fence:

- Section or table for fields: each entry has id, label, optional group, optional default = true/false.
- Key chosen = array of field id strings in display order.

Rules:

- The master list is the source of truth for what exists.
- Chosen may only contain ids that exist in the master list. Drop unknown ids on load and do not crash.
- Available is computed as master fields whose ids are not in chosen, stable-sorted by group then label then id.
- If chosen is missing or empty on first run, seed it from fields marked default = true, preserving master-list order. If no defaults exist, start with an empty chosen list.
- Never rewrite the master field definitions just because the user reordered or filtered. On accept, write only the chosen id list (and keep the rest of the file intact).
- Use an atomic write: write a temp file in the same directory, then replace settings.toml. Do not clobber unrelated keys in the file.
- If the file is malformed, show a clear error in the dialog status area and refuse to save over it until the user cancels or the file is readable.

Include a small default catalog of plausible OmaLook fields so the dialog is usable before the user customizes the file. Examples of ids: email, display_name, status, plan, created_at, last_seen, org_id, notes. Labels should be human. Groups can be identity, activity, org, other. Mark a few as default.

## Layout

One floating dialog titled FIELD CHOOSER.

Top bar:

- Title on the left.
- Counts on the right: N available · M chosen.
- When a filter is active, show that in the header, e.g. filter “plan”.

Main body: two equal-ish columns.

Left column header: CHOSEN
Left rows: numbered 1..N, then the field label. Optionally show the id muted on the far right of the row. The focused chosen row uses a leading pointer such as ▸ after the number or in place of a gutter mark.

Right column header: AVAILABLE, plus a compact /filter indicator when filtering.
Right rows: field label, optional muted id. Focused row uses ▸.

Bottom status bar, one line, lazygit-style, not a modal:

Tab pane   Space add/remove   j/k move   J/K reorder   / filter   Enter accept   Esc cancel

A full keymap appears only when the user presses ?. Keep ? as an overlay or extra status lines, not a separate app screen unless necessary.

Visual states:

- The inactive pane is dimmer than the active pane.
- Only one pane has focus at a time.
- Empty chosen list shows a muted hint: nothing chosen — Space on the right to add.
- Empty available list shows: all fields chosen, or no matches if a filter is active.
- Do not draw Up/Down command buttons. Reorder happens in the chosen list itself.

Window behavior:

- Open as a floating dialog.
- If integrating with Omarchy TUI window classes is straightforward, use a floating TUI-style class. Do not block on that if the existing OmaLook window path already floats.

## Keyboard model

Mouse can click a row or pane, but the design is keyboard-first. Every action below must work with keys.

Pane and movement:

- Tab, Shift-Tab, h, l: switch pane.
- j / k and Down / Up: move cursor in the focused pane.
- Home / End: first / last row in the focused pane.
- 1 through 9: jump to that rank in the chosen list and focus the left pane. Ignore if that rank does not exist.

Transfer:

- Space: if focus is AVAILABLE, add the current field to the end of CHOSEN (or after the current chosen cursor if you have a clear insert rule; default is append). If focus is CHOSEN, remove the current field and send it back to AVAILABLE.
- Enter on a field row may perform the same add/remove as Space. Do not make Enter both add and accept. Prefer Space for transfer and a dedicated accept key.
- x or d: remove the focused chosen field. No-op on the available pane.
- a: add all available rows that match the current filter.

Reorder (chosen pane only):

- J or Ctrl-j: move the focused chosen field down.
- K or Ctrl-k: move the focused chosen field up.
- Clamp at ends. Keep focus on the moved row.

Filter (available pane):

- / opens an inline filter on the available header. Typing filters available rows by label, id, and group, case-insensitive substring is fine; fuzzy is welcome if cheap.
- Esc while the filter editor is focused clears focus from the editor; a second Esc or an explicit clear key clears the filter. Document the exact behavior and keep it consistent.
- Filter never hides chosen rows. It only restricts the right pane.

Accept / cancel:

- Ctrl-s accepts.
- Enter accepts only when focus is not in the filter editor and you are not using Enter as the row-transfer key. Recommended: Enter accepts from either pane when the filter editor is closed; Space transfers. Pick one scheme and document it in the ? help. Do not ship both ambiguous Enter meanings.
- Esc cancels when the filter editor is closed.
- Accept writes chosen ids in current left-pane order to settings.toml and exits 0.
- Cancel writes nothing and exits 1.
- After accept, also print the chosen ids to stdout as a single comma-separated line of ids, in order, so scripts can capture them. No extra chatter on stdout. Logs go to stderr.

Help:

- ?: toggle a short keymap overlay.

## Selection and focus rules

- Adding a field removes it from available immediately and appends it to chosen unless a more specific insert position is documented.
- Removing a field inserts it back into available in the available sort order, not at a random place.
- After add, keep focus on available and move the available cursor to the next remaining row.
- After remove, keep focus on chosen and move the chosen cursor to the next remaining row, or previous if it was the last.
- After reorder, keep focus on the same field.
- Do not allow duplicate ids in chosen.
- Do not allow chosen to contain fields absent from the master list.

## Python architecture

Split cleanly:

- settings loader/saver for ~/.config/omalook/settings.toml
- field catalog model: Field(id, label, group, default)
- chooser session state: chosen ordered ids, available computed list, focused pane, cursors, filter text, dirty flag
- commands: add, remove, add_all_filtered, move_up, move_down, accept, cancel
- QML bridge: expose lists and commands to QML without putting TOML logic in QML

QML should not parse TOML. QML binds to:

- chosen rows with rank, id, label, group
- available rows with id, label, group
- header counts, filter text, active pane, help visible, error text
- invokable methods for every key command

Keep the session state the source of truth. QML displays it.

## Look and feel constraints

- Monospace texture. Inherit the already-selected system / Omarchy font. Do not set font.family to a named face.
- High contrast focused row. Dim inactive pane.
- Numbered chosen rows so order is obvious.
- One-line footer of keys.
- Theme colors should follow the application / system palette already in use. If OmaLook already exposes theme colors, use them. Do not ship a private Tokyo Night palette that fights the desktop theme.
- No splash, no settings-inside-settings, no network, no plugin manifest, no packaging work unless the repo already requires it for a new dialog.

## Files to add or touch

Prefer a tight module, for example:

- python package or module for settings and chooser state
- qml file for the dialog
- a way to launch the dialog from OmaLook, plus a __main__ or CLI entry so field-chooser can be run alone
- default settings written only when the config file does not exist

Do not dump this into a single thousand-line file if the project already has structure. Do not scatter TOML paths.

## Acceptance tests

Implement or manually verify all of the following:

- Missing config directory and file are created with a valid master list and sensible defaults.
- Existing settings.toml keeps unrelated keys after accept.
- Chosen left, available right.
- Space moves a field available → chosen and chosen → available.
- J/K reorder chosen and the numbers update.
- / filters available only.
- a adds every filtered available field.
- Unknown ids in chosen are ignored on load.
- Accept writes ordered ids and exits 0.
- Cancel leaves the file unchanged and exits 1.
- Stdout on accept is only the csv of ids.
- Keyboard-only path can complete a full choose-and-reorder without the mouse.
- Font is whatever the system/app already uses.

## Implementation attitude

Match Omarchy taste: small surface, obvious keys, scriptable output, no wizard. If a choice is ambiguous, pick the lazygit-like one and document it in the ? overlay. Build the feature so it works, then stop.

