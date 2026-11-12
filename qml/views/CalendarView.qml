import QtQuick
import QtQuick.Layouts
import Quickshell
import QtQuick.Controls
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"
import "CalendarUtil.js" as CU
import "../calendar"
import "../calendar/draft.js" as Draft
import "../calendar/frommail.js" as FromMail
import "../calendar/when.js" as When

// CalendarView — header (view dropdown, navigation, title) above one of four
// separate views: DayView, WorkWeekView, FullWeekView, MonthView.
// Data source: http://127.0.0.1:27182/calendar/events?month=YYYY-MM
Item {
    id: root

    // ── theme: everything comes from the Omarchy theme (Commons.Color / Style / Hues); nothing here is a literal.
    //    The sub-views read the same defaults, so none of this is passed down any more.
    readonly property string monoFont:     Style.font.family
    readonly property color  accentColor:  Commons.Color.accent
    readonly property color  textColor:    Commons.Color.foreground
    readonly property color  dimColor:     Qt.darker(Commons.Color.foreground, 1.4)
    readonly property color  veryDimColor: Qt.rgba(Commons.Color.foreground.r, Commons.Color.foreground.g, Commons.Color.foreground.b, 0.3)

    // "day" | "workweek" | "week" | "month"  (AppShell keeps this across tab switches)
    property string mode: "month"
    property date   anchorDate: new Date()
    property date   today: new Date()

    // OMALOOK_PORT lets a scratch instance's UI talk to its own backend (same rule as AppState.backendUrl)
    readonly property string backendUrl: "http://127.0.0.1:" + (Quickshell.env("OMALOOK_PORT") || "27182")
    property var  rawEvents: []
    // keyboard cursor: anchorDate is the cursor day; cursorMin the 30-minute slot in the grids
    property int    cursorMin: CU.defaultMin(new Date(), new Date())
    property string accountFilter: ""          // "" = all accounts, else an account email
    property var    accounts: CU.accountsOf(rawEvents)
    // Queued writes (pending marks) are drawn with the real events until the server has them.
    property var  pendingItems: []
    readonly property var pendingRaws: pendingItems.map(function(i) { return Draft.pendingRaw(i, appAccounts) }).filter(function(r) { return r !== null })
        .map(function(r) { r.subject = Draft.pendingMark(r) + r.subject; return r })
    property var  events: CU.normalise(CU.filterAccount(Draft.applyChanges(rawEvents, pendingItems).concat(pendingRaws), accountFilter))
    property int  selIdx: 0                    // which of the events under the cursor is selected
    property bool detailOpen: false
    property var  editor: null
    readonly property var atCursor: CU.eventsAtCursor(events, anchorDate, cursorMin, mode)
    readonly property var selectedEvent: atCursor.length > 0 ? atCursor[Math.min(selIdx, atCursor.length - 1)] : null
    readonly property string selectedId: selectedEvent ? selectedEvent.raw.id : ""
    property bool loading: false
    property int  _reqSerial: 0

    readonly property var viewOptions: [
        { value: "day",      label: "Day" },
        { value: "workweek", label: "Work week" },
        { value: "week",     label: "Week" },
        { value: "month",    label: "Month" }
    ]

    // ── navigation ────────────────────────────────────────────────
    function step(dir) {
        switch (mode) {
        case "day":      anchorDate = CU.addDays(anchorDate, dir);   break
        case "workweek":
        case "week":     anchorDate = CU.addDays(anchorDate, 7 * dir); break
        default:         anchorDate = CU.addMonths(anchorDate, dir)
        }
    }
    // The masthead and "today" marks follow the clock past midnight.
    Timer {
        interval: 60000; running: true; repeat: true
        onTriggered: { var n = new Date(); if (!CU.sameDay(n, root.today)) root.today = n }
    }
    function goToday() { today = new Date(); anchorDate = today; cursorMin = CU.defaultMin(today, today) }

    // ── new event (docs/PLAN-calendar-events.md phase 2) ───────────
    property var  appAccounts: []                  // [{id, provider, email, signed_in}] from the app
    property var  eventDefaults: ({ minutes: 30, reminder: 15, seriesMonths: 12, account: "" })
    property string lastAccount: ""
    signal accountUsed(string id)
    property bool   editorOpen: false
    readonly property bool editing: editorOpen || askMode !== "" || finderOpen    // AppShell hands every key to the editor's fields, the question or the finder meanwhile
    property var    editorRestore: null
    property var    pendingMail: null          // create-from-mail hand-off from AppState.openEventFromMail (Ctrl-E)
    signal mailPrefillTaken()
    signal notice(string text)
    property string editorAccount: ""
    property var    meetingOptions: []           // the editor's Online meeting rows for editorAccount (GET /calendar/meeting-options)
    property int    _meetSerial: 0
    onEditorAccountChanged: loadMeetingOptions()
    onEditorOpenChanged: if (editorOpen) loadMeetingOptions()
    property string editorStatus: ""
    property date   editorStart: new Date()
    property var    editorSnapshot: null           // what was in the editor at Save, for Undo
    property string undoId: ""                     // the queued write the undo bar is about
    property string undoMessage: ""
    property string undoState: ""                  // pending | sending | sent | failed | ""
    property string undoKind: "event"              // event | change | tags: what the bar is about (Ctrl-Z only reopens the editor for an event)

    // ── edit, cancel, decline, delete (phase 3; docs/PLAN-calendar-events.md 5.3, 6, 12) ──
    property string editorMode: "create"          // create | edit (PATCH on save)
    property var    editorOriginal: null          // edit: { raw (cached row), start, end, allDay }
    property string editorScope: "occurrence"     // edit: occurrence | series
    property var    _changeTargets: ({})          // queued change id -> { target, series } (the outbox list does not carry the target)
    property string askMode: ""                   // "" | scope (occurrence or series?) | confirm (cancel / decline / delete)
    property string action: ""                    // edit | remove: what the question is about
    property var    actionRaw: null               // the cached row it is about
    property string actionKind: ""                // cancel | decline | delete
    property string actionScope: "occurrence"
    property int    actionCount: -1               // invitees from the dry run; -1 while counting
    property string actionStatus: ""              // the owner's refusal, shown on the question
    readonly property bool asking: askMode !== ""
    property var    appContacts: []               // [{name, email}] best first (AppState.composeContacts)
    property var    dirPeople: []                 // the organisation directory's answer for the editor's last query
    property string dirQuery: ""
    property int    _dirSerial: 0
    readonly property var suggestionList: Draft.mergePeople(appContacts, dirPeople)

    // OL-040 phase 6: show only events whose tags match (mail `tag:` syntax). "" = all.
    property string tagFilter: ""
    onTagFilterChanged: loadEvents(true)

    // Set the selected event's tags (a recurring event is tagged as a whole). Returns a message for the status bar, or "".
    function setSelectedTags(tags) {
        if (!selectedEvent) return "No event selected"
        var raw = selectedEvent.raw
        if (raw.pending === true) return "That event is not saved yet"
        _post("POST", "/calendar/tags?account=" + encodeURIComponent(raw.account_id) + "&id=" + encodeURIComponent(raw.id)
              + "&tags=" + encodeURIComponent(JSON.stringify(tags)), null, function(r) {
            if (!r.ok) { undoKind = "tags"; undoId = ""; undoState = "failed"; undoMessage = "Tags not saved: " + (r.error || "unknown error"); hideBar.restart(); return }
            undoKind = "tags"
            undoId = r.id
            undoState = "pending"
            undoMessage = (tags.length === 0 ? "Removing the tags" : "Tags queued") + (r.series ? " on the whole series" : "")
            pollOutbox()
            outboxTimer.start()
        })
        return ""
    }

    function newEvent() {
        if (editorOpen) return ""
        var under = selectedEvent ? selectedEvent.raw.account_email : ""
        var id = Draft.chooseAccount(appAccounts, { filterEmail: accountFilter, cursorEmail: under,
                                                   defaultAccount: eventDefaults.account, lastUsed: lastAccount })
        if (id === "") return "No signed-in account can create events yet (Microsoft work/school accounts only)"
        var d = new Date(anchorDate.getFullYear(), anchorDate.getMonth(), anchorDate.getDate(), 0, mode === "month" ? 9 * 60 : cursorMin)
        editorStart = d
        editorMode = "create"
        editorOriginal = null
        editorAccount = id
        editorStatus = ""
        editorRestore = null
        editorOpen = true
        return ""
    }

    // ── create-from-mail (Ctrl-E; qml/calendar/frommail.js). Opens the editor prefilled from the message; nothing is sent.
    function takeMailPrefill() {
        var p = pendingMail
        if (!p || editorOpen) return
        pendingMail = null
        mailPrefillTaken()
        var m = p.msg || {}
        var busy = events.map(function(e) {
            var s = e.startDate, en = e.endDate
            return { start: new Date(s.getFullYear(), s.getMonth(), s.getDate(), 0, e.allDay ? 0 : e.startMin),
                     end: new Date(en.getFullYear(), en.getMonth(), en.getDate() + (e.allDay ? 1 : 0), 0, e.allDay ? 0 : e.endMin) }
        })
        var d = FromMail.prefill(m, { now: new Date(), ownEmails: p.ownEmails || [], body: p.body || "",
                                      parseWhen: When.parseWhen, defaultMinutes: eventDefaults.minutes,
                                      defaultReminder: eventDefaults.reminder, busy: busy })
        var acc = Draft.chooseAccount(appAccounts, { filterEmail: accountFilter, cursorEmail: m.account_email || "",
                                                     defaultAccount: eventDefaults.account, lastUsed: lastAccount })
        if (acc === "") { notice("No signed-in account can create events yet (Microsoft work/school or Google)"); return }
        editorStart = d.start
        editorMode = "create"
        editorOriginal = null
        editorAccount = acc
        editorStatus = "From the mail. Nothing is sent until you save (Ctrl-Enter)."
        editorRestore = d.restore
        editorOpen = true
    }

    // ── free-time finder (track A; docs/PLAN-calendar-events.md 5.6). Opened from the editor (its attendees, start and length)
    //    or from the calendar (the selected event's attendees, the cursor). A slot goes back to the editor, or opens a new one.
    property bool   finderOpen: false
    property string finderSource: ""              // editor | calendar: where a picked slot goes
    property var    finderState: ({ accountId: "", accountLabel: "", required: [], optional: [], start: new Date(), minutes: 30 })
    readonly property var finderOthers: appAccounts.filter(function(a) { return Draft.canCreate(a) && a.id !== finderState.accountId })
        .map(function(a) { return { id: a.id, email: a.email || "" } })
    onFinderOpenChanged: if (!finderOpen && editorOpen && editor) editor.focusFirst()

    function _accountById(id) {
        for (var i = 0; i < appAccounts.length; i++) if (appAccounts[i].id === id) return appAccounts[i]
        return null
    }
    // Returns a status message ("" when the finder opened).
    function openFinder(fromEditor) {
        if (finderOpen || (!fromEditor && editorOpen)) return ""
        var acc = "", req = [], opt = [], start = anchorDate, mins = eventDefaults.minutes
        if (fromEditor && editor) {
            acc = editorAccount
            var list = editor.people.list || []
            list.forEach(function(a) {
                if (a.role === "resource") return
                if (a.role === "optional") opt.push(a.email); else req.push(a.email)
            })
            var w = editor.whenResult
            start = w.ok ? w.start : editorStart
            if (w.ok && w.end) mins = Math.max(15, Math.round((w.end.getTime() - w.start.getTime()) / 60000))
        } else {
            var under = selectedEvent ? selectedEvent.raw.account_email : ""
            acc = Draft.chooseAccount(appAccounts, { filterEmail: accountFilter, cursorEmail: under,
                                                     defaultAccount: eventDefaults.account, lastUsed: lastAccount })
            var raw = selectedEvent ? selectedEvent.raw : null
            if (raw) (raw.attendees || []).forEach(function(a) {
                if (a.role === "resource" || !a.email) return
                if (a.role === "optional") opt.push(a.email); else req.push(a.email)
            })
            start = new Date(anchorDate.getFullYear(), anchorDate.getMonth(), anchorDate.getDate(), 0, mode === "month" ? 9 * 60 : cursorMin)
        }
        var a = _accountById(acc)
        if (!a || !Draft.canCreate(a)) return "Free time needs a work or school (Exchange) account"
        finderState = { accountId: a.id, accountLabel: a.email || a.id, required: req, optional: opt, start: start, minutes: mins }
        finderSource = fromEditor ? "editor" : "calendar"
        finderOpen = true
        return ""
    }
    // The finder's pick: the editor takes it (open already), or a new editor opens at the slot with the same people.
    function pickFreeSlot(s, e) {
        var fs = finderState
        finderOpen = false
        if (finderSource === "editor" && editor) { editor.applySlot(s, e); return }
        editorRestore = { subject: "", notes: "", location: "", reminder: String(eventDefaults.reminder), showAs: "busy", repeatText: "",
                          base: { start: s, end: e, allDay: false },
                          required: fs.required.map(function(m) { return { email: m, name: "" } }),
                          optional: fs.optional.map(function(m) { return { email: m, name: "" } }) }
        editorMode = "create"
        editorOriginal = null
        editorAccount = fs.accountId
        editorStatus = ""
        editorStart = s
        editorOpen = true
    }

    function _post(method, path, body, cb) {
        var xhr = new XMLHttpRequest()
        xhr.onreadystatechange = function() {
            if (xhr.readyState !== XMLHttpRequest.DONE) return
            var r = {}
            try { r = JSON.parse(xhr.responseText) } catch (e) { r = { ok: false, error: xhr.status === 0 ? "the backend is not answering" : "unreadable answer" } }
            cb(r)
        }
        xhr.open(method, backendUrl + path)
        if (body !== null) xhr.setRequestHeader("Content-Type", "application/json")
        xhr.send(body)
    }

    function loadMeetingOptions() {
        var acct = editorAccount, serial = ++_meetSerial
        if (!editorOpen || acct === "") { meetingOptions = []; return }
        _post("GET", "/calendar/meeting-options?account=" + encodeURIComponent(acct), null, function(r) {
            if (serial !== _meetSerial) return
            meetingOptions = r.ok && r.options ? r.options : []
        })
    }

    function accountOfRaw(raw) {
        for (var i = 0; i < appAccounts.length; i++) if (appAccounts[i].id === raw.account_id) return appAccounts[i]
        return null
    }
    function accountEmailOf(id) {
        for (var i = 0; i < appAccounts.length; i++) if (appAccounts[i].id === id) return appAccounts[i].email || ""
        return ""
    }

    // `e`: the selected event opens in the editor, filled. A repeating event first asks occurrence or series.
    function keyEdit() {
        if (editing) return ""
        var raw = selectedEvent ? selectedEvent.raw : null
        if (!raw) return "No event selected"
        var why = Draft.editRefusal(raw, accountOfRaw(raw))
        if (why !== "") return why
        if (Draft.isRecurring(raw)) { action = "edit"; actionRaw = raw; askMode = "scope"; return "" }
        _openEdit(raw, "occurrence")
        return ""
    }
    function _openEdit(raw, scope) {
        var s = CU.parseStamp(raw.start)
        if (!s) return
        var e = CU.parseStamp(raw.end) || s
        editorOriginal = {
            raw: raw, allDay: raw.is_all_day === true || !s.hasTime,
            start: new Date(s.date.getFullYear(), s.date.getMonth(), s.date.getDate(), 0, s.minutes || 0),
            end: new Date(e.date.getFullYear(), e.date.getMonth(), e.date.getDate(), 0, e.minutes || 0) }
        editorMode = "edit"
        editorScope = scope
        editorAccount = raw.account_id
        editorStart = editorOriginal.start
        editorStatus = ""
        editorRestore = null
        cancelAsk()
        editorOpen = true
    }
    function _savePatch(body) {
        if (!editorOriginal) { editorStatus = "Nothing to save: the event is not open"; return }
        editorStatus = "saving\u2026"
        editorSnapshot = null
        var raw = editorOriginal.raw, scope = editorScope
        _post("PATCH", Draft.changePath("update", raw.id, scope, "", false), JSON.stringify(body), function(r) {
            if (!r.ok) { editorStatus = r.error || "could not save"; return }
            editorOpen = false
            editorStatus = ""
            _queued(r, "update", scope === "series", r.invitees || 0)
        })
    }

    // `x`: delete a personal entry, cancel a meeting you organise, decline one you attend. A repeating event asks
    // occurrence or series first; then the question shows the invitee count from a dry run, and nothing queues until Enter.
    function keyRemove() {
        if (editing) return ""
        var raw = selectedEvent ? selectedEvent.raw : null
        if (!raw) return "No event selected"
        var rk = Draft.removeKind(raw, accountOfRaw(raw))
        if (rk.error) return rk.error
        actionRaw = raw
        actionKind = rk.kind
        action = "remove"
        if (Draft.isRecurring(raw)) { askMode = "scope"; return "" }
        _askRemove("occurrence")
        return ""
    }
    function _askRemove(scope) {
        actionScope = scope
        actionCount = -1
        actionStatus = ""
        askMode = "confirm"
        var acct = accountOfRaw(actionRaw)
        _post("POST", Draft.changePath("cancel", actionRaw.id, scope, actionKind, true),
              JSON.stringify({ account_id: acct ? acct.id : actionRaw.account_id, comment: "" }), function(r) {
            if (!r.ok) { actionStatus = r.error || "could not check who is invited"; actionCount = 0; return }
            actionCount = r.invitees || 0
        })
    }
    function pickScope(scope) {
        if (action === "edit") _openEdit(actionRaw, scope)
        else if (action === "remove") _askRemove(scope)
    }
    function confirmRemove(message) {
        if (actionCount < 0) { actionStatus = "Still counting who is invited. One moment."; return }
        var raw = actionRaw, kind = actionKind, series = actionScope === "series"
        var acct = accountOfRaw(raw)
        actionStatus = ""
        _post("POST", Draft.changePath("cancel", raw.id, actionScope, kind, false),
              JSON.stringify({ account_id: acct ? acct.id : raw.account_id, comment: message || "" }), function(r) {
            if (!r.ok) { actionStatus = r.error || "not queued"; return }
            cancelAsk()
            _queued(r, kind, series, r.invitees || 0)
        })
    }
    function cancelAsk() { askMode = ""; action = ""; actionRaw = null; actionStatus = ""; actionCount = -1 }

    // A queued change: the undo bar takes it, and the outbox target is kept so the pending mark lands on the right row.
    function _queued(r, op, series, count) {
        _changeTargets[r.id] = { target: r.target || "", series: series === true }
        undoKind = "change"
        undoId = r.id
        undoState = "pending"
        undoMessage = Draft.changeMessage(op, count, series)
        pollOutbox()
        outboxTimer.start()
    }

    // The attendee suggestions: the organisation directory for what is typed (two letters or more), after a short pause.
    function queryDirectory(text) { dirQuery = String(text || "").trim(); dirTimer.restart() }
    function _searchDirectory() {
        var q = dirQuery, acct = editorAccount, serial = ++_dirSerial
        if (q.length < 2 || acct === "") { dirPeople = []; return }
        _post("GET", "/contacts/directory?account=" + encodeURIComponent(acct) + "&q=" + encodeURIComponent(q), null, function(r) {
            if (serial !== _dirSerial) return
            dirPeople = r.ok && r.people ? r.people.map(function(p) { return { name: p.name || "", email: p.email || "" } }) : []
        })
    }
    Timer { id: dirTimer; interval: 350; onTriggered: root._searchDirectory() }

    function saveEvent(body) {
        if (editorMode === "edit") { _savePatch(body); return }
        editorStatus = "saving\u2026"
        editorSnapshot = editor ? editor.snapshot() : null
        _post("POST", "/calendar/event", JSON.stringify(body), function(r) {
            if (!r.ok) { editorStatus = r.error || "could not save"; return }
            editorOpen = false
            editorStatus = ""
            accountUsed(body.account_id)
            undoKind = "event"
            undoId = r.id
            undoState = "pending"
            var n = r.attendees || 0
            undoMessage = (body.recurrence ? "Series queued" : "Event queued") + (n > 0 ? " for " + n + " people" : " (personal)")
            pollOutbox()
            outboxTimer.start()
        })
    }

    // Ctrl-Z while the bar shows: take the queued event back and reopen the editor with everything typed.
    function undoLast() {
        if (undoId === "") return "Nothing to undo"
        if (undoState !== "pending" && undoState !== "failed") return "Too late to undo: the event is already saved"
        var id = undoId
        _post("POST", "/calendar/event/undo?id=" + encodeURIComponent(id), null, function(r) {
            if (r.cancelled === true) {
                undoId = ""; undoState = ""; undoMessage = ""
                pollOutbox()
                if (undoKind !== "event") { undoMessage = ""; return }
                if (editorSnapshot && !editorOpen) {
                    editorRestore = editorSnapshot; editorStatus = "Undone. Your event is back in the editor."
                    editorStart = editorSnapshot.base.start
                    editorOpen = true
                }
            } else {
                undoState = r.state || ""
                undoMessage = r.state === "sent" ? "Too late to undo: the event is already saved" : (r.error || "Could not undo")
            }
        })
        return ""
    }

    function pollOutbox() {
        _post("GET", "/calendar/outbox", null, function(r) {
            if (!r.ok) return
            var items = r.items || [], wasLive = {}, i
            for (i = 0; i < pendingItems.length; i++) wasLive[pendingItems[i].id] = true
            var sentNow = false
            for (i = 0; i < items.length; i++) {
                if (items[i].state === "sent" && wasLive[items[i].id]) sentNow = true
                if (items[i].id === undoId) {
                    undoState = items[i].state
                    if (items[i].state === "sent") undoMessage = "Saved"
                    else if (items[i].state === "failed") undoMessage = "Not saved: " + (items[i].error || "the server refused it") + (undoKind === "event" ? "   (Ctrl-Z to edit it again)" : "")
                }
            }
            for (i = 0; i < items.length; i++) {
                var ct = _changeTargets[items[i].id]
                if (ct) { items[i].target = ct.target; items[i].series = ct.series }
            }
            pendingItems = items.filter(function(it) { return it.state === "pending" || it.state === "sending" || it.state === "failed" })
            if (sentNow) loadEvents(true)
            var live = pendingItems.some(function(it) { return it.state !== "failed" })
            if (!live) outboxTimer.stop()
            if (!live && undoState === "sent") hideBar.restart()
        })
    }
    Timer { id: outboxTimer; interval: 1000; repeat: true; onTriggered: root.pollOutbox() }
    Timer { id: hideBar; interval: 4000; onTriggered: { if (root.undoState === "sent") { root.undoId = ""; root.undoState = ""; root.undoMessage = "" } } }

    // ── keyboard (called by the key registry; see AppCommands.move/page/edge/activate) ──
    function keyMove(dx, dy) {
        var c = CU.moveCursor({ date: anchorDate, min: cursorMin }, mode, dx, dy)
        cursorMin = c.min
        anchorDate = c.date
        return true
    }
    function keyPage(frac) {
        if (mode === "month") { step(frac < 0 ? -1 : 1); return }
        var c = CU.moveSlots({ date: anchorDate, min: cursorMin }, Math.round(frac * 16))
        cursorMin = c.min
    }
    function keyEdge(bottom) { if (mode !== "month") cursorMin = bottom ? 24 * 60 - CU.SLOT : 0 }
    function keyActivate() {
        if (selectedEvent) detailOpen = !detailOpen
        else if (mode === "month") openDay(anchorDate)
    }
    function cycleSelection(dir) {
        var n = atCursor.length
        if (n > 0) selIdx = (Math.min(selIdx, n - 1) + dir + n) % n
    }
    function cycleAccount() {
        accountFilter = CU.nextAccount(accounts, accountFilter)
        return accountFilter === "" ? "All accounts" : accountFilter
    }
    // Both return a one-line message for the status bar.
    function joinSelected() {
        var url = selectedEvent ? CU.joinUrl(selectedEvent.raw) : ""
        if (url === "") return selectedEvent ? "No join link on this event" : "No event selected"
        Qt.openUrlExternally(url)
        return "Opening " + CU.joinLabel(selectedEvent.raw) + " link"
    }
    function copyJoin() {
        var url = selectedEvent ? CU.joinUrl(selectedEvent.raw) : ""
        if (url === "") return selectedEvent ? "No join link on this event" : "No event selected"
        Quickshell.execDetached(["wl-copy", url])
        return "Copied join link"
    }

    function openDay(d) { anchorDate = d; mode = "day" }

    // inclusive range of dates the current view can show
    function visibleRange() {
        switch (mode) {
        case "day":      return [CU.startOfDay(anchorDate), CU.startOfDay(anchorDate)]
        case "workweek":
        case "week":     var s = CU.weekStart(anchorDate); return [s, CU.addDays(s, 6)]
        default:         var g = CU.monthGridStart(anchorDate); return [g, CU.addDays(g, 41)]
        }
    }

    readonly property string title: {
        var a = anchorDate
        if (mode === "day")
            return CU.DAYS_SHORT[CU.isoIndex(a)] + "  " + a.getDate() + " " + CU.MONTHS[a.getMonth()] + " " + a.getFullYear()
        if (mode === "workweek" || mode === "week") {
            var s = CU.weekStart(a), e = CU.addDays(s, mode === "workweek" ? 4 : 6)
            var l = s.getDate() + (s.getMonth() !== e.getMonth() ? " " + CU.MONTHS[s.getMonth()].substring(0, 3) : "")
            return l + " – " + e.getDate() + " " + CU.MONTHS[e.getMonth()].substring(0, 3) + " " + e.getFullYear()
        }
        return CU.MONTHS[a.getMonth()] + "  " + a.getFullYear()
    }

    // ── data loading: fetch every month the visible range touches ─
    property var _cache: ({})       // "YYYY-MM" -> array

    function _collect(months) {
        var out = [], seen = {}
        for (var i = 0; i < months.length; i++) {
            var list = _cache[months[i]] || []
            for (var j = 0; j < list.length; j++) {
                var k = JSON.stringify(list[j])
                if (!seen[k]) { seen[k] = true; out.push(list[j]) }
            }
        }
        return out
    }
    function _merge() {
        var r = visibleRange()
        rawEvents = _collect(CU.monthsBetween(r[0], r[1]))
        sideRaw = _collect(CU.sideMonths(anchorDate, today))
    }
    // Events for the sidebar (mini-month dots, agenda): the mini month's grid plus today and the next 30 days.
    property var sideRaw: []
    readonly property var sideEvents: CU.normalise(CU.filterAccount(sideRaw, accountFilter))
    readonly property var sideAccounts: CU.accountsOf(sideRaw)

    function loadEvents(force) {
        var r = visibleRange(), months = CU.monthsBetween(r[0], r[1])
        CU.sideMonths(anchorDate, today).forEach(function(m) { if (months.indexOf(m) < 0) months.push(m) })
        var pending = 0, serial = ++_reqSerial
        if (force) _cache = ({})
        months.forEach(function(m) {
            if (_cache[m] !== undefined) return
            pending++
            var xhr = new XMLHttpRequest()
            xhr.onreadystatechange = function() {
                if (xhr.readyState !== XMLHttpRequest.DONE) return
                var res = []
                if (xhr.status === 200) { try { res = JSON.parse(xhr.responseText) } catch (e) { res = [] } }
                var c = root._cache; c[m] = res; root._cache = c
                if (--pending === 0 && serial === root._reqSerial) { root.loading = false; root._merge() }
            }
            xhr.open("GET", root.backendUrl + "/calendar/events?month=" + m + (root.tagFilter !== "" ? "&tag=" + encodeURIComponent(root.tagFilter) : ""))
            xhr.send()
        })
        if (pending === 0) { loading = false; _merge() } else loading = true
    }

    // Bumped by the app after an accepted / tentative invitation (the backend has just re-expanded
    // the calendar), so the cache is dropped and the visible months are fetched again.
    property int dataRev: 0
    onDataRevChanged:    loadEvents(true)

    onModeChanged:       { selIdx = 0; loadEvents(false) }
    onAnchorDateChanged: { selIdx = 0; loadEvents(false) }
    onCursorMinChanged:  selIdx = 0
    onSelectedEventChanged: if (!selectedEvent) detailOpen = false
    Component.onCompleted: { loadEvents(true); pollOutbox(); takeMailPrefill() }
    onPendingMailChanged: takeMailPrefill()

    // ── sidebar width (OL-031): characters relative to the default, kept as [ui] calendar_pane_offset ──
    TextMetrics { id: charMetrics; text: "0"; font.family: Style.font.family; font.pixelSize: Style.font.body }
    readonly property real charWidth: Math.max(1, charMetrics.advanceWidth)
    readonly property real defaultSideWidth: Style.space(250)
    readonly property real minSideWidth: Style.space(160)
    readonly property real minMainWidth: Style.space(420)
    property int paneOffset: 0
    signal paneOffsetRequested(int offset)
    readonly property int paneMinOffset: Math.ceil((minSideWidth - defaultSideWidth) / charWidth)
    readonly property int paneMaxOffset: Math.max(0, Math.floor((width - minMainWidth - sideRow.spacing - defaultSideWidth) / charWidth))
    function resizePane(dCols) {
        var n = Math.max(paneMinOffset, Math.min(paneMaxOffset, paneOffset + dCols))
        if (n !== paneOffset) paneOffsetRequested(n)
    }
    readonly property real sideWidth: Math.max(minSideWidth, Math.min(defaultSideWidth + paneOffset * charWidth, width - minMainWidth - sideRow.spacing))

    // ── layout ────────────────────────────────────────────────────
    RowLayout {
        id: sideRow
        anchors.fill: parent
        spacing: Style.spacing.sm

        // left pane: mini month, calendars, agenda
        Item {
            Layout.preferredWidth: root.sideWidth
            Layout.fillHeight: true
            PaneFrame { tint: Hues.magenta; focused: false; title: "Agenda" }
            CalendarDateHero {
                id: hero
                anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                anchors.leftMargin: Style.spacing.xl; anchors.rightMargin: Style.spacing.lg
                anchors.topMargin: Style.spacing.xl + Style.spacing.md
                today: root.today
                onClicked: root.goToday()
            }
            CalendarSidebar {
                anchors.left: parent.left; anchors.right: parent.right
                anchors.top: hero.bottom; anchors.bottom: parent.bottom
                anchors.margins: Style.spacing.lg; anchors.topMargin: Style.spacing.lg
                anchorDate: root.anchorDate; today: root.today
                events: root.sideEvents
                accounts: root.sideAccounts
                accountFilter: root.accountFilter
                onDayPicked: function(d) { root.anchorDate = d }
                onMonthStep: function(dir) { root.anchorDate = CU.addMonths(root.anchorDate, dir) }
                onAccountPicked: function(email) { root.accountFilter = email }
            }
        }

        // main pane: header and the grid
        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true
            PaneFrame { tint: Hues.blue; focused: true; title: root.title }

            Loader {
                id: editorLoader
                anchors.fill: parent
                anchors.margins: Style.spacing.sm; anchors.topMargin: Style.spacing.xl
                z: 100
                active: root.editorOpen
                sourceComponent: Component {
                    EventEditor {
                        id: ed
                        now: root.today
                        initialStart: root.editorStart
                        defaultMinutes: root.eventDefaults.minutes
                        defaultReminder: root.eventDefaults.reminder
                        defaultSeriesMonths: root.eventDefaults.seriesMonths
                        accounts: root.appAccounts.filter(Draft.canCreate)
                        accountId: root.editorAccount
                        events: root.events
                        status: root.editorStatus
                        restore: root.editorRestore
                        mode: root.editorMode
                        original: root.editorOriginal
                        ownEmail: root.accountEmailOf(root.editorAccount)
                        suggestions: root.suggestionList
                        meetingOptions: root.meetingOptions
                        onDirectoryQuery: function(text) { root.queryDirectory(text) }
                        onAccountPicked: function(id) { root.editorAccount = id }
                        onSaveRequested: function(body) { root.saveEvent(body) }
                        onCancelRequested: { root.editorOpen = false; root.editorStatus = "" }
                        onFindRequested: { var m = root.openFinder(true); if (m !== "") root.editorStatus = m }
                        Component.onCompleted: root.editor = ed
                        Component.onDestruction: if (root.editor === ed) root.editor = null
                    }
                }
            }

            // the free-time finder (Ctrl-F in the editor, `C-f` on the calendar): over the grid, above the editor
            Loader {
                id: finderLoader
                anchors.fill: parent
                anchors.margins: Style.spacing.sm; anchors.topMargin: Style.spacing.xl
                z: 110
                active: root.finderOpen
                sourceComponent: Component {
                    FreeTimeFinder {
                        backendUrl: root.backendUrl
                        accountId: root.finderState.accountId
                        accountLabel: root.finderState.accountLabel
                        requiredEmails: root.finderState.required
                        optionalEmails: root.finderState.optional
                        others: root.finderOthers
                        initialStart: root.finderState.start
                        initialMinutes: root.finderState.minutes
                        onPickRequested: function(s, e) { root.pickFreeSlot(s, e) }
                        onCloseRequested: root.finderOpen = false
                    }
                }
            }

            // the question for `e` and `x`: occurrence or series, then the confirm line (who it reaches, a cancel message)
            EventChangeDialog {
                id: askDialog
                anchors.fill: parent
                z: 105
                visible: root.asking
                mode: root.askMode === "scope" ? "scope" : "confirm"
                heading: root.action === "edit" ? (root.askMode === "scope" ? "Edit a repeating event" : "Edit this event")
                    : root.actionKind === "cancel" ? "Cancel this meeting"
                    : root.actionKind === "decline" ? "Decline this invitation" : "Delete this event"
                askLine: root.action === "edit" ? "Change only this occurrence, or the whole series?"
                    : root.actionKind === "decline" ? "Decline only this occurrence, or the whole series?"
                    : "Cancel or delete only this occurrence, or the whole series?"
                detail: root.askMode === "confirm" ? Draft.removeLine(root.actionKind, root.actionCount) : ""
                allowMessage: root.askMode === "confirm" && root.actionKind === "cancel"
                status: root.actionStatus
                onScopePicked: function(scope) { root.pickScope(scope) }
                onConfirmed: function(message) { root.confirmRemove(message) }
                onDismissed: root.cancelAsk()
            }

            // the undo bar: after a save, a delete or a cancel, until the server has it
            Rectangle {
                id: undoBar
                visible: root.undoMessage !== "" && !root.editorOpen
                z: 90
                anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom
                anchors.margins: Style.spacing.lg
                height: undoText.implicitHeight + Style.spacing.lg * 2
                radius: Style.cornerRadius
                color: Qt.rgba(Commons.Color.background.r, Commons.Color.background.g, Commons.Color.background.b, 0.95)
                border.width: 1
                border.color: root.undoState === "failed" ? Commons.Color.urgent : Commons.Color.accent
                UiText {
                    id: undoText
                    anchors.fill: parent; anchors.margins: Style.spacing.lg
                    verticalAlignment: Text.AlignVCenter; elide: Text.ElideRight
                    text: root.undoMessage + (root.undoState === "pending" ? ".   Ctrl-Z undo" : "")
                    foreground: root.undoState === "failed" ? Commons.Color.urgent : Commons.Color.foreground
                }
            }

            ColumnLayout {
                anchors.fill: parent
                anchors.margins: Style.spacing.sm; anchors.topMargin: Style.spacing.xl
                spacing: 0

                // header: ‹ Today › · Day / Work week / Week / Month · account · busy
                RowLayout {
                    Layout.fillWidth: true
                    Layout.margins: Style.spacing.lg
                    spacing: Style.spacing.md

                    PanelActionButton { iconText: "\uf053"; tooltipText: "Previous ( [ )"; onClicked: root.step(-1) }
                    Button { text: "Today"; tooltipText: "Go to today ( t )"; onClicked: root.goToday() }
                    PanelActionButton { iconText: "\uf054"; tooltipText: "Next ( ] )"; onClicked: root.step(1) }

                    Item { Layout.fillWidth: true }

                    UiText {
                        visible: root.tagFilter !== ""
                        text: "tag: " + root.tagFilter + "  (f)"
                        foreground: Commons.Color.accent
                        font.pixelSize: Style.font.caption
                    }
                    UiText {
                        visible: root.accounts.length > 1
                        text: (root.accountFilter === "" ? "all accounts" : root.accountFilter) + "  (A)"
                        dim: root.accountFilter === ""
                        foreground: root.accountFilter === "" ? Commons.Color.foreground : Commons.Color.accent
                        font.pixelSize: Style.font.caption
                    }
                    UiText {
                        visible: root.loading
                        text: "\uf021"
                        dim: true
                        RotationAnimation on rotation {
                            from: 0; to: 360; duration: 1200
                            loops: Animation.Infinite; running: root.loading
                        }
                    }
                    ButtonGroup {
                        options: root.viewOptions
                        value: root.mode
                        focusable: false
                        onChanged: function(v) { root.mode = v }
                    }
                }

                PanelSeparator { Layout.fillWidth: true }

                RowLayout {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    spacing: 0

                    // active view
                    Loader {
                        id: viewLoader
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        sourceComponent: {
                            switch (root.mode) {
                            case "day":      return dayComponent
                            case "workweek": return workWeekComponent
                            case "week":     return fullWeekComponent
                            default:         return monthComponent
                            }
                        }
                    }

                    CalendarDetail {
                        visible: root.detailOpen
                        Layout.preferredWidth: Math.min(Style.space(360), root.width * 0.3)
                        Layout.fillHeight: true
                        event: root.selectedEvent
                        accounts: root.accounts
                    }
                }
            }
        }
    }

    Component {
        id: dayComponent
        DayView {
            cursorMin: root.cursorMin; selectedId: root.selectedId; accounts: root.accounts;
            anchorDate: root.anchorDate; events: root.events; today: root.today
            onDayClicked: function(d) { root.anchorDate = d }
        }
    }
    Component {
        id: workWeekComponent
        WorkWeekView {
            cursorMin: root.cursorMin; selectedId: root.selectedId; accounts: root.accounts;
            anchorDate: root.anchorDate; events: root.events; today: root.today
            onDayClicked: function(d) { root.anchorDate = d }
            onDayActivated: function(d) { root.openDay(d) }
        }
    }
    Component {
        id: fullWeekComponent
        FullWeekView {
            cursorMin: root.cursorMin; selectedId: root.selectedId; accounts: root.accounts;
            anchorDate: root.anchorDate; events: root.events; today: root.today
            onDayClicked: function(d) { root.anchorDate = d }
            onDayActivated: function(d) { root.openDay(d) }
        }
    }
    Component {
        id: monthComponent
        MonthView {
            selectedId: root.selectedId; accounts: root.accounts;
            anchorDate: root.anchorDate; events: root.events; today: root.today
            onDayClicked: function(d) { root.anchorDate = d }
            onDayActivated: function(d) { root.openDay(d) }
        }
    }
}
