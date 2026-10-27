.pragma library

// Parser for the calendar event "When" field (docs/PLAN-calendar-events.md, 5.4).
// Pure JS: no imports, no QML-only APIs. All Dates are local wall-clock.
//
//   parseWhen(text, ctx)
//     ctx.now            Date. "today" is its local day.
//     ctx.defaultMinutes length when nothing is typed and there is no base (default 30)
//     ctx.base           {start, end} current value, or null (for relative edits)
//     -> {ok:true, start, end, allDay, minutes, summary}
//      | {ok:false, understood, error}
//
//   nudge(value, kind, dir)          kind: 'start' | 'day' | 'length'; dir: +1 / -1
//   formatSummary(start, end, allDay, refNow)
//
// Decisions where the plan is silent:
// - Time only ('3pm', '10:30'): the date is the base's date if ctx.base is given, else today.
// - Date only ('tomorrow', 'mon'): the time is the base's time of day if ctx.base is given, else 09:00.
// - Length: a typed range or duration wins. Otherwise, with ctx.base, the base's length is kept
//   (unless the base is a day or longer, e.g. an all-day value). Otherwise ctx.defaultMinutes.
// - A weekday alone is the next such weekday on or after today ('mon' on a Monday = today).
//   'next <weekday>' is the first such weekday strictly after today ('next tue' on a Tuesday = +7 days).
// - Day + month with no year is the next such date on or after today. A year may be typed.
// - Bare numbers never become times: '5' and '17' are errors. In a range a bare start is accepted only
//   as 'H-H:MM' with the same hour ('10-10:30' = 10:00-10:30). Everything else needs am/pm or minutes.
// - 'all day' is the whole day; the end is exclusive (00:00 of the next day). Times or a length are an error with it.
// - Ambiguous or contradictory input is always an error, never a guess.

var DAYS_SHORT = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
var MONTHS_SHORT = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
var MONTH_WORDS = { jan: 0, january: 0, feb: 1, february: 1, mar: 2, march: 2, apr: 3, april: 3,
    may: 4, jun: 5, june: 5, jul: 6, july: 6, aug: 7, august: 7, sep: 8, sept: 8, september: 8,
    oct: 9, october: 9, nov: 10, november: 10, dec: 11, december: 11 }
// Values are Date.getDay() numbers (0 = Sunday).
var WEEKDAY_WORDS = { sun: 0, sunday: 0, mon: 1, monday: 1, tue: 2, tues: 2, tuesday: 2,
    wed: 3, wednesday: 3, thu: 4, thur: 4, thurs: 4, thursday: 4, fri: 5, friday: 5,
    sat: 6, saturday: 6 }
var NOISE_WORDS = { at: 1, on: 1, for: 1, from: 1, the: 1, this: 1 }

function own(map, key) {
    return Object.prototype.hasOwnProperty.call(map, key) ? map[key] : undefined
}

// Thrown internally; parseWhen turns it into {ok:false}.
function fail(msg) { return { whenError: msg } }

function pad2(n) { return n < 10 ? "0" + n : "" + n }

function startOfDay(d) { return new Date(d.getFullYear(), d.getMonth(), d.getDate()) }

function addDays(d, n) {
    return new Date(d.getFullYear(), d.getMonth(), d.getDate() + n, d.getHours(), d.getMinutes())
}

function plusMinutes(d, n) { return new Date(d.getTime() + n * 60000) }

function atTime(day, h, m) { return new Date(day.getFullYear(), day.getMonth(), day.getDate(), h, m) }

function daysInMonth(y, m) { return new Date(y, m + 1, 0).getDate() }

function sameDay(a, b) {
    return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate()
}

function minutesBetween(a, b) { return Math.round((b.getTime() - a.getTime()) / 60000) }

function hhmm(d) { return pad2(d.getHours()) + ":" + pad2(d.getMinutes()) }

