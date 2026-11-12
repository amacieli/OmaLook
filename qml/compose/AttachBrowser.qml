import QtQuick
import QtQuick.Layouts
import Quickshell
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"
import "../mail/format.js" as Fmt
import "pick.js" as Pick

// Keyboard-driven file browser for attaching files. An overlay over the compose pane; it lists
// one folder at a time through `listDir(path, showHidden, callback)` (the daemon's /fs/list).
//
//   Type          filter the list           Up / Down / PgUp / PgDn   move
//   Keys are commands (vim style); `/` opens the filter box, `g` or `~` the go-to-path box.
//   j / k / arrows  move    v  tick / untick    l / Right  open folder    h / Left / Backspace  parent
//   Enter         attach the ticked files and close; with none ticked: open folder / attach this file
//   / or ~ first  Enter jumps to that path  Ctrl+H                    show hidden files
//   Esc           clear filter, then close
//   Filter        plain text = part of the name; with * or ? it is a pattern (*.pdf, 2026-*.png)
//   Ctrl+T        tick every file the filter shows      Ctrl+A  tick all files in the folder
//   Ctrl+U        clear all ticks (they survive folder changes)
//   Ctrl+S        sort: name / newest / size            Ctrl+B  places and recent folders
FocusScope {
  id: root

  property bool opened: false
  property var listDir: null              // function(path, hidden, cb(result))
  property string startDir: ""
  property var attached: []               // paths already on the message (shown ticked, not re-added)
  property real capacity: 0               // bytes of files the From account takes (0 = unknown)
  property real baseBytes: 0              // bytes already on the message
  property var recentDirs: []             // folders visited before, newest first

  signal picked(var files)                // [{ path, name, size }]
  property string title: "Attach files"   // "Insert pictures" when the pick goes into the text (OL-054)
  signal canceled()
  signal navigated(string dir)

  visible: opened

  property string path: ""
  property string parentPath: ""
  property var entries: []                // raw listing
  property bool truncated: false
  property bool showHidden: false
  property bool loading: false
  property string error: ""
  property var marked: ({})               // path -> { path, name, size }
  property int cur: 0
  property string sortKey: "name"
  property bool placesMode: false
  readonly property string home: Quickshell.env("HOME")
  readonly property var placeList: Pick.places(home, recentDirs, null)
  readonly property var ticked: Pick.totals(marked)
  readonly property bool overLimit: Pick.over(capacity, baseBytes, ticked.bytes)

  readonly property string filter: input.text.trim().toLowerCase()
  property string inputMode: ""           // "" (keys are commands) | "filter" | "path"
  readonly property bool pathMode: inputMode === "path"

  // Rows: ".." first (when there is a parent), then the entries matching the filter.
  readonly property var rows: {
    var out = []
    if (placesMode) {
      for (var p = 0; p < placeList.length; p++) out.push({ place: true, name: placeList[p].label, path: placeList[p].path, dir: true, size: 0 })
      return out
    }
    if (parentPath !== "" && (filter === "" || pathMode)) out.push({ up: true, name: "..", dir: true, size: 0 })
    if (!pathMode) {
      var list = Pick.sorted(entries, sortKey)
      for (var i = 0; i < list.length; i++)
        if (Pick.matches(filter, list[i].name)) out.push(list[i])
    }
    return out
  }
  readonly property int markedCount: ticked.count

  onRowsChanged: cur = Math.min(Math.max(cur, 0), Math.max(rows.length - 1, 0))

  function _join(dir, name) { return dir === "/" ? "/" + name : dir + "/" + name }

  function open() {
    marked = ({})
    placesMode = false
    input.text = ""
    inputMode = ""
    opened = true
    go(startDir !== "" ? startDir : "~")
    input.forceActiveFocus()
  }

  function close() { opened = false }

  function go(p) {
    if (!listDir) { error = "no file access"; return }
    loading = true
    var asked = p
    listDir(p, showHidden, function(r) {
      loading = false
      if (!r || r.error || !r.path) {
        error = r && r.error ? String(r.error) : "could not read the folder"
        if (inputMode === "path") { input.text = ""; inputMode = "" }
        return
      }
      error = ""
      placesMode = false
      path = r.path
      parentPath = r.parent || ""
      entries = r.entries || []
      truncated = !!r.truncated
      input.text = ""
      inputMode = ""
      cur = 0
      listView.positionViewAtBeginning()
      root.navigated(r.path)
    })
  }

  function _isAttached(p) { return attached.indexOf(p) >= 0 }

  function toggleMark(row) {
    if (!row || row.up || row.dir || row.place) { error = row && row.dir && !row.up ? "folders cannot be attached; zip it first" : ""; return }
    var p = _join(path, row.name)
    if (_isAttached(p)) { error = row.name + " is already attached"; return }
    error = ""
    var m = {}
    for (var k in marked) m[k] = marked[k]
    if (m[p]) delete m[p]; else m[p] = { path: p, name: row.name, size: row.size }
    marked = m
  }

  function _setMarks(add) {
    var m = {}
    for (var k in marked) m[k] = marked[k]
    var n = 0
    for (var i = 0; i < rows.length; i++) {
      var r = rows[i]
      if (r.up || r.dir || r.place) continue
      var p = _join(path, r.name)
      if (_isAttached(p) || m[p]) continue
      m[p] = { path: p, name: r.name, size: r.size }
      n++
    }
    marked = m
    error = n === 0 ? "no files to tick here" : ""
  }

  function tickShown() { if (placesMode) return; _setMarks() }
  function clearTicks() { marked = ({}); error = "" }
  function cycleSort() { sortKey = Pick.nextSort(sortKey); cur = 0; listView.positionViewAtBeginning() }
  function togglePlaces() { placesMode = !placesMode; input.text = ""; inputMode = ""; cur = 0; listView.positionViewAtBeginning() }

  function activate() {
    if (pathMode) { go(input.text.trim()); return }
    var row = rows[cur]
    if (!row) return
    if (row.place) { go(row.path); return }
    // Ticks win: Enter closes the dialog with whatever is ticked, wherever the cursor is.
    if (markedCount > 0) { _finish(); return }
    if (row.up) { go(parentPath); return }
    if (row.dir) { go(_join(path, row.name)); return }
    var p = _join(path, row.name)
    if (_isAttached(p)) { error = row.name + " is already attached"; return }
    opened = false
    picked([{ path: p, name: row.name, size: row.size }])
  }

  function _finish() {
    var files = []
    for (var k in marked) files.push(marked[k])
    opened = false
    picked(files)
  }

  // Right: open the folder under the cursor (Enter closes the dialog once something is ticked).
  function openCurrent() {
    var row = rows[cur]
    if (!row) return
    if (row.place) go(row.path)
    else if (row.up) go(parentPath)
    else if (row.dir) go(_join(path, row.name))
  }

  function _move(d) { if (rows.length > 0) { cur = Math.min(Math.max(cur + d, 0), rows.length - 1); listView.positionViewAtIndex(cur, ListView.Contain) } }

  Rectangle {
    anchors.fill: parent
    color: Util.alpha(Commons.Color.background, 0.8)
    MouseArea { anchors.fill: parent; onClicked: root.canceled() }
  }

  Rectangle {
    id: card
    width: Math.min(parent.width - Style.space(32), Style.space(640))
    height: Math.min(parent.height - Style.space(32), Style.space(520))
    anchors.centerIn: parent
    radius: Style.cornerRadius
    color: Qt.rgba(Commons.Color.popups.background.r, Commons.Color.popups.background.g, Commons.Color.popups.background.b, 1)
    border.width: Style.normalBorderWidth
    border.color: Commons.Color.popups.border

    MouseArea { anchors.fill: parent }

    ColumnLayout {
      anchors.fill: parent
      anchors.margins: Style.spacing.lg
      spacing: Style.spacing.md

      RowLayout {
        Layout.fillWidth: true
        UiText { text: root.title; font.bold: true; font.pixelSize: Style.font.heading }
        Item { Layout.fillWidth: true }
        UiText {
          text: (root.placesMode ? "places · " : "") + "sort: " + root.sortKey + (root.showHidden ? " · hidden files shown" : "")
          dim: true
          font.pixelSize: Style.font.caption
        }
      }

      UiText {
        Layout.fillWidth: true
        text: root.path
        elide: Text.ElideMiddle
        dim: true
        font.pixelSize: Style.font.caption
      }

      TextField {
        id: input
        Layout.fillWidth: true
        placeholderText: root.inputMode === "" ? "/ filter (*.pdf works) · g go to a path" : (root.inputMode === "filter" ? "Filter: name or pattern, Enter keeps it, Esc clears" : "Path, Enter goes there")
        focus: true
        // Vim style: keys are commands until `/` (filter) or `g` / `~` (go to a path) opens the box.
        readOnly: root.inputMode === ""
        Keys.onPressed: function(e) {
          var ctrl = e.modifiers & Qt.ControlModifier
          var plain = !ctrl && !(e.modifiers & Qt.AltModifier)
          var typing = root.inputMode !== ""
          if (e.key === Qt.Key_Escape) {
            if (typing) { root.inputMode = ""; text = "" }
            else if (text !== "") text = ""
            else if (root.placesMode) root.placesMode = false
            else root.canceled()
            e.accepted = true
          } else if (e.key === Qt.Key_Down || (!typing && plain && e.key === Qt.Key_J)) { root._move(1); e.accepted = true }
          else if (e.key === Qt.Key_Up || (!typing && plain && e.key === Qt.Key_K)) { root._move(-1); e.accepted = true }
          else if (e.key === Qt.Key_PageDown) { root._move(10); e.accepted = true }
          else if (e.key === Qt.Key_PageUp) { root._move(-10); e.accepted = true }
          else if (e.key === Qt.Key_Return || e.key === Qt.Key_Enter) {
            if (root.inputMode === "filter") root.inputMode = ""      // keep the filter, back to commands
            else { root.activate(); root.inputMode = "" }
            e.accepted = true
          }
          else if (ctrl && e.key === Qt.Key_H) { root.showHidden = !root.showHidden; root.go(root.path); e.accepted = true }
          else if (ctrl && e.key === Qt.Key_T) { root.tickShown(); e.accepted = true }
          else if (ctrl && e.key === Qt.Key_A) { root.tickShown(); e.accepted = true }
          else if (ctrl && e.key === Qt.Key_U) { root.clearTicks(); e.accepted = true }
          else if (ctrl && e.key === Qt.Key_S) { root.cycleSort(); e.accepted = true }
          else if (ctrl && e.key === Qt.Key_B) { root.inputMode = ""; root.togglePlaces(); e.accepted = true }
          else if (ctrl && e.key === Qt.Key_N) { root._move(1); e.accepted = true }
          else if (ctrl && e.key === Qt.Key_P) { root._move(-1); e.accepted = true }
          else if (typing) {
            // Text goes to the box; nothing may leak to the compose pane's shortcuts.
            if (e.key === Qt.Key_Tab || e.key === Qt.Key_Backtab) e.accepted = true
            else if (ctrl) e.accepted = !(e.key === Qt.Key_C || e.key === Qt.Key_V || e.key === Qt.Key_X || e.key === Qt.Key_Z)
          } else {
            // Command mode: every key is ours; none reaches the box or the compose pane.
            e.accepted = true
            if (!plain) return
            if (e.key === Qt.Key_V) { root.toggleMark(root.rows[root.cur]); root._move(1) }
            else if (e.key === Qt.Key_Slash) { root.placesMode = false; root.inputMode = "filter"; text = "" }
            else if (e.key === Qt.Key_G || e.key === Qt.Key_AsciiTilde) { root.placesMode = false; root.inputMode = "path"; text = "~/" }
            else if (e.key === Qt.Key_Right || e.key === Qt.Key_L) root.openCurrent()
            else if (e.key === Qt.Key_Left || e.key === Qt.Key_H || e.key === Qt.Key_Backspace) {
              if (root.placesMode) root.placesMode = false
              else if (text !== "") text = ""
              else if (root.parentPath !== "") root.go(root.parentPath)
            }
          }
        }
      }

      ListView {
        id: listView
        Layout.fillWidth: true
        Layout.fillHeight: true
        clip: true
        model: root.rows
        currentIndex: root.cur
        boundsBehavior: Flickable.StopAtBounds

        delegate: Rectangle {
          id: row
          required property var modelData
          required property int index
          readonly property string full: (modelData.up || modelData.place) ? "" : root._join(root.path, modelData.name)
          readonly property bool ticked: !modelData.up && !modelData.place && (root.marked[full] !== undefined || root._isAttached(full))
          width: listView.width
          height: Style.spacing.popupRowHeight
          radius: Style.cornerRadius
          color: index === root.cur ? Style.selectedFill : "transparent"

          RowLayout {
            anchors.fill: parent
            anchors.leftMargin: Style.spacing.md
            anchors.rightMargin: Style.spacing.md
            spacing: Style.spacing.md
            UiText { Layout.preferredWidth: Style.space(16); text: row.ticked ? "\uf00c" : ""; foreground: Commons.Color.accent }
            UiText { Layout.preferredWidth: Style.space(18); text: row.modelData.dir ? "\uf07b" : "\uf15b"; dim: !row.modelData.dir }
            UiText {
              Layout.fillWidth: true
              text: row.modelData.place ? row.modelData.name + "   " + Pick.shortDir(row.modelData.path, root.home)
                : (row.modelData.dir && !row.modelData.up ? row.modelData.name + "/" : row.modelData.name)
              elide: Text.ElideMiddle
              foreground: Commons.Color.popups.text
              dim: root._isAttached(row.full)
            }
            UiText {
              text: row.modelData.dir ? "" : Fmt.bytes(row.modelData.size) + (root.sortKey === "newest" && row.modelData.mtime ? "  " + Qt.formatDate(new Date(row.modelData.mtime * 1000), "yyyy-MM-dd") : "")
              dim: true
              font.pixelSize: Style.font.caption
            }
          }
          MouseArea {
            anchors.fill: parent
            onClicked: { root.cur = row.index; root.input.forceActiveFocus() }
            onDoubleClicked: { root.cur = row.index; root.activate() }
          }
        }

        UiText {
          anchors.centerIn: parent
          visible: root.rows.length === 0 && !root.loading
          dim: true
          text: root.pathMode ? "Enter to go to this path" : (root.filter !== "" ? "no match" : "this folder is empty")
        }
      }

      UiText {
        Layout.fillWidth: true
        visible: text !== ""
        text: root.error !== "" ? root.error : (root.truncated ? "Only the first entries are shown; type to filter." : "")
        foreground: root.error !== "" ? Commons.Color.urgent : Commons.Color.foreground
        dim: root.error === ""
        font.pixelSize: Style.font.caption
        elide: Text.ElideRight
      }

      UiText {
        Layout.fillWidth: true
        font.pixelSize: Style.font.caption
        elide: Text.ElideRight
        foreground: root.overLimit ? Commons.Color.urgent : Commons.Color.foreground
        dim: !root.overLimit
        text: (root.markedCount > 0
            ? root.markedCount + " ticked · " + Fmt.bytes(root.ticked.bytes) + (root.capacity > 0 ? " of " + Fmt.bytes(Math.max(root.capacity - root.baseBytes, 0)) + " free" : "")
              + (root.overLimit ? " · TOO BIG, Send will refuse" : "") + " · Enter attaches them · "
            : "")
          + "v tick · / filter · g path · l open · Enter done · Ctrl+T tick shown · Ctrl+U clear · Ctrl+S sort · Ctrl+B places · Esc close"
      }
    }
  }

  property alias input: input
  onCanceled: opened = false
}
