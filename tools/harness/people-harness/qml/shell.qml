import QtQuick
import Quickshell
import Quickshell.Io
import QtTest
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


  TestEvent { id: te }
  property var steps: []
  property int stepIx: 0
  function app() { var r=null; function f(o){ if(!o||r) return; if(o.currentView!==undefined && o.auth!==undefined){r=o;return} var ch=o.data||o.children||[]; for(var i=0;i<ch.length;i++) f(ch[i]) } f(shell); return r }
  function people() { var r=null; function f(o){ if(!o||r) return; if(o.editorOpen!==undefined && o.editContact!==undefined){r=o;return} var ch=o.children||[]; for(var i=0;i<ch.length;i++) f(ch[i]) } f(shell); return r }
  function fi() { var a=shell.Window.activeFocusItem; if(!a) return "NONE"; var s=String(a); return s.split("(")[0] + (a.objectName?("#"+a.objectName):"") + (a.text!==undefined?(" text="+JSON.stringify(String(a.text).slice(0,20))):"") }
  function log(m) { var p=people(); var a=app(); console.log("DRV", m, "| view="+(a?a.currentView:"?"), "editorOpen="+(p?p.editorOpen:"-"), "cur="+(p?p.cur:"-"), "n="+(p&&p.contacts?p.contacts.length:"-"), "focus="+fi()) }
  function key(k, mod, txt) { te.keyClick(k, mod||Qt.NoModifier, -1) }
  Timer { id: tick; interval: 900; repeat: true; running: true; onTriggered: {
    if (stepIx >= steps.length) { stop(); Qt.quit(); return }
    var s = steps[stepIx++]; s()
  } }
  Component.onCompleted: steps = [
    function(){ log("start") }, function(){}, function(){}, function(){}, function(){}, function(){}, function(){}, function(){},
    function(){ var a=app(); a.loadAccounts(); a.gotoView("contacts"); log("switched") }, function(){}, function(){},
    function(){ var a=app(); var acc=JSON.parse(JSON.stringify(a.accounts)); acc.forEach(function(x){x.signed_in=true}); a.accounts=acc; log("accounts patched "+JSON.stringify(acc.map(function(x){return x.id+":"+x.provider+":"+x.signed_in}))) },
    function(){ key(Qt.Key_E) }, function(){}, function(){ log("after e"); shell.grabToImage(function(r){ r.saveToFile("/mnt/d/people-harness/editor2.png") }) },
    function(){ key(Qt.Key_Tab) },
    function(){ key(Qt.Key_Tab) },
    function(){ key(Qt.Key_Tab) },
    function(){ key(Qt.Key_Tab) },
    function(){ key(Qt.Key_Tab) },
    function(){ key(Qt.Key_Tab) },
    function(){ key(Qt.Key_Tab) },
    function(){ key(Qt.Key_Tab) },
    function(){ key(Qt.Key_Tab) },
    function(){ key(Qt.Key_Tab) },
    function(){ key(Qt.Key_Tab) }, function(){ log("after 11 tabs (wrap expected to First name)") },
    function(){ key(Qt.Key_Backtab) }, function(){ log("after shift-tab (expect Notes)") },
    function(){ te.mouseClick(shell, 60, 230, Qt.LeftButton, Qt.NoModifier, -1) }, function(){ log("click over row 2 behind label-free area? (cur must stay 0)") },
    function(){ te.mouseClick(shell, 60, 600, Qt.LeftButton, Qt.NoModifier, -1) }, function(){ log("click empty editor area") },
    function(){ shell.focusKeys() }, function(){ log("forced focus to keyCatcher (lost focus)") },
    function(){ key(Qt.Key_Escape) }, function(){ log("esc via hatch (expect closed, clean)") },
    function(){ key(Qt.Key_E) }, function(){}, function(){ key(Qt.Key_X); log("reopened, typed x") },
    function(){ shell.focusKeys() }, function(){ key(Qt.Key_Escape) }, function(){ log("hatch esc dirty (expect still open, asks)") },
    function(){ key(Qt.Key_Escape) }, function(){ log("esc2 (expect closed)") },
    function(){ key(Qt.Key_E) }, function(){}, function(){ log("reopen for mouse cancel") },
    function(){ var b=null; function f(o){ if(!o||b) return; if(o.text==="Cancel  (Esc)" && o.clicked!==undefined) {b=o;return} var c=o.children||[]; for(var i=0;i<c.length;i++) f(c[i]) } f(shell); var pt=b.mapToItem(shell, b.width/2, b.height/2); console.log("DRV cancel btn at", pt.x, pt.y); te.mouseClick(shell, pt.x, pt.y, Qt.LeftButton, Qt.NoModifier, -1) }, function(){ log("after mouse Cancel (expect closed)") },
    function(){ key(Qt.Key_E); key(Qt.Key_X); key(Qt.Key_D) }, function(){}, function(){ log("type-ahead e x d (expect editor open, n=4, field text not containing commands effect)") },
    function(){ key(Qt.Key_Escape) }, function(){ key(Qt.Key_Escape) }, function(){ log("closed") },
    function(){ key(Qt.Key_G); key(Qt.Key_M) }, function(){ log("after g m") }
  ]

  FloatingWindow {
    id: window
    visible: true
    title: "OmaLook"
    color: Commons.Color.menu.background
    implicitWidth: Style.space(1200)
    implicitHeight: Style.space(800)
    minimumSize: Qt.size(Style.space(720), Style.space(480))

    // Closing the window (or Esc at the top level) ends the app.
    onVisibleChanged: if (!visible) { shell.flushOnQuit(); Qt.quit() }

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
        onCloseRequested: { shell.flushOnQuit(); Qt.quit() }
      }
    }
  }
}
