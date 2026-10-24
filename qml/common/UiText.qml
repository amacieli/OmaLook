import QtQuick
import qs.Commons
import qs.Commons as Commons

// Plain themed text. The kit has no generic label, and every control in it
// repeats these four bindings, so this is the one place we repeat them.
//   size  -> any Style.font.* token
//   dim   -> secondary text (same Qt.darker(fg, 1.4) the kit uses for labels)
Text {
  property bool dim: false
  property color foreground: Commons.Color.foreground

  textFormat: Text.PlainText
  color: dim ? Qt.darker(foreground, 1.4) : foreground
  font.family: Style.font.family
  font.pixelSize: Style.font.body
}
