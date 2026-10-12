.pragma library

// Pure chord/scope resolution, kept free of QML types so it can be tested with node.
//
//   command: { id, title, scope, keys: ["j", "g g", "C-d"], run(), when?(), hidden? }
//   scopes:  innermost first, e.g. ["list", "mail/list", "mail", "global"]
//   seq:     tokens typed so far, e.g. ["g"]
//
// Returns { exact, prefixes }. `exact` is the innermost command bound to the whole
// sequence; `prefixes` are { cmd, next } for commands that continue it.
function resolve(commands, scopes, seq) {
  var key = seq.join(" ")
  var exact = null
  var prefixes = []
  for (var s = 0; s < scopes.length; s++) {
    for (var i = 0; i < commands.length; i++) {
      var c = commands[i]
      if (c.scope !== scopes[s]) continue
      if (c.when && !c.when()) continue
      for (var k = 0; k < c.keys.length; k++) {
        var bound = c.keys[k]
        if (bound === key) { if (!exact) exact = c }
        else if (bound.indexOf(key + " ") === 0) prefixes.push({ cmd: c, next: bound.slice(key.length + 1) })
      }
    }
  }
  return { exact: exact, prefixes: prefixes }
}

// Commands that apply in `scopes`, one entry per id (innermost wins), for help/palette.
function applicable(commands, scopes) {
  var seen = {}
  var out = []
  for (var s = 0; s < scopes.length; s++) {
    for (var i = 0; i < commands.length; i++) {
      var c = commands[i]
      if (c.scope !== scopes[s] || c.hidden || seen[c.id]) continue
      if (c.when && !c.when()) continue
      seen[c.id] = true
      out.push(c)
    }
  }
  return out
}

// Subsequence match: every character of `query` appears in `text` in order.
function fuzzy(query, text) {
  var q = query.toLowerCase().replace(/\s+/g, "")
  var t = text.toLowerCase()
  var j = 0
  for (var i = 0; i < t.length && j < q.length; i++) if (t[i] === q[j]) j++
  return j === q.length
}

function keyLabel(keys) {
  return keys.map(function(k) { return k.replace(/^C-S-/, "Ctrl-Shift-").replace(/^C-/, "Ctrl-").replace(/^S-/, "Shift-") }).join("  ")
}

// ":goto calendar" / "goto calendar" -> { name: "goto", arg: "calendar" }; null when empty.
function parseEx(text) {
  var m = /^\s*:?\s*(\S+)(\s+(.*))?$/.exec(text)
  if (!m) return null
  return { name: m[1].toLowerCase(), arg: (m[3] || "").trim(), hasSpace: m[2] !== undefined }
}

// The command (any scope) whose `ex` names include `name`, honouring `when`.
function findEx(commands, name) {
  for (var i = 0; i < commands.length; i++) {
    var c = commands[i]
    if (!c.ex || (c.when && !c.when())) continue
    if (c.ex.indexOf(name) >= 0) return c
  }
  return null
}

var sectionNames = {
  "global": "Everywhere", "folder": "Folders pane", "list": "List pane",
  "reader": "Reading pane", "mail": "Mail", "calendar/list": "Calendar", "contacts/list": "People",
  "tasks/list": "Tasks", "settings": "Settings", "sms": "SMS"
}
function sectionName(scope) { return sectionNames[scope] || scope }

// Every non-hidden command grouped by scope, scopes in `first` (the active ones) leading.
// Returns [{ scope, name, active, commands }].
function grouped(commands, first) {
  var order = first.slice()
  for (var i = 0; i < commands.length; i++) if (order.indexOf(commands[i].scope) < 0) order.push(commands[i].scope)
  var out = []
  for (var s = 0; s < order.length; s++) {
    var cs = commands.filter(function(c) { return c.scope === order[s] && !c.hidden && c.keys.length > 0 && (!c.helpWhen || c.helpWhen()) })
    if (cs.length) out.push({ scope: order[s], name: sectionName(order[s]), active: first.indexOf(order[s]) >= 0, commands: cs })
  }
  return out
}

// ---------------------------------------------------------------------------------------
// User key configuration ([keys] in settings.toml)
// ---------------------------------------------------------------------------------------

var NAMED = {
  esc: "Esc", escape: "Esc", tab: "Tab", "s-tab": "S-Tab", "shift-tab": "S-Tab", backtab: "S-Tab",
  enter: "Enter", return: "Enter", space: "Space", up: "Up", down: "Down", left: "Left", right: "Right",
  pgup: "PgUp", pageup: "PgUp", pgdown: "PgDown", pagedown: "PgDown", home: "Home", end: "End",
  delete: "Delete", del: "Delete", backspace: "Backspace"
}

