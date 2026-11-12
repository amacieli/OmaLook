.pragma library

// Pure helpers for the event editor (docs/PLAN-calendar-events.md section 5.3 / 6): building the
// request the backend takes, the conflict line, which account a new event goes to, and the pending
// marks. No QML here, so tests/draft.test.js checks it with node.

function pad(n) { return n < 10 ? "0" + n : "" + n }

// "2026-10-28T10:00:00": local wall-clock, no offset (the zone travels in `time_zone`).
function stamp(d) {
    return d.getFullYear() + "-" + pad(d.getMonth() + 1) + "-" + pad(d.getDate())
         + "T" + pad(d.getHours()) + ":" + pad(d.getMinutes()) + ":00"
}

// f = { accountId, subject, notes, when: parseWhen result, location, reminder (minutes or null),
//       showAs, repeat: parseRepeat result or null, transactionId }
// -> { ok:true, body } | { ok:false, error }
function buildRequest(f) {
    if (!f.accountId) return { ok: false, error: "no calendar account selected" }
    if (!f.when || !f.when.ok) return { ok: false, error: f.when && f.when.error ? f.when.error : "the time is not understood" }
    if (f.repeat && f.repeat.ok === false) return { ok: false, error: f.repeat.error || "the repeat is not understood" }
    var body = {
        account_id: f.accountId,
        subject: String(f.subject || "").trim(),
        body: String(f.notes || ""),
        start: stamp(f.when.start),
        end: stamp(f.when.end),
        all_day: f.when.allDay === true,
        show_as: f.showAs || "busy",
        transaction_id: f.transactionId || ""
    }
    var loc = String(f.location || "").trim()
    if (loc !== "") body.location = loc
    if (f.reminder !== null && f.reminder !== undefined && f.reminder >= 0) body.reminder_minutes = f.reminder
    if (f.repeat && f.repeat.ok && f.repeat.pattern)
        body.recurrence = { pattern: f.repeat.pattern, range: f.repeat.range }
    if (f.attendees && f.attendees.length > 0) body.attendees = f.attendees   // attendeeList rows; none = personal
    if (f.online && f.online !== "none") body.online_provider = f.online   // OL-021 B: the backend adds the link (docs 7.x)
    return { ok: true, body: body }
}

// Titles of the timed events that overlap [start, end) (all-day events never count). `events` are
// normalised calendar events (CalendarUtil.normalise).
function conflicts(events, start, end, allDay) {
    if (allDay) return []
    var out = []
    for (var i = 0; i < (events || []).length; i++) {
        var e = events[i]
        if (e.allDay) continue
        var s = new Date(e.startDate.getFullYear(), e.startDate.getMonth(), e.startDate.getDate(), 0, e.startMin)
        var ed = e.endDate || e.startDate   // a timed event can end on a later day
        var en = new Date(ed.getFullYear(), ed.getMonth(), ed.getDate(), 0, e.endMin)
        if (s < end && en > start) out.push(e.title)
    }
    return out
}

// Microsoft work/school (Graph) and Google accounts can create and change events (Google: OL-021 track C).
function canCreate(account) { return !!account && account.signed_in !== false && (account.provider === "exchange" || account.provider === "gmail" || account.provider === "outlook") }

// Which account a new event goes to (plan 5.1): the filtered one, else the one under the cursor, else
// the pinned default, else the one used last, else the first that can create. Returns an account id or "".
function chooseAccount(accounts, o) {
    var ok = (accounts || []).filter(canCreate)
    function byEmail(em) { for (var i = 0; i < ok.length; i++) if (ok[i].email === em) return ok[i].id; return "" }
    function byId(id) { for (var i = 0; i < ok.length; i++) if (ok[i].id === id) return id; return "" }
    return (o.filterEmail && byEmail(o.filterEmail)) || (o.cursorEmail && byEmail(o.cursorEmail))
        || (o.defaultAccount && byId(o.defaultAccount)) || (o.lastUsed && byId(o.lastUsed))
        || (ok.length ? ok[0].id : "")
}

// Next creatable account after `id` (the Account row's cycle key).
function cycleAccount(accounts, id, dir) {
    var ok = (accounts || []).filter(canCreate)
    if (!ok.length) return ""
    var i = 0
    for (var k = 0; k < ok.length; k++) if (ok[k].id === id) i = k
    return ok[(i + (dir || 1) + ok.length) % ok.length].id
}

// A backend outbox item (GET /calendar/outbox) drawn as a calendar event with a pending / failed mark.
// Items that are done are not drawn: the real event is in the cache by then.
function pendingRaw(item, accounts) {
    if (!item || item.op !== "create" || (item.state !== "pending" && item.state !== "sending" && item.state !== "failed")) return null
    var email = ""
    for (var i = 0; i < (accounts || []).length; i++) if (accounts[i].id === item.account_id) email = accounts[i].email
    return {
        id: "pending-" + item.id, subject: item.subject || "(no title)", body: "",
        start: item.start, end: item.end, is_all_day: item.all_day === true,
        account_id: item.account_id, account_email: email,
        attendees: [], location: item.location || "",
        pending: item.state !== "failed", failed: item.state === "failed", error: item.error || ""
    }
}

