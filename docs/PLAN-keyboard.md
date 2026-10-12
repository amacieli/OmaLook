# PLAN — Keyboard model (vim navigation, hierarchy of commands, palette)

Source: `notes/keyboard-design-reference.md` (a TUI-oriented design note: vim nav + NeoMutt workflow + command palette +
Outlook IA). Status: **plan only, nothing built.** Decisions settled in §6.

## 1. What exists today (verified in code)

| Area | Today | Where |
|---|---|---|
| Key dispatcher | Omarchy's shared `Ui/PanelKeyCatcher` (in `/usr/share/omarchy/shell/Ui`, not ours to edit). Fixed keys: `j/k/h/l`+arrows, Enter/Space = activate, **`x` = delete signal**, Esc = close, Tab emits `tabRequested` (**AppShell ignores it**), any other char → `textKey` | `AppShell.qml:30-55` |
| Pane model | `focusPane` = nav → folder → msg. `h`/`l` move between panes, `s` cycles. **Reading pane cannot take focus** (no keyboard scroll of a message) | `AppState.qml:892-945` |
| Mail keys | `c` new, `r` reply, `a` reply-all, `f` forward, `u` undo-send, `Ctrl+N` new, PgUp/PgDn page | `AppShell.qml:44-72` |
| Read/unread | `toggleRead()` exists but only a click button calls it — no key | `MailView.qml:81` |
| Search | (top bar and its box removed in Phase 5; not implemented) | — |
| Delete/archive/move/tag | **No backend.** Only `POST /messages/read`. `delete_email` in `mail_list_bridge.rs` is local-cache only | `src/main.rs:634` |
| Other modules | Calendar, People, Tasks, SMS have **no key handling**; `l` into them does nothing (`moveInto` only handles nav/folder) | `qml/views/*` |
| Compose | Own `Keys.onPressed`: Ctrl+Enter send, Ctrl+B/I/U/K, Esc. Modal — fine | `ComposeView.qml:124` |
| Status bar | Shows `focusPane` — good; satisfies the note's "where am I?" rule | `BottomStatusBar.qml` |

## 2. Where the note and the app do not sync (and proposed resolution)

1. **Note is a TUI; this is a Quickshell/QML GUI.** Mouse stays first-class; keyboard is additive. Nothing to change, but
   `Ctrl-h/j/k/l` pane moves and `Ctrl-p` are fine in a GUI (no terminal ^H/^J/^I aliasing) — I'd offer `Tab` + `1-4` only
   and skip `Ctrl-hjkl` (duplicate, and `Ctrl-K` is "insert link" in compose).
2. **The note contradicts itself.** Level 3 uses bare letters (`r` reply, `m` move, `t` tag, `c` create, `d` delete,
   `a` archive) while the "prefix" section reuses those same letters as prefixes (`mn/mr/mf`, `cn/cd`, `t…`).
   `r` can't be both "reply" and the start of nothing, but `m` (move vs mail-prefix), `c` (compose vs calendar-prefix),
   `t` (tag vs tasks-prefix) genuinely collide. **Proposal:** bare letters = actions on the *selected object in the
   current module* (context-sensitive: `c` = create a mail / event / task / contact depending on module; `d` delete;
   `e` edit); `g` is the only global prefix; drop the `m*/c*/t*` families (they'd be redundant with context-sensitive
   letters and the palette). → **Q1**.
3. **`a` = archive (note) vs `a` = reply-all (shipped).** Proposal: `r` reply, `R` reply-all, `f` forward, `a` archive
   (note wins; matches Gmail/NeoMutt). Breaking change to an existing key, so confirm. → **Q2**.
4. **`x`:** note = "execute queued action" (bulk); PanelKeyCatcher hard-wires `x` = delete. Space: note = select,
   PanelKeyCatcher = activate. Can't remap the shared component → write our own `KeyRouter` (§4) and stop using
   PanelKeyCatcher in AppShell.
