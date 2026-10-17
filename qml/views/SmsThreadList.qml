import QtQuick
import QtQuick.Layouts
import QtQuick.Controls
import qs.Commons
import qs.Ui
import "../common"
import "sms.js" as Sms

// Senders list: one row per phone conversation, newest first. Model roles: title, last_body,
// last_direction, last_date_ms, unread_count, message_count.
Item {
  id: root

  property var model
  property int currentIndex: 0
  property bool paneFocused: false
  property string status: ""
  property int unreadTotal: 0
  property string searchText: ""      // active search: the list shows only matching conversations
  property string hotkey: "\u00b9"

  readonly property int pageRows: list.count > 0 && list.contentHeight > 0
    ? Math.max(1, Math.floor(list.height / (list.contentHeight / list.count)) - 1) : 1

  signal rowClicked(int index)
  signal refreshRequested()

  implicitWidth: Style.space(360)

  PaneFrame { tint: Hues.blue; focused: root.paneFocused; hotkey: root.hotkey; title: "Messages" }

  ColumnLayout {
    anchors.fill: parent; anchors.margins: Style.spacing.sm; anchors.topMargin: Style.spacing.xl
    spacing: 0

    RowLayout {
      Layout.fillWidth: true
      Layout.margins: Style.spacing.lg
      spacing: Style.spacing.md

      UiText {
        text: "\uf27a"
        foreground: root.paneFocused ? Color.accent : Color.foreground
        font.pixelSize: Style.font.icon
      }
      UiText {
        text: "SMS"
        Layout.fillWidth: true
        elide: Text.ElideRight
        foreground: root.paneFocused ? Color.accent : Color.foreground
        font.pixelSize: Style.font.subtitle
        font.bold: true
      }
      UiText {
        text: root.unreadTotal > 0 ? root.unreadTotal + " unread" : ""
        foreground: Hues.blue
        font.pixelSize: Style.font.caption
      }
      PanelActionButton {
        iconText: "\uf021"
        tooltipText: "Reload"
        onClicked: root.refreshRequested()
      }
    }

    // Active search: what is being matched, and how to leave it.
    RowLayout {
      visible: root.searchText !== ""
      Layout.fillWidth: true
      Layout.leftMargin: Style.spacing.lg; Layout.rightMargin: Style.spacing.lg; Layout.bottomMargin: Style.spacing.sm
      spacing: Style.spacing.md
      UiText { text: "\uf002"; foreground: Hues.yellow; font.pixelSize: Style.font.bodySmall }
      UiText {
        text: root.searchText
        Layout.fillWidth: true
        elide: Text.ElideRight
        foreground: Hues.yellow
        font.pixelSize: Style.font.bodySmall
      }
      UiText { text: list.count + (list.count === 1 ? " match" : " matches") + " \u00b7 Esc clears"; dim: true; font.pixelSize: Style.font.caption }
    }

    PanelSeparator { Layout.fillWidth: true }

    ListView {
      id: list
      Layout.fillWidth: true
      Layout.fillHeight: true
      clip: true
      boundsBehavior: Flickable.StopAtBounds
      ScrollBar.vertical: ThemedScrollBar {}
      model: root.model
      currentIndex: root.currentIndex
      onCurrentIndexChanged: positionViewAtIndex(currentIndex, ListView.Contain)

      UiText {
        anchors.centerIn: parent
        width: parent.width - Style.spacing.xl * 2
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.WordWrap
        visible: list.count === 0
        text: root.status !== "" ? root.status : (root.searchText !== "" ? "nothing matches" : "no conversations")
        dim: true
      }

      delegate: BorderSurface {
        id: row
        width: list.width
        height: content.implicitHeight + Style.spacing.lg * 2

        readonly property bool hasCursor: root.paneFocused && root.currentIndex === index
        readonly property bool isCurrent: root.currentIndex === index
        readonly property bool unread: model.unread_count > 0

        radius: Style.cornerRadius
        color: hasCursor ? Style.hoverFillFor(Color.foreground, Color.accent)
             : isCurrent ? Style.selectedFillFor(Color.foreground, Color.accent)
             : mouse.containsMouse ? Style.normalFillFor(Color.foreground, Color.accent)
             : "transparent"
        borderSpec: hasCursor ? Border.controlSpec("hover-cursor", Color.foreground, Color.accent) : Border.none()

        Rectangle {
          visible: row.unread
          anchors.left: parent.left; anchors.top: parent.top; anchors.bottom: parent.bottom
          width: Style.space(2)
          color: Hues.blue
        }

        RowLayout {
          id: content
          anchors.fill: parent
          anchors.leftMargin: Style.spacing.xl
          anchors.rightMargin: Style.spacing.lg
          anchors.topMargin: Style.spacing.lg
          anchors.bottomMargin: Style.spacing.lg
          spacing: Style.spacing.md

          ColumnLayout {
            Layout.fillWidth: true
            spacing: Style.spacing.xxs

            RowLayout {
              Layout.fillWidth: true
              spacing: Style.spacing.md
              UiText {
                text: model.title
                Layout.fillWidth: true
                elide: Text.ElideRight
                foreground: row.unread ? Hues.brightForeground : Hues.lightForeground
                dim: !row.unread
                font.bold: row.unread
              }
              UiText {
                text: Sms.listDate(model.last_date_ms)
                foreground: row.unread ? Hues.blue : Hues.muted
                font.pixelSize: Style.font.caption
              }
            }

            RowLayout {
              Layout.fillWidth: true
              spacing: Style.spacing.md
              UiText {
                text: model.snippet !== "" ? model.snippet : Sms.preview(model)
                Layout.fillWidth: true
                elide: Text.ElideRight
                foreground: model.last_direction === 5 && model.snippet === "" ? Hues.red : Hues.lightForeground
                dim: !row.unread && !(model.last_direction === 5 && model.snippet === "")
                font.pixelSize: Style.font.bodySmall
              }
              UiText {
                visible: model.match !== ""
                text: model.match
                foreground: Hues.yellow
                font.pixelSize: Style.font.caption
              }
              Rectangle {
                visible: row.unread
                Layout.preferredWidth: Math.max(height, badge.implicitWidth + Style.spacing.md * 2)
                Layout.preferredHeight: badge.implicitHeight + Style.spacing.xs
                radius: height / 2
                color: Hues.blue
                UiText {
                  id: badge
                  anchors.centerIn: parent
                  text: model.unread_count
                  foreground: "#000000"
                  font.pixelSize: Style.font.caption
                  font.bold: true
                }
              }
            }
          }
        }

        MouseArea {
          id: mouse
          anchors.fill: parent
          hoverEnabled: true
          cursorShape: Qt.PointingHandCursor
          onClicked: root.rowClicked(index)
        }
      }
    }
  }
}
