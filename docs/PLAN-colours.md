# Plan — per-element colours (tracker: OL-011)

Status: planned, nothing built. Written 2026-10-17; decisions settled the same day (§9).

## Goal

Every piece of OmaLook's own interface (box edges, pane titles, sender, subject, dates, status bar parts, …) is a named
**element**. For each element the user picks a **theme colour** and an **intensity** from two dropdowns and sees a **live
preview**. The whole screen is keyboard-driven with vim-style chords. Choices are stored by theme *key name* (never a hex
value), so they follow theme switches.

Non-goals (v1):
- Hex / free colours. Only colours the active Omarchy theme defines.
- The Omarchy UI-kit controls (`qs.Ui`: buttons, toggles, dropdowns, scroll bars, `Border.controlSpec` frames). They belong to
  Omarchy and follow the theme directly; only what OmaLook draws itself is configurable.
- Calendar event colours (they come from the calendars) and the HTML body of a message / the HTML compose page (white page by design).

## 1. What a theme gives us (checked 2026-10-17, `/usr/share/omarchy/themes/*/colors.toml`)

There is no fixed ramp of shades. Each theme has a fixed key set (24 keys in all 22 bundled themes; `orange`/`brown` in 19):

| Group | Keys |
|---|---|
| UI | `accent`, `selection`, `muted` |
| Backgrounds | `background`, `dark_background`, `darker_background`, `lighter_background` |
| Foregrounds | `foreground`, `dark_foreground`, `light_foreground`, `bright_foreground` |
| Hues | `red yellow green cyan blue magenta` (+ `orange`, `brown`) |
| Bright hues | `bright_red bright_yellow bright_green bright_cyan bright_blue bright_magenta` |

`qml/common/Hues.qml` already reads this file, live-reloads it and falls back to `Color.accent` / `Color.urgent` for a missing key.
So the **colour choices** are those keys, and **intensity** is the one thing OmaLook must define itself (§3).

## 2. Element catalogue

An element is an id (`group.thing[.state]`), a label, a kind (`text` | `line` | `fill`), a default colour and a default intensity.
Defaults reproduce **today's look exactly** (phase 0 must prove it), except the pane borders (see below). States that differ today (unread/read, focused/idle) are
separate elements, not a mode of one. First cut, taken from the current QML (file refs are where each is set today):

| Group | Elements (default colour) |
|---|---|
| **Panes** (`PaneFrame.qml`) | **one shared set for every pane** (no per-pane colours): border focused, border idle, title focused, title idle, hotkey digit. Today each pane has its own hue (folders cyan, list blue, reader green, compose yellow, SMS blue/green); this collapses them into the one shared choice, so the borders are the one place where the default look changes (§9) |
| **Message list** (`MessageList.qml`) | sender unread (bright_foreground), sender read (light_foreground), subject (light_foreground), date unread (blue), date read (muted), unread bar (blue), mark tick (yellow), importance `!` (red), attachment icon (cyan), cursor row fill, list header title (accent focused / foreground idle) |
| **Reading pane** (`MessagePreview.qml`) | subject (bright_foreground), sender name (per-sender hue, or fixed), sender address (cyan), date (muted), body text (system view), importance banner (urgent), attachment line, invite banner title/status |
| **Folders** (`FolderBar.qml`) | folder name, unread count (blue), section label (muted) |
| **Status bar** (`BottomStatusBar.qml`) | hints (bright_foreground), notice (yellow), pending chord (cyan), key hint, error (red), online dot (green), offline dot (red) |
| **SMS** (`Sms*.qml`) | thread name unread/read, snippet, time, unread dot, my bubble fill, their bubble, failed (red), search glyph (yellow) |
| **Compose** | status text, error (urgent), recipient chip valid (accent) / invalid (urgent), attachment chips |
| **Overlays** | help / palette / which-key border (accent), titles (accent), key names, problems (urgent), toast border, toast text |
| **Settings** | page title, section header, row label, value, danger text (urgent) |

Open point: the full list will be produced in phase 0 by auditing all ~130 `Color.*` / `Hues.*` uses under `qml/`
(`grep -rnE "(Color|Hues)\." qml`); the table above is the expected shape, not the final count.

## 3. Intensity

