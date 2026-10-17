import QtQuick
import qs.Commons
import qs.Ui
import "../common"

// Number stepper for the UI scale factor: [ - ]  [ 1.0 ]  [ + ]  [ Reset ].
//
// Keyboard (no hotkey needed, you navigate to it with l / Right / Enter from the category list):
//   Up / Down        +/- 0.1, applied at once          Esc     leave the field (value stays)
//   type digits      enter a value directly             Enter   apply the typed value and leave
// A typed value is also applied when the field loses focus. Anything out of range is clamped
// by the caller (`modified` hands over the raw number).
Item {
  id: root

  property real value: 1.0
  property real from: 0.5
  property real to: 3.0
  property real step: 0.1
  property real defaultValue: 1.0
  property alias field: input

  signal modified(real value)   // user asked for this value (not yet clamped)
  signal leave()                // the field wants the shell to take the keyboard back

  implicitWidth: row.implicitWidth
  implicitHeight: row.implicitHeight

  function fmt(v) {
    var s = String(Math.round(v * 100) / 100)
    return s.indexOf(".") < 0 ? s + ".0" : s
  }
  function clamp(v) { return Math.max(from, Math.min(to, v)) }
  function stepBy(dir) { root.modified(clamp(Math.round((root.value + dir * root.step) * 100) / 100)) }
  function commitText() {
    var n = parseFloat(input.text.replace(",", "."))
    if (isFinite(n)) root.modified(clamp(Math.round(n * 100) / 100))
    input.text = root.fmt(root.value)   // re-sync (also resets junk input)
  }
  function focusField() { input.forceActiveFocus(); input.selectAll() }

  onValueChanged: input.text = fmt(value)

  Row {
    id: row
    spacing: Style.spacing.lg

    Button {
      text: "\u2212"
      bordered: true
      focusable: false
      enabled: root.value > root.from + 0.0001
      onClicked: root.stepBy(-1)
    }

    BorderSurface {
      id: box
      width: Style.space(90)
      implicitHeight: Math.max(Style.spacing.controlHeight, input.implicitHeight + Style.spacing.controlPaddingY * 2)
      height: implicitHeight
      radius: Style.cornerRadius
      color: Style.controlFill(input.activeFocus, hover.hovered, Color.foreground, Color.accent)
      borderSpec: Border.controlSpec(input.activeFocus ? "focus" : (hover.hovered ? "hover-cursor" : "normal"), Color.foreground, Color.accent)

      HoverHandler { id: hover }

      TextInput {
        id: input
        anchors.fill: parent
        anchors.margins: Style.spacing.controlPaddingX
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
        text: root.fmt(root.value)
        color: Color.foreground
        selectionColor: Style.selectionFill
        selectedTextColor: Color.foreground
        font.family: Style.font.family
        font.pixelSize: Style.font.body
        inputMethodHints: Qt.ImhFormattedNumbersOnly
        validator: RegularExpressionValidator { regularExpression: /^[0-9]{0,1}([.,][0-9]{0,2})?$/ }
        activeFocusOnTab: true
        selectByMouse: true

        onActiveFocusChanged: {
          if (activeFocus) selectAll()
          else root.commitText()
        }

        Keys.onUpPressed: function(e) { root.stepBy(1); input.selectAll(); e.accepted = true }
        Keys.onDownPressed: function(e) { root.stepBy(-1); input.selectAll(); e.accepted = true }
        Keys.onReturnPressed: function(e) { root.commitText(); root.leave(); e.accepted = true }
        Keys.onEnterPressed: function(e) { root.commitText(); root.leave(); e.accepted = true }
        Keys.onEscapePressed: function(e) { input.text = root.fmt(root.value); root.leave(); e.accepted = true }
      }
    }

    Button {
      text: "+"
      bordered: true
      focusable: false
      enabled: root.value < root.to - 0.0001
      onClicked: root.stepBy(1)
    }

    Button {
      text: "Reset"
      bordered: true
      focusable: false
      visible: Math.abs(root.value - root.defaultValue) > 0.0001
      onClicked: root.modified(root.defaultValue)
    }
  }
}
