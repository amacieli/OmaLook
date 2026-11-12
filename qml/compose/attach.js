.pragma library

// What the compose attachment strip shows, and how full the message is, as pure functions so
// they are tested with node (tests/attach.test.js). The numbers mirror src/compose.rs
// (`check_attachments`): the daemon measures the real message when Send is pressed, this is the
// live estimate while the user is still adding files.
//
// An item is { name, size, origin: "picked" | "original" }.

var MAX_CHIPS = 3        // up to this many files get a chip each
var SHOWN_WHEN_MORE = 2  // with more files than that: this many chips, then "+N"

// What a limits record holds when the daemon has not answered yet (same figures as compose.rs).
function defaultLimits(provider) {
  if (provider === "gmail") return { provider: "gmail", basis: "encoded", hard: 36700160, warn: 25000000 }
  return { provider: provider || "", basis: "raw", hard: 150 * 1024 * 1024, warn: null }
}

// Which parts of the strip exist for `n` files. n = 0: no strip at all.
function layout(n) {
  if (n <= 0) return { show: false, chips: 0, more: 0 }
  if (n <= MAX_CHIPS) return { show: true, chips: n, more: 0 }
  return { show: true, chips: SHOWN_WHEN_MORE, more: n - SHOWN_WHEN_MORE }
}

// One file as a base64 MIME part: 4 bytes per 3, CRLF every 76 characters, some part headers.
function encodedPart(raw) {
  var b64 = Math.ceil(raw / 3) * 4
  return b64 + Math.ceil(b64 / 76) * 2 + 400
}

var _OVERHEAD = 8192     // headers + a body that may be sent twice (plain and HTML)

// The figure compared with the limits: raw bytes for Graph, estimated message size for Gmail.
function counted(limits, items) {
  var raw = 0, enc = _OVERHEAD
  for (var i = 0; i < items.length; i++) {
    var s = Number(items[i].size) || 0
    raw += s
    enc += encodedPart(s)
  }
  return limits.basis === "encoded" ? enc : raw
}

// The limit in terms of files, for the "x of y" text: for Gmail the encoded limit shrunk by the
// base64 growth (about a third).
function capacity(limits) {
  if (limits.basis !== "encoded") return limits.hard
  return Math.floor((limits.hard - _OVERHEAD) * 76 / 78 * 3 / 4)
}

function rawTotal(items) {
  var t = 0
  for (var i = 0; i < items.length; i++) t += Number(items[i].size) || 0
  return t
}

// level: "ok" | "warn" (above 80 % of the hard limit, or above the provider's soft limit) |
// "over" (the daemon would refuse to send).
function meter(limits, items) {
  var c = counted(limits, items)
  var level = "ok"
  if (c > limits.hard) level = "over"
  else if (c > limits.hard * 0.8 || (limits.warn && c > limits.warn)) level = "warn"
  return { counted: c, raw: rawTotal(items), capacity: capacity(limits), level: level }
}

function accountName(limits) { return limits.provider === "gmail" ? "Gmail" : "this account" }

// Per-item state: "ok", or "big" for the biggest file when the whole is over the limit (so one
// chip turns red and the tooltip says why). `reasons[i]` is the tooltip text, "" when fine.
function states(limits, items) {
  var st = [], reasons = []
  for (var i = 0; i < items.length; i++) { st.push("ok"); reasons.push("") }
  if (items.length > 0 && meter(limits, items).level === "over") {
    var big = 0
    for (var j = 1; j < items.length; j++) if ((Number(items[j].size) || 0) > (Number(items[big].size) || 0)) big = j
    st[big] = "big"
    reasons[big] = "too big for " + accountName(limits) + ": the message limit is about " + _mb(capacity(limits)) + " of files"
  }
  return { state: st, reason: reasons }
}

function _mb(n) { return Math.round(n / 1048576) + " MB" }

// Names as shown in the strip: a second `scan.pdf` becomes `scan.pdf (2)` (display only: the
// real names are sent).
function displayNames(items) {
  var seen = {}, out = []
  for (var i = 0; i < items.length; i++) {
    var n = String(items[i].name), k = n.toLowerCase()
    seen[k] = (seen[k] || 0) + 1
    out.push(seen[k] > 1 ? n + " (" + seen[k] + ")" : n)
  }
  return out
}

// Index of the picked file (in `picked`) behind row `row` of the combined list [originals..., picked...].
function combine(originals, picked) {
  var out = []
  for (var i = 0; i < originals.length; i++) out.push({ name: originals[i].name, size: originals[i].size, origin: "original", ref: i })
  for (var j = 0; j < picked.length; j++) out.push({ name: picked[j].name, size: picked[j].size, origin: "picked", ref: j })
  return out
}


// ---- files on the clipboard (Ctrl+V in the compose pane; phase 5) -----------------------------------------
// `types` is `wl-paste --list-types`, `text` the content of the best file type (see clipboardFileType).
// Pictures the editor shows inline in HTML mode (OL-054). SVG stays a plain file.
function isPicture(name) { return /\.(png|jpe?g|gif|webp|bmp)$/i.test(String(name || "")) }
// Whether `wl-paste --list-types` shows picture data the editor can take (PNG).
function clipboardHasPng(types) {
  return String(types || "").split("\n").map(function(x) { return x.trim() }).indexOf("image/png") >= 0
}

// Which clipboard type carries files, or "" when the clipboard holds none (then Ctrl+V is an ordinary text paste).
function clipboardFileType(types) {
  var t = String(types || "").split("\n").map(function(x) { return x.trim() })
  if (t.indexOf("text/uri-list") >= 0) return "text/uri-list"
  if (t.indexOf("x-special/gnome-copied-files") >= 0) return "x-special/gnome-copied-files"
  return ""
}

// Local file paths out of a uri-list or a gnome-copied-files body (first line "copy" or "cut"): file:// URIs only,
// percent-decoded, comments and anything else ignored, no duplicates.
function pathsFromClipboard(text) {
  var out = [], seen = {}
  String(text || "").split(/\r?\n/).forEach(function(line) {
    line = line.trim()
    if (line === "" || line.charAt(0) === "#" || line === "copy" || line === "cut") return
    var m = /^file:\/\/(?:localhost)?(\/.*)$/i.exec(line)
    if (!m) return
    var p
    try { p = decodeURIComponent(m[1]) } catch (e) { return }
    if (p.indexOf("\0") >= 0 || seen[p]) return
    seen[p] = true
    out.push(p)
  })
  return out
}

// System-font switch (OL-054): the pictures still in the text become ordinary files. An entry whose
// picture was already deleted from the text (its cid is not in `liveCids`) is dropped, and a path listed
// twice is kept once. Returns { files, converted } (converted = pictures turned into files).
function pictureFiles(attachments, liveCids) {
  var live = liveCids || [], seen = {}, files = [], converted = 0
  for (var i = 0; i < attachments.length; i++) {
    var a = attachments[i]
    if (a.inline_cid) {
      if (live.indexOf(a.inline_cid) < 0) continue
      converted += 1
      a = { path: a.path, name: a.name, size: a.size }
    }
    if (seen[a.path]) continue
    seen[a.path] = true
    files.push(a)
  }
  return { files: files, converted: converted }
}
function baseName(p) { var i = String(p).lastIndexOf("/"); return i < 0 ? String(p) : String(p).slice(i + 1) }
function dirName(p) { var i = String(p).lastIndexOf("/"); return i <= 0 ? "/" : String(p).slice(0, i) }