// "Tue 27 Oct"; the year is appended when it differs from refNow's year.
function fmtDay(d, refNow) {
    var s = DAYS_SHORT[(d.getDay() + 6) % 7] + " " + d.getDate() + " " + MONTHS_SHORT[d.getMonth()]
    if (refNow && d.getFullYear() !== refNow.getFullYear()) s += " " + d.getFullYear()
    return s
}

// 90 -> "1h30m", 60 -> "1h", 30 -> "30m"
function formatMinutes(mn) {
    var h = Math.floor(mn / 60)
    var m = mn % 60
    if (h === 0) return m + "m"
    if (m === 0) return h + "h"
    return h + "h" + m + "m"
}

function checkedDate(y, m, d, label) {
    if (m < 0 || m > 11 || d < 1 || d > daysInMonth(y, m)) {
        throw fail("'" + label + "' is not a real date")
    }
    return new Date(y, m, d)
}

// Splits "10", "10:30", "3pm", "3:30pm", "12am". Returns null if not clock-like,
// {bare:true, h} for a bare number (no minutes, no am/pm), {error} or {bare:false, h, m, h24}.
function parseClock(t) {
    var mt = /^(\d{1,2})(?::(\d{2}))?(am|pm)?$/.exec(t)
    if (!mt) return null
    var h = parseInt(mt[1], 10)
    var hasMin = mt[2] !== undefined
    var mer = mt[3]
    var min = hasMin ? parseInt(mt[2], 10) : 0
    if (!hasMin && !mer) return { bare: true, h: h }
    if (min > 59) return { error: "'" + t + "' has minutes over 59" }
    if (mer) {
        if (h < 1 || h > 12) return { error: "'" + t + "' is not a 12-hour time (1 to 12 with am or pm)" }
        return { bare: false, h: (h % 12) + (mer === "pm" ? 12 : 0), m: min, h24: false }
    }
    if (h > 23) return { error: "'" + t + "' is not a 24-hour time" }
    return { bare: false, h: h, m: min, h24: true }
}

// "1h30m" -> 90, "45m" -> 45, "1h" -> 60; null if not a duration.
function parseDuration(t) {
    var m = /^(?:(\d+)h)?(?:(\d+)m)?$/.exec(t)
    if (!m || (m[1] === undefined && m[2] === undefined)) return null
    return (parseInt(m[1] || "0", 10)) * 60 + parseInt(m[2] || "0", 10)
}

// "+30m", "-1h", "+1d", "+1d2h": {days, minutes}; null if not a shift.
function parseShift(t) {
    var m = /^([+-])(?:(\d+)d)?(?:(\d+)h)?(?:(\d+)m)?$/.exec(t)
    if (!m || (m[2] === undefined && m[3] === undefined && m[4] === undefined)) return null
    var sign = m[1] === "-" ? -1 : 1
    return {
        days: sign * parseInt(m[2] || "0", 10),
        minutes: sign * (parseInt(m[3] || "0", 10) * 60 + parseInt(m[4] || "0", 10))
    }
}

function parseRange(t) {
    var parts = t.split("-")
    if (parts.length !== 2) throw fail("'" + t + "' is not a range; write e.g. 10-10:30 or 2pm-3:30pm")
    var a = parseClock(parts[0])
    var b = parseClock(parts[1])
    if (!a || !b) throw fail("a range needs two times, e.g. 10-10:30 or 2pm-3:30pm")
    if (a.error) throw fail(a.error)
    if (b.error) throw fail(b.error)
    if (b.bare) throw fail("'" + parts[1] + "' needs minutes or am/pm, e.g. " + parts[1] + ":00 or " + parts[1] + "am")
    if (a.bare) {
        // Only the 'H-H:MM' form is unambiguous: the start takes the end's hour.
        if (b.h24 && a.h === b.h) {
            a = { bare: false, h: a.h, m: 0, h24: true }
        } else {
            throw fail("'" + parts[0] + "' is ambiguous in this range; write it with minutes or am/pm, e.g. " + parts[0] + ":00 or " + parts[0] + "am")
        }
    }
    return { a: a, b: b }
}

