.pragma library
// Display helpers for the SMS panes. Pure functions.

function _pad(n) { return n < 10 ? "0" + n : "" + n }
var MONTHS = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"]

function sameDay(a, b) {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate()
}

// Thread list: time today, "Mon d" this year, else yyyy-mm-dd.
function listDate(ms) {
  var d = new Date(ms), now = new Date()
  if (sameDay(d, now)) return _pad(d.getHours()) + ":" + _pad(d.getMinutes())
  if (d.getFullYear() === now.getFullYear()) return MONTHS[d.getMonth()] + " " + d.getDate()
  return d.getFullYear() + "-" + _pad(d.getMonth() + 1) + "-" + _pad(d.getDate())
}

function clock(ms) { var d = new Date(ms); return _pad(d.getHours()) + ":" + _pad(d.getMinutes()) }

// Day divider in the conversation: "Today", "Yesterday", "Mon d" / "Mon d, yyyy".
function dayLabel(ms) {
  var d = new Date(ms), now = new Date()
  if (sameDay(d, now)) return "Today"
  var y = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1)
  if (sameDay(d, y)) return "Yesterday"
  var s = MONTHS[d.getMonth()] + " " + d.getDate()
  return d.getFullYear() === now.getFullYear() ? s : s + ", " + d.getFullYear()
}

// Mine = anything not received (sent, queued, failed, ...). Android type 1 = received.
function isMine(m) { return m.direction !== 1 }

function messageKey(m) { return m.device_id + ":" + (m.mms ? "m" : "s") + m.uid }
function threadKey(t) { return t.device_id + ":" + t.thread_id }

function attachmentText(atts) {
  if (!atts || atts.length === 0) return ""
  var kinds = {}
  atts.forEach(function(a) { kinds[String(a.mime || "file").split("/")[0]] = true })
  return atts.length + " attachment" + (atts.length === 1 ? "" : "s") + " (" + Object.keys(kinds).join(", ") + ")"
}

// A one-line preview of a thread's newest message.
function preview(t) {
  var body = String(t.last_body || "").replace(/\s+/g, " ").trim()
  if (body === "") body = "(attachment)"
  if (t.last_direction === 5) return "\u26a0 Send failure: " + body
  return (t.last_direction !== 1 ? "You: " : "") + body
}

// Android message types for texts that have not (or never) left: 6 queued, 5 failed.
function isFailed(m) { return m.direction === 5 }
function isQueued(m) { return m.direction === 6 }

// "name", "number" or "text": why a thread is in the search results.
function matchLabel(t) {
  return t.match === "name" ? "name" : t.match === "number" ? "number" : t.match === "text" ? "text" : ""
}

// A recipient the user typed: digits with the usual phone punctuation, or a short code. 3+ digits.
function looksLikeNumber(s) {
  s = String(s).trim()
  return /^[+()\-.\s\d]+$/.test(s) && s.replace(/\D/g, "").length >= 3
}

// Initial of a title, for the avatar circle ("+1555…" -> "#").
function initial(title) {
  var c = String(title || "").trim().charAt(0)
  return /[A-Za-z]/.test(c) ? c.toUpperCase() : "#"
}
