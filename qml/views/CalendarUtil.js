.pragma library

// Date + event helpers shared by the calendar views. All dates are local.
// Event timestamps are treated as wall-clock strings ("YYYY-MM-DDTHH:MM[:SS]")
// so no timezone shifting happens in the UI.

var MONTHS = ["January","February","March","April","May","June",
              "July","August","September","October","November","December"]
var DAYS_SHORT = ["Mon","Tue","Wed","Thu","Fri","Sat","Sun"]

function pad(n) { return n < 10 ? "0" + n : "" + n }

function startOfDay(d) { return new Date(d.getFullYear(), d.getMonth(), d.getDate()) }

function addDays(d, n) { return new Date(d.getFullYear(), d.getMonth(), d.getDate() + n) }

function addMonths(d, n) {
    var t = new Date(d.getFullYear(), d.getMonth() + n, 1)
    var last = new Date(t.getFullYear(), t.getMonth() + 1, 0).getDate()
    return new Date(t.getFullYear(), t.getMonth(), Math.min(d.getDate(), last))
}

function sameDay(a, b) {
    return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth()
        && a.getDate() === b.getDate()
}

// Monday of the week containing d.
function weekStart(d) {
    var wd = (d.getDay() + 6) % 7   // 0 = Mon
    return addDays(startOfDay(d), -wd)
}

// 0 = Mon … 6 = Sun
function isoIndex(d) { return (d.getDay() + 6) % 7 }

function isWeekend(d) { return isoIndex(d) >= 5 }

function key(d) { return d.getFullYear() + "-" + pad(d.getMonth() + 1) + "-" + pad(d.getDate()) }

// First of 42 cells (6 weeks) shown in the month grid for the month of d.
function monthGridStart(d) {
    return weekStart(new Date(d.getFullYear(), d.getMonth(), 1))
}

// "YYYY-MM" keys of every month touched by [from, to].
function monthsBetween(from, to) {
    var out = [], y = from.getFullYear(), m = from.getMonth()
    while (y < to.getFullYear() || (y === to.getFullYear() && m <= to.getMonth())) {
        out.push(y + "-" + pad(m + 1))
        if (++m > 11) { m = 0; y++ }
    }
    return out
}

function parseStamp(s) {
    if (!s || typeof s !== "string" || s.length < 10) return null
    var y = parseInt(s.substring(0, 4)), mo = parseInt(s.substring(5, 7)) - 1,
        d = parseInt(s.substring(8, 10))
    var hasTime = s.length >= 16
    var h = hasTime ? parseInt(s.substring(11, 13)) : 0
    var mi = hasTime ? parseInt(s.substring(14, 16)) : 0
    if (isNaN(y) || isNaN(mo) || isNaN(d)) return null
    return { date: new Date(y, mo, d), minutes: h * 60 + mi, hasTime: hasTime }
}

// Normalise raw backend events -> { title, location, startDate, endDate,
// startMin, endMin, allDay, raw }.  endDate is inclusive.
function normalise(events) {
    var out = []
    for (var i = 0; i < (events || []).length; i++) {
        var e = events[i], s = parseStamp(e.start)
        if (!s) continue
        var en = parseStamp(e.end)
        var allDay = e.allDay === true || e.is_all_day === true || !s.hasTime
                     || (s.minutes === 0 && en && en.minutes === 0 && en.date > s.date)
        var endDate = en ? en.date : s.date
        var endMin = en ? en.minutes : s.minutes + 60
        if (en && en.minutes === 0 && endDate > s.date) endDate = addDays(endDate, -1)  // exclusive midnight end
        if (endDate < s.date) endDate = s.date
        if (!allDay && endDate > s.date) { endDate = s.date; endMin = 24 * 60 }          // clip to start day
        if (!allDay && endMin <= s.minutes) endMin = Math.min(s.minutes + 30, 24 * 60)
        out.push({
            title: e.subject || e.summary || "Untitled",
            location: e.location || "",
            startDate: s.date, endDate: endDate,
            startMin: s.minutes, endMin: endMin,
            allDay: allDay, raw: e
        })
    }
    return out
}

