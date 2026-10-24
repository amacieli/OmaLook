import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"

// Shown after a sign-in that turned out to be for a mailbox that is already signed in.
// Nothing has been changed yet: "Replace sign-in" swaps the account's credentials in place
// (id, settings and cached data are kept); "Cancel" discards the new credentials.
Item {
  id: root

  required property var auth

  readonly property var prompt: auth.reauthPrompt
  visible: prompt !== null

  Rectangle {
    anchors.fill: parent
    color: Util.alpha(Commons.Color.background, 0.7)

    // clicking outside backs out — the safe choice
    MouseArea { anchors.fill: parent; onClicked: root.auth.answerReauth("cancel") }

    BorderSurface {
      id: card
      anchors.centerIn: parent
      width: Math.min(parent.width - Style.space(60), Style.space(520))
      height: content.implicitHeight + card.contentTopInset + card.contentBottomInset
      color: Commons.Color.background
      padding: Style.space(18)
      radius: Style.cornerRadius
      borderSpec: Border.flat(Commons.Color.urgent, Style.normalBorderWidth)

      MouseArea { anchors.fill: parent }   // swallow clicks so the scrim doesn't cancel

      ColumnLayout {
        id: content
        anchors.fill: parent
        anchors.topMargin: card.contentTopInset
        anchors.leftMargin: card.contentLeftInset
        anchors.rightMargin: card.contentRightInset
        spacing: Style.spacing.xxxl

        UiText {
          Layout.fillWidth: true
          text: "Account already signed in"
          horizontalAlignment: Text.AlignHCenter
          foreground: Commons.Color.urgent
          font.pixelSize: Style.font.title
          font.bold: true
        }

        UiText {
          Layout.fillWidth: true
          text: root.prompt ? root.prompt.email : ""
          horizontalAlignment: Text.AlignHCenter
          font.bold: true
          elide: Text.ElideMiddle
        }

        UiText {
          Layout.fillWidth: true
          wrapMode: Text.WordWrap
          text: "This mailbox is already signed in. If you continue, its current sign-in is replaced "
              + "with the one you just completed. Mail, calendar and contacts already downloaded are "
              + "kept and syncing carries on. Use this to grant newly requested permissions."
        }

        UiText {
          Layout.fillWidth: true
          wrapMode: Text.WordWrap
          dim: true
          text: "Cancel leaves the account exactly as it is."
        }

        RowLayout {
          Layout.fillWidth: true
          spacing: Style.spacing.lg

          Button {
            Layout.fillWidth: true
            text: "Cancel"
            iconText: "\uf00d"
            bordered: true
            onClicked: root.auth.answerReauth("cancel")
          }
          Button {
            Layout.fillWidth: true
            text: "Replace sign-in"
            iconText: "\uf021"
            bordered: true
            foreground: Commons.Color.urgent
            onClicked: root.auth.answerReauth("replace")
          }
        }
      }
    }
  }
}