5. **`s` (cycle focus) is taken**, and mail-list-views Phase D wants `s` = sort. Proposal: Tab replaces `s`
   (keep `s` as hidden alias for one release); `o` = sort/order menu in the list.
6. **`q` close view / `Q` quit vs Esc:** Esc today = back one pane, and at top level closes the window.
   Proposal: Esc = back/cancel (unchanged), `q` = back out of the *view* to nav pane (or close overlay), `Q` = close
   window. Esc-at-top-level closing the window is easy to hit by accident once modes get deep — suggest removing it
   (only `Q`/`:q` closes). → **Q3**.
7. **`gt` goto tasks, `gp` people, `gm`, `gc`:** nav also has **SMS** and **Settings** → add `gs` SMS, `g,` settings
   (the note's `g?` "show destinations" becomes the which-key popup). Note says "contacts (people)"; nav label is People,
   internal view id is `contacts` — use `gp`.
8. **Panes `1-5`:** note lists folders/list/details/calendar/search. We have nav, folder, list, **reader** (new focus
   target), plus search. Proposal: `1` nav, `2` folders, `3` list, `4` reader, `/` search (not a number). Calendar is a
   module, not a pane.
9. **Threading / tagging / "Space then x" queue** presume features that don't exist (threads, categories = README Phase 5,
   bulk ops). Plan them as later phases, don't ship dead keys.
10. **Most actions have no backend** (archive, move, delete, flag, mark read via key is trivial, search). Keys and the
    palette can land first; each action is enabled when its endpoint exists (palette greys out unavailable ones).
11. **OL-001** (UI pins a CPU on rapid scrolling): `gg`, `G`, `Ctrl-d/u`, held `j/k` hammer the same path (body fetch per
    selection). **Debounce body fetch (~120 ms) before shipping fast navigation.**
12. **Mail-list-views plan:** its FieldChooser overlay uses `Tab, Space, J/K, b, o, r, p, /, Enter, Esc, ?`. Fine because
    it's modal (`editing = true`), but its `?` and `/` must be routed through the same help/search machinery, and it must
    register as a key *scope* so the router stays out of its way. Also its DESIGN §9 hotkey decision should be settled
    together with this plan.
13. **Compose is modal text editing:** the router must be fully off there (`Ctrl-U` = underline, not page-up; `Ctrl-K` =
    link). Router scope `compose` = only Esc/Ctrl+Enter, already handled.
14. **Discoverability gaps the note calls for but we lack:** `?` help, `:` palette, which-key popup after `g`, hint line in
    the bottom status bar, pending-chord indicator.

## 3. Phases

### Phase 0 — prerequisites (DONE 2026-10-09 except Q-pending: dirty tree left uncommitted by choice; branch `feature/keyboard`; body fetch debounced 120 ms — OL-001 not yet re-tested under load)
- [ ] Commit/stash the unrelated dirty tree (`build.sh`, `omalook`, `packaging/`); branch `feature/keyboard`.
- [ ] OL-001 mitigation: debounce body fetch, ignore stale responses by message id.
- [ ] Settle §6 questions.

### Phase 1 — Command registry + router (BUILT 2026-10-09, also pulled in: g-chords, status hints, command list, outlines)
- [ ] `qml/keys/Commands.js` — single registry: `{ id, title, scope, keys: ["j","Down"], when(), run() , available }`.
- [ ] `qml/keys/KeyRouter.qml` — replaces PanelKeyCatcher in AppShell. Scopes (innermost wins): `overlay` → `compose` →
      `module:<view>` → `pane:<nav|folder|list|reader>` → `global`. Chord buffer (`g` + key) with 1 s timeout, count
      prefix optional (`5j`) — see Q5.
