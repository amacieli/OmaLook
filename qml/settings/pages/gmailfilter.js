.pragma library

// Form logic of the Gmail filter editor (GmailFilters.qml), kept here so tests/gmailfilter.test.js can run it.
// The backend (src/gmail_filters.rs) has the last word on what Gmail can hold; this only shapes the request.

// "a@x.com, @y.org" -> one predicate. Full addresses are exact (from_addresses); anything else is sender text
// (an address or a domain; the backend refuses fragments Gmail cannot match). null when nothing was typed.
function pred(text) {
  var vals = String(text || "").split(/[\s,;]+/).map(function(v) { return v.trim() }).filter(function(v) { return v !== "" })
  if (vals.length === 0) return null
  var allAddr = vals.every(function(v) { return /^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(v) })
  return { kind: allAddr ? "from_addresses" : "sender_contains", value: vals }
}

function ruleJson(tag, from, unless) {
  var c = pred(from), x = pred(unless), t = String(tag || "").trim()
  return JSON.stringify({ name: "Gmail filter: " + t, conditions: c ? [c] : [], exceptions: x ? [x] : [], actions: { tags: [t], stop: false } })
}

function isSender(p) { return p.kind === "from_addresses" || p.kind === "sender_contains" }

// The form can show a filter only when it is one sender predicate with at most one "unless" of the same kind.
function editable(f) {
  if (!f || !f.editable || !f.rule) return false
  var r = f.rule
  return r.conditions.length === 1 && isSender(r.conditions[0]) && r.exceptions.length <= 1 && (r.exceptions.length === 0 || isSender(r.exceptions[0]))
}

// Form fields for a filter that `editable` accepted.
function fields(f) {
  var r = f.rule
  return { tag: r.actions.tags[0], from: r.conditions[0].value.join(", "), unless: r.exceptions.length > 0 ? r.exceptions[0].value.join(", ") : "" }
}
