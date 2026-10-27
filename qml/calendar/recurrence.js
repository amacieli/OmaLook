.pragma library

// Parser for the calendar event Repeat row (docs/PLAN-calendar-events.md, 5.7).
// Pure JS: no imports, no QML-only APIs. Local wall-clock Dates only.
//
//   parseRepeat(text, ctx)   ctx = {start: Date (event start), defaultSeriesMonths: 12}
//     -> {ok:true, pattern, range, sentence, next, explicitEnd}
//        pattern: {type: 'daily'|'weekly'|'absoluteMonthly'|'relativeMonthly', interval,
//                  daysOfWeek? (weekly, relativeMonthly; lower-case English names),
//                  dayOfMonth? (absoluteMonthly), index? (relativeMonthly: first|second|third|fourth|last)}
//        range:   {type:'endDate', endDate:'YYYY-MM-DD'} | {type:'noEnd'} | {type:'numbered', numberOfOccurrences}
//        next:    up to three Dates after the first occurrence (fewer if the series ends first)
//     -> {ok:true, pattern:null, range:null, sentence:'Does not repeat', next:[], explicitEnd:false}  for '' / 'none'
//     -> {ok:false, error}
//   listChoices(start)       the picker list: [{label, text}] computed from the start date
//
// The names mirror Microsoft Graph patternedRecurrence (src/calendar_write.rs converts them).
//
// Decisions where the plan is silent or the spec decides:
// - The start is the first occurrence. A pattern that would not begin on the start is an error,
//   never a silent move: weekly without the start's weekday, 'weekdays' from a weekend, a relative
//   monthly whose position is not the start's, a monthly day number that is not the start's day.
// - 'weekly' and 'every N weeks' without days use the start's weekday.
// - No explicit end: the series ends defaultSeriesMonths after the start (0 = no end). The sentence says so.
// - 'until D M [YYYY]' without a year is the next such date on or after the start. An end before the start is an error.
// - 'monthly' on the 31st skips months without a 31st (Graph and Outlook do the same). The
//   'next' dates show this.
// - 'yearly' and 'annually' are errors: the Rust model has no yearly pattern yet.
// - A range phrase with no pattern ('until 29 dec', 'never') is an error: the pattern is not known.
// - 'every N days' and 'every N weeks' are accepted in addition to the listed forms.

var DAY_NAMES = ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"] // index = getDay()
var DAY_SHORT = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
var MON_FIRST = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"]
var MONTHS_SHORT = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
var MONTH_WORDS = { jan: 0, january: 0, feb: 1, february: 1, mar: 2, march: 2, apr: 3, april: 3,
    may: 4, jun: 5, june: 5, jul: 6, july: 6, aug: 7, august: 7, sep: 8, sept: 8, september: 8,
    oct: 9, october: 9, nov: 10, november: 10, dec: 11, december: 11 }
// Values are Date.getDay() numbers (0 = Sunday).
var WEEKDAY_WORDS = { sun: 0, sunday: 0, mon: 1, monday: 1, tue: 2, tues: 2, tuesday: 2,
    wed: 3, wednesday: 3, thu: 4, thur: 4, thurs: 4, thursday: 4, fri: 5, friday: 5,
    sat: 6, saturday: 6 }
var ORDINAL_WORDS = { first: "first", "1st": "first", second: "second", "2nd": "second",
    third: "third", "3rd": "third", fourth: "fourth", "4th": "fourth", last: "last" }
var ORD_NUM = { first: 1, second: 2, third: 3, fourth: 4 }

function own(map, key) {
    return Object.prototype.hasOwnProperty.call(map, key) ? map[key] : undefined
}

// Thrown internally; parseRepeat turns it into {ok:false}.
function fail(msg) { return { recurError: msg } }

function pad2(n) { return n < 10 ? "0" + n : "" + n }

function startOfDay(d) { return new Date(d.getFullYear(), d.getMonth(), d.getDate()) }

function addDays(d, n) {
    return new Date(d.getFullYear(), d.getMonth(), d.getDate() + n, d.getHours(), d.getMinutes())
}