Themes have no shade ramps, so intensity is a **blend between two colours of the active theme**, computed in one place and the same
for every element. Nothing outside the theme is ever introduced: the result is always "this theme colour, pulled toward the theme's
own `bright_foreground` or `background`". That is what keeps a saved preference meaningful when the theme changes: "date = blue, one
step soft" means the same relative thing in every theme.

| Level | Name | Rule |
|---|---|---|
| +2 | Brightest | mix 50 % toward `bright_foreground` |
| +1 | Bright | mix 25 % toward `bright_foreground` |
| 0 | Normal | the theme colour as is (default) |
| −1 | Soft | mix 30 % toward `background` |
| −2 | Dim | mix 50 % toward `background` |
| −3 | Faint | mix 70 % toward `background` |

Text and lines use the blend (opaque), not alpha, so they stay readable and keep their hue over a translucent or blurred window.
`fill` elements (row cursor, bubbles) are the one exception: same six levels, applied as alpha, because a fill has to let the
background show through. The user sees the same Intensity dropdown either way.
The percentages are a starting point; they are tuned by eye in phase 3 using the preview. Existing alphas (`0.45` idle border, `0.28` bubbles, …)
become the default level of the matching element (≈ −2 for the idle border), so nothing shifts visually.

Colour choices offered for an element: the keys in §1 plus **Auto** (only where one exists today, e.g. the per-sender hue)
and **Default** (= "whatever the element ships with", the reset state).

## 4. Storage and delivery

`~/.config/omalook/settings.toml`, hot-reloaded like `[keys]`:

```toml
[colours]
"mail.list.date.unread" = "blue"            # colour only: level 0
"mail.list.subject"     = "light_foreground@-1"
"pane.border.idle"      = "cyan@-2"
"pane.title.focused"    = "accent@+1"
```

- Value syntax `colour[@level]`; unknown id, unknown colour or level outside −3..+2 is ignored and listed as a problem in the
  `?` overlay (same mechanism as bad `[keys]`).
- Backend: a `[colours]` map in `models.rs` (`BTreeMap<String,String>`, `#[serde(default)]`), `GET /settings/colours`,
  `POST /settings/colours` (whole map; validated server-side), same pattern as `/settings/folder_pane` and `/settings/list_width_offset`.
- Because values are key names, a theme switch re-resolves automatically (`Hues` already reloads).
- One set of choices for all themes. No per-theme overrides (decided): a theme switch re-resolves the same key names against the new theme.

## 5. Runtime (QML)

- New registry `qml/common/Roles.js`: `ROLES = { id: { label, group, kind, colour: "blue", level: 0 }, … }` (single source for defaults,
  the settings list and the help text).
- New singleton `qml/common/Roles.qml`: holds the parsed overrides (`property var overrides`, reassigned wholesale so bindings update)
  and `function c(id)` returning the resolved `color` (theme key → `Hues.pick`/`Color` → intensity blend). A call site becomes
  `foreground: Roles.c("mail.list.subject")`; it re-evaluates on theme change and on settings change.
- Pure functions (parse, validate, blend) live in `Roles.js` so `node tests/` can test them without Qt (as `commands.test.js` does).
- Phase 2 replaces each hard-wired `Hues.*` / `Color.*` in OmaLook's own components with `Roles.c(...)`; `Hues` stays as the theme reader.

## 6. The settings screen — Settings → Colours

```
┌ colours ───────────────────────────────┐┌ message list · date (unread) ────────────┐
│ ▾ Panes                                ││ Colour     [ blue            ▾ ]          │
│     border · focused                   ││ Intensity  [ Normal (0)      ▾ ]          │
│     border · idle                      ││ Resolves to  #89b4fa  (blue)              │
│ ▾ Message list                         ││                                           │
│   > sender · unread                    ││ Preview                                   │
│     sender · read                      ││  ┌ ¹ Inbox ────────────────┐              │
│     subject                            ││  │ ▌ Jane Roe        10:42 │  <- element  │
│     date · unread          ◀ selected  ││  │   Quarterly numbers     │     in place │
│ ▸ Reading pane                         ││  └─────────────────────────┘              │
└────────────────────────────────────────┘└───────────────────────────────────────────┘
```

- Left: elements grouped, folds per group, a `*` marks ones changed from default.
- Right: the two Omarchy `Dropdown`s, the resolved hex + theme key, and a **preview**: a small mock of the actual surface
  (a message row, a pane frame, a status bar segment, an SMS bubble…) drawn with the *staged* values, the selected element
  highlighted by a marker. Each group supplies one mock; elements in the group change colour in it.
