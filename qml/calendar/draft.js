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
        var en = new Date(e.startDate.getFullYear(), e.startDate.getMonth(), e.startDate.getDate(), 0, e.endMin)
        if (s < end && en > start) out.push(e.title)
    }
    return out
}

// Only a Microsoft work/school (Graph) account can create events so far (plan: Google is track C).
function canCreate(account) { return !!account && account.signed_in !== false && account.provider === "exchange" }

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
