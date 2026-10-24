import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"

// Progress of messages that were sent from the compose pane: a small pill per message in the
// corner of the window. It never takes the keyboard or covers the lists, so the app stays fully
// usable while a message waits out its delay. Undo (or `u`) takes the message back into the editor.
Column {
  id: root

  property var app: null

  spacing: Style.spacing.md
  visible: !!app && app.sends.length > 0

  Repeater {
    model: root.app ? root.app.sends : []

    delegate: Rectangle {
      id: pill
      required property var modelData
      readonly property bool pending: modelData.state === "pending"
      readonly property bool failed: modelData.state === "failed"
      readonly property int secondsLeft: Math.max(0, Math.ceil((modelData.sendAt - root.app.sendNow) / 1000))
      readonly property real fraction: modelData.delaySecs > 0
        ? Math.max(0, Math.min(1, (modelData.sendAt - root.app.sendNow) / (modelData.delaySecs * 1000))) : 0

      // Solid, not translucent: it floats over the lists.
      color: Qt.rgba(Commons.Color.popups.background.r, Commons.Color.popups.background.g, Commons.Color.popups.background.b, 1)
      border.width: Style.normalBorderWidth
      border.color: failed ? Commons.Color.urgent : Commons.Color.popups.border
      radius: Style.cornerRadius
      implicitWidth: row.implicitWidth + Style.spacing.lg * 2
      implicitHeight: row.implicitHeight + Style.spacing.md * 2 + bar.height
      width: Math.min(implicitWidth, Style.space(460))

      RowLayout {
        id: row
        x: Style.spacing.lg
        y: Style.spacing.md
        width: pill.width - Style.spacing.lg * 2
        spacing: Style.spacing.lg

        UiText {
          Layout.fillWidth: true
          elide: Text.ElideRight
          foreground: pill.failed ? Commons.Color.urgent : Commons.Color.popups.text
          font.pixelSize: Style.font.caption
          text: pill.failed ? "Not sent: " + pill.modelData.error
            : pill.modelData.state === "sent" ? "\uf00c  Sent"
            : pill.pending && pill.secondsLeft > 0 ? "Sending in " + pill.secondsLeft + "s"
            : "Sending\u2026"
        }

        Button {
          visible: pill.pending && pill.secondsLeft > 0
          enabled: !root.app.composing
          opacity: enabled ? 1 : 0.4
          bordered: true
          text: "Undo"
          fontSize: Style.font.caption
          tooltipText: root.app.composing ? "Finish or discard the open message first" : "Take it back and keep editing (u)"
          verticalPadding: Style.spacing.xs
          onClicked: root.app.undoSend(pill.modelData.id)
        }
        Button {
          visible: pill.failed
          enabled: !root.app.composing
          opacity: enabled ? 1 : 0.4
          bordered: true
          text: "Edit"
          fontSize: Style.font.caption
          tooltipText: "Reopen the message to fix and send again"
          verticalPadding: Style.spacing.xs
          onClicked: root.app.reopenFailedSend(pill.modelData.id)
        }
        Button {
          visible: pill.failed
          bordered: false
          iconText: "\uf00d"
          tooltipText: "Dismiss"
          verticalPadding: Style.spacing.xs
          onClicked: root.app.dismissSend(pill.modelData.id)
        }
      }

      // Time left in the delay, as a thin line along the bottom edge.
      Rectangle {
        id: bar
        visible: pill.pending && pill.secondsLeft > 0
        height: visible ? 2 : 0
        anchors.left: parent.left
        anchors.bottom: parent.bottom
        anchors.leftMargin: pill.radius
        width: (pill.width - pill.radius * 2) * pill.fraction
        color: Commons.Color.accent
      }
    }
  }
}
