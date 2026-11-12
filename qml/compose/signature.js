.pragma library

// OL-050: signatures in the compose pane. Pure text logic; ComposeView applies the results to the editor.
//
// System-font mode: the block starts with the plain-text delimiter "-- " (RFC 3676), so mail clients
// can recognise the signature. HTML mode: the signature is plain lines with no delimiter.
// Above the quoted text for replies and forwards; for a new message the block goes below the
// cursor line, where the user starts typing.

// The text to insert for a signature, or "" when there is none.
function block(sig, system) {
  var s = String(sig || "")
  if (s.trim() === "") return ""
  return system ? "-- \n" + s : s
}

// Where and what to insert. `plain` is the editor's plain text, `quoteAt` the index where the
// quoted original starts (-1 when there is none). Returns { at, text } or null when there is no
// signature to insert. `text` is exactly what gets inserted, so it can be removed again later.
// Replies: the block sits right above the quote, with a blank line after it.
// New message: the block sits on the line below the cursor (cursor at 0).
function insertion(plain, sig, system, quoteAt) {
  var b = block(sig, system)
  if (b === "") return null
  if (quoteAt !== undefined && quoteAt !== null && quoteAt >= 0) return { at: quoteAt, text: b + "\n\n" }
  return { at: 0, text: "\n" + b + "\n" }
}

// Where the inserted text sits in `plain`, or -1 when the user has changed it (then it is left alone).
function locate(plain, inserted) {
  if (!inserted) return -1
  return String(plain || "").indexOf(inserted)
}

// The index of the quoted original in `plain`: the "On <date>, <from> wrote:" line, or -1.
function quoteStart(plain, quoteFirstLine) {
  if (!quoteFirstLine) return -1
  return String(plain || "").indexOf(quoteFirstLine)
}
