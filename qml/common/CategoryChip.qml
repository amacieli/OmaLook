import QtQuick
import qs.Commons
import qs.Commons as Commons

// A category / tag: its name on a tint of its own colour, with a solid strip on the left.
Rectangle {
  id: root

  property string label: ""
  property color tint: Commons.Color.accent
  property int maxTextWidth: Style.space(160)

  implicitWidth: Math.min(text.implicitWidth, maxTextWidth) + Style.spacing.md * 2 + Style.space(4)
  implicitHeight: text.implicitHeight + Style.spacing.xs * 2
  radius: Math.max(Style.cornerRadius, 2)
  color: Qt.rgba(tint.r, tint.g, tint.b, 0.22)
  border.width: 1
  border.color: Qt.rgba(tint.r, tint.g, tint.b, 0.75)

  Rectangle {
    anchors.left: parent.left
    anchors.top: parent.top
    anchors.bottom: parent.bottom
    width: Style.space(3)
    color: root.tint
  }

  Text {
    id: text
    anchors.left: parent.left
    anchors.leftMargin: Style.space(4) + Style.spacing.md
    anchors.verticalCenter: parent.verticalCenter
    width: Math.min(implicitWidth, root.maxTextWidth)
    text: root.label
    elide: Text.ElideRight
    color: Commons.Color.foreground
    font.family: Style.font.family
    font.pixelSize: Style.font.caption
  }
}
