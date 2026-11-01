import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"
import "../mail/format.js" as Fmt
import "attach.js" as Att

// The attachment list panel (docs/PLAN-compose-attachments.md 3.5): a bounded popup that hangs
// under the attachment strip, at most maxRows rows tall, scrolling. Same look as the file browser.
//
//   j / Down, k / Up   move          x / Delete   remove the highlighted file
//   a                  add more (opens the file browser)        Esc   back to the editor
FocusScope {
  id: root

  readonly property int maxRows: 6

  property bool opened: false
  // Same rows as the strip: { name, size, origin, ref }
  property var items: []
  property var limits: Att.defaultLimits("")
  property int cur: 0

  signal removeRequested(int row)
  signal addRequested()
  signal closed()

  readonly property var verdict: Att.states(limits, items)
  readonly property var names: Att.displayNames(items)
  readonly property real rowHeight: Style.spacing.popupRowHeight

  visible: opened
  height: card.height

  function open() {
    cur = Math.min(cur, Math.max(items.length - 1, 0))
    opened = true
    forceActiveFocus()
  }
  function close() {
    opened = false
    closed()
  }

  onItemsChanged: {
    if (items.length === 0) { if (opened) close(); return }
    cur = Math.min(cur, items.length - 1)
  }

  function _move(d) {
    if (items.length === 0) return
    cur = Math.min(Math.max(cur + d, 0), items.length - 1)
    listView.positionViewAtIndex(cur, ListView.Contain)
  }

  Keys.onPressed: function(e) {
    if (!opened) return
    var ctrl = e.modifiers & Qt.ControlModifier
    if (e.key === Qt.Key_Escape) { close(); e.accepted = true }
    else if (e.key === Qt.Key_Down || (!ctrl && e.key === Qt.Key_J)) { _move(1); e.accepted = true }
    else if (e.key === Qt.Key_Up || (!ctrl && e.key === Qt.Key_K)) { _move(-1); e.accepted = true }
    else if (e.key === Qt.Key_PageDown) { _move(root.maxRows); e.accepted = true }
    else if (e.key === Qt.Key_PageUp) { _move(-root.maxRows); e.accepted = true }
    else if (e.key === Qt.Key_Home) { _move(-items.length); e.accepted = true }
    else if (e.key === Qt.Key_End) { _move(items.length); e.accepted = true }
    else if (e.key === Qt.Key_Delete || e.key === Qt.Key_Backspace || (!ctrl && e.key === Qt.Key_X)) { if (items.length > 0) removeRequested(cur); e.accepted = true }
    else if (!ctrl && e.key === Qt.Key_A) { addRequested(); e.accepted = true }
    // While the list is open nothing may reach the compose pane's own shortcuts (Ctrl+Enter would send).
    else e.accepted = true
  }

  Rectangle {
    id: card
    width: parent.width
    height: Style.spacing.lg * 2 + head.height + Math.min(root.items.length, root.maxRows) * root.rowHeight + foot.height + Style.spacing.md * 2
    radius: Style.cornerRadius
    color: Qt.rgba(Commons.Color.popups.background.r, Commons.Color.popups.background.g, Commons.Color.popups.background.b, 1)
    border.width: Style.normalBorderWidth
    border.color: Commons.Color.popups.border

    MouseArea { anchors.fill: parent }

    ColumnLayout {
      anchors.fill: parent
      anchors.margins: Style.spacing.lg
      spacing: Style.spacing.md

      UiText {
        id: head
        Layout.fillWidth: true
        text: "Attachments (" + root.items.length + ")"
        font.bold: true
      }

      ListView {
        id: listView
        Layout.fillWidth: true
        Layout.preferredHeight: Math.min(root.items.length, root.maxRows) * root.rowHeight
        clip: true
        model: root.items
        currentIndex: root.cur
        boundsBehavior: Flickable.StopAtBounds

        delegate: Rectangle {
          id: row
          required property var modelData
          required property int index
          readonly property bool big: root.verdict.state[index] === "big"
          width: listView.width
          height: root.rowHeight
          radius: Style.cornerRadius
          color: index === root.cur ? Style.selectedFill : "transparent"

          RowLayout {
            anchors.fill: parent
            anchors.leftMargin: Style.spacing.md
            anchors.rightMargin: Style.spacing.md
            spacing: Style.spacing.md
            UiText { Layout.preferredWidth: Style.space(22); text: String(row.index + 1); dim: true; font.pixelSize: Style.font.caption; horizontalAlignment: Text.AlignRight }
            UiText { Layout.preferredWidth: Style.space(18); text: row.modelData.origin === "original" ? "\uf064" : "\uf0c6"; dim: true }
            UiText {
              Layout.fillWidth: true
              Layout.minimumWidth: 0
              text: root.names[row.index] || ""
              elide: Text.ElideMiddle
              foreground: row.big ? Commons.Color.urgent : Commons.Color.popups.text
            }
            UiText { text: row.modelData.origin === "original" ? "forwarded" : ""; dim: true; font.pixelSize: Style.font.caption }
            UiText { Layout.preferredWidth: Style.space(16); text: row.big ? "!" : ""; foreground: Commons.Color.urgent; font.bold: true }
            UiText { text: Fmt.bytes(row.modelData.size); dim: true; font.pixelSize: Style.font.caption }
          }
          MouseArea {
            anchors.fill: parent
            onClicked: { root.cur = row.index; root.forceActiveFocus() }
          }
        }
      }

      UiText {
        id: foot
        Layout.fillWidth: true
        elide: Text.ElideRight
        font.pixelSize: Style.font.caption
        readonly property string why: root.verdict.reason[root.cur] || ""
        text: why !== "" ? why : "j/k move \u00b7 x remove \u00b7 a add more \u00b7 Esc close"
        foreground: why !== "" ? Commons.Color.urgent : Commons.Color.foreground
        dim: why === ""
      }
    }
  }
}
