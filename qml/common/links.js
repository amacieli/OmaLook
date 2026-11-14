.pragma library

// Every link the app hands to the desktop goes through here (SEC-011). Views import it as
//   import "../common/links.js" as Links
// and call Links.openExternal(url). Only the schemes below are opened; anything else is refused.

var DEFAULT_SCHEMES = ["http", "https", "mailto"]

// The decision, without side effects (node-testable).
// Returns { ok, url, scheme, reason }. `url` is the trimmed input when ok.
// opts.tel: also allow tel: (phone call sites only).
function decide(raw, opts) {
  var s = (raw === undefined || raw === null) ? "" : String(raw).trim()
  if (s === "") return { ok: false, url: "", scheme: "", reason: "empty" }
  // Browsers drop tabs and newlines inside a URL, so a scheme split by them ("java\tscript:") is refused.
  if (/[\x00-\x1f\x7f]/.test(s)) return { ok: false, url: "", scheme: "", reason: "control character" }
  var m = /^([a-zA-Z][a-zA-Z0-9+.\-]*):/.exec(s)
  if (!m) return { ok: false, url: "", scheme: "", reason: "no scheme" }
  var scheme = m[1].toLowerCase()
  var allowed = DEFAULT_SCHEMES.slice()
  if (opts && opts.tel) allowed.push("tel")
  if (allowed.indexOf(scheme) < 0) return { ok: false, url: "", scheme: scheme, reason: "scheme not allowed: " + scheme }
  return { ok: true, url: s, scheme: scheme, reason: "" }
}

// Opens the link in the desktop's handler. Returns true when it was handed over, false when refused.
function openExternal(raw, opts) {
  var d = decide(raw, opts)
  if (!d.ok) {
    console.warn("openExternal refused: " + d.reason)
    return false
  }
  Qt.openUrlExternally(d.url)
  return true
}
