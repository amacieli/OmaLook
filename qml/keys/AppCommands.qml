import QtQuick
import "Commands.js" as Cmd
import "../mail/format.js" as Fmt
import "Search.js" as Srch

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
    if (dx > 0 && enterSettingsPage()) return
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

  // Settings: l / Right / Enter on a category moves the keyboard into that page's controls.
  function enterSettingsPage() {
    return app.currentView === "settings" && app.focusPane === "msg" && content && content.enterPage ? content.enterPage() === true : false
  }

  function activate() {
    if (enterSettingsPage()) return
    var m = mod()
    if (m && m.keyActivate) m.keyActivate()
    else app.activate()
  }

  // The mail view while the reading pane has the keyboard, else null: owner of the keyboard links.
  function lk() { return (inMail && app.focusPane === "reader" && content && content.linkCount !== undefined) ? content : null }
  function linkCount() { var c = lk(); return c ? c.linkCount : 0 }
  function linkCurrent() { var c = lk(); return !!c && c.linkIndex >= 0 && c.linkIndex < c.linkCount }

  // Mail actions work on the message list and the reading pane only.
  readonly property bool onMessages: inMail && (app.focusPane === "msg" || app.focusPane === "reader")

  function deleteItem() {
    var m = mod()
    if (onMessages) app.deleteSelected()
    else if (app.currentView === "sms") say("texts can't be deleted from here: KDE Connect has no delete")
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

  // ---- search ----------------------------------------------------------------
  // `/` or `:search <pattern>`. Typing a pattern gives one row that runs it. With nothing typed the
  // list is, in order: clear / edit the current search (when one is active, so `/` Enter clears it),
  // recent patterns (so with no search active `/` Enter repeats the last one), then the syntax.
  function runSearch(text) { app.applySearch(text, function(why) { say("search: " + why) }) }

  function pickSearch(q) {
    if (app.currentView === "sms") return pickSmsSearch(q)
    var query = q.trim()
    if (query !== "") {
      return [{ id: "search.run", title: "Search " + app.selectedFolderName + ":  " + query, keys: [],
                run: function() { runSearch(query) } }]
    }
    var h = function(term, what) {
      return { id: "search.h." + term, title: term + "   " + what, keys: [], run: function() { root.paletteRequested("search " + term) } }
    }
    var out = []
    if (app.searchQuery !== "") {
      out.push({ id: "search.h.clear", title: "Clear current search  (" + app.searchQuery + ")", keys: [], run: function() { app.clearSearch() } })
      out.push({ id: "search.h.edit", title: "Edit current search", keys: [], run: function() { root.paletteRequested("search " + app.searchQuery) } })
    }
    app.searchHistory.slice(0, 10).forEach(function(p, n) {
      if (p === app.searchQuery) return
      out.push({ id: "search.hist." + n, title: "\u21ba  " + p, keys: [], run: function() { runSearch(p) } })
    })
    return out.concat([
      h("from:", "sender name or address"), h("to:", "recipient (also cc: bcc:)"), h("subject:", "words in the subject"),
      h("body:", "words in the cached preview"), h("fullbody:", "whole message text, asked of the mail server (takes a few seconds)"),
      h("is:unread", "also is:read, is:important"), h("has:attachment", "files attached"),
      h("after:7d", "also before:, date:2026-10-01, today, 2w, 3m"), h("cat:", "category name"),
      { id: "search.h.tip", title: "   plain words match subject, sender and preview; \"quotes\" for phrases, -word to exclude", keys: [], run: function() {} }
    ])
  }

  // SMS: one row runs the search; with nothing typed, offer to clear the active one.
  function pickSmsSearch(q) {
    var query = q.trim()
    var run = function(t) { return function() { if (content && content.applySearch) content.applySearch(t) } }
    if (query !== "") return [{ id: "search.sms.run", title: "Search texts:  " + query, keys: [], run: run(query) }]
    var out = []
    if (app.smsSearch !== "") {
      out.push({ id: "search.sms.clear", title: "Clear current search  (" + app.smsSearch + ")", keys: [], run: run("") })
      out.push({ id: "search.sms.edit", title: "Edit current search", keys: [], run: function() { root.paletteRequested("search " + app.smsSearch) } })
    }
    out.push({ id: "search.sms.tip", title: "   type a name, a phone number, or words from a message", keys: [], run: function() {} })
    return out
  }

  // `:filter` — quick filters built from the selected message. Each row toggles its term in the
  // current search (a check mark = already in it), so filters stack: from:bob + is:unread.
  function pickFilters(q) {
    var query = q.trim(), m = app.currentMessage, rows = []
    var add = function(label, term) {
      var on = Srch.has(app.searchQuery, term)
      rows.push({ id: "filter." + term, title: (on ? "\u2713 " : "   ") + label, keys: [],
                  run: function() { runSearch(Srch.toggle(app.searchQuery, term)) } })
    }
    if (m) {
      if (m.from_email) add("from " + (m.from_name ? m.from_name + " <" + m.from_email + ">" : m.from_email), "from:" + Srch.quote(m.from_email))
      var subj = Srch.stripReplyPrefix(m.subject || "")
      if (subj !== "") add("same subject  \u201c" + subj + "\u201d", "subject:" + Srch.quote(subj))
      var cats = []
      try { cats = JSON.parse(m.cats || "[]") } catch (e) {}
      cats.forEach(function(c) { add("category " + c[0], "cat:" + Srch.quote(c[0])) })
    }
    add("unread only", "is:unread")
    add("has attachment", "has:attachment")
    if (query !== "") rows = rows.filter(function(r) { return _fz(query, r.title) })
    if (rows.length === 0) rows.push({ id: "filter.none", title: "no filter matches \u201c" + query + "\u201d", keys: [], run: function() {} })
    if (app.searchQuery !== "" && query === "")
      rows.push({ id: "filter.clear", title: "   clear search  (" + app.searchQuery + ")", keys: [], run: function() { app.clearSearch() } })
    return rows
  }

  function sync() { app.syncMail(); say("syncing…") }

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
    { id: "pane.back",  title: "Back / cancel",       scope: "global", keys: ["Esc"],        run: function() {
      if (app.markCount > 0) app.clearMarks()
      else if (app.searchQuery !== "" && inMail && app.focusPane === "msg") app.clearSearch()
      else if (app.smsSearch !== "" && app.currentView === "sms" && app.focusPane === "msg" && content) content.applySearch("")
      else app.back()
    } },
    // Pane numbers follow what is on screen: with the folder pane hidden, list = 1, reader = 2.
    { id: "pane.widen",   title: "Widen message list (reading pane narrower)", scope: "mail", keys: ["C-Right", "C-w >"], when: function() { return app.canResizeList }, helpWhen: function() { return app.canResizeList }, run: function(n) { app.resizeList(n || 1) } },
    { id: "pane.narrow",  title: "Narrow message list (reading pane wider)",   scope: "mail", keys: ["C-Left", "C-w <"],  when: function() { return app.canResizeList }, helpWhen: function() { return app.canResizeList }, run: function(n) { app.resizeList(-(n || 1)) } },
    { id: "pane.folders", title: "Focus folders",     scope: "mail",   keys: ["1"], when: function() { return app.showFolderPane }, helpWhen: function() { return app.showFolderPane }, run: function() { app.focusPaneNamed("folder") } },
    { id: "pane.list",    title: "Focus list",        scope: "mail",   keys: ["2"], when: function() { return app.showFolderPane }, helpWhen: function() { return app.showFolderPane }, run: function() { app.focusPaneNamed("msg") } },
    { id: "pane.reader",  title: "Focus reading pane", scope: "mail",  keys: ["3"], when: function() { return app.showFolderPane }, helpWhen: function() { return app.showFolderPane }, run: function() { app.focusPaneNamed("reader") } },
    { id: "pane.list.nf",   title: "Focus list",        scope: "mail", keys: ["1"], when: function() { return !app.showFolderPane }, helpWhen: function() { return !app.showFolderPane }, run: function() { app.focusPaneNamed("msg") } },
    { id: "pane.reader.nf", title: "Focus reading pane", scope: "mail", keys: ["2"], when: function() { return !app.showFolderPane }, helpWhen: function() { return !app.showFolderPane }, run: function() { app.focusPaneNamed("reader") } },
    { id: "view.folders", title: "Show / hide folder pane", scope: "mail", keys: ["F"], ex: ["folders"], run: function() { app.toggleFolderPane() } },
    { id: "pane.reader.sms", title: "Focus conversation", scope: "global", keys: ["2"], hidden: true, when: function() { return app.currentView === "sms" }, run: function() { app.focusPaneNamed("reader") } },
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
    { id: "app.search", title: "Search mail / texts…",    scope: "global", keys: ["/"], ex: ["search"], pick: pickSearch, run: function() { if (!inMail && app.currentView !== "sms") app.gotoView("mail"); root.paletteRequested("search ") } },
    { id: "search.filter", title: "Filter by this message…", scope: "mail", keys: [], ex: ["filter"], pick: pickFilters, run: function() { root.paletteRequested("filter ") } },
    { id: "search.clear", title: "Clear search",   scope: "global", keys: [], ex: ["clear"], when: function() { return app.searchQuery !== "" }, run: function() { app.clearSearch() } },

    // reader
    { id: "reader.pagedown", title: "Scroll page down", scope: "reader", keys: ["Space"], run: function() { page(1, 1) } },
    { id: "reader.pageup",   title: "Scroll page up",   scope: "reader", keys: ["b"],     run: function() { page(-1, 1) } },
    // Links in the open message (system and HTML view). Each applies only while there is a link to act on,
    // so Tab, Enter, y, f and Esc keep their other meanings otherwise.
    { id: "reader.link.next",  title: "Next link",     scope: "reader", keys: ["Tab", "]"],   when: function() { return linkCount() > 0 }, run: function() { lk().linkStep(1) } },
    { id: "reader.link.prev",  title: "Previous link", scope: "reader", keys: ["S-Tab", "["], when: function() { return linkCount() > 0 }, run: function() { lk().linkStep(-1) } },
    { id: "reader.link.open",  title: "Open link",     scope: "reader", keys: ["Enter", "o"], when: function() { return linkCurrent() }, run: function() { lk().linkOpen() } },
    { id: "reader.link.copy",  title: "Copy link address", scope: "reader", keys: ["y"],      when: function() { return linkCurrent() }, run: function() { lk().linkCopy() } },
    { id: "reader.link.clear", title: "Drop link selection", scope: "reader", keys: ["Esc"],   when: function() { return linkCurrent() }, run: function() { lk().linkClear() } },
    { id: "reader.link.hints", title: "Label links (type a label to open)", scope: "reader", keys: ["f"], when: function() { return linkCount() > 0 }, run: function() { lk().linkHints() } },

    // mail
    { id: "mail.new",      title: "New message",   scope: "mail", keys: ["c", "C-n"], ex: ["compose", "new"], run: function() { if (!inMail) app.gotoView("mail"); app.openCompose("new") } },
    { id: "mail.refresh",  title: "Refresh mail from server", scope: "mail", keys: ["F5"], ex: ["refreshmail"], run: function() { app.syncMail() } },
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
    { id: "mail.render",   title: "Cycle how this message is shown (HTML / system; message / sender / default)", scope: "mail", keys: ["H"], ex: ["render", "html"], when: function() { return app.currentMessageId !== "" }, run: function() { app.toggleHtml() } },
    { id: "mail.images",   title: "Cycle remote images (blocked / this message / this sender)", scope: "mail", keys: ["I"], ex: ["images"], when: function() { return app.currentMessageId !== "" && app.currentHtmlMode }, run: function() { app.toggleImages() } },
    { id: "mail.undo",     title: "Undo last archive / delete / move, or unsend", scope: "mail", keys: ["u"], run: function() { if (!app.undoAction()) app.undoLatest() } },

    // sms (the view's own item is `content`)
    { id: "sms.read",   title: "Mark conversation read / unread", scope: "sms", keys: ["z"], run: function() { if (content && content.toggleRead) content.toggleRead() } },
    { id: "sms.reply",  title: "Reply (write in the conversation)", scope: "sms", keys: ["r", "i"], ex: ["reply"], run: function() { if (content && content.replyHere) content.replyHere() } },
    { id: "sms.new",    title: "New text message", scope: "sms", keys: ["c", "C-n"], ex: ["compose", "new"], run: function() { if (content && content.newConversation) content.newConversation() } },
    { id: "sms.retry",  title: "Retry failed texts", scope: "sms", keys: ["R"], ex: ["retry"], run: function() { if (content && content.retryFailed) content.retryFailed() } },

    // calendar
    { id: "cal.next",  title: "Next period",     scope: "calendar/list", keys: ["]", "PgDown"], run: function() { content.step(1) } },
    { id: "cal.prev",  title: "Previous period", scope: "calendar/list", keys: ["[", "PgUp"],   run: function() { content.step(-1) } },
    { id: "cal.sel.next", title: "Next event on this day",     scope: "calendar/list", keys: ["Tab"],   when: function() { return !!content && content.atCursor !== undefined && content.atCursor.length > 1 }, run: function() { content.cycleSelection(1) } },
    { id: "cal.sel.prev", title: "Previous event on this day", scope: "calendar/list", keys: ["S-Tab"], when: function() { return !!content && content.atCursor !== undefined && content.atCursor.length > 1 }, run: function() { content.cycleSelection(-1) } },
    { id: "cal.detail.close", title: "Close event details", scope: "calendar/list", keys: ["Esc"], when: function() { return !!content && content.detailOpen === true }, run: function() { content.detailOpen = false } },
    { id: "cal.join",  title: "Join the selected event's meeting", scope: "calendar/list", keys: ["J"], ex: ["join"], run: function() { say(content.joinSelected()) } },
    { id: "cal.copyjoin", title: "Copy the selected event's join link", scope: "calendar/list", keys: ["y"], ex: ["copyjoin"], run: function() { say(content.copyJoin()) } },
    { id: "cal.account", title: "Cycle calendar account (all / each)", scope: "calendar/list", keys: ["A"], ex: ["account"], when: function() { return !!content && content.accounts !== undefined && content.accounts.length > 1 }, run: function() { say(content.cycleAccount()) } },
    { id: "cal.today", title: "Go to today",     scope: "calendar/list", keys: ["t"], run: function() { content.goToday() } },
    { id: "cal.day",   title: "Day view",        scope: "calendar/list", keys: ["d"], run: function() { content.mode = "day" } },
    { id: "cal.work",  title: "Work week view",  scope: "calendar/list", keys: ["W"], run: function() { content.mode = "workweek" } },
    { id: "cal.week",  title: "Week view",       scope: "calendar/list", keys: ["w"], run: function() { content.mode = "week" } },
    { id: "cal.month", title: "Month view",      scope: "calendar/list", keys: ["m"], run: function() { content.mode = "month" } },
    { id: "cal.refresh", title: "Refresh events", scope: "calendar/list", keys: ["r"], run: function() { content.loadEvents(true) } },
    { id: "cal.pane.widen",  title: "Widen the calendar sidebar",  scope: "calendar/list", keys: ["C-Right", "C-w >"], run: function(n) { content.resizePane(n || 1) } },
    { id: "cal.pane.narrow", title: "Narrow the calendar sidebar", scope: "calendar/list", keys: ["C-Left", "C-w <"],  run: function(n) { content.resizePane(-(n || 1)) } },
    { id: "cal.new",   title: "New event",       scope: "calendar/list", keys: ["c", "n"], run: function() { var m = content.newEvent(); if (m) say(m) } },
    { id: "cal.undo",  title: "Undo the event just saved (while its undo bar shows)", scope: "calendar/list", keys: ["C-z"], when: function() { return !!content && content.undoId !== undefined && content.undoId !== "" }, run: function() { var m = content.undoLast(); if (m) say(m) } },

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
