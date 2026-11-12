import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"
import "../mail/format.js" as Fmt
import "attach.js" as Att

// The compose pane's attachment line (docs/PLAN-compose-attachments.md 3.5): one row of constant
// height between the header and the editor, however many files there are.
//
//   📎 3 files · 4.2 MB of 25 MB   [report.pdf ×] [budget.xlsx ×] [notes.txt ×]     Ctrl+Shift+A add · Ctrl+Shift+L list
//   📎 40 files · 12 MB of 25 MB   [a.pdf ×] [b.pdf ×] +38                          Ctrl+Shift+A add · Ctrl+Shift+L list
//
// With no files the strip does not exist (it takes no height). The meter is text, in the warning
// colour above 80 % of the limit and the error colour when the daemon would refuse to send.
// Pure view: layout rules and numbers come from attach.js (tested with node).
Item {
  id: root

  // Rows of [originals of a forward..., picked files...]: { name, size, origin: "original"|"picked", ref }
  property var items: []
  // What the From account's provider accepts: { provider, basis, hard, warn } (attach.js defaultLimits).
  property var limits: Att.defaultLimits("")

  signal removeRequested(int row)
  signal listRequested()

  readonly property var shape: Att.layout(items.length)
  readonly property var meter: Att.meter(limits, items)
  readonly property var verdict: Att.states(limits, items)
  readonly property var names: Att.displayNames(items)
  property var missing: []     // file names of draft attachments whose spool copy is gone

  readonly property color levelColor: meter.level === "over" ? Commons.Color.urgent
    : meter.level === "warn" ? Hues.yellow : Commons.Color.foreground

  // Constant when shown: this is what keeps the editor from jumping between 1 and 40 files.
  readonly property real rowHeight: Style.spacing.popupRowHeight + Style.spacing.sm * 2
  visible: shape.show
  implicitHeight: shape.show ? rowHeight : 0
  Layout.preferredHeight: shape.show ? rowHeight : 0

  MouseArea {
    anchors.fill: parent
    cursorShape: Qt.PointingHandCursor
    onClicked: root.listRequested()
  }

  RowLayout {
    anchors.fill: parent
    anchors.leftMargin: Style.spacing.lg
    anchors.rightMargin: Style.spacing.lg
    spacing: Style.spacing.md

    UiText {
      text: "\uf0c6 " + root.items.length + (root.items.length === 1 ? " file" : " files")
      font.pixelSize: Style.font.caption
      font.bold: true
    }
    UiText {
      text: "\u00b7 " + Fmt.bytes(root.meter.raw) + " of " + Fmt.bytes(root.meter.capacity)
      foreground: root.levelColor
      dim: root.meter.level === "ok"
      font.pixelSize: Style.font.caption
    }

    // The chips take what is left; a chip never pushes the right-hand hint or the strip taller.
    Row {
      id: chips
      Layout.fillWidth: true
      Layout.minimumWidth: 0
      Layout.alignment: Qt.AlignVCenter
      spacing: Style.spacing.sm
      clip: true

      Repeater {
        model: root.shape.chips
        delegate: Rectangle {
          id: chip
          required property int index
          readonly property var item: root.items[index]
          readonly property bool big: root.verdict.state[index] === "big"
          // A draft attachment whose spool copy is gone: kept, flagged, never dropped silently.
          readonly property bool gone: !!item && root.missing.indexOf(item.name || "") >= 0
          readonly property bool original: !!item && item.origin === "original"
          // Chips share what the row has left, so the last of three is never pushed out of sight.
          readonly property real share: Math.max(Style.space(90),
            (chips.width - (root.shape.chips - 1) * chips.spacing - (root.shape.more > 0 ? moreLabel.width + chips.spacing : 0)) / Math.max(root.shape.chips, 1))
          width: Math.min(chipRow.implicitWidth + Style.spacing.md * 2, Style.space(220), share)
          height: Style.spacing.popupRowHeight
          radius: Style.cornerRadius
          color: big || gone ? Qt.rgba(Commons.Color.urgent.r, Commons.Color.urgent.g, Commons.Color.urgent.b, 0.25) : Style.selectedFill

          RowLayout {
            id: chipRow
            anchors.fill: parent
            anchors.leftMargin: Style.spacing.md
            anchors.rightMargin: Style.spacing.md
            spacing: Style.spacing.sm
            // state glyph: ! too big for this account, forwarded original, else a plain file
            UiText {
              text: chip.big || chip.gone ? "!" : chip.original ? "\uf064" : "\uf0c6"
              foreground: chip.big || chip.gone ? Commons.Color.urgent : Commons.Color.foreground
              dim: !(chip.big || chip.gone)
              font.bold: chip.big || chip.gone
            }
            UiText {
              Layout.fillWidth: true
              Layout.minimumWidth: 0
              text: (root.names[chip.index] || "") + (chip.gone ? "  (file missing)" : "")
              elide: Text.ElideMiddle
              font.pixelSize: Style.font.caption
            }
            UiText {
              text: "\uf00d"
              foreground: Commons.Color.urgent
              MouseArea {
                anchors.fill: parent
                anchors.margins: -Style.spacing.xs
                cursorShape: Qt.PointingHandCursor
                onClicked: root.removeRequested(chip.index)
              }
            }
          }
          MouseArea {
            id: hover
            anchors.fill: parent
            anchors.rightMargin: Style.spacing.xl
            hoverEnabled: true
            acceptedButtons: Qt.NoButton
          }
          PanelToolTip {
            visible: hover.containsMouse
            text: chip.item
              ? chip.item.name + " (" + Fmt.bytes(chip.item.size) + ")"
                + (chip.original ? "\nforwarded from the original message" : "")
                + (root.verdict.reason[chip.index] !== "" ? "\n" + root.verdict.reason[chip.index] : "")
              : ""
          }
        }
      }

      UiText {
        id: moreLabel
        visible: root.shape.more > 0
        anchors.verticalCenter: parent.verticalCenter
        text: "+" + root.shape.more
        font.pixelSize: Style.font.caption
        foreground: root.verdict.state.indexOf("big") >= 0 ? Commons.Color.urgent : Commons.Color.foreground
      }
    }

    UiText {
      visible: root.width > Style.space(640)
      text: "Ctrl+Shift+A add \u00b7 Ctrl+Shift+L list"
      dim: true
      font.pixelSize: Style.font.caption
    }
  }
}
