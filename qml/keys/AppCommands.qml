import QtQuick
import "Commands.js" as Cmd
import "../mail/format.js" as Fmt

// The command registry: every key binding in the app, in one place. The router,
// the palette and the status-bar hints all read `list`, so they cannot drift from
// what the keys really do. Commands call AppState intents; no backend logic here.
//
// Scopes, innermost first (built in AppShell): pane ( "folder" | "list" |
// "reader"), "<view>/<pane>", "<view>", "global".
Item {
  id: root

  required property var app
  property var content: null            // the active view item (MailView, TasksView, …)
  property int folderPageRows: 1

  signal paletteRequested(string prefill)
  signal helpRequested()
  signal quitRequested()

  // One-line feedback for keys that cannot act yet. Shown in the bottom status bar.
  property string notice: ""
  Timer { id: noticeTimer; interval: 3000; onTriggered: root.notice = "" }
  function say(text) { notice = text; noticeTimer.restart() }

  readonly property bool inMail: app.currentView === "mail"

  // The active non-mail view while the cursor is in its main pane, else null.
  function mod() {
    return (app.focusPane === "msg" && !inMail && app.currentView !== "settings") ? content : null
  }

  // ---- motion -----------------------------------------------------------
  function move(dx, dy) {
    var m = mod()
    if (m && m.keyMove && m.keyMove(dx, dy) === true) return
    if (app.focusPane === "reader") {
      if (dy !== 0 && content) content.scrollReader(dy * 60)
      else if (dx < 0) app.back()
      return
    }
    app.moveCursor(dx, dy)
  }

  function pageRows() {
    if (app.focusPane === "folder") return folderPageRows
    if (app.focusPane === "msg" && inMail && content) return content.pageRows
    return 1
  }

  // dir: +1 down / -1 up; frac: 1 = a screenful, 0.5 = half.
  function page(dir, frac) {
    var p = app.focusPane
    if (p === "reader") { if (content) content.pageReader(dir * frac); return }
    var m = mod()
    if (m) {
      if (m.keyPage) m.keyPage(dir * frac)
      else if (m.pageScroll) m.pageScroll(dir)
      return
    }
    if (p === "folder" || p === "msg") app.moveVertical(dir * Math.max(1, Math.round(pageRows() * frac)))
  }

  function edge(bottom) {
    var p = app.focusPane
    if (p === "reader") { if (content) content.readerEdge(bottom); return }
    var m = mod()
    if (m) { if (m.keyEdge) m.keyEdge(bottom); return }
    app.moveVertical(bottom ? 1000000 : -1000000)
  }

  function activate() {
    var m = mod()
    if (m && m.keyActivate) m.keyActivate()
    else app.activate()
  }

  // Mail actions work on the message list and the reading pane only.
  readonly property bool onMessages: inMail && (app.focusPane === "msg" || app.focusPane === "reader")

  function deleteItem() {
    var m = mod()
    if (onMessages) app.deleteSelected()
    else if (m && m.keyDelete) m.keyDelete()
    else say("delete: nothing selected here")
  }

  // Pickers for typed commands (":goto cal", ":folder inb"): fuzzy-filtered entries
  // shaped like commands, so the palette can list and run them the same way.
  function _fz(q, text) {
    var a = q.toLowerCase().replace(/\s+/g, ""), t = text.toLowerCase(), j = 0
    for (var i = 0; i < t.length && j < a.length; i++) if (t[i] === a[j]) j++
    return j === a.length
  }
  function pickModules(q) {
    var out = []
    app.navItems.forEach(function(n) {
      if (_fz(q, n.label)) out.push({ id: "goto." + n.view, title: n.label, keys: [], run: function() { app.gotoView(n.view) } })
    })
    return out
  }
  function pickFolders(q) {
    var out = [], m = app.folderModel
    for (var i = 0; i < m.count; i++) {
      var f = m.get(i)
      var name = String(f.display_name || "")
      if (name === "" || !_fz(q, name)) continue
      var unread = f.unread_item_count > 0 ? "  (" + f.unread_item_count + ")" : ""
      var acct = (f.account_email && !f.all_accounts) ? "  · " + f.account_email : ""
      out.push({ id: "folder." + i, title: name + acct + unread, keys: [], run: (function(idx) { return function() { app.gotoFolder(idx) } })(i) })
    }
    return out
  }

  // Folders the selected messages can move to: same account, not the folder they are in.
  function pickMoveTargets(q) {
    var out = [], m = app.folderModel, cur = app.selectedFolder
    if (!cur || cur.all_accounts) return out   // moves are per account: not from the all-accounts view
    for (var i = 0; i < m.count; i++) {
      var f = m.get(i)
      if (f.account_id !== cur.account_id || f.id === cur.id) continue
      var name = String(f.display_name || "")
      if (name === "" || !_fz(q, name)) continue
      out.push({ id: "move." + i, title: name, keys: [], run: (function(fid, fname) { return function() { app.moveSelected(fid, fname) } })(f.id, name) })
    }
    return out
  }

  // ---- categories / tags -------------------------------------------------------
  // Category picker entries for the message(s) under the cursor: existing ones (✓ all, – some
  // carry it), then "New …". Typed text filters; a name that matches nothing offers to create it.
  function pickTags(q) {
    var acct = app.targetAccountId()
    if (acct === "") return []
    if (app.catAccountId !== acct) {
      app.loadCategories(acct)
      return [{ id: "tag.loading", title: "loading…", keys: [], run: function() {} }]
    }
    var out = []
    var noun = app.tagNoun(acct)
    if (!app.catMasterList && app.accountProvider(acct) === "exchange")
      out.push({ id: "tag.signin", title: "\u26a0 category colours need a fresh Exchange sign-in (Settings \u203a Accounts)", keys: [], run: function() { app.openSettingsCategory("account") } })
    var exact = false, query = q.trim()
    app.catDefs.forEach(function(d) {
      if (String(d.name).toLowerCase() === query.toLowerCase()) exact = true
      if (query !== "" && !_fz(query, d.name)) return
      var st = app.tagState(d.name)
      out.push({ id: "tag." + d.name, title: (st === "all" ? "\u2713 " : st === "some" ? "\u2013 " : "   ") + d.name,
                 swatch: d.color, keys: [], run: (function(n) { return function() { app.toggleTag(n) } })(d.name) })
    })
    if (query !== "" && !exact)
      out.push({ id: "tag.new", title: "+ New " + noun + " \u201c" + query + "\u201d\u2026", keys: [], run: function() { app.beginNewCategory(query) } })
    if (out.length === 0) out.push({ id: "tag.none", title: "no " + noun + "s yet \u2014 type a name to create one", keys: [], run: function() {} })
    return out
  }

  function pickColors(q) {
    if (app.pendingCatName === "") return []
    var out = []
    app.catPalette.forEach(function(c) {
      if (q.trim() !== "" && !_fz(q, c.label)) return
      out.push({ id: "color." + c.key, title: c.label, swatch: c.hex, keys: [], run: function() { app.createCategory(app.pendingCatName, c.key) } })
    })
    return out
  }

  function openTagPicker() {
    var acct = app.targetAccountId()
    if (acct === "") return
    app.loadCategories(acct, function() { root.paletteRequested("tag ") })
  }

  // ---- attachments -----------------------------------------------------------
  // Picker for the selected message's attachments: Enter saves the highlighted file (the list stays
  // up, saved ones get a check mark), the first row saves them all. Typing filters by name.
  function pickAttachments(q) {
    var a = app.attachments
    var none = function(id, t) { return [{ id: id, title: t, keys: [], run: function() {} }] }
    if (a.id !== app.currentMessageId || a.state === "idle") return none("att.none", "this message has no attachments")
    if (a.state === "loading") return none("att.loading", "loading attachments\u2026")
    if (a.state === "error") return none("att.error", "\u26a0 " + a.error)
    var items = app.realAttachments(), query = q.trim(), out = []
    if (items.length === 0) return none("att.none", "this message has no attachments")
    if (items.length > 1 && query === "")
      out.push({ id: "att.all", title: "Save all " + items.length + " attachments", keys: [], run: function() { app.saveAttachments("") } })
    items.forEach(function(it) {
      if (query !== "" && !_fz(query, it.name)) return
      var link = it.kind === "reference"
      out.push({ id: "att." + it.id, keepOpen: !link,
                 title: (app.attachmentSaved(it.id) ? "\u2713 " : "   ") + it.name + "   " + (link ? "(cloud link, nothing to save)" : Fmt.bytes(it.size)),
                 keys: [], run: link ? function() { say("a cloud link has no file to save") } : (function(i) { return function() { app.saveAttachments(i) } })(it.id) })
    })
    if (out.length === 0) return none("att.nomatch", "no attachment matches \u201c" + query + "\u201d")
    out.push({ id: "att.dir", title: "   \u2192 saves to " + (a.dir || "Downloads"), keys: [], run: function() {} })
    return out
  }

  function sync() { app.loadFolders(); app.loadMessages(); say("syncing…") }

  function soon(what) { return function() { say(what + ": not implemented yet") } }

  // ---- user key configuration ([keys] in settings.toml) ---------------------
  // `list` holds the built-in bindings; `effective` is what the router, palette and help
  // use: `list` with the preset and the user's per-command bindings applied.
  property var config: ({ preset: "", bindings: ({}), error: "" })
  readonly property var applied: Cmd.applyBindings(list, config)
  readonly property var effective: applied.commands
  readonly property var problems: applied.problems

  // First key of a command (for the status-bar hints), as typed: "a", "Ctrl-r", "g m".
  function keyOf(id) {
    for (var i = 0; i < effective.length; i++)
      if (effective[i].id === id) return effective[i].keys.length > 0 ? Cmd.keyLabel([effective[i].keys[0]]) : ""
    return ""
  }

  // ---- registry ---------------------------------------------------------
  readonly property var list: [
    // motion (global; reader/module scopes override where they differ)
    { id: "move.down",  title: "Move down",           scope: "global", keys: ["j", "Down"],  run: function() { move(0, 1) } },
    { id: "move.up",    title: "Move up",             scope: "global", keys: ["k", "Up"],    run: function() { move(0, -1) } },
    { id: "move.left",  title: "Back one pane",       scope: "global", keys: ["h", "Left"],  run: function() { move(-1, 0) } },
    { id: "move.right", title: "Open / move right",   scope: "global", keys: ["l", "Right"], run: function() { move(1, 0) } },
    { id: "move.top",   title: "Jump to top",         scope: "global", keys: ["g g", "Home"], run: function() { edge(false) } },
    { id: "move.bottom", title: "Jump to bottom",     scope: "global", keys: ["G", "End"],   run: function() { edge(true) } },
    { id: "page.down",  title: "Page down",           scope: "global", keys: ["PgDown"],     run: function() { page(1, 1) } },
    { id: "page.up",    title: "Page up",             scope: "global", keys: ["PgUp"],       run: function() { page(-1, 1) } },
    { id: "half.down",  title: "Half page down",      scope: "global", keys: ["C-d"],        run: function() { page(1, 0.5) } },
    { id: "half.up",    title: "Half page up",        scope: "global", keys: ["C-u"],        run: function() { page(-1, 0.5) } },
    { id: "item.activate", title: "Open / activate",  scope: "global", keys: ["Enter", "Space"], run: function() { activate() } },
    { id: "item.delete",   title: "Delete",           scope: "global", keys: ["x"],          run: function() { deleteItem() } },

    // panes
    { id: "pane.next",  title: "Next pane",           scope: "global", keys: ["Tab"],        run: function() { app.cyclePane(1) } },
    { id: "pane.prev",  title: "Previous pane",       scope: "global", keys: ["S-Tab"],      run: function() { app.cyclePane(-1) } },
    { id: "pane.cycle", title: "Cycle pane (old key)", scope: "global", keys: ["s"], hidden: true, run: function() { app.cycleFocus() } },
    { id: "pane.back",  title: "Back / cancel",       scope: "global", keys: ["Esc"],        run: function() { if (app.markCount > 0) app.clearMarks(); else app.back() } },
    // Pane numbers follow what is on screen: with the folder pane hidden, list = 1, reader = 2.
    { id: "pane.folders", title: "Focus folders",     scope: "mail",   keys: ["1"], when: function() { return app.showFolderPane }, helpWhen: function() { return app.showFolderPane }, run: function() { app.focusPaneNamed("folder") } },
    { id: "pane.list",    title: "Focus list",        scope: "mail",   keys: ["2"], when: function() { return app.showFolderPane }, helpWhen: function() { return app.showFolderPane }, run: function() { app.focusPaneNamed("msg") } },
    { id: "pane.reader",  title: "Focus reading pane", scope: "mail",  keys: ["3"], when: function() { return app.showFolderPane }, helpWhen: function() { return app.showFolderPane }, run: function() { app.focusPaneNamed("reader") } },
    { id: "pane.list.nf",   title: "Focus list",        scope: "mail", keys: ["1"], when: function() { return !app.showFolderPane }, helpWhen: function() { return !app.showFolderPane }, run: function() { app.focusPaneNamed("msg") } },
    { id: "pane.reader.nf", title: "Focus reading pane", scope: "mail", keys: ["2"], when: function() { return !app.showFolderPane }, helpWhen: function() { return !app.showFolderPane }, run: function() { app.focusPaneNamed("reader") } },
    { id: "view.folders", title: "Show / hide folder pane", scope: "mail", keys: ["F"], ex: ["folders"], run: function() { app.toggleFolderPane() } },
    { id: "pane.list2",   title: "Focus list",        scope: "global", keys: ["1"], hidden: true, when: function() { return !inMail }, run: function() { app.focusPaneNamed("msg") } },
    { id: "view.close", title: "Back (one pane left)", scope: "global", keys: ["q"],          run: function() { app.back() } },
    { id: "app.quit",   title: "Quit OmaLook",    scope: "global", keys: ["Q"], ex: ["q", "quit"], run: function() { root.quitRequested() } },

    // go to a module
    { id: "go.mail",     title: "Go to Mail",     scope: "global", keys: ["g m"], run: function() { app.gotoView("mail") } },
    { id: "go.calendar", title: "Go to Calendar", scope: "global", keys: ["g c"], run: function() { app.gotoView("calendar") } },
    { id: "go.people",   title: "Go to People",   scope: "global", keys: ["g p"], run: function() { app.gotoView("contacts") } },
    { id: "go.tasks",    title: "Go to Tasks",    scope: "global", keys: ["g t"], run: function() { app.gotoView("tasks") } },
    { id: "go.sms",      title: "Go to SMS",      scope: "global", keys: ["g s"], run: function() { app.gotoView("sms") } },
    { id: "go.settings", title: "Go to Settings", scope: "global", keys: ["g ,"], ex: ["settings"], run: function() { app.gotoView("settings") } },
    { id: "go.accounts", title: "Go to Accounts", scope: "global", keys: ["g a"], ex: ["accounts", "account"], run: function() { app.openSettingsCategory("account") } },
    { id: "go.any",      title: "Go to…",         scope: "global", keys: [], ex: ["goto", "go"], pick: pickModules, run: function() { app.gotoView("mail") } },
    { id: "go.folder",   title: "Go to folder…",  scope: "global", keys: ["g f"], ex: ["folder"], pick: pickFolders, run: function() { root.paletteRequested("folder ") } },

    // command list
    { id: "palette",    title: "Show commands",   scope: "global", keys: [":", "C-p"], run: function() { root.paletteRequested("") } },
    { id: "help",       title: "Keyboard help",   scope: "global", keys: ["?"], ex: ["help"], run: function() { root.helpRequested() } },
    { id: "app.sync",   title: "Sync now",        scope: "global", keys: [], ex: ["sync", "refresh"], run: function() { sync() } },
    { id: "app.search", title: "Search mail…",    scope: "global", keys: [], ex: ["search"], run: soon("search") },

    // reader
    { id: "reader.pagedown", title: "Scroll page down", scope: "reader", keys: ["Space"], run: function() { page(1, 1) } },
    { id: "reader.pageup",   title: "Scroll page up",   scope: "reader", keys: ["b"],     run: function() { page(-1, 1) } },

    // mail
    { id: "mail.new",      title: "New message",   scope: "mail", keys: ["c", "C-n"], ex: ["compose", "new"], run: function() { if (!inMail) app.gotoView("mail"); app.openCompose("new") } },
    { id: "mail.reply",    title: "Reply",         scope: "mail", keys: ["r"], run: function() { app.openCompose("reply") } },
    { id: "mail.replyall", title: "Reply all",     scope: "mail", keys: ["R"], run: function() { app.openCompose("replyAll") } },
    { id: "mail.forward",  title: "Forward",       scope: "mail", keys: ["f"], run: function() { app.openCompose("forward") } },
    { id: "mail.read",     title: "Mark read / unread", scope: "mail", keys: ["z"], run: function() { app.toggleRead() } },
    { id: "mail.archive",  title: "Archive",       scope: "mail", keys: ["a"], when: function() { return onMessages }, run: function() { app.archiveSelected() } },
    { id: "mail.trash",    title: "Delete (Trash; permanent in Trash)", scope: "mail", keys: [], ex: ["delete", "trash"], when: function() { return onMessages }, run: function() { app.deleteSelected() } },
    // Invitation answers. Listed before mail.move (which also uses `m`): these only apply while an
    // open invitation is selected, so `m` keeps moving mail everywhere else.
    { id: "invite.accept",    title: "Accept invitation",           scope: "mail", keys: ["y"], ex: ["accept"], when: function() { return onMessages && app.invite.actionable }, run: function() { app.respondInvite("accept") } },
    { id: "invite.tentative", title: "Tentatively accept invitation", scope: "mail", keys: ["m"], ex: ["tentative"], when: function() { return onMessages && app.invite.actionable }, run: function() { app.respondInvite("tentative") } },
    { id: "invite.decline",   title: "Decline invitation",          scope: "mail", keys: ["n"], ex: ["decline"], when: function() { return onMessages && app.invite.actionable }, run: function() { app.respondInvite("decline") } },
    { id: "mail.move",     title: "Move to folder…", scope: "mail", keys: ["m"], ex: ["move", "mv"], pick: pickMoveTargets, when: function() { return onMessages }, run: function() { root.paletteRequested("move ") } },
    { id: "mail.mark",     title: "Mark / unmark message", scope: "mail", keys: ["v"], when: function() { return app.focusPane === "msg" }, run: function() { app.toggleMark(); app.moveVertical(1) } },
    { id: "mail.markrange", title: "Mark range to cursor", scope: "mail", keys: ["V"], when: function() { return app.focusPane === "msg" }, run: function() { app.markRange() } },
    { id: "mail.markall",  title: "Mark all in list", scope: "mail", keys: ["*"], when: function() { return app.focusPane === "msg" }, run: function() { app.markAll() } },
    { id: "mail.attachments", title: "Attachments\u2026", scope: "mail", keys: ["A"], ex: ["attachments", "att"], pick: pickAttachments, when: function() { return onMessages && app.realAttachments().length > 0 }, run: function() { root.paletteRequested("attachments ") } },
    { id: "mail.attachments.saveall", title: "Save all attachments", scope: "mail", keys: ["S"], ex: ["saveattachments", "saveatt"], when: function() { return onMessages && app.realAttachments().length > 0 }, run: function() { app.saveAttachments("") } },
    { id: "mail.tag",      title: "Set category / tag\u2026", scope: "mail", keys: ["t"], ex: ["tag", "category", "cat"], pick: pickTags, when: function() { return onMessages }, run: function() { openTagPicker() } },
    { id: "mail.tagcolor", title: "Colour for the new category\u2026", scope: "mail", keys: [], hidden: true, ex: ["tagcolor"], pick: pickColors, run: function() {} },
    { id: "mail.undo",     title: "Undo last archive / delete / move, or unsend", scope: "mail", keys: ["u"], run: function() { if (!app.undoAction()) app.undoLatest() } },

    // calendar
    { id: "cal.next",  title: "Next period",     scope: "calendar/list", keys: ["j", "Down", "]"], run: function() { content.step(1) } },
    { id: "cal.prev",  title: "Previous period", scope: "calendar/list", keys: ["k", "Up", "["],   run: function() { content.step(-1) } },
    { id: "cal.today", title: "Go to today",     scope: "calendar/list", keys: ["t"], run: function() { content.goToday() } },
    { id: "cal.day",   title: "Day view",        scope: "calendar/list", keys: ["d"], run: function() { content.mode = "day" } },
    { id: "cal.work",  title: "Work week view",  scope: "calendar/list", keys: ["W"], run: function() { content.mode = "workweek" } },
    { id: "cal.week",  title: "Week view",       scope: "calendar/list", keys: ["w"], run: function() { content.mode = "week" } },
    { id: "cal.month", title: "Month view",      scope: "calendar/list", keys: ["m"], run: function() { content.mode = "month" } },
    { id: "cal.refresh", title: "Refresh events", scope: "calendar/list", keys: ["r"], run: function() { content.loadEvents(true) } },
    { id: "cal.new",   title: "New event",       scope: "calendar/list", keys: ["c"], run: soon("new event") },

    // people
    { id: "people.refresh", title: "Refresh contacts", scope: "contacts/list", keys: ["r"], run: function() { content.load() } },
    { id: "people.new",     title: "New contact",      scope: "contacts/list", keys: ["c"], run: soon("new contact") },

    // tasks
    { id: "tasks.filter",  title: "Cycle filter (all / active / done)", scope: "tasks/list", keys: ["f"], run: function() {
        content.filter = content.filter === "all" ? "active" : (content.filter === "active" ? "done" : "all") } },
    { id: "tasks.refresh", title: "Refresh tasks", scope: "tasks/list", keys: ["r"], run: function() { content.loadTasks() } },
    { id: "tasks.new",     title: "New task",      scope: "tasks/list", keys: ["c"], run: soon("new task") }
  ]
}