// Merges tokens that belong together: "3 pm" -> "3pm", "10 to 10:30" -> "10-10:30".
function tokenize(text) {
    var s = String(text == null ? "" : text).toLowerCase()
    s = s.replace(/[\u2013\u2014]/g, "-").replace(/->|\u2192/g, "-").replace(/,/g, " ")
    s = s.replace(/\s*-\s*/g, "-").replace(/\s+/g, " ").replace(/^ | $/g, "")
    var raw = s === "" ? [] : s.split(" ")
    var out = []
    for (var i = 0; i < raw.length; i++) {
        var t = raw[i]
        if (/^\d{1,2}(:\d{2})?$/.test(t) && (raw[i + 1] === "am" || raw[i + 1] === "pm")) {
            out.push(t + raw[i + 1])
            i++
            continue
        }
        if (/\d/.test(t) && raw[i + 1] === "to" && i + 2 < raw.length && /^\d/.test(raw[i + 2])) {
            out.push(t + "-" + raw[i + 2])
            i += 2
            continue
        }
        out.push(t)
    }
    return out
}

function setDate(st, obj) {
    if (st.date) throw fail("more than one date ('" + st.date.label + "' and '" + obj.label + "'); pick one")
    st.date = obj
}

function setTime(st, c, label) {
    if (st.range || st.time) throw fail("more than one time ('" + label + "'); use a range such as 10-10:30 for a span")
    st.time = c
}

// A day number followed by a month (or the reverse) and an optional year.
// `last` is the index of the month/day token just consumed.
function takeDayMonth(st, day, month, toks, last) {
    if (day < 1 || day > 31) throw fail("'" + day + "' is not a day of the month")
    var year = null
    if (/^\d{4}$/.test(toks[last + 1] || "")) {
        year = parseInt(toks[last + 1], 10)
        last++
    }
    var label = day + " " + MONTHS_SHORT[month] + (year === null ? "" : " " + year)
    setDate(st, { kind: "md", label: label, d: day, m: month, y: year })
    return last
}

// Consumes toks[i] (and possibly more); returns the index of the last token consumed.
function takeToken(toks, i, st) {
    var t = toks[i]
    var next = toks[i + 1]

    if (own(NOISE_WORDS, t) !== undefined) return i

    if ((t === "all" && next === "day") || t === "allday") {
        if (st.allDay) throw fail("'all day' given twice")
        st.allDay = true
        return t === "all" ? i + 1 : i
    }

    if (t === "next") {
        var nwd = own(WEEKDAY_WORDS, next || "")
        if (nwd === undefined) throw fail("'next' must come before a weekday, e.g. next tue")
        setDate(st, { kind: "wd", label: "next " + next, wd: nwd, next: true })
        return i + 1
    }

    if (t === "today" || t === "tomorrow") {
        setDate(st, { kind: t, label: t })
        return i
    }

    var wd = own(WEEKDAY_WORDS, t)
    if (wd !== undefined) {
        setDate(st, { kind: "wd", label: t, wd: wd, next: false })
        return i
    }

    var ymd = /^(\d{4})-(\d{2})-(\d{2})$/.exec(t)
    if (ymd) {
        setDate(st, { kind: "fixed", label: t, date: checkedDate(+ymd[1], +ymd[2] - 1, +ymd[3], t) })
        return i
    }

    if (own(MONTH_WORDS, t) !== undefined && /^\d{1,2}$/.test(next || "")) {
        return takeDayMonth(st, parseInt(next, 10), own(MONTH_WORDS, t), toks, i + 1)
    }
    if (/^\d{1,2}$/.test(t) && own(MONTH_WORDS, next || "") !== undefined) {
        return takeDayMonth(st, parseInt(t, 10), own(MONTH_WORDS, next), toks, i + 1)
    }
    if (own(MONTH_WORDS, t) !== undefined) {
        throw fail("'" + t + "' needs a day number, e.g. 27 " + t)
    }

    if (/^\d{4}$/.test(t)) {
        throw fail("'" + t + "' is a year; it must follow a day and month, e.g. 27 oct " + t)
    }

    if (/^[+-]/.test(t)) {
        var sh = parseShift(t)
        if (sh === null) throw fail("'" + t + "' is not a shift; use e.g. +30m, -1h or +1d")
        if (st.shift) throw fail("more than one shift")
        st.shift = sh
        return i
    }

    var du = parseDuration(t)
    if (du !== null) {
        if (st.dur !== null) throw fail("length given twice")
        st.dur = du
        return i
    }

    if (t.indexOf("-") > 0) {
        if (st.range || st.time) throw fail("more than one time ('" + t + "'); a range is one span")
        st.range = parseRange(t)
        return i
    }

    var c = parseClock(t)
    if (c !== null) {
        if (c.error) throw fail(c.error)
        if (c.bare) throw fail("'" + t + "' needs am/pm or minutes, e.g. " + t + "pm or 17:00")
        setTime(st, c, t)
        return i
    }

    throw fail("'" + t + "' is not understood")
}

