import QtQuick
import qs.Commons
import qs.Ui
import "../common"

// One MCP write tool: what it does on the left, Off / Ask / Allow on the right.
Item {
  id: root

  property string toolName: ""
  property string description: ""
  property string mode: "ask"
  property string defaultMode: "ask"
  property bool implemented: true

  signal modeChosen(string mode)
  // Tab focus moved onto this row's chips (the page scrolls it into view).
  signal focusReached(Item item)

  readonly property alias group: chips

  implicitHeight: Math.max(textCol.implicitHeight, chips.implicitHeight) + Style.spacing.md * 2

  Column {
    id: textCol
    anchors.left: parent.left
    anchors.right: chips.left
    anchors.rightMargin: Style.spacing.lg
    anchors.verticalCenter: parent.verticalCenter
    spacing: 2

    UiText {
      width: parent.width
      text: root.toolName
      font.pixelSize: Style.font.bodySmall
      font.bold: true
      elide: Text.ElideRight
    }
    UiText {
      width: parent.width
      text: (root.implemented ? "" : "Not available yet. ") + root.description + (root.mode === root.defaultMode ? "" : "  (default: " + root.defaultMode + ")")
      dim: true
      wrapMode: Text.WordWrap
      font.pixelSize: Style.font.caption
    }
  }

  ButtonGroup {
    id: chips
    anchors.right: parent.right
    anchors.verticalCenter: parent.verticalCenter
    options: [{ value: "off", label: "Off" }, { value: "ask", label: "Ask" }, { value: "allow", label: "Allow" }]
    value: root.mode
    onChanged: function(v) { root.modeChosen(v) }
    onActiveFocusChanged: if (activeFocus) root.focusReached(root)
  }
}
