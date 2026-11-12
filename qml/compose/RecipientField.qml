import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"
import "modes.js" as Modes

// One address row (To / Cc / Bcc): a label, address chips and an inline input.
// `recipients` is a list of { email, name, valid }. Typing a , ; Enter or Tab (or pasting a
// list) turns the text into chips; Backspace on an empty input takes the last chip back
// into the input. `contacts` ([{ name, email }]) feeds the suggestion list, and so do the
// distribution `lists` ([{ id, name, count }]): a list is offered first (marked with the list glyph)
// and becomes ONE chip { email: "", name, list: id }; the backend replaces it with the members
// when the message is queued, so editing a list before Send is honoured. Typing `@` offers lists only.
FocusScope {
  id: root

  property string label: ""
  property var recipients: []
  property var contacts: []
  property var lists: []

  signal changed()

  // Above the rows below it while it is the one being typed in (its suggestion list overhangs them).
  z: activeFocus ? 50 : 0

  readonly property alias input: field
  readonly property bool hasInvalid: recipients.some(function(r) { return !isValid(r) })

  // A list chip is valid while the list still exists and has members.
  function listInfo(id) {
    for (var i = 0; i < lists.length; i++) if (lists[i].id === id) return lists[i]
    return null
  }
  function isValid(r) {
    if (r.list) { var l = listInfo(r.list); return !!l && l.count > 0 }
    return r.valid !== false
  }
  function chipText(r) {
    if (r.list) { var l = listInfo(r.list); return "\u2261 " + (l ? l.name : r.name) + " (" + (l ? l.count : "?") + ")" }
    return r.name !== "" ? r.name : r.email
  }
  implicitHeight: Math.max(Style.spacing.controlHeight, flow.implicitHeight + Style.spacing.md * 2)

  // -- suggestions
  property var _matches: []
  property int _pick: -1

  function _refreshMatches() {
    var q = field.text.trim().toLowerCase()
    if (q.length < 1) { _matches = []; _pick = -1; return }
    var listsOnly = q.charAt(0) === "@"
    var lq = listsOnly ? q.slice(1) : q
    var have = {}, haveList = {}
    for (var i = 0; i < recipients.length; i++) {
      if (recipients[i].list) haveList[recipients[i].list] = true
      else have[recipients[i].email.toLowerCase()] = true
    }
    var out = []
    // Lists first: a name that starts with what is typed before one that merely contains it.
    var starts = [], inside = []
    for (var k = 0; k < lists.length; k++) {
      var ln = lists[k].name.toLowerCase()
      if (haveList[lists[k].id]) continue
      var at = ln.indexOf(lq)
      if (at === 0) starts.push(lists[k]); else if (at > 0) inside.push(lists[k])
    }
    var ls = starts.concat(inside).slice(0, 4)
    for (var m = 0; m < ls.length; m++) out.push({ list: ls[m].id, name: ls[m].name, count: ls[m].count, email: "" })
    if (listsOnly) { _matches = out; _pick = out.length > 0 ? 0 : -1; return }
    for (var j = 0; j < contacts.length && out.length < 6 + ls.length; j++) {
      var c = contacts[j]
      if (have[c.email.toLowerCase()]) continue
      if (c.email.toLowerCase().indexOf(q) >= 0 || (c.name || "").toLowerCase().indexOf(q) >= 0) out.push(c)
    }
    _matches = out
    _pick = out.length > 0 ? 0 : -1
  }

  function addRecipients(list) {
    if (!list || list.length === 0) return
    var next = recipients.slice()
    for (var i = 0; i < list.length; i++) {
      var dup = next.some(function(r) {
        if (list[i].list || r.list) return list[i].list === r.list
        return r.email.toLowerCase() === list[i].email.toLowerCase()
      })
      if (!dup) next.push(list[i])
    }
    recipients = next
    changed()
  }

  // Turn whatever is typed into chips. Returns true if anything was committed.
  // `raw` skips the suggestion list (used by Send: what was typed is what is sent).
  function commit(raw) {
    if (field.text.trim() === "") return false
    if (raw !== true && _pick >= 0 && _matches.length > 0) {
      var c = _matches[_pick]
      if (c.list) addRecipients([{ email: "", name: c.name, list: c.list, valid: c.count > 0 }])
      else addRecipients([{ email: c.email, name: c.name || "", valid: true }])
    } else {
      addRecipients(Modes.parseRecipients(field.text))
    }
    field.text = ""
    _matches = []; _pick = -1
    return true
  }

  function removeAt(i) {
    var next = recipients.slice()
    next.splice(i, 1)
    recipients = next
    changed()
  }

  // Programmatic fill, e.g. "a@b.c, d@e.f" for a reply.
  function setFromText(s) { recipients = []; addRecipients(Modes.parseRecipients(s)) }

  // Fill from message-shaped entries ({ email, name, list }), e.g. when a send is taken back.
  function setRecipients(arr) {
    recipients = []
    addRecipients((arr || []).map(function(a) {
      return a.list ? { email: "", name: a.name || "", list: a.list, valid: true }
                    : { email: a.email, name: a.name || "", valid: Modes.isValidEmail(a.email) }
    }))
  }

  RowLayout {
    anchors.fill: parent
    spacing: Style.spacing.lg

    UiText {
      Layout.preferredWidth: Style.space(52)
      Layout.alignment: Qt.AlignTop
      Layout.topMargin: Style.spacing.md + 2
      text: root.label
      dim: true
      font.pixelSize: Style.font.caption
    }

    BorderSurface {
      id: box
      Layout.fillWidth: true
      Layout.fillHeight: true
      radius: Style.cornerRadius
      color: Style.normalFill
      borderSpec: Border.controlSpec(field.activeFocus ? "focus" : "normal", Commons.Color.foreground, Commons.Color.accent)

      MouseArea { anchors.fill: parent; onClicked: field.forceActiveFocus() }

      Flow {
        id: flow
        anchors.fill: parent
        anchors.margins: Style.spacing.md
        spacing: Style.spacing.sm

        Repeater {
          model: root.recipients
          delegate: BorderSurface {
            id: chip
            required property var modelData
            required property int index
            implicitWidth: chipRow.implicitWidth + Style.spacing.lg * 2
            implicitHeight: chipRow.implicitHeight + Style.spacing.xs * 2
            radius: Style.cornerRadius
            readonly property bool ok: { root.lists; return root.isValid(chip.modelData) }
            color: Style.selectedFillFor(chip.ok ? Commons.Color.accent : Commons.Color.urgent, Commons.Color.accent)
            borderSpec: Border.flat(chip.ok ? Commons.Color.accent : Commons.Color.urgent, Style.normalBorderWidth)

            Row {
              id: chipRow
              anchors.centerIn: parent
              spacing: Style.spacing.md
              UiText {
                text: { root.lists; return root.chipText(chip.modelData) }
                foreground: chip.ok ? Commons.Color.accent : Commons.Color.urgent
                font.pixelSize: Style.font.caption
              }
              UiText {
                text: "\u00d7"
                dim: true
                font.pixelSize: Style.font.caption
                MouseArea { anchors.fill: parent; anchors.margins: -4; cursorShape: Qt.PointingHandCursor; onClicked: root.removeAt(chip.index) }
              }
            }
          }
        }

        TextInput {
          id: field
          focus: true
          width: root.recipients.length === 0 ? flow.width - 4 : Math.min(Style.space(220), flow.width - 4)
          height: Style.font.body + Style.spacing.md
          verticalAlignment: TextInput.AlignVCenter
          color: Commons.Color.foreground
          selectionColor: Style.selectionFill
          font.family: Style.font.family
          font.pixelSize: Style.font.body
          clip: true
          onTextChanged: {
            // A pasted list / typed separator becomes chips at once.
            if (/[,;\n]/.test(text)) {
              var parts = text.split(/[,;\n]+/)
              var tail = /[,;\n]\s*$/.test(text) ? "" : parts.pop()
              root.addRecipients(Modes.parseRecipients(parts.join(",")))
              text = tail
            }
            root._refreshMatches()
          }
          onActiveFocusChanged: if (!activeFocus) { root.commit(); root._matches = [] }
          Keys.onPressed: function(e) {
            if (e.key === Qt.Key_Backspace && !(e.modifiers & Qt.ControlModifier) && text === "" && root.recipients.length > 0) {   // Ctrl+Shift+Backspace belongs to the attachments
              var last = root.recipients[root.recipients.length - 1]
              root.removeAt(root.recipients.length - 1)
              text = last.list ? last.name : (last.name !== "" ? last.name + " <" + last.email + ">" : last.email)
              e.accepted = true
            } else if (e.key === Qt.Key_Down && root._matches.length > 0) {
              root._pick = Math.min(root._pick + 1, root._matches.length - 1); e.accepted = true
            } else if (e.key === Qt.Key_Up && root._matches.length > 0) {
              root._pick = Math.max(root._pick - 1, 0); e.accepted = true
            } else if (e.key === Qt.Key_Return || e.key === Qt.Key_Enter) {
              if (!(e.modifiers & Qt.ControlModifier)) { root.commit(); e.accepted = true }
            } else if (e.key === Qt.Key_Tab) {
              root.commit()      // then let the focus chain move on
            } else if (e.key === Qt.Key_Escape && root._matches.length > 0) {
              root._matches = []; root._pick = -1; e.accepted = true
            }
          }
        }
      }
    }
  }

  // Suggestion list: an overlay under the field, not a Popup, so typing keeps focus.
  // A plain Rectangle, not BorderSurface: the kit surface paints its fill translucent, and this
  // list floats over the fields below it, so it needs a solid background.
  Rectangle {
    id: sugg
    visible: root._matches.length > 0 && field.activeFocus
    z: 100
    x: box.x
    y: root.height + Style.spacing.xs
    width: Math.min(box.width, Style.space(420))
    height: suggCol.implicitHeight + Style.spacing.sm * 2
    radius: Style.cornerRadius
    color: Qt.rgba(Commons.Color.popups.background.r, Commons.Color.popups.background.g, Commons.Color.popups.background.b, 1)
    border.width: Style.normalBorderWidth
    border.color: Commons.Color.popups.border

    Column {
      id: suggCol
      x: Style.spacing.sm; y: Style.spacing.sm
      width: parent.width - Style.spacing.sm * 2
      Repeater {
        model: root._matches
        delegate: Rectangle {
          id: row
          required property var modelData
          required property int index
          width: suggCol.width
          height: Style.spacing.popupRowHeight
          radius: Style.cornerRadius
          color: index === root._pick ? Style.selectedFill : "transparent"
          UiText {
            anchors.verticalCenter: parent.verticalCenter
            x: Style.spacing.lg
            width: parent.width - Style.spacing.lg * 2
            elide: Text.ElideRight
            foreground: Commons.Color.popups.text
            text: row.modelData.list ? "\u2261 " + row.modelData.name + "   list \u00b7 " + row.modelData.count + (row.modelData.count === 1 ? " member" : " members")
              : (row.modelData.name ? row.modelData.name + "  <" + row.modelData.email + ">" : row.modelData.email)
          }
          MouseArea {
            anchors.fill: parent
            hoverEnabled: true
            onEntered: root._pick = row.index
            onClicked: { root._pick = row.index; root.commit(); field.forceActiveFocus() }
          }
        }
      }
    }
  }
}
