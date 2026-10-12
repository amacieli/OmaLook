import QtQuick
import Quickshell
import OmaLook.Compose

ShellRoot {
  FloatingWindow {
    visible: true
    implicitWidth: 400; implicitHeight: 200
    TextEdit {
      id: ed
      anchors.fill: parent
      textFormat: TextEdit.RichText
      text: "hello world"
      DocumentHandler { id: dh; document: ed.textDocument; selectionStart: 0; selectionEnd: 5 }
    }
    Timer {
      interval: 500; running: true
      onTriggered: {
        dh.toggleBold()
        console.log("SPIKE QT=" + dh.qtVersion()); console.log("SPIKE HTML=" + dh.html().replace(/\n/g," "))
        quitT.start()
      }
    }
    Timer { id: quitT; interval: 800; onTriggered: Qt.quit() }
  }
}