function pendingMark(raw) { return raw && raw.failed ? "\u2717 " : raw && raw.pending ? "\u23f2 " : "" }

// ---- phase 3: attendees, edits, cancel / decline / delete (docs/PLAN-calendar-events.md 5.5, 6, 9, 12) ----

function looksLikeAddress(s) { return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(String(s || "").trim()) }

// The attendee rows as the request carries them: the Required and Optional chips ({email, name}) plus the rooms
// kept from an edit ({email, name, role: "resource"}). The account's own address is dropped (the organiser is
// implicit) and reported in `own`; an address listed twice keeps its first row. -> { list: [{name, email, role}],
// own: bool, bad: [text that is not an address] }
function attendeeList(required, optional, kept, ownEmail) {
    var seen = {}, list = [], bad = [], own = false
    function add(a, role) {
        var em = String(a.email || "").trim()
        if (em === "") return
        var k = em.toLowerCase()
        if (ownEmail && k === String(ownEmail).toLowerCase()) { own = true; return }
        if (seen[k]) return
        seen[k] = true
        if (role !== "resource" && !looksLikeAddress(em)) { bad.push(em); return }
        list.push({ name: String(a.name || ""), email: em, role: role })
    }
    ;(required || []).forEach(function(a) { add(a, "required") })
    ;(optional || []).forEach(function(a) { add(a, "optional") })
    ;(kept || []).forEach(function(a) { add(a, "resource") })
    return { list: list, own: own, bad: bad }
}

// People who reach the event (rooms do not count as invitees; the backend counts the same way).
function inviteeCount(list) { return (list || []).filter(function(a) { return a.role !== "resource" }).length }

// The footer line: who the save reaches and when (plan 12: the undo window is the safety net).
function reachLine(n, mode, scope, day) {
    var people = n + (n === 1 ? " person" : " people")
    if (mode === "edit" && scope === "following") return followingLine("update", n, day)
    if (mode === "edit") return n === 0 ? "No attendees: nobody is told about the change."
                                        : "Invitations with the update go out to " + people + " when the undo window ends."
    return n === 0 ? "Personal entry. No invitations." : people + " will be invited when the undo window ends."
}

// The day before an ISO date (YYYY-MM-DD), as YYYY-MM-DD.
function dayBefore(iso) {
    var d = new Date(String(iso || "").slice(0, 10) + "T12:00:00")
    d.setDate(d.getDate() - 1)
    return d.getFullYear() + "-" + String(d.getMonth() + 1).padStart(2, "0") + "-" + String(d.getDate()).padStart(2, "0")
}

// The day of an event as the cache holds it (its start, YYYY-MM-DD).
function dayOf(raw) { return String((raw && raw.start) || "").slice(0, 10) }

// The confirm / footer line for "this and following" (plan 5.7). day = the picked occurrence's date.
// n = invitees from the dry run (-1 while counting). The old series ends the day before; a new series starts on day.
function followingLine(op, n, day) {
    if (op === "delete") return "Removes this and the following occurrences from your calendar when the undo window ends. Nobody is told."
    if (n < 0) return "Counting who is invited\u2026"
    var people = n + (n === 1 ? " person" : " people")
    var reach = n === 0 ? "personal" : "sent to " + people + " when the undo window ends"
    return "The series ends " + dayBefore(day) + ". A new series from " + day + " is " + reach + "."
}

// Merge suggestion lists, first one wins; each entry becomes {name, email}. Entries without an address are dropped.
function mergePeople(first, second) {
    var seen = {}, out = [], all = (first || []).concat(second || [])
    for (var i = 0; i < all.length; i++) {
        var k = String(all[i].email || "").toLowerCase()
        if (k === "" || seen[k]) continue
        seen[k] = true
        out.push({ name: all[i].name || "", email: all[i].email })
    }
    return out
}

// The state an edit is compared on and sent from, in the request's own field names. f = { subject, notes, when
// (valid parseWhen result), location, reminder (minutes or null), showAs, attendees: [{name, email, role}] }.
// Attendees are sorted, so the same people in another order are not a change.
function stateOf(f) {
    var att = (f.attendees || []).map(function(a) { return { name: a.name || "", email: a.email, role: a.role || "required" } })
    att.sort(function(x, y) {
        var a = String(x.email).toLowerCase(), b = String(y.email).toLowerCase()
        return a < b ? -1 : a > b ? 1 : 0
    })
    return {
        subject: String(f.subject || "").trim(), body: String(f.notes || ""),
        start: stamp(f.when.start), end: stamp(f.when.end), all_day: f.when.allDay === true,
        location: String(f.location || "").trim(), show_as: f.showAs || "busy",
        reminder_minutes: f.reminder === null || f.reminder === undefined || f.reminder < 0 ? null : f.reminder,
        attendees: att, online_provider: f.online || ""
    }
}

