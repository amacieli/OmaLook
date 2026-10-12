import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Ui
import "../common"

// Context (where am I), counts, and the key hints for the focused pane.
Item {
  id: root

  property bool backendOnline: false
  property bool isAuthenticated: true
  property string viewLabel: ""
  property string folderName: ""
  property int messageCount: 0
  property int unreadCount: 0
  property string searchQuery: ""   // active search pattern; replaces the folder counts
  property int searchCount: 0
  property bool searchMore: false
  property bool searchPartial: false   // provider search was capped: only the newest matches
  property string focusPane: "msg"
  property string currentView: "mail"

  function fmt(n) { return Number(n).toLocaleString(Qt.locale("en_US"), "f", 0) }

  property string pendingText: ""
  property var pendingOptions: []
  property string notice: ""
  property int markCount: 0
  property bool metaActive: false
  property int metaRemaining: 0
  // keyOf(commandId) -> the key currently bound to it, so hints follow rebinding.
  property var keyOf: function(id) { return "" }
  function k(id, label) { var key = keyOf(id); return key === "" ? "" : key + " " + label }
  function hintLine(parts) { return parts.filter(function(p) { return p !== "" }).join("   ") }

  // Hints for the focused pane. ":" always opens the full command list.
  readonly property string hints: {
    if (focusPane === "folder") return hintLine([k("move.down", "move"), k("move.right", "select"), k("move.left", "back"), k("pane.next", "next pane")])
    if (focusPane === "reader") return hintLine([k("move.down", "scroll"), k("reader.pagedown", "page"), k("mail.reply", "reply"), k("move.left", "back")])
    if (currentView === "settings") return hintLine([k("move.down", "category"), k("move.left", "back"), k("pane.next", "next pane")])
    if (currentView === "mail") return hintLine([k("move.down", "move"), k("move.right", "read"), k("mail.reply", "reply"), k("mail.archive", "archive"), k("item.delete", "delete"), k("mail.move", "move"), k("mail.mark", "mark"), k("mail.read", "read/unread"), k("app.search", "search")])
    if (currentView === "calendar") return hintLine([k("cal.next", "period"), k("cal.today", "today"), k("cal.month", "month"), k("pane.next", "next pane")])
    if (currentView === "tasks") return hintLine([k("move.down", "move"), k("item.activate", "done"), k("tasks.filter", "filter"), k("pane.next", "next pane")])
    return hintLine([k("move.down", "move"), k("move.left", "back"), k("pane.next", "next pane")])
  }

  // While a chord is half typed ("g"), show what can follow instead of the hints.
  readonly property string pendingHelp: {
    var t = pendingText + " …   "
    for (var i = 0; i < pendingOptions.length; i++) {
      var o = pendingOptions[i]
      t += o.next + " " + o.cmd.title.replace(/^Go to /, "").toLowerCase() + "   "
    }
    return t
  }

  implicitHeight: row.implicitHeight + Style.spacing.md * 2

  RowLayout {
    id: row
    anchors.fill: parent
    anchors.leftMargin: Style.spacing.xl
    anchors.rightMargin: Style.spacing.xl
    spacing: Style.spacing.xxl

    UiText {
      text: root.currentView === "mail" ? root.viewLabel + " › " + root.folderName : root.viewLabel
      foreground: Hues.brightForeground
      font.pixelSize: Style.font.caption
      font.bold: true
    }

    UiText {
      visible: root.currentView === "mail" && root.markCount > 0
      text: root.markCount + " marked"
      foreground: Hues.yellow
      font.bold: true
      font.pixelSize: Style.font.caption
    }

    UiText {
      visible: root.metaActive
      text: "filling in message details \u00b7 " + root.fmt(root.metaRemaining) + " to go"
      dim: true
      font.pixelSize: Style.font.caption
    }

    UiText {
      visible: root.currentView === "mail" && root.searchQuery !== ""
      text: "\uf002 " + root.searchQuery + "  \u00b7  " + root.fmt(root.searchCount) + (root.searchMore ? "+" : "") + (root.searchCount === 1 ? " match" : " matches") + (root.searchPartial ? " (newest only)" : "") + "  \u00b7  Esc clears"
      foreground: Hues.cyan
      font.bold: true
      font.pixelSize: Style.font.caption
      elide: Text.ElideRight
    }

    UiText {
      visible: root.currentView === "mail" && root.searchQuery === ""
      text: root.fmt(root.messageCount) + (root.messageCount === 1 ? " message · " : " messages · ") + root.fmt(root.unreadCount) + " unread"
      dim: true
      font.pixelSize: Style.font.caption
    }

    Item { Layout.fillWidth: true }

    UiText {
      text: root.pendingText !== "" ? root.pendingHelp : (root.notice !== "" ? root.notice : root.hints)
      dim: root.pendingText === "" && root.notice === ""
      foreground: root.pendingText !== "" ? Hues.cyan : (root.notice !== "" ? Hues.yellow : Color.foreground)
      font.pixelSize: Style.font.caption
    }

    UiText {
      text: "  " + (root.keyOf("palette") || ":") + "  commands   " + (root.keyOf("help") || "?") + "  help   " + (root.keyOf("go.mail") || "g m").split(" ")[0] + "  go to"
      color: Hues.cyan
      font.pixelSize: Style.font.caption
      font.bold: true
    }

    UiText {
      visible: !root.isAuthenticated
      text: "signed out (" + (root.keyOf("go.accounts") || "g a") + " accounts)"
      color: Hues.red
      font.pixelSize: Style.font.caption
      font.bold: true
    }

    UiText {
      text: root.backendOnline ? "online" : "offline"
      color: root.backendOnline ? Hues.green : Hues.red
      font.pixelSize: Style.font.caption
      font.bold: true
    }
  }
}
