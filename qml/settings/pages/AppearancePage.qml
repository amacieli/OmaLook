import QtQuick
import qs.Commons
import qs.Ui
import "../../common"
import ".."

// Fonts, colours and corner rounding are owned by Omarchy and shown read-only.
// Density / preview position are placeholders.
SettingsPage {
  id: page

  property var app: null

  // Moves the keyboard into the scale field; SettingsView calls this on l / Right / Enter.
  function focusFirst() { scaleField.focusField(); return true }
  editing: scaleField.field.activeFocus

  // placeholder state
  property string density: "comfortable"
  property string previewPosition: "right"

  title: "Appearance"
  description: "Typography and colours follow your Omarchy theme. Change them with `omarchy font set`, `omarchy display text size` or by switching theme."

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

    PanelSectionHeader { text: "SCALE FACTOR" }
    ScaleField {
      id: scaleField
      value: page.app ? page.app.uiScale : 1.0
      from: page.app ? page.app.minUiScale : 0.5
      to: page.app ? page.app.maxUiScale : 3.0
      onModified: function(v) { if (page.app) page.app.setUiScale(v) }
      onLeave: if (page.app) page.app.focusRequested()
    }
    UiText {
      width: parent.width
      wrapMode: Text.WordWrap
      dim: true
      font.pixelSize: Style.font.caption
      text: "Scales the whole window, 0.5 to 3.0 (default 1.0). Up / Down step by 0.1, or type a value and press Enter. Esc returns to the list. Saved to settings.toml [font] scale_factor."
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
