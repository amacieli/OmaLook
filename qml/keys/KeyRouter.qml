import QtQuick
import "Commands.js" as Cmd

// Turns key presses into command lookups. Owns the chord buffer (`g` then `m`).
// It does not know what any command does: the registry (AppCommands) does.
//
// Keys.priority: BeforeItem so arrows reach us before an inner Flickable eats them.
// While `blocked` every key goes to descendants (text fields, modals) untouched.
Item {
  id: root

  property bool blocked: false
  property var commands: []
  // Innermost first. Changing them mid-chord cancels it.
  property var scopes: ["global"]

  // Called first with every key press (even while a dialog is up); return true to consume it.
  property var interceptor: null

  property var pending: []
  // Vim-style count typed between Ctrl-w and its key (`Ctrl-w 5 >`); handed to the command's run().
  property int count: 0
  readonly property string pendingText: pending.join(" ") + (count > 0 ? " " + count : "")
  readonly property var pendingOptions: pending.length > 0 ? Cmd.resolve(commands, scopes, pending).prefixes : []

  focus: true
  Keys.priority: Keys.BeforeItem

  onPendingChanged: if (pending.length === 0) count = 0
  onScopesChanged: pending = []
  onBlockedChanged: pending = []

  Timer { id: chordTimer; interval: 2000; onTriggered: root.pending = [] }

  function token(e) {
    var m = e.modifiers
    if (m & (Qt.AltModifier | Qt.MetaModifier)) return ""
    if (m & Qt.ControlModifier) {
      var prefix = (m & Qt.ShiftModifier) ? "C-S-" : "C-"
      if (e.key >= Qt.Key_A && e.key <= Qt.Key_Z) return prefix + String.fromCharCode(e.key + 32)
      if (e.key >= Qt.Key_0 && e.key <= Qt.Key_9) return prefix + String.fromCharCode(e.key)
      if (e.key === Qt.Key_Left)  return prefix + "Left"
      if (e.key === Qt.Key_Right) return prefix + "Right"
      return ""
    }
    if (e.key >= Qt.Key_F1 && e.key <= Qt.Key_F12) return "F" + (e.key - Qt.Key_F1 + 1)
    switch (e.key) {
      case Qt.Key_Escape:    return "Esc"
      case Qt.Key_Tab:       return "Tab"
      case Qt.Key_Backtab:   return "S-Tab"
      case Qt.Key_Return:
      case Qt.Key_Enter:     return "Enter"
      case Qt.Key_Space:     return "Space"
      case Qt.Key_Up:        return "Up"
      case Qt.Key_Down:      return "Down"
      case Qt.Key_Left:      return "Left"
      case Qt.Key_Right:     return "Right"
      case Qt.Key_PageUp:    return "PgUp"
      case Qt.Key_PageDown:  return "PgDown"
      case Qt.Key_Home:      return "Home"
      case Qt.Key_End:       return "End"
      case Qt.Key_Delete:    return "Delete"
      case Qt.Key_Backspace: return "Backspace"
    }
    return (e.text.length === 1 && e.text > " ") ? e.text : ""
  }

  Keys.onPressed: function(e) {
    if (interceptor && interceptor(e) === true) { e.accepted = true; return }
    if (blocked) return
    var t = token(e)
    if (t === "") return

    // A count is only taken right after the Ctrl-w prefix; a leading 0 is not part of one.
    if (pending.length === 1 && pending[0] === "C-w" && t.length === 1 && t >= "0" && t <= "9" && (count > 0 || t !== "0")) {
      count = Math.min(999, count * 10 + Number(t))
      chordTimer.restart()
      e.accepted = true
      return
    }
    var seq = pending.concat([t])
    var r = Cmd.resolve(commands, scopes, seq)
    if (r.exact) {
      var n = count
      pending = []; chordTimer.stop()
      e.accepted = true
      r.exact.run(n > 0 ? n : undefined)
    } else if (r.prefixes.length > 0) {
      pending = seq; chordTimer.restart()
      e.accepted = true
    } else if (pending.length > 0) {
      // A chord that goes nowhere (including Esc) just cancels.
      pending = []; chordTimer.stop()
      e.accepted = true
    }
  }
}