function onDay(norm, day) {
    var t = startOfDay(day).getTime(), out = []
    for (var i = 0; i < norm.length; i++) {
        var e = norm[i]
        if (e.startDate.getTime() <= t && t <= e.endDate.getTime()) out.push(e)
    }
    out.sort(function(a, b) {
        if (a.allDay !== b.allDay) return a.allDay ? -1 : 1
        if (a.startMin !== b.startMin) return a.startMin - b.startMin
        return a.title < b.title ? -1 : (a.title > b.title ? 1 : 0)   // stable order for same-time events
    })
    return out
}

function timedOnDay(norm, day) {
    return onDay(norm, day).filter(function(e) { return !e.allDay })
}
function allDayOnDay(norm, day) {
    return onDay(norm, day).filter(function(e) { return e.allDay })
}

// Greedy column layout so overlapping timed events sit side by side.
// Returns the same objects annotated with .col and .cols.
function layoutOverlaps(list) {
    var items = list.slice().sort(function(a, b) { return a.startMin - b.startMin || b.endMin - a.endMin })
    var out = [], group = [], groupEnd = -1
    function flush() {
        var colEnds = []
        group.forEach(function(e) {
            var c = 0
            while (c < colEnds.length && colEnds[c] > e.startMin) c++
            colEnds[c] = e.endMin
            e.col = c
        })
        group.forEach(function(e) { e.cols = colEnds.length; out.push(e) })
        group = []
    }
    items.forEach(function(e) {
        if (group.length && e.startMin >= groupEnd) { flush(); groupEnd = -1 }
        group.push(e)
        groupEnd = Math.max(groupEnd, e.endMin)
    })
    if (group.length) flush()
    return out
}

function timeLabel(min) {
    var h = Math.floor(min / 60) % 24, m = min % 60
    return pad(h) + ":" + pad(m)
}

function hourLabel(h) { return pad(h) + ":00" }

// ── cursor, selection, accounts, join links (docs/PLAN-calendar-events.md 5.1, 5.2, 7.4) ──

var SLOT = 30   // minutes per cursor step in the day and week grids

// Move the cursor. `c` = { date, min }; mode = day | workweek | week | month.
// Month: h/l = day, j/k = week. Grids: h/l = day (work week skips the weekend), j/k = 30 minutes,
// stepping past midnight lands on the neighbouring day's first / last slot.
function moveCursor(c, mode, dx, dy) {
    var date = startOfDay(c.date), min = c.min
    if (dx !== 0) {
        date = addDays(date, dx)
        if (mode === "workweek") while (isWeekend(date)) date = addDays(date, dx)
    }
    if (dy !== 0) {
        if (mode === "month") date = addDays(date, 7 * dy)
        else {
            min += SLOT * dy
            if (min < 0)        { date = addDays(date, -1); min = 24 * 60 - SLOT }
            if (min >= 24 * 60) { date = addDays(date, 1);  min = 0 }
        }
    }
    return { date: date, min: min }
}

// Same, for `slots` 30-minute steps at once (half page); clamps inside the day instead of wrapping.
function moveSlots(c, slots) {
    return { date: startOfDay(c.date), min: Math.max(0, Math.min(24 * 60 - SLOT, c.min + slots * SLOT)) }
}

// Where a newly shown day should put the cursor when none was set: the current half hour if the
// day is today, else 09:00.
function defaultMin(date, now) {
    if (sameDay(date, now)) return Math.floor((now.getHours() * 60 + now.getMinutes()) / SLOT) * SLOT
    return 9 * 60
}