var STATE_KEYS = ["subject", "body", "start", "end", "all_day", "location", "show_as", "reminder_minutes", "attendees", "online_provider"]

// Which fields differ between two stateOf results (empty array: nothing to send).
function changedKeys(before, after) {
    var out = []
    for (var i = 0; i < STATE_KEYS.length; i++) {
        var k = STATE_KEYS[i]
        if (JSON.stringify(before[k] === undefined ? null : before[k]) !== JSON.stringify(after[k] === undefined ? null : after[k])) out.push(k)
    }
    return out
}

// A recurring event (an occurrence or an exception of a series) gets the occurrence-or-series question first.
function isRecurring(raw) {
    return !!raw && (!!raw.series_master_id || raw.event_type === "occurrence" || raw.event_type === "exception")
}

// Mine: Graph says I organise it (isOrganizer, true for entries on my own calendar), or its organiser is my address.
function isMine(raw, account) {
    if (raw.is_organizer === true) return true
    var org = String(raw.organizer_email || "").trim().toLowerCase()
    var me = String((account && account.email) || "").trim().toLowerCase()
    return org !== "" && org === me
}

// Why `e` cannot edit this event, or "" when it can. account = the appAccounts entry for raw.account_id.
function editRefusal(raw, account) {
    if (!raw) return "No event selected"
    if (raw.pending === true) return "That event is not saved yet"
    if (!account) return "This calendar is not connected any more"
    if (account.provider !== "exchange" && account.provider !== "gmail" && account.provider !== "outlook" && account.provider !== "outlook") return "This calendar cannot be changed in OmaLook"
    if (!isMine(raw, account)) return "Only the organiser can change this meeting. x declines it."
    return ""
}

// What `x` does: delete a personal entry, cancel a meeting you organise, decline one you attend.
// -> { kind: "delete" | "cancel" | "decline" } or { error }
function removeKind(raw, account) {
    if (!raw) return { error: "No event selected" }
    if (raw.pending === true) return { error: "That event is not saved yet" }
    if (!account) return { error: "This calendar is not connected any more" }
    if (account.provider !== "exchange" && account.provider !== "gmail" && account.provider !== "outlook" && account.provider !== "outlook") return { error: "This calendar cannot be changed in OmaLook" }
    if (!isMine(raw, account)) {
        if ((raw.organizer_email || "") !== "") return { kind: "decline" }
        return { error: "Not your event: only its organiser can delete it" }
    }
    var invitees = (raw.attendees || []).filter(function(a) { return a.role !== "resource" && String(a.email || "").trim() !== "" }).length
    return { kind: invitees > 0 ? "cancel" : "delete" }
}

// Path for an edit (PATCH) or a cancel / decline / delete (POST), with dry_run=1 for the invitee count.
function changePath(op, id, scope, kind, dry) {
    var known = scope === "series" || scope === "following" ? scope : "occurrence"
    var q = "id=" + encodeURIComponent(id) + "&scope=" + known
    if (op === "update") return "/calendar/event?" + q + (dry ? "&dry_run=1" : "")
    return "/calendar/event/cancel?" + q + "&kind=" + kind + (dry ? "&dry_run=1" : "")
}

// The undo bar's line once the server has queued the change.
function changeMessage(op, n, series, following) {
    var whole = following ? " on this and following occurrences" : series ? " on the whole series" : ""
    if (op === "update") return "Change queued" + whole + (n > 0 ? " for " + n + (n === 1 ? " person" : " people") : " (personal)")
    if (op === "cancel") return "Cancellation queued for " + n + (n === 1 ? " person" : " people") + whole
    if (op === "decline") return "Decline queued (the organiser is told)" + whole
    return "Delete queued (personal)" + whole
}

// The confirm line for cancel / decline / delete. n = invitees from the dry run, -1 while it is counting.
function removeLine(op, n, following, day) {
    if (op === "decline") return "Your decline goes to the organiser only. Nothing else is sent."
    if (following) return followingLine(op, n, day)
    if (op === "delete") return "Removed from your calendar when the undo window ends. Nobody is told."
    if (n < 0) return "Counting who is invited\u2026"
    return "The cancellation goes to " + n + (n === 1 ? " person" : " people") + " when the undo window ends."
}

var CHANGE_OPS = { update: true, cancel: true, decline: true, delete: true }

// The pending mark on an event that has a queued change: \u23f2 an edit, \u2296 a removal, \u2717 refused by the server.
function changeMark(op, failed) { return failed ? "\u2717 " : op === "update" ? "\u23f2 " : "\u2296 " }