// Clamped: Jan 31 + 1 month = Feb 28 (or 29).
function addMonths(d, n) {
    var t = new Date(d.getFullYear(), d.getMonth() + n, 1)
    var last = daysInMonth(t.getFullYear(), t.getMonth())
    return new Date(t.getFullYear(), t.getMonth(), Math.min(d.getDate(), last))
}

function daysInMonth(y, m) { return new Date(y, m + 1, 0).getDate() }

function atTime(day, h, m) { return new Date(day.getFullYear(), day.getMonth(), day.getDate(), h, m) }

function ordinal(n) {
    var v = n % 100
    if (v >= 11 && v <= 13) return n + "th"
    var r = n % 10
    return n + (r === 1 ? "st" : r === 2 ? "nd" : r === 3 ? "rd" : "th")
}

// "29 Dec 2026"
function fmtDate(d) { return d.getDate() + " " + MONTHS_SHORT[d.getMonth()] + " " + d.getFullYear() }

function isoDay(d) { return d.getFullYear() + "-" + pad2(d.getMonth() + 1) + "-" + pad2(d.getDate()) }

function parseIso(s) {
    var m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s)
    return new Date(+m[1], +m[2] - 1, +m[3])
}

function checkedDate(y, m, d, label) {
    if (m < 0 || m > 11 || d < 1 || d > daysInMonth(y, m)) throw fail("'" + label + "' is not a real date")
    return new Date(y, m, d)
}

function normalise(text) {
    return String(text == null ? "" : text).toLowerCase().replace(/\s*,\s*/g, ",").replace(/\s+/g, " ").replace(/^ | $/g, "")
}

// Splits off the range phrase: "daily until 29 dec", "weekly for 10", "daily never".
function splitRange(t) {
    var m = /^(?:(.*?) )?until (.+)$/.exec(t)
    if (m) return { head: m[1] || "", kind: "until", value: m[2] }
    m = /^(?:(.*?) )?for (\d+)(?: (?:times|occurrences|x))?$/.exec(t)
    if (m) return { head: m[1] || "", kind: "count", value: m[2] }
    m = /^(?:(.*?) )?never$/.exec(t)
    if (m) return { head: m[1] || "", kind: "never" }
    return { head: t, kind: "none" }
}

// "29 dec", "dec 29", "29 dec 2027", "2027-12-29". No year: the next such date on or after startDay.
function parseEndDate(s, startDay) {
    var m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s)
    if (m) return checkedDate(+m[1], +m[2] - 1, +m[3], s)
    var day, word, year = null
    m = /^(\d{1,2}) ?([a-z]+)(?: (\d{4}))?$/.exec(s)
    if (m) { day = +m[1]; word = m[2]; year = m[3] ? +m[3] : null }
    else {
        m = /^([a-z]+) (\d{1,2})(?: (\d{4}))?$/.exec(s)
        if (!m) throw fail("'until' needs a date such as 29 dec or 29 dec 2027")
        word = m[1]; day = +m[2]; year = m[3] ? +m[3] : null
    }
    var month = own(MONTH_WORDS, word)
    if (month === undefined) throw fail("'" + word + "' is not a month")
    if (day < 1 || day > 31) throw fail("'" + day + "' is not a day of the month")
    var label = day + " " + MONTHS_SHORT[month] + (year === null ? "" : " " + year)
    if (year !== null) return checkedDate(year, month, day, label)
    for (var y = startDay.getFullYear(); y <= startDay.getFullYear() + 8; y++) {
        if (day <= daysInMonth(y, month)) {
            var c = new Date(y, month, day)
            if (c >= startDay) return c
        }
    }
    throw fail("there is no " + label + " on or after the start")
}

function parseIntervalN(s) {
    var n = parseInt(s, 10)
    if (n < 1 || n > 99) throw fail("'" + s + "' is not a usable interval")
    return n
}

