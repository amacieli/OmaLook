pragma Singleton
import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Commons as Commons

// The extra hues of the active Omarchy theme (what btop colours its boxes and graphs with).
// The shell's Color singleton only exposes foreground / background / accent / urgent / muted;
// the theme's colors.toml also carries red, orange, yellow, green, cyan, blue, magenta, their
// bright variants and a few foreground / background shades. They are read here and follow theme
// switches. Every role falls back to something Color already provides, so a theme (or a missing
// file) that lacks a key still renders sanely, and nothing here hard-codes a colour.
QtObject {
  id: root

  readonly property string path: Quickshell.env("HOME") + "/.local/state/omarchy/current/theme/colors.toml"

  // raw "key" -> "#rrggbb" from colors.toml; reassigned wholesale so bindings re-evaluate.
  property var values: ({})

  function pick(key, fallback) {
    var v = values[key]
    return (typeof v === "string" && /^#[0-9a-fA-F]{6,8}$/.test(v)) ? v : fallback
  }

  function parse(raw) {
    var out = {}
    var lines = String(raw || "").split("\n")
    for (var i = 0; i < lines.length; i++) {
      var m = lines[i].match(/^\s*([A-Za-z0-9_-]+)\s*=\s*["']([^"']+)["']/)
      if (m) out[m[1]] = m[2]
    }
    values = out
  }

  // Hues. Fallbacks: warm/danger hues -> urgent, everything else -> accent.
  readonly property color red:     pick("red",     Commons.Color.urgent)
  readonly property color orange:  pick("orange",  pick("yellow", Commons.Color.urgent))
  readonly property color yellow:  pick("yellow",  Commons.Color.accent)
  readonly property color green:   pick("green",   Commons.Color.accent)
  readonly property color cyan:    pick("cyan",    Commons.Color.accent)
  readonly property color blue:    pick("blue",    Commons.Color.accent)
  readonly property color magenta: pick("magenta", Commons.Color.accent)

  readonly property color brightRed:     pick("bright_red",     red)
  readonly property color brightYellow:  pick("bright_yellow",  yellow)
  readonly property color brightGreen:   pick("bright_green",   green)
  readonly property color brightCyan:    pick("bright_cyan",    cyan)
  readonly property color brightBlue:    pick("bright_blue",    blue)
  readonly property color brightMagenta: pick("bright_magenta", magenta)

  // Neutrals, brightest to dimmest.
  readonly property color brightForeground: pick("bright_foreground", Commons.Color.foreground)
  readonly property color lightForeground:  pick("light_foreground",  Commons.Color.foreground)
  readonly property color dimForeground:    pick("dark_foreground",   Commons.Color.muted)
  readonly property color muted:            pick("muted",             Commons.Color.muted)
  readonly property color selection:        pick("selection",         Qt.rgba(Commons.Color.foreground.r, Commons.Color.foreground.g, Commons.Color.foreground.b, 0.12))

  // One colour per signed-in account, in account order (calendar dots, bars and agenda marks).
  readonly property var palette: [blue, magenta, cyan, green, orange, yellow]
  function accountColor(accounts, email) {
    for (var i = 0; i < accounts.length; i++)
      if (accounts[i].email === email) return palette[i % palette.length]
    return palette[0]
  }

  // Green -> yellow -> red, as btop's meters: 0..1 in, colour out.
  function ramp(t) {
    t = Math.max(0, Math.min(1, t))
    return t < 0.5 ? Qt.tint(green, Qt.rgba(yellow.r, yellow.g, yellow.b, t * 2))
                   : Qt.tint(yellow, Qt.rgba(red.r, red.g, red.b, (t - 0.5) * 2))
  }

  // Stable colour for a string (a sender, an account): same input, same hue, any theme.
  function hueFor(key) {
    var cycle = [blue, magenta, cyan, green, orange, yellow, red]
    var h = 0
    var s = String(key || "")
    for (var i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0
    return cycle[h % cycle.length]
  }

  property FileView file: FileView {
    path: root.path
    watchChanges: true
    printErrors: false
    onLoaded: root.parse(text())
    onFileChanged: reload()
  }

  // A theme switch replaces the theme directory, which a watch on the old file can miss; the
  // shell's own Color does change then, so re-read when it does.
  property Timer _rereadSoon: Timer { interval: 250; onTriggered: root.file.reload() }
  property Connections _themeWatch: Connections {
    target: Commons.Color
    function onAccentChanged() { root._rereadSoon.restart() }
    function onBackgroundChanged() { root._rereadSoon.restart() }
  }
}
