import QtQuick
import qs.Commons
import qs.Commons as Commons

// btop-style panel outline: a box with the pane's title set into the top edge,
// prefixed by the key that jumps to it ("³ Inbox"). Idle panes are dim and thin; the
// pane that owns the keyboard is accent-coloured, thicker, and its title is bold.
// Drop it inside a pane as a child; it fills the pane and ignores the mouse.
//
// The edge is four plain lines (not one bordered Rectangle) so the top line can
// stop short of the title: nothing is painted behind the label, which keeps it
// correct over translucent / blurred window backgrounds.
Item {
  id: root

  property bool focused: false
  property string title: ""
  property string hotkey: ""
  // The pane's own hue (a Hues colour), as btop gives each box one. Dimmed until focused.
  property color tint: Commons.Color.accent

  property color lineColor: focused
    ? tint
    : Qt.rgba(tint.r, tint.g, tint.b, 0.45)
  readonly property int lw: focused ? 2 : 1
  readonly property real edgeY: Math.round(labelText.implicitHeight / 2)
  readonly property real gapStart: Style.spacing.xl
  readonly property real gapEnd: title === "" ? gapStart : gapStart + labelText.implicitWidth + Style.spacing.md * 2

  anchors.fill: parent
  z: 50

  Behavior on lineColor { ColorAnimation { duration: 120 } }

  // top: left of the title, then right of it
  Rectangle { x: 0; y: root.edgeY; width: root.gapStart; height: root.lw; color: root.lineColor }
  Rectangle { x: root.gapEnd; y: root.edgeY; width: Math.max(0, root.width - root.gapEnd); height: root.lw; color: root.lineColor }
  // bottom, left, right
  Rectangle { x: 0; y: root.height - root.lw; width: root.width; height: root.lw; color: root.lineColor }
  Rectangle { x: 0; y: root.edgeY; width: root.lw; height: Math.max(0, root.height - root.edgeY); color: root.lineColor }
  Rectangle { x: root.width - root.lw; y: root.edgeY; width: root.lw; height: Math.max(0, root.height - root.edgeY); color: root.lineColor }

  Text {
    id: labelText
    visible: root.title !== ""
    x: root.gapStart + Style.spacing.md
    text: (root.hotkey !== "" ? root.hotkey + " " : "") + root.title
    color: root.lineColor
    font.family: Style.font.family
    font.pixelSize: Style.font.bodySmall
    font.bold: root.focused
  }
}