- [ ] Move *current* behaviour into the registry unchanged (c/r/a/f/u/Ctrl+N/PgUp/PgDn/Esc/s) so Phase 1 is a pure refactor.
- [ ] Add `Tab`/`Shift-Tab` pane cycling, `Ctrl-d/Ctrl-u` half-page, `gg`/`G`, `Home/End`.
- [ ] Reader becomes a focus pane (`focusPane = "reader"`): `j/k` scroll, `Space/b` page, `h` back to list.
- Done when: no behaviour regression (manual matrix in §5), build clean, perf timeline unchanged.

### Phase 2 — Modules, help, discoverability (DONE 2026-10-09)
- [x] `g` chords: `gm gc gp gt gs g,` (+ `gg` is top-of-list — resolved by scope: `g` followed by `g` stays navigation).
- [x] Which-key popup after a 400 ms pause on a pending prefix (this is also `g?`).
- [x] `?` help overlay generated from the registry (never hand-maintained).
- [x] Bottom status bar: pane, pending chord, 3-4 context hints.
- [x] Per-module `1-4` pane jumps.

### Phase 3 — Command palette (`:` and `Ctrl-P`) (DONE 2026-10-09; `:search` is a stub until Phase 5)
- [x] `qml/keys/CommandPalette.qml`: fuzzy filter over registry titles; shows bound key, greys unavailable commands.
- [x] `:` accepts typed commands with args: `:goto calendar`, `:search invoices`, `:sync`, `:settings`, `:compose`, `:q`.
- [x] Palette also hosts "Go to folder…" (fuzzy over folders) — high-value, cheap.

### Phase 4 — Mail object actions (BUILT 2026-10-09; provider push untested against live mail)
- [ ] Now (no backend work): `e`/`Enter` open, `m`→ renamed per Q1, key for `toggleRead` (`u` is undo-send; propose `Shift-U`
      or `z`/`Z` read/unread — Q4).
- [ ] Backend: `POST /messages/{archive,delete,move,flag}` + Graph (`graph.rs`) and Gmail (`gmail.rs`) implementations,
      optimistic UI with rollback like `toggleRead`, undo toast for delete/archive (reuse SendToast).
- [ ] Selection: `Space` toggles mark (with cursor advance), `Shift-Space`/`V` range, `*` all; actions apply to marks;
      `x` = run queued/bulk only if the queued-action model in Q6 is chosen.
- [ ] Folder picker (move) = palette in "folder" mode.

### Phase 5 — Search (`/`, `n`, `N`) — DEFERRED; instead (2026-10-09) the top bar and its search box were removed
- [ ] Backend `GET /messages/search?q=` (local SQLite FTS first; provider search later).
- [ ] `/` focuses top-bar search; Enter → results become the list; `n/N` next/prev match; Esc clears.
- [ ] Decide scope: current folder / all folders (palette option `:search -a`).

### Phase 6 — Keyboard support for other modules
- [ ] Calendar: `h/l` day, `j/k` week, `t` today (Q: collides with tag → `T`?), `[`/`]` period, `1/2/3` day/week/month, `c` new event.
- [ ] People, Tasks (`x`/Space complete), SMS (j/k list, Enter open, `c` new). Today none respond to keys.

### Phase 7 — Configurable bindings (BUILT 2026-10-09; reference in docs/KEYS.md)
- [x] `[keys]` in `settings.toml` (`gm = "goto mail"`), hot-reload like mail-list-views; conflicts reported in `?`.
- [x] Optional "Outlook" preset (Ctrl+R reply, Del delete, Ctrl+Shift+M new…) since the note's audience includes non-vim users.

## 4. Architecture notes
- Router lives in `qml/keys/`; `AppShell` only instantiates it. Pages declare scope via `property string keyScope` and expose
  `editing` (already the convention) so the router yields.
- Commands call existing `AppState` intent functions — no backend logic in the router.
- Help, palette, which-key and status hints all read the one registry, so they cannot drift from real bindings.
- Keep `Shortcut {}` only for PgUp/PgDn if still needed (Qt `Keys` lacks page signals); everything else via router.

## 5. Verification
- Manual key matrix per phase (every key × every pane × mail/settings/compose/overlay), recorded in
  `notes/YYYY-MM-DD-KEYBOARD-PHASE-N-COMPLETE.md`.
