import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Commons as Commons

// OmaLook entry point. Run standalone with:
//   quickshell -p <this dir>        (the Rust launcher does this)
//
// Quickshell is used as a plain Qt Quick host here, not as a shell plugin:
// it gives us the Omarchy UI kit (qs.Commons / qs.Ui) and live theming.
// `Commons/` and `Ui/` in this directory are symlinks to the installed kit
// (/usr/share/omarchy/shell); the launcher creates them if missing.
ShellRoot {
  // Omarchy's kit no longer loads the theme by itself: its Color singleton keeps built-in greys
  // until the shell pushes colors.toml / shell.toml into it (Omarchy 4.0.6). Do that here, and
  // follow theme switches. Reference it as Commons.Color: Qt 6.12 has its own `Color` type that
  // shadows the kit's when written bare.
  FileView {
    path: Commons.Color.currentThemePath + "/colors.toml"
    watchChanges: true
    printErrors: false
    onLoaded: Commons.Color.loadColors(text())
    onFileChanged: reload()
  }
  FileView {
    path: Commons.Color.currentThemePath + "/shell.toml"
    watchChanges: true
    printErrors: false
    onLoaded: Commons.Color.loadShell(text())
    onFileChanged: reload()
  }

  FloatingWindow {
    id: window
    visible: true
    title: "OmaLook"
    color: Commons.Color.menu.background
    implicitWidth: Style.space(1200)
    implicitHeight: Style.space(800)
    minimumSize: Qt.size(Style.space(720), Style.space(480))

    // Closing the window (or Esc at the top level) ends the app.
    onVisibleChanged: if (!visible) Qt.quit()

    // Settings -> Appearance -> Scale factor: the whole UI is laid out at window/scale and
    // then scaled up, so every size (fonts, spacing, icons) follows at once, no restart.
    // (The Omarchy kit in Commons/Ui is a read-only symlink, so scaling happens here.)
    Item {
      anchors.fill: parent
      clip: true

      AppShell {
        id: shell
        width: parent.width / shell.uiScale
        height: parent.height / shell.uiScale
        scale: shell.uiScale
        transformOrigin: Item.TopLeft
        onCloseRequested: Qt.quit()
      }
    }
  }
}
