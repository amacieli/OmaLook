.pragma library

// The compose-mode matrix (PLAN-compose.md section 2) lives here and nowhere else.
// FormatBar reads `allowed()` to enable/grey each control; ComposeView reads it to decide
// what a mode switch strips.

var SYSTEM = "system"
var HTML = "html"

// feature -> { modes it works in, tooltip when greyed, soon: not built yet }
var FEATURES = {
  bold:        { modes: ["system", "html"] },
  italic:      { modes: ["system", "html"] },
  underline:   { modes: ["system", "html"] },
  strike:      { modes: ["html"] },
  bulletList:  { modes: ["system", "html"] },
  numberList:  { modes: ["system", "html"] },
  link:        { modes: ["system", "html"] },
  quote:       { modes: ["system", "html"] },
  undo:        { modes: ["system", "html"] },
  attach:      { modes: ["system", "html"], soon: "attachments arrive with the send backend" },
  fontFamily:  { modes: ["html"] },
  fontSize:    { modes: ["html"] },
  textColor:   { modes: ["html"] },
  highlight:   { modes: ["html"] },
  align:       { modes: ["html"] },
  clear:       { modes: ["html"] },
  image:       { modes: ["html"], soon: "inline images are part of the HTML-mode phase" },
  table:       { modes: ["html"], soon: "tables are part of the HTML-mode phase" }
}

function allowed(mode, feature) {
  var f = FEATURES[feature]
  return !!f && !f.soon && f.modes.indexOf(mode) >= 0
}

function reason(mode, feature) {
  var f = FEATURES[feature]
  if (!f) return ""
  if (f.soon) return f.soon
  if (f.modes.indexOf(mode) < 0) return "not available in system-font mode — switch to HTML"
  return ""
}

// Recipient parsing shared by the field and the send check.
var EMAIL_RE = /^[^\s@<>,;]+@[^\s@<>,;]+\.[^\s@<>,;]+$/

function isValidEmail(s) { return EMAIL_RE.test(String(s).trim()) }

// "Name <a@b.c>, d@e.f; g@h.i" -> [{ text, email, name }]
function parseRecipients(s) {
  var out = []
  // Split on , ; and newlines, but not inside "quotes" or <angle brackets> ("Doe, Bob" <b@y.org>).
  var parts = [], cur = "", q = false, ang = false, str = String(s)
  for (var k = 0; k < str.length; k++) {
    var ch = str.charAt(k)
    if (ch === '"') q = !q
    else if (!q && ch === "<") ang = true
    else if (!q && ch === ">") ang = false
    if (!q && !ang && (ch === "," || ch === ";" || ch === "\n")) { parts.push(cur); cur = "" } else cur += ch
  }
  parts.push(cur)
  for (var i = 0; i < parts.length; i++) {
    var p = parts[i].trim()
    if (p === "") continue
    var m = /^(.*?)\s*<([^>]+)>$/.exec(p)
    var email = m ? m[2].trim() : p
    var name = m ? m[1].replace(/^"|"$/g, "").trim() : ""
    out.push({ text: p, email: email, name: name, valid: isValidEmail(email) })
  }
  return out
}