// "tue,thu" -> ["tuesday", "thursday"], sorted Monday first, duplicates removed.
function parseDayList(s) {
    var toks = s.split(/[\s,]+/).filter(function (x) { return x.length > 0 })
    if (toks.length === 0) throw fail("list the days, e.g. tue,thu")
    var names = []
    for (var i = 0; i < toks.length; i++) {
        var v = own(WEEKDAY_WORDS, toks[i])
        if (v === undefined) throw fail("'" + toks[i] + "' is not a day name")
        var name = DAY_NAMES[v]
        if (names.indexOf(name) < 0) names.push(name)
    }
    return sortMonFirst(names)
}

function sortMonFirst(names) {
    return names.slice().sort(function (a, b) { return MON_FIRST.indexOf(a) - MON_FIRST.indexOf(b) })
}

function shortOf(name) { return DAY_SHORT[DAY_NAMES.indexOf(name)] }

function weeklyPattern(interval, days, start) {
    var wd = start.getDay()
    var startName = DAY_NAMES[wd]
    var names = sortMonFirst(days)
    if (names.indexOf(startName) < 0) {
        var all = sortMonFirst(names.concat([startName])).map(shortOf).join(",").toLowerCase()
        throw fail("this event starts on " + fmtDate(start) + " (" + DAY_SHORT[wd] + "), so the days must include "
            + DAY_SHORT[wd] + ". Try 'weekly on " + all + "'")
    }
    return { type: "weekly", interval: interval, daysOfWeek: names }
}

// Describes the start's position in its month: "the fourth Tuesday" or "the last Tuesday",
// and both when the date is the nth and the last such weekday (e.g. 27 Oct 2026).
function describeNth(start) {
    var dom = start.getDate()
    var dim = daysInMonth(start.getFullYear(), start.getMonth())
    var dayFull = DAY_NAMES[start.getDay()]
    var words = ["first", "second", "third", "fourth", "fifth"]
    var nth = "the " + words[Math.floor((dom - 1) / 7)] + " " + dayFull
    if (dom + 7 > dim) return dom <= 28 ? nth + " and the last " + dayFull : "the last " + dayFull
    return nth
}

function parsePattern(h, start) {
    var wd = start.getDay()
    var dom = start.getDate()
    var m

    if (h === "daily" || h === "every day") return { type: "daily", interval: 1 }
    if (h === "weekdays" || h === "every weekday") {
        if (wd === 0 || wd === 6) {
            throw fail("weekdays needs a Mon-Fri start, but this event starts on " + fmtDate(start)
                + ". Move the start or pick another pattern")
        }
        return weeklyPattern(1, MON_FIRST.slice(0, 5), start)
    }
    if (h === "weekly" || h === "every week") return weeklyPattern(1, [DAY_NAMES[wd]], start)
    if ((m = /^weekly on (.+)$/.exec(h))) return weeklyPattern(1, parseDayList(m[1]), start)
    if ((m = /^every (\d+) weeks?(?: on (.+))?$/.exec(h))) {
        var days = m[2] ? parseDayList(m[2]) : [DAY_NAMES[wd]]
        return weeklyPattern(parseIntervalN(m[1]), days, start)
    }
    if ((m = /^every (\d+) days?$/.exec(h))) return { type: "daily", interval: parseIntervalN(m[1]) }

    if (h === "monthly") return { type: "absoluteMonthly", interval: 1, dayOfMonth: dom }
    if ((m = /^monthly(?: on the)? (\d{1,2})(?:st|nd|rd|th)?$/.exec(h))) {
        var n = parseInt(m[1], 10)
        if (n < 1 || n > 31) throw fail("'" + n + "' is not a day of the month")
        if (n !== dom) {
            throw fail("this event starts on the " + ordinal(dom) + ", so a monthly series on the " + ordinal(n)
                + " would not begin on its first occurrence. Use 'monthly' or change the start date")
        }
        return { type: "absoluteMonthly", interval: 1, dayOfMonth: n }
    }
    if ((m = /^monthly(?: on the)? (\S+) (\S+)$/.exec(h))) {
        var ord = own(ORDINAL_WORDS, m[1])
        if (ord === undefined) {
            if (m[1] === "5th") throw fail("there is no fifth weekday option; use 'last' for the final one")
            throw fail("'" + m[1] + "' is not a week position (first, second, third, fourth or last)")
        }
        var dayWd = own(WEEKDAY_WORDS, m[2])
        if (dayWd === undefined) throw fail("'" + m[2] + "' is not a weekday")
        var matches = dayWd === wd && (ord === "last"
            ? dom + 7 > daysInMonth(start.getFullYear(), start.getMonth())
            : Math.floor((dom - 1) / 7) + 1 === ORD_NUM[ord])
        if (!matches) {
            throw fail("this event starts on " + fmtDate(start) + ", which is " + describeNth(start)
                + ", so 'monthly " + m[1] + " " + m[2] + "' would not begin on its first occurrence. "
                + "Use the start's position or change the start date")
        }
        return { type: "relativeMonthly", interval: 1, index: ord, daysOfWeek: [DAY_NAMES[dayWd]] }
    }

    if (h === "yearly" || h === "annually" || h === "every year") throw fail("yearly is not supported yet")
    throw fail("didn't understand '" + h + "'. Try: daily, weekdays, weekly, weekly on tue,thu, "
        + "every 2 weeks on tue, monthly, monthly 2nd tue, monthly last fri")
}

