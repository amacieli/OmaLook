import QtQuick
import QtTest
import Quickshell
import "views"

// R4 photo harness (scratch, not shipped): PeopleView alone, fake contacts from the fake backend on
// OMALOOK_PORT=28199, logging stubs for registerFiles / openPicker / listDir. Drives keys with TestEvent.
FloatingWindow {
  id: win
  implicitWidth: 1100; implicitHeight: 760; color: "black"
  visible: true

  PeopleView {
    id: pv
    anchors.fill: parent
    appAccounts: [{ id: "exchange-x", email: "x@example.com", provider: "exchange", signed_in: true }]
    registerFiles: function(purpose, paths, cb) {
      console.log("HARNESS registerFiles purpose=" + purpose + " paths=" + JSON.stringify(paths))
      cb([{ handle: "fh-photo-1", name: "alice.jpg", size: 1234 }], [], "")
    }
    openPicker: function(purpose, cb) { console.log("HARNESS openPicker purpose=" + purpose); cb("pk-1") }
    listDir: function(picker, path, hidden, cb) {
      console.log("HARNESS listDir picker=" + picker + " path=" + path)
      cb({ path: "/home/adam", parent: "/", truncated: false, entries: [
        { name: "Pictures", dir: true, size: 0, mtime: 0 },
        { name: "alice.jpg", dir: false, size: 20480, mtime: 1700000000 },
        { name: "bob.jpeg", dir: false, size: 30720, mtime: 1700000100 },
        { name: "shot.png", dir: false, size: 9000, mtime: 1700000200 },
        { name: "notes.txt", dir: false, size: 120, mtime: 1700000300 }
      ] })
    }
    onNotice: function(t) { console.log("HARNESS notice: " + t) }
  }

  TestEvent { id: te }

  function findIt(it, pred) {
    if (!it) return null
    if (pred(it)) return it
    var kids = it.children || []
    for (var i = 0; i < kids.length; i++) { var r = findIt(kids[i], pred); if (r) return r }
    return null
  }
  function editor() { return findIt(pv, function(it) { return it.photoCanSet !== undefined && it.photoListDir !== undefined }) }
  function browser() { return findIt(pv, function(it) { return it.purpose === "photo" && it.jpegOnly === true }) }
  function shot(name) {
    pv.grabToImage(function(r) { r.saveToFile("/mnt/d/r4photo-harness/" + name + ".png"); console.log("HARNESS shot " + name) })
  }
  function key(k, mods) { te.keyClick(k, mods === undefined ? Qt.NoModifier : mods, -1) }

  // One action per tick (1.2 s apart): a grabToImage renders on a later frame, so a key pressed in the same
  // tick as its screenshot would be in the picture.
  property int step: 0
  readonly property var plan: [
    function() { console.log("HARNESS contacts=" + pv.contacts.length); pv.cur = 0; console.log("HARNESS editContact msg=[" + pv.editContact() + "]") },
    function() { var e = win.editor(); console.log("HARNESS editor open=" + (e !== null) + " photoCanSet=" + (e && e.photoCanSet) + " photoLine=" + (e && e.photoLine)); win.shot("editor-open") },
    function() { win.key(Qt.Key_O, Qt.ControlModifier) },
    function() { var b = win.browser(); console.log("HARNESS picker opened=" + (b !== null && b.opened) + " path=" + (b && b.path) + " rows=" + (b && b.rows.map(function(r) { return r.name }).join(","))); win.shot("picker-open") },
    function() { win.key(Qt.Key_Escape) },
    function() { var b2 = win.browser(); console.log("HARNESS after Esc: picker opened=" + (b2 && b2.opened) + " editorOpen=" + pv.editorOpen); win.key(Qt.Key_O, Qt.ControlModifier) },
    function() { win.key(Qt.Key_J); win.key(Qt.Key_J) },
    function() { var b3 = win.browser(); console.log("HARNESS cursor on=" + (b3 && b3.rows[b3.cur] ? b3.rows[b3.cur].name : "?")); win.key(Qt.Key_Return) },
    function() { console.log("HARNESS after pick: editorPhotoStatus=" + pv.editorPhotoStatus + " photoPath=" + pv.photoPath + " editorOpen=" + pv.editorOpen) },
    function() { win.shot("after-pick") },
    function() { win.key(Qt.Key_Escape) },
    function() { console.log("HARNESS editor closed=" + !pv.editorOpen); pv.cur = 1; console.log("HARNESS editContact(local) msg=[" + pv.editContact() + "]") },
    function() { var e3 = win.editor(); console.log("HARNESS local editor open=" + (e3 !== null) + " photoCanSet=" + (e3 && e3.photoCanSet) + " photoLine=" + (e3 && e3.photoLine)); win.shot("editor-local") },
    function() { win.key(Qt.Key_O, Qt.ControlModifier) },
    function() { console.log("HARNESS local ctrl-O opened browser=" + (win.browser() !== null && win.browser().opened === true)); win.key(Qt.Key_Escape) },
    function() { console.log("HARNESS done"); Qt.quit() }
  ]
  Timer {
    id: runner
    interval: 1200; running: true; repeat: true
    onTriggered: { if (win.step < win.plan.length) win.plan[win.step](); win.step++ }
  }
}
