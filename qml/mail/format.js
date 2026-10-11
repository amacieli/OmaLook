.pragma library

// Display helpers for message rows / headers. Pure functions.

function senderName(m) {
  if (!m) return ""
  return m.from_name ? m.from_name : (m.from_email || "")
}

// Cached timestamps are UTC instants ("...Z" / with offset). Always render them in the
// machine's local zone; never slice the string, which shows UTC wall-clock time.
function _pad(n) { return n < 10 ? "0" + n : "" + n }

function _parse(s) {
  if (!s) return null
  var str = String(s).trim()
  // "YYYY-MM-DD HH:MM:SS" (SQLite style) with no zone designator is UTC by convention here.
  if (/^\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}(:\d{2}(\.\d+)?)?$/.test(str)) str = str.replace(" ", "T") + "Z"
  var d = new Date(str)
  return isNaN(d.getTime()) ? null : d
}

function listDate(s) {
  var d = _parse(s)
  if (!d) return (s || "").substring(0, 10)
  var now = new Date()
  if (d.toDateString() === now.toDateString()) return _pad(d.getHours()) + ":" + _pad(d.getMinutes())
  return _pad(d.getMonth() + 1) + "-" + _pad(d.getDate())
}

function fullDate(s) {
  var d = _parse(s)
  if (!d) return (s || "").substring(0, 10)
  return d.getFullYear() + "-" + _pad(d.getMonth() + 1) + "-" + _pad(d.getDate()) + "  " + _pad(d.getHours()) + ":" + _pad(d.getMinutes())
}

// "to  Bob <bob@x.org>, …" lines for the reading pane; empty when there is nothing to show.
function recipientLine(label, text) {
  return text && text.length > 0 ? label + "  " + text : ""
}

function recipients(m) {
  if (!m) return ""
  return recipientLine("to ", m.to_text || m.to_recipients || m.to || "")
}

// Categories of a message row: [[name, "#rrggbb"], …] (the list model stores them as JSON text).
function categories(m) {
  if (!m || !m.cats) return []
  try { var c = JSON.parse(m.cats); return Array.isArray(c) ? c : [] } catch (e) { return [] }
}

function body(m) {
  if (!m) return ""
  return m.body_preview || m.body || m.snippet || ""
}

// ---- message bodies --------------------------------------------------------

var _ENTITIES = { nbsp: " ", amp: "&", lt: "<", gt: ">", quot: "\"", apos: "'", copy: "©",
                  reg: "®", trade: "™", hellip: "…", mdash: "—", ndash: "–", lsquo: "‘",
                  rsquo: "’", ldquo: "“", rdquo: "”", bull: "•", middot: "·", euro: "€" }

function decodeEntities(s) {
  return s.replace(/&(#x[0-9a-f]+|#\d+|[a-z]+);/gi, function(all, e) {
    if (e.charAt(0) === "#") {
      var code = e.charAt(1).toLowerCase() === "x" ? parseInt(e.substring(2), 16) : parseInt(e.substring(1), 10)
      return code > 0 && code < 0x110000 ? String.fromCodePoint(code) : all
    }
    var v = _ENTITIES[e.toLowerCase()]
    return v === undefined ? all : v
  })
}

function escapeHtml(s) {
  return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
}

// HTML mail as readable plain text for the system-font view: no markup, block
// elements become line breaks, list items get bullets, links keep their address.
function htmlToText(html) {
  var s = String(html || "")
  s = s.replace(/<!--[\s\S]*?-->/g, "")
  s = s.replace(/<(head|style|script|title)\b[\s\S]*?<\/\1\s*>/gi, "")
  s = s.replace(/<br\s*\/?>/gi, "\n")
  s = s.replace(/<li\b[^>]*>/gi, "\n• ")
  s = s.replace(/<\/(p|div|tr|table|ul|ol|h[1-6]|blockquote|pre|section|article|header|footer)\s*>/gi, "\n")
  s = s.replace(/<\/t[dh]\s*>/gi, "  ")
  s = s.replace(/<a\b[^>]*href\s*=\s*["']([^"']+)["'][^>]*>([\s\S]*?)<\/a\s*>/gi, function(all, href, label) {
    var text = label.replace(/<[^>]+>/g, "").trim()
    // Tracking links run to hundreds of characters; only a readable address is worth showing.
    if (!/^https?:/i.test(href) || href.length > 80 || text === "" || text === href || text.indexOf(href) >= 0) return label
    return label + " (" + href + ")"
  })
  s = s.replace(/<[^>]+>/g, "")
  s = decodeEntities(s)
  s = s.replace(/[ \t\u00a0]+\n/g, "\n").replace(/\n[ \t]+/g, "\n")
  s = s.replace(/[ \t]{3,}/g, "  ").replace(/\n{3,}/g, "\n\n")
  return s.trim()
}

