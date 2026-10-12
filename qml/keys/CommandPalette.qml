import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Ui
import "../common"
import "Commands.js" as Cmd

// `:` / `?` / Ctrl-P — searchable list of every command that applies right now,
// each with its key. Type to filter, Up/Down (or Ctrl-N/Ctrl-P) to choose, Enter to run,
// Esc to close. Reads the same registry the keys use.
Item {
  id: root

  property var commands: []
  property var scopes: ["global"]
  property bool open: false

  signal closed()

  // After a keepOpen row ran, the list rebuilds (a saved attachment gets its check mark) and a
  // rebuilt ListView starts at row 0; put the highlight back where it was.
  property int _restore: -1
  onEntriesChanged: {
    if (_restore < 0) return
    var r = _restore
    _restore = -1
    Qt.callLater(function() { list.currentIndex = Math.min(r, Math.max(0, root.entries.length - 1)) })
  }

  visible: open
  function show(prefill) {
    input.text = prefill || ""
    list.currentIndex = 0
    open = true
    input.forceActiveFocus()
    input.cursorPosition = input.text.length
  }
  function hide() { if (!open) return; open = false; closed() }

  // Typed commands: "goto cal", "folder inb", "q", "sync". A command whose `ex` names
  // match the first word leads the list; with a space after it and a `pick`, the list
  // becomes that picker's choices (modules, folders).
  readonly property var entries: {
    var all = Cmd.applicable(commands, scopes)
    var q = input.text
    if (q === "") return all
    var p = Cmd.parseEx(q)
    var ex = p ? Cmd.findEx(commands, p.name) : null
    if (ex && ex.pick && p.hasSpace) return ex.pick(p.arg)
    var rest = all.filter(function(c) { return c !== ex && Cmd.fuzzy(q, c.title + " " + c.id) })
    return ex ? [ex].concat(rest) : rest
  }

  function runCurrent() {
    if (list.currentIndex < 0 || list.currentIndex >= entries.length) return
    var c = entries[list.currentIndex]
    // A picker command chosen without an argument asks for one instead of running.
    if (c.pick && c.ex && Cmd.parseEx(input.text) && !Cmd.parseEx(input.text).hasSpace) {
      input.text = c.ex[0] + " "
      input.cursorPosition = input.text.length
      return
    }
    // A picker row marked keepOpen (saving an attachment) leaves the list up for the next choice.
    if (c.keepOpen) { root._restore = list.currentIndex; c.run(); return }
    hide()
    c.run()
  }

  // Dim the app behind the card; a click outside closes.
  Rectangle { anchors.fill: parent; color: "#99000000" }
  MouseArea { anchors.fill: parent; onClicked: root.hide() }

  Rectangle {
    id: card
    anchors.horizontalCenter: parent.horizontalCenter
    y: Math.round(parent.height * 0.14)
    width: Math.min(parent.width - Style.space(40), Style.space(560))
    height: Math.min(parent.height - y - Style.space(40), Style.space(60) + list.contentHeight + Style.spacing.lg * 2)
    color: Color.background
    border.width: 2
    border.color: Color.accent
    radius: Style.cornerRadius

    MouseArea { anchors.fill: parent }   // swallow clicks on the card

    ColumnLayout {
      anchors.fill: parent
      anchors.margins: Style.spacing.lg
      spacing: Style.spacing.md

      TextField {
        id: input
        Layout.fillWidth: true
        placeholderText: "Type a command, or: goto <view>   folder <name>   search <pattern>   sync   q"
        onTextChanged: { root._restore = -1; list.currentIndex = 0 }
        Keys.priority: Keys.BeforeItem
        Keys.onPressed: function(e) {
          var ctrl = e.modifiers & Qt.ControlModifier
          if (e.key === Qt.Key_Escape) { root.hide(); e.accepted = true }
          else if (e.key === Qt.Key_Return || e.key === Qt.Key_Enter) { root.runCurrent(); e.accepted = true }
          else if (e.key === Qt.Key_Down || (ctrl && (e.key === Qt.Key_N || e.key === Qt.Key_J))) {
            list.currentIndex = Math.min(list.currentIndex + 1, root.entries.length - 1); e.accepted = true
          } else if (e.key === Qt.Key_Up || (ctrl && (e.key === Qt.Key_P || e.key === Qt.Key_K))) {
            list.currentIndex = Math.max(list.currentIndex - 1, 0); e.accepted = true
          }
        }
      }

      ListView {
        id: list
        Layout.fillWidth: true
        Layout.fillHeight: true
        clip: true
        interactive: contentHeight > height
        boundsBehavior: Flickable.StopAtBounds
        model: root.entries
        onCurrentIndexChanged: positionViewAtIndex(currentIndex, ListView.Contain)

        UiText {
          anchors.centerIn: parent
          visible: list.count === 0
          text: "no matching command"
          dim: true
        }

        delegate: Rectangle {
          required property var modelData
          required property int index
          width: list.width
          height: rowText.implicitHeight + Style.spacing.md * 2
          color: list.currentIndex === index ? Style.selectedAccentFill : "transparent"

          RowLayout {
            anchors.fill: parent
            anchors.leftMargin: Style.spacing.md
            anchors.rightMargin: Style.spacing.md
            spacing: Style.spacing.lg

            Rectangle {
              visible: !!modelData.swatch
              Layout.preferredWidth: Style.space(12)
              Layout.preferredHeight: Style.space(12)
              color: modelData.swatch ? modelData.swatch : "transparent"
              radius: Math.max(Style.cornerRadius, 2)
            }

            UiText {
              id: rowText
              Layout.fillWidth: true
              text: modelData.title
              elide: Text.ElideRight
              foreground: list.currentIndex === index ? Color.accent : Color.foreground
            }
            UiText {
              text: Cmd.keyLabel(modelData.keys)
              dim: true
              font.pixelSize: Style.font.caption
            }
          }

          MouseArea {
            anchors.fill: parent
            onClicked: { list.currentIndex = index; root.runCurrent() }
          }
        }
      }
    }
  }
}
