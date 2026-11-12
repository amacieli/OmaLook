import QtQuick
import qs.Commons
import qs.Commons as Commons

// A question with up to three choices (ConfirmDialog has two), styled like it. Left / Right / Tab move the
// selection, Enter picks it, a click picks that button. Esc picks `escapeIndex`; -1 means Esc only dismisses.
// `chosen(index)` fires with the index of the choice, or -1 when dismissed.
Item {
  id: root

  property bool opened: false
  property string message: ""
  // [{ text, destructive }]: the buttons, left to right.
  property var choices: []
  property int selectedIndex: 0
  property int escapeIndex: -1
  property color background: Commons.Color.background
  property color foreground: Commons.Color.foreground
  property color scrim: Util.alpha(Commons.Color.background, 0.7)
  property color selectedBackground: Util.alpha(Commons.Color.foreground, 0.08)
  property color selectedText: Commons.Color.accent
  property string fontFamily: Style.font.family
  property int cornerRadius: Style.cornerRadius

  signal chosen(int index)

  function pick(i) { root.chosen(i) }

  function handleKey(event) {
    if (!root.opened) return false
    var n = root.choices.length
    if (event.key === Qt.Key_Escape) {
      root.pick(root.escapeIndex)
      return true
    } else if (event.key === Qt.Key_Left || event.key === Qt.Key_Backtab) {
      root.selectedIndex = (root.selectedIndex + n - 1) % n
      return true
    } else if (event.key === Qt.Key_Right || event.key === Qt.Key_Tab) {
      root.selectedIndex = (root.selectedIndex + 1) % n
      return true
    } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
      root.pick(root.selectedIndex)
      return true
    }
    return false
  }

  visible: opened

  Rectangle {
    anchors.fill: parent
    color: root.scrim

    MouseArea { anchors.fill: parent; onClicked: root.pick(root.escapeIndex) }

    BorderSurface {
      id: card
      width: Math.min(parent.width - Style.space(32), Style.space(460))
      height: card.contentTopInset + card.contentBottomInset + messageText.implicitHeight + Style.space(20) + Style.space(34)
      anchors.centerIn: parent
      color: root.background
      borderSpec: Border.flat(root.selectedText, Style.normalBorderWidth)
      padding: Style.space(18)
      radius: root.cornerRadius

      MouseArea { anchors.fill: parent; onClicked: {} }

      Item {
        anchors.fill: parent
        anchors.topMargin: card.contentTopInset
        anchors.rightMargin: card.contentRightInset
        anchors.bottomMargin: card.contentBottomInset
        anchors.leftMargin: card.contentLeftInset

        Text {
          id: messageText
          textFormat: Text.PlainText
          anchors.left: parent.left
          anchors.right: parent.right
          anchors.top: parent.top
          text: root.message
          color: root.foreground
          font.family: root.fontFamily
          font.pixelSize: Style.font.title
          wrapMode: Text.WordWrap
        }

        Row {
          anchors.right: parent.right
          anchors.bottom: parent.bottom
          spacing: Style.space(10)

          Repeater {
            model: root.choices

            BorderSurface {
              required property int index
              required property var modelData

              readonly property bool selected: root.selectedIndex === index
              readonly property bool destructive: !!modelData.destructive

              width: Style.space(112)
              height: Style.space(34)
              color: selected
                ? (destructive ? Util.alpha(Commons.Color.urgent, 0.22) : root.selectedBackground)
                : "transparent"
              borderSpec: Border.flat(destructive
                ? (selected ? Commons.Color.urgent : Util.alpha(Commons.Color.urgent, 0.56))
                : (selected ? root.selectedText : Util.alpha(root.foreground, 0.38)), Style.normalBorderWidth)
              radius: 0

              Text {
                textFormat: Text.PlainText
                anchors.centerIn: parent
                text: modelData.text
                color: destructive ? (selected ? Commons.Color.urgent : root.foreground) : (selected ? root.selectedText : root.foreground)
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption
              }

              MouseArea {
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onEntered: root.selectedIndex = index
                onClicked: root.pick(index)
              }
            }
          }
        }
      }
    }
  }
}