// HTML mail prepared for Qt's rich-text engine, which is the closest thing to a
// small mail renderer available here (no web engine can run inside Quickshell).
// Like Outlook it shows the body on a white page and blocks remote content, so
// nothing is fetched just because a message was opened. Returns { html, blocked }
// where `blocked` counts the remote images left out. With `allowRemote` (the user
// unblocked this message or its sender) remote images are kept and nothing is blocked.
function sanitizeHtml(html, maxWidth, allowRemote) {
  var s = String(html || "")
  var blocked = 0
  var body = /<body\b[^>]*>([\s\S]*?)(<\/body\s*>|$)/i.exec(s)
  if (body) s = body[1]
  s = s.replace(/<!--[\s\S]*?-->/g, "")
  s = s.replace(/<(head|style|script|title|object|embed|iframe|form|button|select|textarea|svg|video|audio|noscript)\b[\s\S]*?<\/\1\s*>/gi, "")
  s = s.replace(/<(meta|link|input|base|source|track|area)\b[^>]*>/gi, "")
  s = s.replace(/<\/?(o:[a-z]+|v:[a-z]+|w:[a-z]+|m:[a-z]+|xml)\b[^>]*>/gi, "")
  s = s.replace(/\s+on[a-z]+\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)/gi, "")
  s = s.replace(/(href\s*=\s*["']?)\s*javascript:[^"'>\s]*/gi, "$1#")

  // Images: inline data stays; anything remote (or cid:) is replaced by its alt text.
  s = s.replace(/<img\b[^>]*>/gi, function(tag) {
    var src = /\bsrc\s*=\s*["']?([^"'\s>]+)/i.exec(tag)
    if (src && /^data:image\//i.test(src[1])) return tag
    var alt = /\balt\s*=\s*"([^"]*)"|\balt\s*=\s*'([^']*)'/i.exec(tag)
    var text = alt ? (alt[1] || alt[2] || "") : ""
    var remote = src && /^https?:/i.test(src[1])
    // Qt's Text fetches a remote <img> itself and, when that request finishes or fails after the
    // text has changed, crashes in QQuickText::resourceRequestFinished (QUrl::resolved, SIGSEGV,
    // seen 2026-10-10). So Qt is never handed a remote URL, even for allowed senders: the image
    // becomes a link to open in the browser instead.
    if (remote && allowRemote) {
      var label = text.trim() === "" ? "image" : text.trim()
      var href = decodeEntities(src[1]).replace(/"/g, "%22")
      return "<a href=\"" + href + "\">[" + escapeHtml(decodeEntities(label)) + "]</a>"
    }
    if (remote) blocked++
    return text.trim() === "" ? "" : "[" + escapeHtml(decodeEntities(text.trim())) + "]"
  })
  // Qt also loads images named by background= on table/td/body, and by srcset/poster. Drop them from tags.
  s = s.replace(/<[a-z][^>]*>/gi, function(tag) {
    return tag.replace(/\s(background|srcset|poster|dynsrc|lowsrc)\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)/gi, "")
  })

  // Qt's table layout shrinks a table with no stated width to its narrowest content
  // (a column a few characters wide), and mail usually states its width in CSS that
  // is gone by now. So every table gets an explicit width: its own if it fits the
  // pane, otherwise the full width available.
  s = s.replace(/<table\b[^>]*>/gi, function(tag) {
    var attr = /\bwidth\s*=\s*["']?(\d+)(%|px)?/i.exec(tag)
    var css = /\bstyle\s*=\s*["'](?:[^"']*?[\s;])?width\s*:\s*(\d+)(%|px)/i.exec(tag)
    var found = attr || css
    var width = "100%"
    if (found) {
      var n = parseInt(found[1], 10)
      if (found[2] === "%") width = n + "%"
      else if (maxWidth <= 0 || n <= maxWidth) width = String(n)
    }
    tag = tag.replace(/\swidth\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)/gi, "")
    return tag.replace(/^<table/i, "<table width=\"" + width + "\"")
  })
  // Other fixed widths wider than the pane would run off its edge: drop them.
  if (maxWidth > 0) {
    s = s.replace(/<(td|th|div|img)\b[^>]*>/gi, function(tag) {
      return tag.replace(/\bwidth\s*=\s*["']?(\d+)(px)?["']?/gi, function(all, n) {
        return parseInt(n, 10) > maxWidth ? "" : all
      })
    })
  }
  return { html: s, blocked: blocked }
}

// Plain-text mail shown through the HTML view: escaped, line breaks kept, links live.
function textToHtml(text) {
  var s = escapeHtml(text || "")
  s = s.replace(/(https?:\/\/[^\s<]+)/g, "<a href=\"$1\">$1</a>")
  return s.replace(/\r?\n/g, "<br>")
}

// "2.1 MB", "340 KB", "12 B"
function bytes(n) {
  n = Number(n) || 0
  if (n < 1024) return n + " B"
  if (n < 1048576) return (n / 1024).toFixed(n < 10240 ? 1 : 0) + " KB"
  return (n / 1048576).toFixed(n < 10485760 ? 1 : 0) + " MB"
}