// Events under the cursor, in display order. Month view: everything on the day. Grids: all-day
// events plus the timed ones overlapping the cursor's 30-minute slot.
function eventsAtCursor(norm, date, min, mode) {
    var day = onDay(norm, date)
    if (mode === "month") return day
    return day.filter(function(e) { return e.allDay || (e.startMin < min + SLOT && e.endMin > min) })
}

// Distinct accounts in raw backend events, in first-seen order: [{ id, email }].
function accountsOf(raw) {
    var seen = {}, out = []
    for (var i = 0; i < (raw || []).length; i++) {
        var k = raw[i].account_email || raw[i].account_id || ""
        if (k === "" || seen[k]) continue
        seen[k] = true
        out.push({ id: raw[i].account_id || "", email: raw[i].account_email || k })
    }
    out.sort(function(a, b) { return a.email < b.email ? -1 : (a.email > b.email ? 1 : 0) })
    return out
}

// Account filter cycle: "" (all) -> account 1 -> ... -> last -> "".
function nextAccount(accounts, current) {
    if (!accounts.length) return ""
    if (current === "") return accounts[0].email
    for (var i = 0; i < accounts.length; i++)
        if (accounts[i].email === current) return i + 1 < accounts.length ? accounts[i + 1].email : ""
    return ""
}

function filterAccount(raw, email) {
    if (!email) return raw
    return (raw || []).filter(function(e) { return (e.account_email || e.account_id) === email })
}


// The https join link of a raw backend event ("" if none). Anything but https is refused.
function joinUrl(raw) {
    var u = raw && raw.join_url ? String(raw.join_url) : ""
    return u.indexOf("https://") === 0 ? u : ""
}

var JOIN_LABELS = { teams: "Teams", zoom: "Zoom", meet: "Google Meet", webex: "Webex", jitsi: "Jitsi", link: "Link" }
function joinLabel(raw) { return JOIN_LABELS[raw && raw.join_provider] || "Link" }

var RESPONSE_LABELS = { organizer: "Organizer", accepted: "Accepted", tentativelyAccepted: "Tentative", declined: "Declined",
                        notResponded: "Not responded", none: "Not responded" }
function responseLabel(r) { return RESPONSE_LABELS[r] || "" }

// Agenda rows for the calendar sidebar, from `now` on: a flat list of
// { kind: "head", text, date } and { kind: "ev", ev, date, time } (Today / Tomorrow / dated heads).
// Timed events that have already ended today are left out; multi-day events appear on their first day
// (or today, if they started earlier). At most `limit` events over the next `days` days.
function agenda(norm, now, days, limit) {
    var rows = [], count = 0, today = startOfDay(now), nowMin = now.getHours() * 60 + now.getMinutes()
    for (var i = 0; i < days && count < limit; i++) {
        var d = addDays(today, i)
        var list = onDay(norm, d).filter(function(e) {
            if (i === 0) return e.allDay || e.endMin > nowMin
            return sameDay(e.startDate, d)
        })
        if (list.length === 0) continue
        rows.push({ kind: "head", date: d,
            text: i === 0 ? "Today" : i === 1 ? "Tomorrow"
                : DAYS_SHORT[isoIndex(d)] + " " + d.getDate() + " " + MONTHS[d.getMonth()].substring(0, 3) })
        for (var j = 0; j < list.length && count < limit; j++, count++)
            rows.push({ kind: "ev", ev: list[j], date: d, time: list[j].allDay ? "All day" : timeLabel(list[j].startMin) })
    }
    return rows
}

// Months ("YYYY-MM") the sidebar needs besides the visible ones: the mini month's 6-week grid and
// this month plus next (for the agenda).
function sideMonths(anchor, now) {
    var g = monthGridStart(anchor)
    var a = monthsBetween(g, addDays(g, 41))
    var n = monthsBetween(startOfDay(now), addDays(startOfDay(now), 31))
    var out = a.slice()
    n.forEach(function(m) { if (out.indexOf(m) < 0) out.push(m) })
    return out
}
