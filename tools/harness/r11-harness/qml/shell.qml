import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Commons as Commons
import "views"
import "calendar"

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

  // ---- R1.1 harness: drive every converted call site, then quit ----
  property var app: null
  function findApp() {
    for (var i = 0; i < shell.data.length; i++) { var o = shell.data[i]; if (o && o.auth !== undefined && o.currentView !== undefined) return o }
    return null
  }
  TasksView { id: tv; visible: false }
  PeopleView { id: pv; visible: false }
  FreeTimeFinder { id: ftf; visible: false; accountId: "exchange-x"; requiredEmails: ["a@example.com"] }
  property int step: 0
  Timer {
    interval: 1500; repeat: true; running: true
    onTriggered: {
      step++
      console.log("HARNESS step", step)
      if (step === 1) { app = findApp(); console.log("HARNESS app found:", app !== null) }
      if (step === 2) { app.auth.startLogin("exchange"); app.auth.loginAccount("exchange-x", "exchange"); app.auth.logout("exchange-x"); app.auth.removeAccount("exchange-x") }
      if (step === 3) { app.auth.reauthPrompt = { pending_id: "p1" }; app.auth.answerReauth("cancel"); app.auth.startLogin("gmail"); app.auth.submitEmail("a@example.com"); app.auth.cancel() }
      if (step === 4) { app.currentView = "tasks" }
      if (step === 5) { tv.loadTasks() }
      if (step === 6) { tv.toggleTask(0); app.currentView = "contacts" }
      if (step === 7) { pv.load(); pv.newContact(); pv.loadFolders(); pv._post("/contacts/harness-post", null); pv._postJson("/contacts/quick", { a: 1 }, function(r) { console.log("HARNESS postJson ok", JSON.stringify(r)) }) }
      if (step === 8) { app.currentView = "calendar"; ftf.query() }
      if (step === 10) { app.currentView = "mail"; app.loadFolders(); app.loadMessages() }
      if (step === 11) { app.messageRendering = "html"; console.log("HARNESS msgs", app.messageModel ? app.messageModel.count : "n/a"); if (app.selectMessage) app.selectMessage(0); else app.loadBody("m1") }
      if (step === 12) { app.composing = true; app.composeFlush = function() { return { subject: "harness draft" } }; console.log("HARNESS composing", app.composing, typeof app.composeFlush); shell.flushOnQuit(); console.log("HARNESS sync flush returned") }
      if (step === 13) { console.log("HARNESS done"); Qt.quit() }
    }
  }
}