function patternText(p) {
    if (p.type === "daily") return p.interval === 1 ? "Daily" : "Every " + p.interval + " days"
    if (p.type === "weekly") {
        if (p.interval === 1 && p.daysOfWeek.join(",") === MON_FIRST.slice(0, 5).join(",")) return "Weekdays"
        var names = p.daysOfWeek.map(shortOf).join(", ")
        return p.interval === 1 ? "Weekly on " + names : "Every " + p.interval + " weeks on " + names
    }
    if (p.type === "absoluteMonthly") return "Monthly on the " + ordinal(p.dayOfMonth)
    var pos = p.index === "last" ? "last" : ordinal(ORD_NUM[p.index])
    return "Monthly on the " + pos + " " + shortOf(p.daysOfWeek[0])
}

function rangeText(range, explicit, defMonths) {
    if (range.type === "endDate") {
        var text = " until " + fmtDate(parseIso(range.endDate))
        if (!explicit) text += " (default: " + defMonths + (defMonths === 1 ? " month)" : " months)")
        return text
    }
    if (range.type === "numbered") {
        var n = range.numberOfOccurrences
        return ", " + n + (n === 1 ? " occurrence" : " occurrences")
    }
    return ", no end date"
}

// Dates for period k (k = 0 is the period of the start), ascending. Dates may fall before the start.
function periodDates(p, start, k) {
    var h = start.getHours()
    var mi = start.getMinutes()
    var sd = startOfDay(start)
    var out = []
    var y, mo, mm, day
    if (p.type === "daily") return [addDays(start, k * p.interval)]
    if (p.type === "weekly") {
        var monday = addDays(sd, -((start.getDay() + 6) % 7))
        for (var i = 0; i < p.daysOfWeek.length; i++) {
            var off = MON_FIRST.indexOf(p.daysOfWeek[i])
            out.push(atTime(addDays(monday, k * p.interval * 7 + off), h, mi))
        }
        return out
    }
    mm = start.getMonth() + k * p.interval
    y = start.getFullYear() + Math.floor(mm / 12)
    mo = mm - 12 * Math.floor(mm / 12)
    if (p.type === "absoluteMonthly") {
        if (p.dayOfMonth > daysInMonth(y, mo)) return []   // no such day this month: skipped
        return [atTime(new Date(y, mo, p.dayOfMonth), h, mi)]
    }
    // relativeMonthly
    var wdIdx = DAY_NAMES.indexOf(p.daysOfWeek[0])
    if (p.index === "last") {
        day = daysInMonth(y, mo)
        while (new Date(y, mo, day).getDay() !== wdIdx) day--
    } else {
        var firstWd = new Date(y, mo, 1).getDay()
        day = 1 + ((wdIdx - firstWd + 7) % 7) + 7 * (ORD_NUM[p.index] - 1)
    }
    return [atTime(new Date(y, mo, day), h, mi)]
}

