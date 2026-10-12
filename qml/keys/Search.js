.pragma library

// Pure helpers for editing a search pattern ("from:bob is:unread") term by term, kept free of
// QML types so node can test them (tests/search.test.js). Mirrors the backend tokenizer in
// src/search.rs: terms split on whitespace outside double quotes.

function tokens(q) {
  var out = [], cur = "", inq = false
  for (var i = 0; i < q.length; i++) {
    var c = q[i]
    if (c === '"') { inq = !inq; cur += c }
    else if (/\s/.test(c) && !inq) { if (cur !== "") out.push(cur); cur = "" }
    else cur += c
  }
  if (cur !== "") out.push(cur)
  return out
}

// Terms that cannot both be active; adding one removes the other.
var EXCLUSIVE = [["is:unread", "is:read"]]

function same(a, b) { return a.toLowerCase() === b.toLowerCase() }

function has(q, term) {
  return tokens(q).some(function(t) { return same(t, term) })
}

// Add `term` to `q`, or remove it if it is already there.
function toggle(q, term) {
  var toks = tokens(q)
  if (toks.some(function(t) { return same(t, term) }))
    return toks.filter(function(t) { return !same(t, term) }).join(" ")
  EXCLUSIVE.forEach(function(g) {
    if (g.some(function(x) { return same(x, term) }))
      toks = toks.filter(function(t) { return !g.some(function(x) { return same(x, t) }) })
  })
  toks.push(term)
  return toks.join(" ")
}

// A field value, quoted when it holds whitespace (the pattern language has no escapes, so
// embedded quotes are dropped).
function quote(v) {
  v = String(v).replace(/"/g, "").trim()
  return /\s/.test(v) ? '"' + v + '"' : v
}

// "Re: Fwd: Lunch" -> "Lunch"
function stripReplyPrefix(s) {
  return String(s).replace(/^\s*((re|fw|fwd)\s*:\s*)+/i, "").trim()
}
