import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"

// "Archived 3 messages   Undo (4s)": the window in which an archive / delete / move can still
// be taken back (`u`). It never takes the keyboard.
Rectangle {
  id: root

  property var app: null
  readonly property var action: app ? app.lastAction : null
  readonly property int secondsLeft: action ? Math.max(0, Math.ceil((action.until - app.actionNow) / 1000)) : 0

  visible: action !== null
  color: Qt.rgba(Commons.Color.popups.background.r, Commons.Color.popups.background.g, Commons.Color.popups.background.b, 1)
  border.width: Style.normalBorderWidth
  border.color: Commons.Color.popups.border
  radius: Style.cornerRadius
  implicitWidth: row.implicitWidth + Style.spacing.lg * 2
  implicitHeight: row.implicitHeight + Style.spacing.md * 2
  width: Math.min(implicitWidth, Style.space(460))

  RowLayout {
    id: row
    x: Style.spacing.lg
    y: Style.spacing.md
    width: root.width - Style.spacing.lg * 2
    spacing: Style.spacing.lg

    UiText {
      Layout.fillWidth: true
      elide: Text.ElideRight
      foreground: Commons.Color.popups.text
      font.pixelSize: Style.font.caption
      text: (root.action ? root.action.label : "") + "   u undo · " + root.secondsLeft + "s"
    }

    Button {
      bordered: true
      text: "Undo"
      onClicked: { root.app.undoAction(); root.app.focusRequested() }
    }
  }
}
