import QtQuick
import qs.Commons
import qs.Ui
import "../../common"
import ".."

// Placeholder: none of these values are persisted yet.
SettingsPage {
  id: page

  property bool desktop: true
  property bool sound: false
  property string scope: "inbox"

  title: "Notifications"
  description: "How OmaLook tells you about new mail and upcoming events."
  placeholder: true

  Toggle {
    width: parent.width
    label: "Desktop notifications"
    description: page.desktop ? "Shown through the Omarchy notification daemon" : "Disabled"
    checked: page.desktop
    onClicked: page.desktop = !page.desktop
  }

  Toggle {
    width: parent.width
    label: "Sound"
    description: page.sound ? "Play a sound for new mail" : "Silent"
    checked: page.sound
    onClicked: page.sound = !page.sound
  }

  Column {
    width: parent.width
    spacing: Style.space(6)

    PanelSectionHeader { text: "NOTIFY FOR" }
    ButtonGroup {
      options: [{ value: "inbox", label: "Inbox" }, { value: "all", label: "All folders" }, { value: "none", label: "Nothing" }]
      value: page.scope
      onChanged: function(v) { page.scope = v }
    }
  }
}
