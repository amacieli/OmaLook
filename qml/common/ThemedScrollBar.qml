import QtQuick
import QtQuick.Controls
import qs.Commons
import qs.Commons as Commons

// Always-visible vertical scroll bar for lists. Shown whenever the content
// overflows its view (the stock bar hides itself until you scroll, so a long
// list looks like it ends at the fold) and gone when everything fits.
// Attach with `ScrollBar.vertical: ThemedScrollBar {}`.
ScrollBar {
  id: control

  policy: size < 1.0 ? ScrollBar.AlwaysOn : ScrollBar.AlwaysOff
  minimumSize: 0.08
  padding: 2
  implicitWidth: 10

  // The colour the bar is drawn in (translucent). Light foreground on the dark
  // shell by default; the HTML view sets it dark, since that sits on a white page.
  property color tint: Commons.Color.foreground

  contentItem: Rectangle {
    implicitWidth: 6
    radius: Style.cornerRadius
    color: Qt.rgba(control.tint.r, control.tint.g, control.tint.b,
                   control.pressed ? 0.65 : (control.hovered ? 0.5 : 0.35))
  }

  background: Rectangle {
    radius: Style.cornerRadius
    color: Qt.rgba(control.tint.r, control.tint.g, control.tint.b, 0.07)
  }
}