// Date resolution. Throws fail() for impossible dates.
function resolveDate(d, now) {
    var today = startOfDay(now)
    if (d.kind === "today") return today
    if (d.kind === "tomorrow") return addDays(today, 1)
    if (d.kind === "fixed") return d.date
    if (d.kind === "wd") {
        var diff = (d.wd - today.getDay() + 7) % 7
        if (d.next && diff === 0) diff = 7
        return addDays(today, diff)
    }
    // md
    if (d.y !== null) return checkedDate(d.y, d.m, d.d, d.label)
    for (var y = today.getFullYear(); y <= today.getFullYear() + 8; y++) {
        if (d.d <= daysInMonth(y, d.m)) {
            var c = new Date(y, d.m, d.d)
            if (c >= today) return c
        }
    }
    throw fail("there is no " + d.d + " " + MONTHS_SHORT[d.m] + " to choose")
}

function describeSoFar(st, now) {
    var parts = []
    if (st.allDay) parts.push("all day")
    if (st.date) {
        var label = st.date.label
        try { label = fmtDay(resolveDate(st.date, now), now) } catch (e) { /* keep the typed label */ }
        parts.push(label)
    }
    if (st.range) {
        parts.push(pad2(st.range.a.h) + ":" + pad2(st.range.a.m) + "-" + pad2(st.range.b.h) + ":" + pad2(st.range.b.m))
    } else if (st.time) {
        parts.push(pad2(st.time.h) + ":" + pad2(st.time.m))
    }
    if (st.dur !== null) parts.push("length " + formatMinutes(st.dur))
    if (st.shift) parts.push("shift " + formatMinutes(Math.abs(st.shift.days * 1440 + st.shift.minutes)))
    return parts.join(", ")
}

function result(start, end, allDay, now) {
    var minutes = allDay ? 1440 : minutesBetween(start, end)
    return {
        ok: true, start: start, end: end, allDay: allDay, minutes: minutes,
        summary: formatSummary(start, end, allDay, now)
    }
}

// A length kept from the base. A base of a day or longer (all-day) does not count.
function keptLength(base, defaultMinutes) {
    if (base) {
        var bm = minutesBetween(base.start, base.end)
        if (bm > 0 && bm < 1440) return bm
    }
    return defaultMinutes
}

