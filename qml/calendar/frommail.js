.pragma library

// Create-from-mail (Ctrl-E on a message in the mail view). Pure: turns one cached message into the
// prefill the calendar editor opens with. Nothing is sent or saved here; the editor's own Ctrl-Enter
// still queues the event behind the undo window.
//
// Caller passes the time parser (`when.js` parseWhen) in `o.parseWhen`, so this file has no imports
// and is testable in node (tests/frommail.test.js).

var MONTHS = "jan|feb|mar|apr|may|jun|jul|aug|sep|sept|oct|nov|dec|january|february|march|april|june|july|august|september|october|november|december"
var DAYS = "today|tonight|tomorrow|mon|tue|tues|wed|thu|thur|thurs|fri|sat|sun|monday|tuesday|wednesday|thursday|friday|saturday|sunday"
var DATEY = new RegExp("\\b(" + DAYS + ")\\b|\\b(" + MONTHS + ")\\b|\\d\\s*(am|pm)\\b|\\b\\d{1,2}:\\d{2}\\b", "i")

// "Re: Fwd: Lunch" -> "Lunch". Stops at the first non-prefix so "Re: Re: x" and "AW: x" both work.
function cleanSubject(s) {
    var t = String(s || "").trim()
    var prev
    do {
        prev = t
        t = t.replace(/^(re|fwd?|aw|sv|wg)\s*(\[\d+\])?\s*:\s*/i, "")
    } while (t !== prev)
    return t.trim()
}

// Address list ("Ann <ann@x.com>, bob@y.org; \"Smith, Jo\" <jo@z.net>") -> [{name, email}], no duplicates.
// Entries without an '@' are dropped. Email is lower-cased for comparison.
function parseAddresses(text) {
    var out = [], seen = {}
    var parts = [], cur = "", inQuote = false, inAngle = false
    var src = String(text || "")
    for (var i = 0; i < src.length; i++) {
        var c = src.charAt(i)
        if (c === "\"") inQuote = !inQuote
        else if (c === "<" && !inQuote) inAngle = true
        else if (c === ">" && !inQuote) inAngle = false
        if ((c === "," || c === ";") && !inQuote && !inAngle) { parts.push(cur); cur = ""; continue }
        cur += c
    }
    parts.push(cur)
    parts.forEach(function(p) {
        var s = p.trim()
        if (s === "") return
        var email = "", name = ""
        var m = s.match(/<([^<>]*)>/)
        if (m) {
            email = m[1].trim()
            name = s.slice(0, m.index).trim()
        } else {
            email = s
        }
        name = name.replace(/^"(.*)"$/, "$1").trim()
        email = email.replace(/^mailto:/i, "").trim()
        if (email.indexOf("@") < 1 || /\s/.test(email)) return
        var key = email.toLowerCase()
        if (seen[key]) return
        seen[key] = true
        out.push({ name: name, email: email })
    })
    return out
}

// Body text shortened to `max` characters on a word boundary, whitespace collapsed.
function excerpt(text, max) {
    var t = String(text || "").replace(/\s+/g, " ").trim()
    var limit = max > 0 ? max : 600
    if (t.length <= limit) return t
    var cut = t.slice(0, limit)
    var sp = cut.lastIndexOf(" ")
    if (sp > limit * 0.6) cut = cut.slice(0, sp)
    return cut.replace(/[\s.,;:-]+$/, "") + "…"
}

// Candidate phrases for the time parser: each clause (split at line ends and sentence/clause breaks) and
// every run of up to four words inside it, so "Can we do tomorrow 3pm" yields "tomorrow 3pm".
function candidates(text) {
    var out = [], seen = {}
    String(text || "").split(/[\n\r;!?]+|\.\s+|,\s*/).forEach(function(clause) {
        var words = clause.trim().split(/\s+/)
        var here = []
        for (var i = 0; i < words.length; i++) {
            for (var n = 1; n <= 4 && i + n <= words.length; n++) {
                var s = words.slice(i, i + n).join(" ").replace(/[.:]+$/, "")
                if (s.length < 2 || s.length > 60 || !DATEY.test(s) || seen[s]) continue
                seen[s] = true
                here.push(s)
            }
        }
        // Longest first: "tomorrow 3pm" before "tomorrow", which the parser would read as a default hour.
        here.sort(function(a, b) { return b.length - a.length })
        out = out.concat(here)
    })
    return out
}