// Outbox changes (edit, cancel, decline, delete) drawn on the cached events until the server has them. An edit shows
// its new subject, times and place; a removal gets a mark. Returns a new array; the cache is not touched.
function applyChanges(rawEvents, items) {
    var live = (items || []).filter(function(it) {
        return CHANGE_OPS[it.op] === true && (it.state === "pending" || it.state === "sending" || it.state === "failed")
    })
    if (live.length === 0) return rawEvents || []
    return (rawEvents || []).map(function(r) {
        for (var i = 0; i < live.length; i++) {
            var it = live[i], t = it.target || ""
            if (t === "" || !(r.id === t || (it.series === true && r.series_master_id === t))) continue
            var out = Object.assign({}, r), failed = it.state === "failed"
            out.pendingOp = it.op; out.failed = failed; out.pending = !failed; out.error = it.error || ""
            // "this and following": the mark lands on the picked occurrence and the ones after it; no new times yet.
            if (it.following === true) {
                if (dayOf(r) < String(it.occurrence || "")) continue
                out.subject = changeMark("update", failed) + r.subject
            } else if (it.op === "update") {
                out.subject = changeMark("update", failed) + (it.subject || r.subject)
                out.start = it.start || r.start
                out.end = it.end || r.end
                out.is_all_day = it.all_day === true
                out.location = it.location || r.location || ""
            } else {
                out.subject = changeMark(it.op, failed) + r.subject
            }
            return out
        }
        return r
    })
}

// The edit's request body (PATCH). The backend takes the whole editor state, so every field is sent, the attendee list
// included: an empty list would remove everyone. f = stateOf inputs + { accountId, changed (changedKeys result),
// originalLocation }. -> { ok, body } | { ok: false, nothing: true } | { ok: false, error }
function buildPatch(f) {
    if (!f.accountId) return { ok: false, error: "no calendar account selected" }
    if (!f.when || !f.when.ok) return { ok: false, error: f.when && f.when.error ? f.when.error : "the time is not understood" }
    if (!f.changed || f.changed.length === 0) return { ok: false, nothing: true, error: "Nothing changed yet" }
    var body = stateOf(f)
    if (body.subject === "") return { ok: false, error: "a title is needed" }
    if (body.reminder_minutes === null) delete body.reminder_minutes
    if (body.location === "" && f.originalLocation) return { ok: false, error: "clearing the place is not supported yet: type a new one" }
    if (body.location === "") delete body.location
    body.account_id = f.accountId
    if (body.online_provider === "") delete body.online_provider   // absent: the backend keeps the event's meeting as it is
    return { ok: true, body: body }
}

// The Online meeting picker's rows from GET /calendar/meeting-options: [{id, label, kind, available, reason, default}] ->
// [{id, label, enabled, note}]. A provider for another account kind is left out; Zoom / Webex stay but disabled
// ("needs setup"); on an edit a native provider (Teams, Meet) is disabled: Graph and Google only add it when the event is created.
function onlineRows(options, editing) {
    var out = []
    ;(options || []).forEach(function(o) {
        var why = String(o.reason || "")
        if (o.available === false && why.indexOf("needs a ") === 0) return
        var enabled = o.available !== false, note = ""
        if (!enabled) note = o.kind === "external" ? "needs setup" : (why || "not available")
        else if (editing && o.kind === "native") { enabled = false; note = "new events only" }
        out.push({ id: o.id, label: o.label, enabled: enabled, note: note, isDefault: o.default === true })
    })
    return out
}

// The default choice for a new event: the row the backend marked, else none.
function defaultOnline(rows) { for (var i = 0; i < (rows || []).length; i++) if (rows[i].isDefault && rows[i].enabled) return rows[i].id; return "none" }

// Left / Right on the picker: the next enabled row.
function stepOnline(rows, current, dir) {
    var en = (rows || []).filter(function(r) { return r.enabled || r.id === current })
    if (en.length === 0) return current
    var i = 0
    for (var k = 0; k < en.length; k++) if (en[k].id === current) i = k
    for (var n = 1; n <= en.length; n++) {
        var c = en[(((i + dir * n) % en.length) + en.length) % en.length]
        if (c.enabled) return c.id
    }
    return current
}

// The footer sentence for the picked meeting ("" for none): says what the save does, since the link is made on the server.
function onlineLine(choice, rows, editing) {
    if (!choice || choice === "none") return ""
    var label = choice
    ;(rows || []).forEach(function(r) { if (r.id === choice) label = r.label })
    var native = choice === "teamsForBusiness" || choice === "googleMeet"
    if (native) return label + " meeting link is created when it is saved."
    return label + (editing ? " link replaces the one the app added." : " link is added to the place and notes.")
}
