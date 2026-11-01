.pragma library

// Pure helpers for the attach file browser (AttachBrowser.qml), tested with node (tests/pick.test.js).

// Does `text` hold a glob (`*` or `?`)? Without one the browser filters by substring, as before.
function isGlob(text) { return /[*?]/.test(text) }

// Case-insensitive glob over a whole name: `*` any run, `?` one character. Everything else literal.
function globRegex(pattern) {
  var re = ""
  for (var i = 0; i < pattern.length; i++) {
    var c = pattern[i]
    if (c === "*") re += ".*"
    else if (c === "?") re += "."
    else re += c.replace(/[.+^${}()|[\]\\\/-]/g, "\\$&")
  }
  return new RegExp("^" + re + "$", "i")
}

// The filter the box applies: glob match when it holds `*`/`?`, otherwise substring.
function matches(filter, name) {
  if (filter === "") return true
  if (isGlob(filter)) return globRegex(filter).test(name)
  return name.toLowerCase().indexOf(filter.toLowerCase()) >= 0
}

var SORTS = ["name", "newest", "size"]

function nextSort(s) { return SORTS[(SORTS.indexOf(s) + 1) % SORTS.length] }

// Folders always first; then by name (A-Z), newest first, or biggest first. Returns a new array.
function sorted(entries, key) {
  var out = entries.slice()
  var byName = function(a, b) {
    var x = a.name.toLowerCase(), y = b.name.toLowerCase()
    return x < y ? -1 : x > y ? 1 : (a.name < b.name ? -1 : a.name > b.name ? 1 : 0)
  }
  out.sort(function(a, b) {
    if (a.dir !== b.dir) return a.dir ? -1 : 1
    if (!a.dir) {
      if (key === "newest" && (b.mtime || 0) !== (a.mtime || 0)) return (b.mtime || 0) - (a.mtime || 0)
      if (key === "size" && (b.size || 0) !== (a.size || 0)) return (b.size || 0) - (a.size || 0)
    }
    return byName(a, b)
  })
  return out
}

// `marked` is path -> { size }. Sum in bytes and count.
function totals(marked) {
  var n = 0, bytes = 0
  for (var k in marked) { n++; bytes += Number(marked[k].size) || 0 }
  return { count: n, bytes: bytes }
}

// Is a total (bytes already on the message + ticked) beyond what the account takes?
function over(capacity, baseBytes, tickedBytes) {
  return capacity > 0 && baseBytes + tickedBytes > capacity
}

function shortDir(path, home) {
  if (home && (path === home || path.indexOf(home + "/") === 0)) return "~" + path.slice(home.length)
  return path
}

// Folders offered on Ctrl+B: the fixed places, then the folders visited recently (newest first, no repeats).
function places(home, recent, existing) {
  var out = [], seen = {}
  var add = function(p, label) { if (p && !seen[p]) { seen[p] = 1; out.push({ path: p, label: label || shortDir(p, home) }) } }
  add(home, "Home")
  var names = ["Desktop", "Documents", "Downloads", "Pictures", "Videos", "Music"]
  for (var i = 0; i < names.length; i++) {
    var p = home + "/" + names[i]
    if (!existing || existing.indexOf(p) >= 0) add(p, names[i])
  }
  for (var j = 0; j < (recent || []).length; j++) add(recent[j])
  return out
}