// Up to `howMany` occurrences after the start, honouring the end date or count.
function nextOccurrences(p, start, range, howMany) {
    var out = []
    var endDay = range.type === "endDate" ? parseIso(range.endDate) : null
    var limit = range.type === "numbered" ? range.numberOfOccurrences : Infinity
    var count = 1          // the start itself is occurrence 1
    var k = 0
    var guard = 0
    var stop = false
    while (!stop && out.length < howMany && count < limit && guard < 5000) {
        var cands = periodDates(p, start, k)
        for (var i = 0; i < cands.length; i++) {
            var c = cands[i]
            if (c.getTime() <= start.getTime()) continue
            if (endDay && startOfDay(c) > endDay) { stop = true; break }
            count++
            out.push(c)
            if (out.length >= howMany || count >= limit) { stop = true; break }
        }
        k++
        guard++
    }
    return out
}

function repeatOrThrow(text, ctx) {
    ctx = ctx || {}
    if (!ctx.start) throw fail("there is no event start to repeat from")
    var start = ctx.start
    var startDay = startOfDay(start)
    var defMonths = typeof ctx.defaultSeriesMonths === "number" ? ctx.defaultSeriesMonths : 12

    var t = normalise(text)
    if (t === "" || t === "none") {
        return { ok: true, pattern: null, range: null, sentence: "Does not repeat", next: [], explicitEnd: false }
    }

    var sp = splitRange(t)
    if (sp.head === "") throw fail("say what repeats first, e.g. 'daily until 29 dec' or 'weekly'")
    var pattern = parsePattern(sp.head, start)

    var range
    var explicit = true
    if (sp.kind === "until") {
        var end = parseEndDate(sp.value, startDay)
        if (end < startDay) throw fail("the series would end on " + fmtDate(end) + ", before it starts")
        range = { type: "endDate", endDate: isoDay(end) }
    } else if (sp.kind === "count") {
        var n = parseInt(sp.value, 10)
        if (n < 1) throw fail("'for 0' is not a count; use at least 1")
        range = { type: "numbered", numberOfOccurrences: n }
    } else if (sp.kind === "never") {
        range = { type: "noEnd" }
    } else {
        // No end typed: a starting value only. The sentence says so.
        explicit = false
        range = defMonths > 0
            ? { type: "endDate", endDate: isoDay(addMonths(startDay, defMonths)) }
            : { type: "noEnd" }
    }

    return {
        ok: true,
        pattern: pattern,
        range: range,
        sentence: patternText(pattern) + rangeText(range, explicit, defMonths),
        next: nextOccurrences(pattern, start, range, 3),
        explicitEnd: explicit
    }
}

function parseRepeat(text, ctx) {
    try {
        return repeatOrThrow(text, ctx)
    } catch (e) {
        if (e && e.recurError !== undefined) return { ok: false, error: e.recurError }
        throw e
    }
}

// The picker list, from the start date. 'Weekdays' only when the start is Mon-Fri.
// The nth-weekday entry is offered only for the 1st to 4th occurrence (there is no 5th option);
// the 'last' entry is offered when the start is in the last 7 days of the month.
function listChoices(start) {
    var wd = start.getDay()
    var dom = start.getDate()
    var dim = daysInMonth(start.getFullYear(), start.getMonth())
    var short = DAY_SHORT[wd]
    var lower = short.toLowerCase()
    var list = [{ label: "None", text: "" }, { label: "Daily", text: "daily" }]
    if (wd >= 1 && wd <= 5) list.push({ label: "Weekdays", text: "weekdays" })
    list.push({ label: "Weekly on " + short, text: "weekly on " + lower })
    list.push({ label: "Every 2 weeks on " + short, text: "every 2 weeks on " + lower })
    list.push({ label: "Monthly on the " + ordinal(dom), text: "monthly" })
    var nth = Math.floor((dom - 1) / 7) + 1
    if (nth <= 4) list.push({ label: "Monthly on the " + ordinal(nth) + " " + short, text: "monthly " + ordinal(nth) + " " + lower })
    if (dom + 7 > dim) list.push({ label: "Monthly on the last " + short, text: "monthly last " + lower })
    return list
}
