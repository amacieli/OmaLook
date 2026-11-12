import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Ui
import "../common"
import "modes.js" as Modes

// Formatting toolbar. `fmt` is the editor's DocumentHandler (duck-typed, so this file does
// not import the native plugin). Which controls are live comes from modes.js; a greyed
// control says why in its tooltip.
ColumnLayout {
  id: root

  property var fmt: null
  property string mode: "system"
  property bool canUndo: false
  property bool canRedo: false

  signal undoRequested()
  signal redoRequested()
  signal linkRequested()
  signal attachRequested()
  signal pictureRequested()

  spacing: Style.spacing.sm

  function _ok(f) { return Modes.allowed(root.mode, f) }
  function _tip(f, label) { var r = Modes.reason(root.mode, f); return r === "" ? label : label + " (" + r + ")" }

  // colour strip: which attribute the swatches apply to ("" = closed)
  property string colorTarget: ""
  readonly property var swatches: ["#000000", "#c00000", "#e36c09", "#bf9000", "#2e7d32", "#0563c1", "#7030a0", "#7f7f7f", "#ffffff",
                                   "#ffff00", "#ffd966", "#a9d18e", "#9dc3e6", "#d9b3ff", "#f4b6c2"]

  component Tool: Button {
    id: t
    property string feature: ""
    property string label: ""
    Layout.alignment: Qt.AlignVCenter
    bordered: false
    horizontalPadding: Style.spacing.lg
    verticalPadding: Style.spacing.md
    enabled: root._ok(feature)
    opacity: enabled ? 1 : 0.35
    tooltipText: root._tip(feature, label)
  }

  component Sep: Rectangle {
    Layout.alignment: Qt.AlignVCenter
    Layout.preferredWidth: 1
    Layout.preferredHeight: Style.font.body * 1.2
    Layout.leftMargin: Style.spacing.sm
    Layout.rightMargin: Style.spacing.sm
    color: Style.normalBorderColor
  }

  Flow {
    Layout.fillWidth: true
    spacing: Style.spacing.xs

    RowLayout {
      spacing: 0
      Tool { feature: "undo"; label: "Undo (Ctrl+Z)"; iconText: "\uf0e2"; enabled: root.canUndo; onClicked: root.undoRequested() }
      Tool { feature: "undo"; label: "Redo (Ctrl+Shift+Z)"; iconText: "\uf01e"; enabled: root.canRedo; onClicked: root.redoRequested() }
      Sep {}
      Tool { feature: "bold";      label: "Bold (Ctrl+B)";      iconText: "\uf032"; selected: !!root.fmt && root.fmt.bold;      onClicked: root.fmt.toggleBold() }
      Tool { feature: "italic";    label: "Italic (Ctrl+I)";    iconText: "\uf033"; selected: !!root.fmt && root.fmt.italic;    onClicked: root.fmt.toggleItalic() }
      Tool { feature: "underline"; label: "Underline (Ctrl+U)"; iconText: "\uf0cd"; selected: !!root.fmt && root.fmt.underline; onClicked: root.fmt.toggleUnderline() }
      Tool { feature: "strike";    label: "Strikethrough";      iconText: "\uf0cc"; selected: !!root.fmt && root.fmt.strike;    onClicked: root.fmt.toggleStrike() }
      Sep {}
      Tool { feature: "bulletList"; label: "Bulleted list (Ctrl+Shift+8)"; iconText: "\uf0ca"; selected: !!root.fmt && root.fmt.bulletList;   onClicked: root.fmt.toggleBulletList() }
      Tool { feature: "numberList"; label: "Numbered list (Ctrl+Shift+7)"; iconText: "\uf0cb"; selected: !!root.fmt && root.fmt.numberedList; onClicked: root.fmt.toggleNumberedList() }
      Tool { feature: "quote";      label: "Quote";                         iconText: "\uf10d"; onClicked: root.fmt.toggleQuote() }
      Tool { feature: "link";       label: "Link (Ctrl+K)";                 iconText: "\uf0c1"; onClicked: root.linkRequested() }
      Sep {}
      Tool { feature: "attach"; label: "Attach files"; iconText: "\uf0c6"; onClicked: root.attachRequested() }
    }

    // HTML-only controls: greyed (not hidden) in system-font mode so the mode difference is visible.
    RowLayout {
      spacing: 0
      Sep { visible: false }
      Dropdown {
        Layout.preferredWidth: Style.space(150)
        Layout.alignment: Qt.AlignVCenter
        showLabel: false
        enabled: root._ok("fontFamily")
        opacity: enabled ? 1 : 0.35
        value: root.fmt && root.fmt.fontFamily ? root.fmt.fontFamily : "Calibri"
        options: ["Calibri", "Arial", "Georgia", "Times New Roman", "Verdana", "Courier New"].map(function(f) { return { value: f, label: f } })
        onChanged: function(v) { root.fmt.setFontFamily(v) }
      }
      Item { Layout.preferredWidth: Style.spacing.md }
      Dropdown {
        Layout.preferredWidth: Style.space(80)
        Layout.alignment: Qt.AlignVCenter
        showLabel: false
        enabled: root._ok("fontSize")
        opacity: enabled ? 1 : 0.35
        value: root.fmt && root.fmt.fontSize > 0 ? String(root.fmt.fontSize) : "15"
        options: ["11", "12", "13", "15", "17", "20", "24", "32"].map(function(s) { return { value: s, label: s } })
        onChanged: function(v) { root.fmt.setFontSize(parseInt(v)) }
      }
      Sep {}
      Tool { feature: "textColor"; label: "Text colour"; iconText: "\uf1fc"; selected: root.colorTarget === "text"
             onClicked: root.colorTarget = root.colorTarget === "text" ? "" : "text" }
      Tool { feature: "highlight"; label: "Highlight"; iconText: "\uf0eb"; selected: root.colorTarget === "highlight"
             onClicked: root.colorTarget = root.colorTarget === "highlight" ? "" : "highlight" }
      Tool { feature: "align"; label: "Align left";   iconText: "\uf036"; selected: !!root.fmt && root.fmt.alignment === Qt.AlignLeft;    onClicked: root.fmt.setAlignment(Qt.AlignLeft) }
      Tool { feature: "align"; label: "Align centre"; iconText: "\uf037"; selected: !!root.fmt && root.fmt.alignment === Qt.AlignHCenter; onClicked: root.fmt.setAlignment(Qt.AlignHCenter) }
      Tool { feature: "align"; label: "Align right";  iconText: "\uf038"; selected: !!root.fmt && root.fmt.alignment === Qt.AlignRight;   onClicked: root.fmt.setAlignment(Qt.AlignRight) }
      Sep {}
      Tool { feature: "image"; label: "Insert picture (Ctrl+Shift+I)"; iconText: "\uf03e"; onClicked: root.pictureRequested() }
      Tool { feature: "table"; label: "Insert table"; iconText: "\uf0ce" }
      Tool { feature: "clear"; label: "Clear formatting"; iconText: "\uf12d"; onClicked: root.fmt.clearFormatting() }
    }
  }

  // swatch strip
  Row {
    visible: root.colorTarget !== "" && root._ok("textColor")
    spacing: Style.spacing.sm
    Repeater {
      model: root.swatches
      delegate: Rectangle {
        required property string modelData
        width: Style.space(20); height: width; radius: 3
        color: modelData
        border.width: 1; border.color: Style.normalBorderColor
        MouseArea {
          anchors.fill: parent
          cursorShape: Qt.PointingHandCursor
          onClicked: {
            if (root.colorTarget === "text") root.fmt.setTextColor(parent.modelData)
            else root.fmt.setHighlight(parent.modelData)
            root.colorTarget = ""
          }
        }
      }
    }
  }
}
