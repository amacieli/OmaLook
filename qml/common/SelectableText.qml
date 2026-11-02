import QtQuick
import qs.Commons
import qs.Commons as Commons

// UiText that can be selected with the mouse (drag, double-click word, triple-click line) and
// copied. It never takes keyboard focus: global key bindings keep working, and Ctrl+C is wired
// by whoever owns it (see MessagePreview). The selection also goes to the primary selection, so
// middle-click pastes it elsewhere.
//   dim / foreground -> same as UiText
// Not supported (TextEdit has no equivalent): elide, lineHeight. Use UiText where those matter.
TextEdit {
  id: te
  property bool dim: false
  property color foreground: Commons.Color.foreground
  // Cursor over a link is a hand, elsewhere the text cursor.
  property bool linkCursor: true

  readOnly: true
  selectByMouse: true
  activeFocusOnPress: false
  activeFocusOnTab: false
  persistentSelection: true
  textFormat: TextEdit.PlainText
  wrapMode: TextEdit.WordWrap
  selectionColor: Commons.Color.accent
  selectedTextColor: Commons.Color.background
  color: dim ? Qt.darker(foreground, 1.4) : foreground
  font.family: Style.font.family
  font.pixelSize: Style.font.body

  MouseArea {
    anchors.fill: parent
    acceptedButtons: Qt.NoButton
    cursorShape: te.linkCursor && te.hoveredLink !== "" ? Qt.PointingHandCursor : Qt.IBeamCursor
  }
}
