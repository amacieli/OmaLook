import QtQuick
import qs.Commons
import qs.Ui
import "../../common"
import ".."

// Read-only diagnostics.
SettingsPage {
  id: page

  property string backendUrl: ""
  property string configDir: ""
  property bool backendOnline: false

  title: "About"
  description: "OmaLook runs inside omarchy-shell and talks to a local Rust backend."

  Column {
    width: parent.width
    spacing: Style.space(8)

    PanelSectionHeader { text: "DIAGNOSTICS" }

    Repeater {
      model: [
        { k: "Plugin",    v: "adam.omalook" },
        { k: "Backend",   v: page.backendUrl + (page.backendOnline ? "  (online)" : "  (offline)") },
        { k: "Config",    v: page.configDir },
        { k: "Font",      v: Style.font.resolvedFamily + " " + Style.font.baseSize + "px" }
      ]

      Row {
        required property var modelData
        spacing: Style.spacing.xxl
        UiText { text: modelData.k; dim: true; font.pixelSize: Style.font.caption; width: Style.space(80) }
        UiText { text: modelData.v; font.pixelSize: Style.font.caption }
      }
    }
  }
}