- Router scope logic is pure JS → unit-testable with `qmltestrunner`/node on `Commands.js` (chord resolution, scope
  precedence, timeouts).
- Build clean; perf: hold `j` for 5 s on a 5k-message folder, no CPU pin (OL-001).

## 6. Decisions (settled 2026-10-09)
- Q1 Context-sensitive bare letters per module; `g` is the only prefix. No `mn/cn/tn` families.
- Q2 `a` = archive, `R` = reply-all (`r` reply, `f` forward). Breaking change to today's `a`.
- Q3 Esc never closes the window; `Q` / `:q` does. Esc = back/cancel only.
- Q4 Read/unread = `z`. `u` stays undo-send.
- Q5 No count prefixes in v1.
- Q6 Bulk actions apply immediately to marked messages with an undo toast; no queued `x` execute.
- Follow-up (2026-10-09): **`x` stays delete and `Space` stays activate** (Omarchy pattern), so Space does not mark
  messages. Marking will use another key (proposal: `v` toggle mark, `V` range) when Phase 4 gets there. `:` opens the
  command list (also `?`, `Ctrl-P`); the bottom status bar always shows `: commands`.
- Follow-up: pane outlines are btop-style boxes with the title in the top edge and the jump key (`¹ menu`, `² folders`,
  `³ Inbox`, `⁴ message`); the focused pane is accent-coloured, 2 px, bold title. Replaces the 1 px separators.
- Follow-up: other modules get the same keys through a view hook (`keyMove/keyPage/keyEdge/keyActivate/keyDelete`) plus
  per-view commands (Calendar `j/k` period, `t`, `d/w/W/m`; Tasks `j/k`, Space done, `f`; People `j/k` scroll).
- Still open (minor): Calendar `t` = today vs tag — resolved by context (tag has no backend; categories are README Phase 5).

## 7. Phase 4 as built
- Keys (message list / reader): `a` archive, `x` delete (to Trash; in Trash/Deleted Items asks, Cancel is the default, then deletes for good),
  `m` move (picker: same account, not the current folder; also `:move <name>`), `z` read/unread, `u` undo (action first, else unsend).
  List only: `v` mark + advance, `V` mark range, `*` mark all, Esc clears marks. Actions apply to the marks, else the cursor row.
- Backend: `POST /messages/action?ids=&op=archive|trash|delete|move[&dest=]`, `POST /messages/action/undo?ids=`. Rows get
  `action_pending`/`action_due` (hidden from `GET /messages` at once, folder unread counts follow). The undo window is the mail
  "send delay" setting; the daemon then calls `apply_message_action` (Graph: move to `archive`/`deleteditems`/folder, DELETE;
  Gmail: INBOX label off / `trash` / label swap; Gmail permanent delete is refused: `gmail.modify` cannot do it). Transient errors
  retry, refusals put the message back. Local row is deleted after the provider accepts; a moved message arrives via sync.
- Folder pane is hidden by default (`ui.folder_pane` in settings.toml, `F` / `:folders` toggles). The list title carries the folder
  and, with several accounts, the account. Pane numbers follow what is on screen.
- Tests: db `queued_actions_*`, models `message_action_tests`; node `tests/commands.test.js`.
- Not verified: an actual archive/trash/move/delete reaching Graph or Gmail (only queue + undo were exercised on the live mailbox).

## 8. Phase 5 as done
- Top bar removed (title, search box, backend icon, account button). Its jobs: view/folder and online/offline were already in the bottom
  status bar; the account button became `g a` / `:accounts` (Settings → Accounts) and the bottom bar shows `signed out (g a accounts)` when
  not signed in; search stays the `:search` stub. No search was implemented (no `/` key, no endpoint). The 1 px rule under the bar went too.
- Search, when it comes back: `/` opens a search prompt (not a permanent box), results replace the list, `n`/`N` step through, Esc clears.