- Changes also apply live to the real app behind/around the screen, so the preview is the app itself too. `u` undoes.

## 7. Keyboard (vim-style)

Scope `settings.colours`; follows the repo rule that text fields own their keys (`/` filter is the only text field).
All are registered commands in `AppCommands.qml` (so `?`, `:` palette and `[keys]` rebinding work) and documented in `KEYS.md`.

| Keys | Action |
|---|---|
| `j` `k` / `Down` `Up`, `gg` `G`, `Ctrl-d` `Ctrl-u` | move through elements (existing list keys) |
| `za` / `zo` / `zc`, `Enter` on a group | fold / unfold a group (vim fold keys) |
| `/` then text, `n` `N` | filter / next / previous match |
| `Tab` / `h` `l` | move between the Colour and Intensity fields; `Enter` opens the dropdown, `j` `k` move, `Enter` picks, `Esc` closes |
| `c` + letter | **set colour directly** (no dropdown): `r` red, `o` orange, `y` yellow, `g` green, `c` cyan, `b` blue, `p` magenta (purple), `f` foreground, `a` accent, `m` muted, `d` dark foreground, `l` light foreground; capital letter = the bright variant (`c R` = bright red); `c 0` = Default, `c *` = Auto |
| `+` / `Ctrl-a`, `-` / `Ctrl-x` | intensity up / down one level (vim's increment/decrement; a count works: `2+`) |
| `i` + digit | set intensity directly: `i 0` Normal, `i 1` Bright, `i 2` Brightest, `i 7` / `i 8` / `i 9` Soft / Dim / Faint (final map decided in phase 4) |
| `r` | reset this element to default; `R` reset the group; `:colours reset` everything |
| `yy`, `p` | **yank** this element's colour+intensity and **put** it on another (`p` applies; so "make all dates the same" is `yy`, `j…`, `p`) |
| `u`, `Ctrl-r` | undo / redo changes made in this screen |
| `Esc` / `q` | back to the Settings category list |

Needs from the router: a generic count prefix (today only after `Ctrl-w`) and a two-key sequence with a trailing
free character (`c` + letter). Both are small additions to `KeyRouter.qml` / `Commands.js`; the node test gets cases.

## 8. Phases

0. **Inventory** — audit every hard-wired colour in `qml/`, fill `Roles.js` with ids/labels/defaults, no behaviour change.
   Check: a screenshot of each main view before and after is identical (or a diff of the resolved colours per element).
1. **Core** — `Roles.js` (parse/validate/blend) + node tests; `[colours]` in `models.rs`, GET/POST routes, `cargo test`;
   `Roles.qml` loading and hot-reloading the map; problems reported in the `?` overlay.
2. **Wire elements** — group by group: panes, message list, reading pane, status bar, folders, SMS, compose, overlays, settings.
   Each group is its own commit; each verified against the defaults-identical check.
3. **Colours screen** — list, dropdowns, resolved-colour line, per-group preview mocks; tune the intensity percentages by eye.
4. **Keyboard** — router additions (count, `c`+letter), commands, `yy`/`p`, undo, fold keys; help text and `KEYS.md`; node tests.
5. **Polish** — contrast warning when the resolved text colour is below ~3:1 against its background (shown in the preview, not blocking),
   light-theme check (`catppuccin-latte`, `white`, `flexoki-light`), reset-all.

## 9. Decisions (settled by the user, 2026-10-17)

1. **Live apply** while editing (the app itself is a preview); `u` undoes.
2. **Theme keys only**, no free hex colours, so everything follows the active theme.
3. **One shared border colour** for all panes (focused and idle are the two border elements; there are no per-pane colours).
4. **No per-theme overrides.** One set of choices; it re-resolves against whatever theme is active.
5. **Per-sender hue stays as an "Auto" choice** for the sender name.
6. **Intensity = blend toward the theme's own `background` / `bright_foreground`** (alpha only for fills), per §3. Chosen because it
   uses only theme colours and keeps a preference meaningful across themes.

Settled (2026-10-17, later): the shared border keeps **one colour for focused and another for idle** (separate elements, as today).
Defaults: focused = `accent`, idle = `accent@-2`. Today's per-pane hues go away; the borders are the one place the default look changes
(phase 2 notes it in the commit). No open decisions remain; phase 0 can start.
