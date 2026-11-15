import QtQuick
import QtTest
import Quickshell
import "compose"
FloatingWindow {
  id: win
  implicitWidth: 1000; implicitHeight: 700; color: "black"
  ComposeView {
    id: cv
    anchors.fill: parent
    defaultMode: "html"
    initial: ({ kind: "new", accountId: "exchange-x", to: "", cc: "", bcc: "", subject: "t" })
    accounts: [{ id: "exchange-x", email: "x@example.com", provider: "exchange" }]
    registerFiles: function(purpose, paths, cb) { console.log("HARNESS register", purpose, JSON.stringify(paths)); cb(paths.map(function(p) { return { handle: "fh-test", name: p.split("/").pop(), size: 10 } }), [], "") }
    onLocalStatusChanged: console.log("HARNESS status:", localStatus)
  }
  TestEvent { id: te }
  Timer { interval: 2500; running: true; onTriggered: {
    var ed = null
    function find(it) { if (!it) return; if (it.textDocument !== undefined && it.persistentSelection === true) ed = it; for (var i = 0; i < it.children.length; i++) find(it.children[i]) }
    find(cv)
    console.log("HARNESS editor found:", ed !== null)
    ed.forceActiveFocus()
    console.log("HARNESS focus:", ed.activeFocus)
    te.keyClick(Qt.Key_V, Qt.ControlModifier, -1)
  } }
  Timer { interval: 6000; running: true; onTriggered: { console.log("HARNESS attachments:", JSON.stringify(cv.attachments)); Qt.quit() } }
}
