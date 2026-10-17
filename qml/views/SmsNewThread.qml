import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Ui
import "../common"
import "sms.js" as Sms

// New text message: recipients (contact names from the phone, or phone numbers) as chips, then
// the text. Replaces the conversation pane while open, like mail's compose.
//
// Keys (this pane owns the keyboard while it is open):
//   To:    type a name or number; Down/Up pick a suggestion; Enter, Tab, comma or ; turn it into a chip;
//          Backspace on an empty field pulls the last chip back; Enter/Tab on an empty field moves to the text
//   Text:  Shift+Tab back to To
//   Ctrl+Enter sends (needs one recipient and some text) · Esc discards (asks first when something is typed)
Item {
  id: root

  required property var app

  property bool opened: false
  property var recipients: []             // [{ label, number }]
  property string status: ""
  property bool sending: false
  property var suggestions: []            // [{ name, number }]
  property int suggestIndex: -1
  property string device: ""

  readonly property bool dirty: recipients.length > 0 || toInput.text.trim() !== "" || body.text.trim() !== ""

  signal sent(var result)
  signal closed()

  visible: opened

  function open() {
    recipients = []; status = ""; sending = false; suggestions = []; suggestIndex = -1
    toInput.text = ""; body.text = ""
    opened = true
    toInput.forceActiveFocus()
  }
  function close() { opened = false; root.closed(); root.app.focusRequested() }
  function requestClose() {
    if (!dirty) { close(); return }
    root.app.pendingConfirm = { message: "Discard this message?", confirmText: "Discard",
                                run: function() { root.close() } }
  }

  // ---- recipients
  function addChip(label, number) {
    var key = String(number).replace(/\D/g, "")
    for (var i = 0; i < recipients.length; i++) if (String(recipients[i].number).replace(/\D/g, "") === key) { toInput.text = ""; return }
    recipients = recipients.concat([{ label: label, number: number }])
    toInput.text = ""; suggestions = []; suggestIndex = -1; status = ""
  }
  // Turn the typed text into a chip: a suggestion, a lone matching contact, or a phone number.
  // Returns false (with a message) when it is neither.
  function commitText() {
    var t = toInput.text.trim().replace(/[,;]+$/, "").trim()
    if (t === "") return true
    if (suggestIndex >= 0 && suggestIndex < suggestions.length) {
      var c = suggestions[suggestIndex]; addChip(c.name, c.number); return true
    }
    if (Sms.looksLikeNumber(t)) { addChip(t, t); return true }
    if (suggestions.length === 1) { addChip(suggestions[0].name, suggestions[0].number); return true }
    if (suggestions.length > 1) { suggestIndex = 0; status = "Several contacts match \u2014 Down/Up to choose, Enter to pick"; return false }
    status = "No contact \u201c" + t + "\u201d \u2014 type a phone number or pick a suggestion"
    return false
  }
  function pullBack() {
    if (recipients.length === 0) return
    var last = recipients[recipients.length - 1]
    recipients = recipients.slice(0, recipients.length - 1)
    toInput.text = last.label; toInput.cursorPosition = toInput.text.length
  }

  function loadSuggestions() {
    var q = toInput.text.trim()
    if (q === "") { suggestions = []; suggestIndex = -1; return }
    var typed = q
    root.app.request("GET", "/sms/contacts?q=" + encodeURIComponent(q) + (device !== "" ? "&device=" + encodeURIComponent(device) : ""), function(xhr) {
      if (xhr.status !== 200 || typed !== toInput.text.trim()) return
      try {
        var r = JSON.parse(xhr.responseText)
        if (r.device) root.device = r.device
        root.suggestions = r.contacts || []
        root.suggestIndex = -1
      } catch (e) {}
    })
  }
  Timer { id: suggestTimer; interval: 120; onTriggered: root.loadSuggestions() }

  // ---- send
  function trySend() {
    if (sending) return
    if (toInput.text.trim() !== "" && !commitText()) return
    if (recipients.length === 0) { status = "Add at least one recipient"; toInput.forceActiveFocus(); return }
    if (body.text.trim() === "") { status = "Nothing to send"; body.forceActiveFocus(); return }
    sending = true; status = ""
    var req = { to: recipients.map(function(r) { return r.number }), text: body.text }
    if (device !== "") req.device = device
    root.app.request("POST", "/sms/send", function(xhr) {
      sending = false
      var r = null
      try { r = JSON.parse(xhr.responseText) } catch (e) {}
      if (!r) { status = "Send failure: the backend did not answer"; return }
      if (r.ok) { opened = false; root.sent(r); return }
      status = "Send failure: " + (r.error || "unknown error")
      // The text was stored as failed when the phone refused it: the conversation shows it with a retry.
      if (r.uid) { opened = false; root.sent(r) }
    }, JSON.stringify(req))
  }

  PaneFrame { tint: Hues.green; focused: true; hotkey: "\u00b2"; title: "New text message" }

  ColumnLayout {
    anchors.fill: parent; anchors.margins: Style.spacing.sm; anchors.topMargin: Style.spacing.xl
    spacing: 0

    // ---- To
    Item {
      Layout.fillWidth: true
      Layout.margins: Style.spacing.lg
      Layout.preferredHeight: toFlow.implicitHeight

      UiText { id: toLabel; text: "To"; dim: true; anchors.left: parent.left; anchors.top: parent.top; anchors.topMargin: Style.spacing.xs; width: Style.space(36) }

      Flow {
        id: toFlow
        anchors.left: toLabel.right; anchors.right: parent.right; anchors.top: parent.top
        spacing: Style.spacing.sm

        Repeater {
          model: root.recipients
          Rectangle {
            required property var modelData
            readonly property bool named: !Sms.looksLikeNumber(modelData.label)
            height: chipText.implicitHeight + Style.spacing.sm * 2
            width: chipText.implicitWidth + Style.spacing.lg * 2
            radius: height / 2
            color: Qt.rgba(Color.accent.r, Color.accent.g, Color.accent.b, 0.22)
            UiText {
              id: chipText
              anchors.centerIn: parent
              text: modelData.label + (named ? "  " + modelData.number : "")
              foreground: Hues.brightForeground
              font.pixelSize: Style.font.bodySmall
            }
          }
        }

        TextInput {
          id: toInput
          width: Math.max(Style.space(160), toFlow.width - 0)
          height: Style.font.body * 2
          verticalAlignment: TextInput.AlignVCenter
          color: Hues.brightForeground
          selectionColor: Color.accent
          selectedTextColor: Color.background
          font.family: Style.font.family
          font.pixelSize: Style.font.body
          clip: true
          onTextChanged: {
            if (/[,;]$/.test(text)) { root.commitText(); return }
            root.status = ""; suggestTimer.restart()
          }
          Keys.onPressed: function(e) {
            var ctrl = e.modifiers & Qt.ControlModifier
            if (e.key === Qt.Key_Escape) {
              if (root.suggestions.length > 0) { root.suggestions = []; root.suggestIndex = -1 } else root.requestClose()
              e.accepted = true
            } else if (ctrl && (e.key === Qt.Key_Return || e.key === Qt.Key_Enter)) { root.trySend(); e.accepted = true }
            else if (e.key === Qt.Key_Down && root.suggestions.length > 0) { root.suggestIndex = Math.min(root.suggestions.length - 1, root.suggestIndex + 1); e.accepted = true }
            else if (e.key === Qt.Key_Up && root.suggestions.length > 0) { root.suggestIndex = Math.max(-1, root.suggestIndex - 1); e.accepted = true }
            else if (e.key === Qt.Key_Return || e.key === Qt.Key_Enter || e.key === Qt.Key_Tab) {
              if (toInput.text.trim() === "") { body.forceActiveFocus() } else root.commitText()
              e.accepted = true
            } else if (e.key === Qt.Key_Backspace && toInput.text === "") { root.pullBack(); e.accepted = true }
          }
          UiText {
            visible: toInput.text === "" && root.recipients.length === 0
            text: "Name or phone number"
            dim: true
            anchors.verticalCenter: parent.verticalCenter
          }
        }
      }
    }

    // ---- suggestions
    Column {
      visible: root.suggestions.length > 0
      Layout.fillWidth: true
      Layout.leftMargin: Style.spacing.lg + Style.space(36); Layout.rightMargin: Style.spacing.lg
      Repeater {
        model: root.suggestions
        Rectangle {
          required property var modelData
          required property int index
          width: parent.width
          height: row.implicitHeight + Style.spacing.md * 2
          color: root.suggestIndex === index ? Style.hoverFillFor(Color.foreground, Color.accent) : "transparent"
          RowLayout {
            id: row
            anchors.fill: parent; anchors.leftMargin: Style.spacing.lg; anchors.rightMargin: Style.spacing.lg
            UiText { text: modelData.name; foreground: Hues.brightForeground; Layout.fillWidth: true; elide: Text.ElideRight }
            UiText { text: modelData.number; dim: true; font.pixelSize: Style.font.bodySmall }
          }
          MouseArea { anchors.fill: parent; onClicked: { root.suggestIndex = index; root.commitText(); body.forceActiveFocus() } }
        }
      }
    }

    PanelSeparator { Layout.fillWidth: true }

    // ---- text
    Item {
      Layout.fillWidth: true
      Layout.fillHeight: true

      Flickable {
        id: bodyFlick
        anchors.fill: parent
        anchors.margins: Style.spacing.lg
        contentWidth: width; contentHeight: body.contentHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds

        TextEdit {
          id: body
          width: bodyFlick.width
          height: Math.max(implicitHeight, bodyFlick.height)
          wrapMode: TextEdit.Wrap
          color: Hues.brightForeground
          selectionColor: Color.accent
          selectedTextColor: Color.background
          font.family: Style.font.family
          font.pixelSize: Style.font.body
          onCursorRectangleChanged: {
            if (cursorRectangle.y < bodyFlick.contentY) bodyFlick.contentY = cursorRectangle.y
            else if (cursorRectangle.y + cursorRectangle.height > bodyFlick.contentY + bodyFlick.height) bodyFlick.contentY = cursorRectangle.y + cursorRectangle.height - bodyFlick.height
          }
          Keys.onPressed: function(e) {
            var ctrl = e.modifiers & Qt.ControlModifier
            if (e.key === Qt.Key_Escape) { root.requestClose(); e.accepted = true }
            else if (ctrl && (e.key === Qt.Key_Return || e.key === Qt.Key_Enter)) { root.trySend(); e.accepted = true }
            else if (e.key === Qt.Key_Backtab) { toInput.forceActiveFocus(); e.accepted = true }
          }
          UiText { visible: body.text === ""; text: "Write a text message\u2026"; dim: true }
        }
      }
    }

    PanelSeparator { Layout.fillWidth: true }

    UiText {
      Layout.fillWidth: true
      Layout.margins: Style.spacing.lg
      text: root.sending ? "Sending\u2026"
          : root.status !== "" ? (root.status.indexOf("Send failure") === 0 ? "\u26a0 " : "") + root.status
          : "Ctrl+Enter send \u00b7 Esc discard"
      foreground: root.status !== "" ? Hues.red : Hues.muted
      elide: Text.ElideRight
      font.pixelSize: Style.font.caption
    }
  }
}