// Next half hour strictly after `now` that does not overlap a busy span (up to two weeks on). Slots are
// 30 minutes apart, matching the calendar's default event length.
function nextFreeHour(now, busy, minutes) {
    var len = minutes > 0 ? minutes : 30
    var STEP = 30 * 60000
    var start = new Date(Math.floor(now.getTime() / STEP) * STEP + STEP)
    if (start.getTime() <= now.getTime()) start = new Date(start.getTime() + STEP)
    for (var n = 0; n < 48 * 14; n++) {
        var end = new Date(start.getTime() + len * 60000)
        var clash = (busy || []).some(function(b) { return b.start < end && b.end > start })
        if (!clash) break
        start = new Date(start.getTime() + STEP)
    }
    return { start: start, end: new Date(start.getTime() + len * 60000), source: "next free half hour" }
}

// The first phrase in `texts` that the time parser reads as a timed event after `now`, else the next free half hour.
function suggestTime(texts, o) {
    var now = o.now || new Date()
    var minutes = o.defaultMinutes > 0 ? o.defaultMinutes : 30
    if (typeof o.parseWhen === "function") {
        for (var t = 0; t < texts.length; t++) {
            var cs = candidates(texts[t])
            for (var i = 0; i < cs.length; i++) {
                var r = null
                try { r = o.parseWhen(cs[i], { now: now, defaultMinutes: minutes }) } catch (e) { r = null }
                if (r && r.ok === true && r.start && r.end && !r.allDay && r.start.getTime() > now.getTime()) {
                    return { start: r.start, end: r.end, source: "\"" + cs[i] + "\"" }
                }
            }
        }
    }
    return nextFreeHour(now, o.busy, minutes)
}

// The editor prefill for a message. `msg` is a cached mail row (subject, from_name, from_email, to_text,
// cc_text, body_preview, account_id). Options: now, ownEmails (the account's own addresses, never invited),
// parseWhen, defaultMinutes, defaultReminder, busy ([{start, end}]), body (plain text, if already loaded).
// Returns { accountId, subject, notes, start, end, source, required, optional, restore } where `restore`
// has the shape the editor takes for a handed-back draft (EventEditor `restore`).
function prefill(msg, o) {
    o = o || {}
    msg = msg || {}
    var own = {}
    ;(o.ownEmails || []).forEach(function(e) { own[String(e).trim().toLowerCase()] = true })

    var subject = cleanSubject(msg.subject)
    var body = String(o.body || msg.body_preview || "")

    var from = msg.from_email ? [{ name: String(msg.from_name || "").trim(), email: String(msg.from_email).trim() }] : []
    // Sender first, then To (required); Cc (optional). Own addresses are never invited; each address once.
    var seen = {}
    function take(list) {
        return list.filter(function(p) {
            var k = p.email.toLowerCase()
            if (own[k] || seen[k]) return false
            seen[k] = true
            return true
        })
    }
    var required = take(from.concat(parseAddresses(msg.to_text || "")))
    var optional = take(parseAddresses(msg.cc_text || ""))

    var when = suggestTime([subject, body.slice(0, 3000)], o)
    var notes = excerpt(body, 600)
    var reminder = o.defaultReminder === undefined || o.defaultReminder === null ? 15 : o.defaultReminder

    return {
        accountId: String(msg.account_id || ""),
        subject: subject,
        notes: notes,
        start: when.start,
        end: when.end,
        source: when.source,
        required: required,
        optional: optional,
        restore: {
            subject: subject,
            notes: notes,
            location: "",
            reminder: String(reminder),
            showAs: "busy",
            repeatText: "",
            base: { start: when.start, end: when.end, allDay: false },
            required: required.map(function(p) { return { email: p.email, name: p.name } }),
            optional: optional.map(function(p) { return { email: p.email, name: p.name } })
        }
    }
}