function finish(st, now, base, defaultMinutes) {
    if (st.shift) {
        if (st.allDay || st.date || st.time || st.range || st.dur !== null) {
            throw fail("a shift such as +30m must be typed on its own")
        }
        if (!base) throw fail("there is no time to shift yet; type a time such as tomorrow 3pm first")
        var s2 = plusMinutes(addDays(base.start, st.shift.days), st.shift.minutes)
        var e2 = plusMinutes(addDays(base.end, st.shift.days), st.shift.minutes)
        return result(s2, e2, false, now)
    }

    if (st.allDay) {
        if (st.time || st.range || st.dur !== null) {
            throw fail("an all-day event has no times or length; drop 'all day' or the time")
        }
        var dayA = st.date ? resolveDate(st.date, now) : startOfDay(base ? base.start : now)
        var sA = startOfDay(dayA)
        return result(sA, addDays(sA, 1), true, now)
    }

    if (st.range && st.dur !== null) throw fail("a range already sets the length; drop the duration")
    if (st.dur === 0) throw fail("a length must be more than 0")

    var day = st.date ? resolveDate(st.date, now) : startOfDay(base ? base.start : now)
    var start
    var end
    var minutes
    if (st.range) {
        start = atTime(day, st.range.a.h, st.range.a.m)
        end = atTime(day, st.range.b.h, st.range.b.m)
        minutes = minutesBetween(start, end)
        if (minutes <= 0) throw fail("the end " + hhmm(end) + " is not after the start " + hhmm(start))
    } else {
        if (st.time) start = atTime(day, st.time.h, st.time.m)
        else if (base) start = atTime(day, base.start.getHours(), base.start.getMinutes())
        else start = atTime(day, 9, 0)
        minutes = st.dur !== null ? st.dur : keptLength(base, defaultMinutes)
        end = plusMinutes(start, minutes)
    }
    return result(start, end, false, now)
}

function parseWhen(text, ctx) {
    ctx = ctx || {}
    var now = ctx.now || new Date()
    var defaultMinutes = ctx.defaultMinutes > 0 ? ctx.defaultMinutes : 30
    var base = (ctx.base && ctx.base.start && ctx.base.end) ? ctx.base : null
    var st = { allDay: false, date: null, time: null, range: null, dur: null, shift: null }
    try {
        var toks = tokenize(text)
        if (toks.length === 0) throw fail("type a time, e.g. tomorrow 3pm or fri 10-10:30")
        for (var i = 0; i < toks.length; i++) i = takeToken(toks, i, st)
        return finish(st, now, base, defaultMinutes)
    } catch (e) {
        if (e && e.whenError !== undefined) {
            return { ok: false, understood: describeSoFar(st, now), error: e.whenError }
        }
        throw e
    }
}

// Up/Down: start +/-15 min (length kept). Shift: +/-1 day. Alt: length +/-15 min, minimum 15.
// All-day values only move by day; start and length nudges leave them unchanged.
function nudge(value, kind, dir) {
    var allDay = !!value.allDay
    var sign = dir < 0 ? -1 : 1
    if (kind === "day") {
        return { start: addDays(value.start, sign), end: addDays(value.end, sign), allDay: allDay }
    }
    if (kind !== "start" && kind !== "length") throw new Error("nudge: unknown kind " + kind)
    if (allDay) {
        return { start: new Date(value.start.getTime()), end: new Date(value.end.getTime()), allDay: true }
    }
    if (kind === "start") {
        return { start: plusMinutes(value.start, 15 * sign), end: plusMinutes(value.end, 15 * sign), allDay: false }
    }
    var len = minutesBetween(value.start, value.end) + 15 * sign
    if (len < 15) len = 15
    return { start: new Date(value.start.getTime()), end: plusMinutes(value.start, len), allDay: false }
}

// 'Tue 27 Oct 10:00 → 10:30 (30m)'; all-day: 'Mon 2 Nov (all day)'.
// refNow is optional; when given, years other than its year are shown.
function formatSummary(start, end, allDay, refNow) {
    if (allDay) {
        var last = addDays(end, -1)
        var span = sameDay(start, last)
            ? fmtDay(start, refNow)
            : fmtDay(start, refNow) + " \u2192 " + fmtDay(last, refNow)
        return span + " (all day)"
    }
    var txt = fmtDay(start, refNow) + " " + hhmm(start) + " \u2192 "
    txt += sameDay(start, end) ? hhmm(end) : fmtDay(end, refNow) + " " + hhmm(end)
    return txt + " (" + formatMinutes(minutesBetween(start, end)) + ")"
}
