import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import "../common"

// After a pause on a half-typed chord ("g"), lists what can follow it. Quick typists
// never see it; the bottom status bar still carries the short form.
Item {
  id: root

  property string pendingText: ""
  property var options: []        // [{ cmd, next }] from KeyRouter.pendingOptions
  property bool shown: false

  visible: shown && options.length > 0
  implicitWidth: card.implicitWidth
  implicitHeight: card.implicitHeight

  onPendingTextChanged: { shown = false; if (pendingText !== "") delay.restart(); else delay.stop() }
  Timer { id: delay; interval: 400; onTriggered: root.shown = true }

  Rectangle {
    id: card
    anchors.fill: parent
    implicitWidth: col.implicitWidth + Style.spacing.xl * 2
    implicitHeight: col.implicitHeight + Style.spacing.lg * 2
    color: Commons.Color.background
    border.width: 2
    border.color: Commons.Color.accent

    Column {
      id: col
      anchors.centerIn: parent
      spacing: Style.spacing.xs

      UiText {
        text: root.pendingText + " …"
        foreground: Commons.Color.accent
        font.bold: true
      }

      Repeater {
        model: root.options
        Row {
          required property var modelData
          spacing: Style.spacing.lg
          UiText { width: Style.space(28); text: modelData.next; foreground: Commons.Color.accent; font.bold: true }
          UiText { text: modelData.cmd.title }
        }
      }
    }
  }
}
