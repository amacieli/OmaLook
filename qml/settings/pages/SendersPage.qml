import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../../common"
import ".."

// Senders: email addresses with two display preferences each. Add an address,
// then switch "Always HTML" / "Always load images" per sender. The preferences
// are stored in the backend (sender_prefs); the reading pane does not use them yet.
SettingsPage {
  id: page

  property var app: null

  editing: addField.activeFocus

  title: "Senders"
  description: "Per-sender display preferences. They are saved now; the reading pane does not apply them yet."

  readonly property int switchColumn: Style.space(150)

  Component.onCompleted: if (page.app) page.app.loadSenders()

  function submitAdd() {
    if (!page.app) return
    page.app.addSender(addField.text, function(added) { if (added) addField.text = "" })
  }

  Column {
    width: parent.width
    spacing: Style.space(6)

    PanelSectionHeader { text: "ADD SENDER" }

    RowLayout {
      width: parent.width
      spacing: Style.spacing.lg

      TextField {
        id: addField
        Layout.fillWidth: true
        placeholderText: "name@example.com"
        onAccepted: page.submitAdd()
        onTextChanged: if (page.app && page.app.senderNotice !== "") page.app.senderNotice = ""
      }

      Button {
        Layout.alignment: Qt.AlignVCenter
        bordered: true
        iconText: "\uf067"
        text: "Add"
        onClicked: page.submitAdd()
      }
    }

    UiText {
      visible: !!page.app && page.app.senderNotice !== ""
      width: parent.width
      text: page.app ? page.app.senderNotice : ""
      foreground: Commons.Color.urgent
      wrapMode: Text.WordWrap
    }
  }

  PanelSeparator { width: parent.width }

  Column {
    width: parent.width
    spacing: Style.space(4)

    // Column captions, aligned with the switches below.
    RowLayout {
      width: parent.width
      spacing: Style.spacing.lg

      PanelSectionHeader { Layout.fillWidth: true; text: "SENDERS" }
      UiText {
        Layout.preferredWidth: page.switchColumn
        horizontalAlignment: Text.AlignHCenter
        text: "Always HTML"
        dim: true
        font.pixelSize: Style.font.caption
        font.bold: true
      }
      UiText {
        Layout.preferredWidth: page.switchColumn
        horizontalAlignment: Text.AlignHCenter
        text: "Always load images"
        dim: true
        font.pixelSize: Style.font.caption
        font.bold: true
      }
      // Same width as the Remove button in each row.
      Item { Layout.preferredWidth: Style.space(96) }
    }

    UiText {
      visible: !page.app || page.app.senderModel.count === 0
      width: parent.width
      text: "No senders yet. Add an address above."
      dim: true
    }

    Repeater {
      model: page.app ? page.app.senderModel : null

      delegate: RowLayout {
        id: row
        required property string email
        required property bool always_html
        required property bool always_images
        width: parent.width
        spacing: Style.spacing.lg

        UiText {
          Layout.fillWidth: true
          text: row.email
          elide: Text.ElideRight
        }

        Item {
          Layout.preferredWidth: page.switchColumn
          Layout.preferredHeight: htmlSwitch.implicitHeight
          ToggleSwitch {
            id: htmlSwitch
            anchors.centerIn: parent
            checked: row.always_html
            onToggled: page.app.setSenderPref(row.email, "html", !row.always_html)
          }
        }

        Item {
          Layout.preferredWidth: page.switchColumn
          Layout.preferredHeight: imageSwitch.implicitHeight
          ToggleSwitch {
            id: imageSwitch
            anchors.centerIn: parent
            checked: row.always_images
            onToggled: page.app.setSenderPref(row.email, "images", !row.always_images)
          }
        }

        // Two-step, like removing an account.
        Button {
          id: removeButton
          property bool confirming: false
          Layout.preferredWidth: Style.space(96)
          bordered: true
          iconText: "\uf1f8"
          text: confirming ? "Sure?" : "Remove"
          foreground: confirming ? Commons.Color.urgent : Commons.Color.foreground
          onClicked: {
            if (confirming) { confirming = false; page.app.removeSender(row.email) }
            else confirming = true
          }
          Timer {
            interval: 4000
            running: removeButton.confirming
            onTriggered: removeButton.confirming = false
          }
        }
      }
    }
  }
}
