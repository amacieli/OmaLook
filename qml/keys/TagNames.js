.pragma library

// Tag-name helpers shared with src/db/tags.rs (`squash` / `near_match`): keep the two in step, the
// cases in tests/tagnames.test.js mirror the Rust tests.

function key(name) { return String(name || "").trim().toLowerCase() }

// Lower case, spaces and punctuation dropped (no unicode property escapes: Qt's JS engine may lack them),
// "saint" written "st": "Saint Lucia", "SaintLucia" and "St Lucia" all give "stlucia".
function squash(name) {
  return String(name || "").toLowerCase().split("saint").join("st").replace(/[\s\-_.,'"!?()\[\]{}:;\/\\&+*#@$%^=|<>~`]/g, "")
}

// One tag written two ways (not the same tag ignoring case, which is just that tag).
function nearMatch(a, b) {
  if (key(a) === key(b)) return false
  var x = squash(a), y = squash(b)
  if (x === "" || y === "") return false
  if (x === y) return true
  var shortS = x.length <= y.length ? x : y, longS = x.length <= y.length ? y : x
  return shortS.length >= 4 && longS.indexOf(shortS) >= 0
}

// Names in `names` that look like `name` but are not the same tag.
function similar(name, names) {
  var out = []
  for (var i = 0; i < names.length; i++) if (nearMatch(name, names[i])) out.push(names[i])
  return out
}
