import QtQuick
import QtQuick.Layouts
import QtQuick.Controls
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"
import "format.js" as Fmt

// Message list pane. Model roles: from_name, from_email, subject,
// received_at, is_read.
Item {
  id: root

  PaneFrame { tint: Hues.blue; focused: root.paneFocused; hotkey: root.hotkey; title: root.frameTitle !== "" ? root.frameTitle : root.folderName }

  property var model
  property string folderName: "Inbox"
  property string status: ""
  property int currentIndex: 0
  property bool paneFocused: false
  property var marks: ({})
  property string frameTitle: ""
  property string hotkey: "\u00b2"

  // Rows that fit in one screenful (less one for overlap): the step PageUp/PageDown take.
  readonly property int pageRows: list.count > 0 && list.contentHeight > 0
    ? Math.max(1, Math.floor(list.height / (list.contentHeight / list.count)) - 1) : 1

  signal rowClicked(int index)
  signal refreshRequested()
  signal endReached()   // scrolled (or j/k'd) near the bottom: ask for the next page

  implicitWidth: Style.space(360)

  ColumnLayout {
    anchors.fill: parent; anchors.margins: Style.spacing.sm; anchors.topMargin: Style.spacing.xl
    spacing: 0

    // ---- header
    RowLayout {
      Layout.fillWidth: true
      Layout.margins: Style.spacing.lg
      spacing: Style.spacing.md

      UiText {
        text: "\uf0e0"
        foreground: root.paneFocused ? Commons.Color.accent : Commons.Color.foreground
        font.pixelSize: Style.font.icon
      }

      UiText {
        text: root.folderName
        Layout.fillWidth: true
        elide: Text.ElideRight
        foreground: root.paneFocused ? Commons.Color.accent : Commons.Color.foreground
        font.pixelSize: Style.font.subtitle
        font.bold: true
      }

      UiText {
        text: root.status
        dim: true
        font.pixelSize: Style.font.caption
      }

      PanelActionButton {
        iconText: "\uf021"
        tooltipText: "Refresh mail from server (F5)"
        onClicked: root.refreshRequested()
      }
    }

    PanelSeparator { Layout.fillWidth: true }

    // ---- list
    ListView {
      id: list
      Layout.fillWidth: true
      Layout.fillHeight: true
      clip: true
      boundsBehavior: Flickable.StopAtBounds
      ScrollBar.vertical: ThemedScrollBar {}
      model: root.model
      currentIndex: root.currentIndex
      onCurrentIndexChanged: {
        positionViewAtIndex(currentIndex, ListView.Contain)
        if (currentIndex >= count - 20) root.endReached()
      }
      onAtYEndChanged: if (atYEnd && count > 0) root.endReached()
      onContentYChanged: if (count > 0 && contentY + height > contentHeight - height * 2) root.endReached()

      UiText {
        anchors.centerIn: parent
        visible: list.count === 0
        text: root.status.length > 0 && root.status !== "…" ? root.status : "no messages"
        dim: true
      }

      delegate: BorderSurface {
        id: row
        width: list.width
        height: content.implicitHeight + Style.spacing.lg * 2

        readonly property bool hasCursor: root.paneFocused && root.currentIndex === index
        readonly property bool isCurrent: root.currentIndex === index
        readonly property bool unread: !model.is_read
        readonly property bool marked: root.marks[model.id] === true
        readonly property var cats: Fmt.categories(model)

        radius: Style.cornerRadius
        color: hasCursor ? Style.hoverFillFor(Commons.Color.foreground, Commons.Color.accent)
             : marked ? Style.selectedAccentFill
             : isCurrent ? Style.selectedFillFor(Commons.Color.foreground, Commons.Color.accent)
             : mouse.containsMouse ? Style.normalFillFor(Commons.Color.foreground, Commons.Color.accent)
             : "transparent"
        borderSpec: hasCursor ? Border.controlSpec("hover-cursor", Commons.Color.foreground, Commons.Color.accent) : Border.none()

        // unread strip
        Rectangle {
          visible: row.unread
          anchors.left: parent.left
          anchors.top: parent.top
          anchors.bottom: parent.bottom
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

          UiText {
            visible: row.marked
            text: "\u2713"
            foreground: Hues.yellow
            font.bold: true
          }

          ColumnLayout {
            Layout.fillWidth: true
            spacing: Style.spacing.xxs

            RowLayout {
              Layout.fillWidth: true
              spacing: Style.spacing.md

              UiText {
                text: Fmt.senderName(model)
                Layout.fillWidth: true
                elide: Text.ElideRight
                foreground: row.unread ? Hues.brightForeground : Hues.lightForeground
                dim: !row.unread
                font.bold: row.unread
              }

              UiText {
                visible: model.importance === "high"
                text: "!"
                foreground: Hues.red
                font.bold: true
                font.pixelSize: Style.font.caption
              }

              UiText {
                visible: model.has_attachments === true
                text: "\uf0c6"
                foreground: Hues.cyan
                font.pixelSize: Style.font.caption
              }

              UiText {
                text: Fmt.listDate(model.received_at)
                foreground: row.unread ? Hues.blue : Hues.muted
                font.pixelSize: Style.font.caption
              }
            }

            UiText {
              text: model.subject || "(no subject)"
              Layout.fillWidth: true
              elide: Text.ElideRight
              foreground: Hues.lightForeground
              dim: !row.unread
              font.pixelSize: Style.font.bodySmall
            }

            // Categories / tags, only when the message has any.
            Row {
              visible: row.cats.length > 0
              spacing: Style.spacing.sm
              Repeater {
                model: row.cats.slice(0, 4)
                CategoryChip {
                  required property var modelData
                  label: modelData[0]
                  tint: modelData[1]
                  maxTextWidth: Style.space(110)
                }
              }
              UiText {
                visible: row.cats.length > 4
                text: "+" + (row.cats.length - 4)
                dim: true
                font.pixelSize: Style.font.caption
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
