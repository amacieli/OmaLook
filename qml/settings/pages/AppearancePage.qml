import QtQuick
import qs.Commons
import qs.Ui
import "../../common"
import ".."

// Fonts, colours and corner rounding are owned by Omarchy and shown read-only.
// Density / preview position are placeholders.
SettingsPage {
  id: page

  // placeholder state
  property string density: "comfortable"
  property string previewPosition: "right"

  title: "Appearance"
  description: "Typography and colours follow your Omarchy theme. Change them with `omarchy font set`, `omarchy display text size` or by switching theme."
  placeholder: true

  Column {
    width: parent.width
    spacing: Style.space(8)

    PanelSectionHeader { text: "FROM OMARCHY (READ-ONLY)" }

    Row {
      spacing: Style.spacing.xxl
      UiText { text: "Font"; dim: true; font.pixelSize: Style.font.caption; width: Style.space(110) }
      UiText { text: Style.font.resolvedFamily; font.pixelSize: Style.font.caption }
    }
    Row {
      spacing: Style.spacing.xxl
      UiText { text: "Base size"; dim: true; font.pixelSize: Style.font.caption; width: Style.space(110) }
      UiText { text: Style.font.baseSize + " px"; font.pixelSize: Style.font.caption }
    }
    Row {
      spacing: Style.spacing.xxl
      UiText { text: "Corner radius"; dim: true; font.pixelSize: Style.font.caption; width: Style.space(110) }
      UiText { text: Style.cornerRadius + " px"; font.pixelSize: Style.font.caption }
    }
  }

  Column {
    width: parent.width
    spacing: Style.space(6)

    PanelSectionHeader { text: "DENSITY" }
    ButtonGroup {
      options: [{ value: "compact", label: "Compact" }, { value: "comfortable", label: "Comfortable" }]
      value: page.density
      onChanged: function(v) { page.density = v }
    }
  }

  Column {
    width: parent.width
    spacing: Style.space(6)

    PanelSectionHeader { text: "PREVIEW PANE" }
    ButtonGroup {
      options: [{ value: "right", label: "Right" }, { value: "bottom", label: "Bottom" }, { value: "off", label: "Off" }]
      value: page.previewPosition
      onChanged: function(v) { page.previewPosition = v }
    }
  }
}
