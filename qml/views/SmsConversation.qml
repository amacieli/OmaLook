import QtQuick
import QtQuick.Layouts
import QtQuick.Controls
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"
import "sms.js" as Sms

// Conversation pane: the open thread's history, oldest at the top, newest at the bottom.
// My messages sit on the right, the other side's on the left. A composer line sits under the
// history: Ctrl+Enter sends, Esc hands the keyboard back (the draft stays). Texts still
// waiting for the phone show "sending…"; ones that never went out show a red "Send failure".
// Model (oldest first) roles: body, date_ms, direction, att_text (non-image attachments), images (JSON [{part_id, unique_id}]).
Item {
  id: root

  property var model
  property var app
  property string title: ""
  property string subtitle: ""
  property bool paneFocused: false
  property bool hasThread: false
  property bool loading: false
  property bool hasMore: false
  property string hotkey: "\u00b2"

  // composer
  property string sendError: ""          // last send problem, shown under the composer
  property bool sending: false
  readonly property bool composerFocused: input.activeFocus
  readonly property string draft: input.text
  signal olderRequested()
  signal sendRequested(string text)
  signal leaveRequested()
  signal retryRequested()

  function focusComposer() { input.forceActiveFocus() }
  function setDraft(t) { input.text = t || "" }
  function clearDraft() { input.text = "" }

  PaneFrame { tint: Hues.green; focused: root.paneFocused; hotkey: root.hotkey; title: root.hasThread ? root.title : "Conversation" }

  // ---- keyboard scrolling (called through SmsView by the key registry)
  function scrollBy(px) {
    var max = Math.max(0, list.contentHeight - list.height)
    list.contentY = Math.max(0, Math.min(max, list.contentY + px))
  }
  function scrollPages(frac) { scrollBy(frac * list.height * 0.9) }
  function scrollEdge(bottom) {
    if (bottom) list.positionViewAtEnd()
    else { list.positionViewAtBeginning(); root.olderRequested() }
  }
  function toBottom() { list.positionViewAtEnd(); list.positionViewAtEnd() }
  readonly property bool atBottom: list.atYEnd
  // Keep the visible messages in place after older ones were inserted above them.
  function keepAfterPrepend(count) { list.positionViewAtIndex(count, ListView.Beginning) }

  ColumnLayout {
    anchors.fill: parent; anchors.margins: Style.spacing.sm; anchors.topMargin: Style.spacing.xl
    spacing: 0

    RowLayout {
      visible: root.hasThread
      Layout.fillWidth: true
      Layout.margins: Style.spacing.lg
      spacing: Style.spacing.md
      UiText {
        text: root.title
        Layout.fillWidth: true
        elide: Text.ElideRight
        foreground: root.paneFocused ? Commons.Color.accent : Commons.Color.foreground
        font.pixelSize: Style.font.subtitle
        font.bold: true
      }
      UiText { text: root.subtitle; dim: true; font.pixelSize: Style.font.caption }
    }

    PanelSeparator { visible: root.hasThread; Layout.fillWidth: true }

    ListView {
      id: list
      Layout.fillWidth: true
      Layout.fillHeight: true
      clip: true
      boundsBehavior: Flickable.StopAtBounds
      ScrollBar.vertical: ThemedScrollBar {}
      model: root.model
      spacing: Style.spacing.sm
      topMargin: Style.spacing.lg
      bottomMargin: Style.spacing.lg

      // Scrolled to the top: pull in the next older page.
      onAtYBeginningChanged: if (atYBeginning && count > 0 && root.hasMore && !root.loading) root.olderRequested()

      UiText {
        anchors.centerIn: parent
        visible: !root.hasThread || (list.count === 0 && !root.loading)
        text: root.hasThread ? "no messages" : "Select a conversation"
        dim: true
      }

      header: Item {
        width: list.width
        height: root.hasMore || root.loading ? Style.space(28) : 0
        UiText {
          anchors.centerIn: parent
          visible: root.hasMore || root.loading
          text: root.loading ? "loading…" : "scroll up for older messages"
          dim: true
          font.pixelSize: Style.font.caption
        }
      }

      delegate: Item {
        id: msg
        width: list.width
        height: col.implicitHeight

        readonly property bool mine: Sms.isMine(model)
        readonly property real maxBubble: Math.max(Style.space(160), msg.width * 0.7)
        readonly property var prev: index > 0 ? root.model.get(index - 1) : null
        readonly property bool newDay: prev === null || !Sms.sameDay(new Date(prev.date_ms), new Date(model.date_ms))
        readonly property string attText: model.att_text
        readonly property string deviceId: model.device_id
        readonly property var images: { try { return JSON.parse(model.images || "[]") } catch (e) { return [] } }
        readonly property real imageWidth: Math.min(Style.space(260), msg.maxBubble - Style.spacing.xl * 2)
        readonly property bool failed: model.direction === 5
        readonly property bool queued: model.direction === 6

        Column {
          id: col
          width: parent.width
          spacing: Style.spacing.xs

          // day divider
          Item {
            visible: msg.newDay
            width: parent.width
            height: visible ? dayText.implicitHeight + Style.spacing.lg : 0
            UiText {
              id: dayText
              anchors.centerIn: parent
              text: Sms.dayLabel(model.date_ms)
              foreground: Hues.muted
              font.pixelSize: Style.font.caption
            }
          }

          Item {
            width: parent.width
            height: bubble.height + stamp.implicitHeight + Style.spacing.xxs

            Rectangle {
              id: bubble
              width: Math.min(msg.maxBubble, Math.max(bodyText.implicitWidth, attLine.implicitWidth, msg.images.length > 0 ? msg.imageWidth : 0) + Style.spacing.xl * 2)
              height: inner.implicitHeight + Style.spacing.lg * 2
              x: msg.mine ? parent.width - width - Style.spacing.xl : Style.spacing.xl
              radius: Style.space(10)
              color: msg.mine ? Qt.rgba(Commons.Color.accent.r, Commons.Color.accent.g, Commons.Color.accent.b, 0.28)
                              : Qt.rgba(Commons.Color.foreground.r, Commons.Color.foreground.g, Commons.Color.foreground.b, 0.10)
              border.width: msg.failed ? 1 : 0
              border.color: Hues.red
              opacity: msg.queued ? 0.6 : 1

              Column {
                id: inner
                anchors.fill: parent
                anchors.margins: Style.spacing.lg
                spacing: Style.spacing.xs
                UiText {
                  id: bodyText
                  visible: model.body !== ""
                  width: parent.width
                  text: model.body
                  wrapMode: Text.Wrap
                  foreground: Hues.brightForeground
                }
                Repeater {
                  model: msg.images
                  delegate: SmsImage {
                    required property var modelData
                    app: root.app
                    device: msg.deviceId
                    partId: modelData.part_id
                    uniqueId: modelData.unique_id
                    preview: modelData.preview || ""
                    width: msg.imageWidth
                  }
                }
                UiText {
                  id: attLine
                  visible: msg.attText !== ""
                  width: parent.width
                  text: "\uf0c6 " + msg.attText
                  foreground: Hues.cyan
                  font.pixelSize: Style.font.bodySmall
                  elide: Text.ElideRight
                }
              }
            }

            UiText {
              id: stamp
              anchors.top: bubble.bottom
              anchors.topMargin: Style.spacing.xxs
              x: msg.mine ? parent.width - width - Style.spacing.xl : Style.spacing.xl
              text: Sms.clock(model.date_ms) + (msg.failed ? "   \u26a0 Send failure \u2014 R to retry" : msg.queued ? "   sending\u2026" : "")
              foreground: msg.failed ? Hues.red : Hues.muted
              font.pixelSize: Style.font.caption
            }
          }
        }
      }
    }

    // ---- composer
    PanelSeparator { visible: root.hasThread; Layout.fillWidth: true }

    Item {
      id: composer
      visible: root.hasThread
      Layout.fillWidth: true
      Layout.preferredHeight: visible ? Math.min(Style.space(160), Math.max(Style.space(48), input.contentHeight + Style.spacing.lg * 2)) + hint.implicitHeight + Style.spacing.md : 0

      Flickable {
        id: flick
        anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
        anchors.margins: Style.spacing.lg
        height: composer.height - hint.implicitHeight - Style.spacing.md - Style.spacing.lg
        contentWidth: width; contentHeight: input.contentHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds

        TextEdit {
          id: input
          width: flick.width
          wrapMode: TextEdit.Wrap
          color: Hues.brightForeground
          selectionColor: Commons.Color.accent
          selectedTextColor: Commons.Color.background
          font.family: Style.font.family
          font.pixelSize: Style.font.body
          activeFocusOnTab: false
          onCursorRectangleChanged: {
            if (cursorRectangle.y < flick.contentY) flick.contentY = cursorRectangle.y
            else if (cursorRectangle.y + cursorRectangle.height > flick.contentY + flick.height) flick.contentY = cursorRectangle.y + cursorRectangle.height - flick.height
          }
          Keys.onPressed: function(e) {
            var ctrl = e.modifiers & Qt.ControlModifier
            if (e.key === Qt.Key_Escape) { root.leaveRequested(); e.accepted = true }
            else if (ctrl && (e.key === Qt.Key_Return || e.key === Qt.Key_Enter)) {
              if (input.text.trim() !== "" && !root.sending) root.sendRequested(input.text)
              e.accepted = true
            }
          }
          UiText {
            visible: input.text === "" && !input.activeFocus
            text: "Press r to reply\u2026"
            dim: true
          }
          UiText {
            visible: input.text === "" && input.activeFocus
            text: "Write a text message\u2026"
            dim: true
          }
        }
      }

      UiText {
        id: hint
        anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom
        anchors.leftMargin: Style.spacing.lg; anchors.rightMargin: Style.spacing.lg
        text: root.sendError !== "" ? "\u26a0 Send failure: " + root.sendError
            : root.composerFocused ? "Ctrl+Enter send \u00b7 Esc leave (draft is kept)" : ""
        foreground: root.sendError !== "" ? Hues.red : Hues.muted
        elide: Text.ElideRight
        font.pixelSize: Style.font.caption
      }
    }
  }
}
