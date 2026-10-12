import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Ui
import "../common"

// Folder list for the mail view. Model roles: id, display_name, unread_item_count.
Item {
  id: root

  PaneFrame { tint: Hues.cyan; focused: root.paneFocused; hotkey: "\u00b9"; title: "folders" }

  property var model
  property int currentIndex: 0
  property string selectedId: ""
  property bool paneFocused: false

  // With more than one account, folders are grouped under the account's email address
  // (section headers do not change model indices, so the keyboard cursor is unaffected).
  property bool multiAccount: false
  function recount() {
    var seen = {}, n = 0
    for (var i = 0; model && i < model.count; i++) {
      var e = model.get(i).account_email || ""
      if (!seen[e]) { seen[e] = true; n++ }
    }
    multiAccount = n > 1
  }
  onModelChanged: recount()
  Connections {
    target: root.model
    ignoreUnknownSignals: true
    function onCountChanged() { root.recount() }
  }

  // Rows that fit in one screenful (less one for overlap): the step PageUp/PageDown take.
  readonly property int pageRows: list.count > 0 && list.contentHeight > 0
    ? Math.max(1, Math.floor(list.height / (list.contentHeight / list.count)) - 1) : 1

  signal folderClicked(int index)
  signal refreshRequested()

  implicitWidth: Style.space(180)
  clip: true

  ColumnLayout {
    anchors.fill: parent; anchors.margins: Style.spacing.md; anchors.topMargin: Style.spacing.xl
    spacing: Style.spacing.sm

    RowLayout {
      Layout.fillWidth: true
      Layout.leftMargin: Style.spacing.sm

      PanelSectionHeader {
        text: "FOLDERS"
        Layout.fillWidth: true
      }

      PanelActionButton {
        iconText: "\uf021"
        tooltipText: "Refresh folders"
        onClicked: root.refreshRequested()
      }
    }

    ListView {
      id: list
      Layout.fillWidth: true
      Layout.fillHeight: true
      model: root.model
      clip: true
      interactive: false          // keyboard-driven; mouse via delegates
      spacing: Style.spacing.xxs
      currentIndex: root.currentIndex
      onCurrentIndexChanged: positionViewAtIndex(currentIndex, ListView.Contain)

      section.property: root.multiAccount ? "account_email" : ""
      section.criteria: ViewSection.FullString
      section.delegate: UiText {
        width: list.width
        topPadding: Style.spacing.sm
        text: section
        foreground: Hues.muted
        elide: Text.ElideRight
        font.pixelSize: Style.font.caption
      }

      delegate: Item {
        id: rowItem
        width: list.width
        height: folderButton.implicitHeight

        Button {
          id: folderButton
          anchors.fill: parent
          leftAlign: true
          text: model.display_name
          selected: model.id === root.selectedId
          hasCursor: root.paneFocused && root.currentIndex === index
          onClicked: root.folderClicked(index)
        }

        UiText {
          visible: model.unread_item_count > 0
          anchors.right: parent.right
          anchors.rightMargin: Style.spacing.controlPaddingX
          anchors.verticalCenter: parent.verticalCenter
          text: model.unread_item_count > 99 ? "99+" : String(model.unread_item_count)
          foreground: Hues.blue
          font.bold: true
          font.pixelSize: Style.font.caption
        }
      }
    }
  }
}
