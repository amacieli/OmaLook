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
// into the input. `contacts` ([{ name, email }]) feeds the suggestion list.
FocusScope {
  id: root

  property string label: ""
  property var recipients: []
  property var contacts: []

  signal changed()

  // Above the rows below it while it is the one being typed in (its suggestion list overhangs them).
  z: activeFocus ? 50 : 0

  readonly property alias input: field
  readonly property bool hasInvalid: recipients.some(function(r) { return !r.valid })
  implicitHeight: Math.max(Style.spacing.controlHeight, flow.implicitHeight + Style.spacing.md * 2)

  // -- suggestions
  property var _matches: []
  property int _pick: -1

  function _refreshMatches() {
    var q = field.text.trim().toLowerCase()
    if (q.length < 1) { _matches = []; _pick = -1; return }
    var have = {}
    for (var i = 0; i < recipients.length; i++) have[recipients[i].email.toLowerCase()] = true
    var out = []
    for (var j = 0; j < contacts.length && out.length < 6; j++) {
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
      var dup = next.some(function(r) { return r.email.toLowerCase() === list[i].email.toLowerCase() })
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
      addRecipients([{ email: c.email, name: c.name || "", valid: true }])
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
            color: Style.selectedFillFor(chip.modelData.valid ? Commons.Color.accent : Commons.Color.urgent, Commons.Color.accent)
            borderSpec: Border.flat(chip.modelData.valid ? Commons.Color.accent : Commons.Color.urgent, Style.normalBorderWidth)

            Row {
              id: chipRow
              anchors.centerIn: parent
              spacing: Style.spacing.md
              UiText {
                text: chip.modelData.name !== "" ? chip.modelData.name : chip.modelData.email
                foreground: chip.modelData.valid ? Commons.Color.accent : Commons.Color.urgent
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
            if (e.key === Qt.Key_Backspace && text === "" && root.recipients.length > 0) {
              var last = root.recipients[root.recipients.length - 1]
              root.removeAt(root.recipients.length - 1)
              text = last.name !== "" ? last.name + " <" + last.email + ">" : last.email
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
            text: row.modelData.name ? row.modelData.name + "  <" + row.modelData.email + ">" : row.modelData.email
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