// "Ctrl-r" -> "C-r", "ctrl-shift-v" -> "C-S-v", "PageUp" -> "PgUp"; null when it is not a key.
function normToken(t) {
  if (t.length === 1) return t > " " ? t : null
  var lower = t.toLowerCase()
  var m = /^(?:c|ctrl)-(?:(s|shift)-)?([a-z0-9])$/.exec(lower)
  if (m) return "C-" + (m[1] ? "S-" : "") + m[2]
  if (NAMED[lower]) return NAMED[lower]
  m = /^f([1-9]|1[0-2])$/.exec(lower)
  if (m) return "F" + m[1]
  return null
}

// "g  m" -> "g m"; null when any token is not a key.
function normKey(seq) {
  var parts = String(seq).split(/\s+/).filter(function(p) { return p !== "" })
  if (parts.length === 0) return null
  var out = []
  for (var i = 0; i < parts.length; i++) {
    var n = normToken(parts[i])
    if (n === null) return null
    out.push(n)
  }
  return out.join(" ")
}

// Extra bindings a preset adds on top of the defaults (vim keys keep working).
var presets = {
  outlook: {
    "mail.reply": ["C-r"], "mail.replyall": ["C-S-r"], "mail.forward": ["C-f"],
    "item.delete": ["Delete"], "mail.archive": ["Backspace"], "mail.read": ["C-q"],
    "mail.move": ["C-S-v"], "mail.undo": ["C-z"], "app.sync": ["F9"],
    "go.mail": ["C-1"], "go.calendar": ["C-2"], "go.people": ["C-3"], "go.tasks": ["C-4"]
  }
}

// Apply { preset, bindings: { id: [keys] }, error } to the default registry.
// A preset adds keys; a binding for an id REPLACES that command's keys ([] unbinds it).
// Returns { commands, problems } where problems are human-readable strings (bad keys,
// unknown ids, two commands on one key in the same scope).
function applyBindings(commands, config) {
  var problems = []
  config = config || {}
  if (config.error) problems.push(config.error)
  var preset = config.preset ? String(config.preset).toLowerCase() : ""
  var extra = {}
  if (preset !== "" && preset !== "default") {
    if (presets[preset]) extra = presets[preset]
    else problems.push("unknown preset '" + config.preset + "' (known: default, " + Object.keys(presets).join(", ") + ")")
  }
  var overrides = config.bindings || {}
  var known = {}
  commands.forEach(function(c) { known[c.id] = true })
  Object.keys(overrides).forEach(function(id) { if (!known[id]) problems.push("unknown command '" + id + "'") })

  var out = commands.map(function(c) {
    var n = {}
    for (var k in c) n[k] = c[k]
    var keys = c.keys.slice()
    if (extra[c.id]) extra[c.id].forEach(function(k) { if (keys.indexOf(k) < 0) keys.push(k) })
    if (overrides[c.id] !== undefined) {
      keys = []
      overrides[c.id].forEach(function(raw) {
        var nk = normKey(raw)
        if (nk === null) problems.push(c.id + ": '" + raw + "' is not a key (use e.g. j, G, g m, C-r, C-S-v, Tab, F9)")
        else if (keys.indexOf(nk) < 0) keys.push(nk)
      })
      n.custom = true
    }
    n.keys = keys
    return n
  })

  // Conflicts: same scope, same key (or one key is the start of another's chord).
  // Commands with a `when` guard share keys on purpose.
  var byScope = {}
  out.forEach(function(c) { if (c.keys.length && !c.when) (byScope[c.scope] = byScope[c.scope] || []).push(c) })
  Object.keys(byScope).forEach(function(scope) {
    var list = byScope[scope]
    for (var i = 0; i < list.length; i++) for (var j = i + 1; j < list.length; j++) {
      list[i].keys.forEach(function(a) {
        list[j].keys.forEach(function(b) {
          if (a === b) problems.push("'" + a + "' is bound to both " + list[i].id + " and " + list[j].id + " (" + sectionName(scope) + ")")
          else if (b.indexOf(a + " ") === 0 || a.indexOf(b + " ") === 0)
            problems.push("'" + (a.length < b.length ? a : b) + "' (" + (a.length < b.length ? list[i].id : list[j].id) + ") hides the chord '" + (a.length < b.length ? b : a) + "' (" + (a.length < b.length ? list[j].id : list[i].id) + ")")
        })
      })
    }
  })
  return { commands: out, problems: problems }
}
