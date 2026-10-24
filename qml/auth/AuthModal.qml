import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"

// Login overlay: Microsoft device code, or Gmail (address entry, then browser sign-in). Same visual recipe as the kit's ConfirmDialog
// (scrim + accent-bordered card); its content is ours because ConfirmDialog
// only supports a message and two buttons.
Item {
  id: root

  required property var auth

  visible: auth.showModal

  Rectangle {
    anchors.fill: parent
    color: Util.alpha(Commons.Color.background, 0.7)

    MouseArea { anchors.fill: parent; onClicked: root.auth.cancel() }

    BorderSurface {
      id: card
      anchors.centerIn: parent
      width: Math.min(parent.width - Style.space(60), Style.space(520))
      height: content.implicitHeight + card.contentTopInset + card.contentBottomInset
      color: Commons.Color.background
      padding: Style.space(18)
      radius: Style.cornerRadius
      borderSpec: Border.flat(Commons.Color.accent, Style.normalBorderWidth)

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
          text: root.auth.loginProvider === "gmail" ? "Google Authentication" : "Microsoft Authentication"
          horizontalAlignment: Text.AlignHCenter
          foreground: Commons.Color.accent
          font.pixelSize: Style.font.title
          font.bold: true
        }

        // Gmail step 1: which address?
        ColumnLayout {
          id: emailStep
          Layout.fillWidth: true
          visible: root.auth.needsEmail
          spacing: Style.spacing.lg
          onVisibleChanged: if (visible) { emailField.text = ""; emailField.forceActiveFocus() }

          PanelSectionHeader { text: "YOUR GMAIL ADDRESS" }

          RowLayout {
            Layout.fillWidth: true
            spacing: Style.spacing.lg

            TextField {
              id: emailField
              Layout.fillWidth: true
              placeholderText: "name@gmail.com"
              onAccepted: root.auth.submitEmail(text)
            }
            Button {
              Layout.alignment: Qt.AlignVCenter
              bordered: true
              iconText: "\uf090"
              text: "Continue"
              onClicked: root.auth.submitEmail(emailField.text)
            }
          }

          UiText {
            Layout.fillWidth: true
            visible: root.auth.errorMessage !== ""
            text: root.auth.errorMessage
            foreground: Commons.Color.urgent
            wrapMode: Text.Wrap
            horizontalAlignment: Text.AlignHCenter
          }
          UiText {
            Layout.fillWidth: true
            text: "Your browser opens Google's own sign-in page. omalook never sees your password."
            dim: true
            wrapMode: Text.Wrap
            horizontalAlignment: Text.AlignHCenter
            font.pixelSize: Style.font.caption
          }
        }

        // Gmail step 2: waiting for the browser
        ColumnLayout {
          Layout.fillWidth: true
          visible: root.auth.loginProvider === "gmail" && !root.auth.needsEmail && root.auth.errorMessage === ""
          spacing: Style.spacing.lg

          UiText {
            Layout.fillWidth: true
            text: "Finish signing in with Google in your browser…"
            horizontalAlignment: Text.AlignHCenter
          }
          UiText {
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            text: {
              var s = root.auth.secondsRemaining
              var ss = s % 60
              return "Waiting — gives up in " + Math.floor(s / 60) + ":" + (ss < 10 ? "0" + ss : ss)
            }
            dim: true
            font.pixelSize: Style.font.caption
          }
          Button {
            Layout.fillWidth: true
            visible: root.auth.googleUrl !== ""
            text: "Browser didn't open? Open sign-in page"
            iconText: "\uf08e"
            bordered: true
            onClicked: Qt.openUrlExternally(root.auth.googleUrl)
          }
        }

        // waiting for code
        ColumnLayout {
          Layout.fillWidth: true
          visible: root.auth.loginProvider !== "gmail" && root.auth.userCode === "" && root.auth.errorMessage === ""
          spacing: Style.spacing.lg

          UiText {
            Layout.fillWidth: true
            text: "Initiating device authentication…"
            horizontalAlignment: Text.AlignHCenter
          }
          UiText {
            Layout.fillWidth: true
            text: "Waiting for Microsoft to send a code"
            horizontalAlignment: Text.AlignHCenter
            dim: true
            font.pixelSize: Style.font.caption
          }
        }

        // code received
        ColumnLayout {
          Layout.fillWidth: true
          visible: root.auth.loginProvider !== "gmail" && root.auth.userCode !== "" && root.auth.errorMessage === ""
          spacing: Style.spacing.lg

          PanelSectionHeader { text: "1. OPEN THIS URL IN YOUR BROWSER" }

          BorderSurface {
            Layout.fillWidth: true
            implicitHeight: urlText.implicitHeight + Style.spacing.lg * 2
            color: "transparent"
            radius: Style.cornerRadius
            borderSpec: Border.controlSpec("normal", Commons.Color.foreground, Commons.Color.accent)

            UiText {
              id: urlText
              anchors.fill: parent
              anchors.margins: Style.spacing.lg
              text: root.auth.verificationUri
              foreground: Commons.Color.accent
              elide: Text.ElideRight
              horizontalAlignment: Text.AlignHCenter
            }
          }

          PanelSectionHeader { text: "2. ENTER THIS CODE WHEN PROMPTED" }

          BorderSurface {
            Layout.fillWidth: true
            implicitHeight: codeText.implicitHeight + Style.spacing.xxxl * 2
            color: Style.selectedFillFor(Commons.Color.foreground, Commons.Color.accent)
            radius: Style.cornerRadius
            borderSpec: Border.flat(Commons.Color.accent, Style.normalBorderWidth)

            UiText {
              id: codeText
              anchors.centerIn: parent
              text: root.auth.userCode
              font.pixelSize: Style.font.display
              font.bold: true
              font.letterSpacing: Style.space(6)
            }
          }

          UiText {
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            text: {
              var s = root.auth.secondsRemaining
              var ss = s % 60
              return "Code expires in " + Math.floor(s / 60) + ":" + (ss < 10 ? "0" + ss : ss)
            }
            foreground: root.auth.secondsRemaining < 60 ? Commons.Color.urgent : Commons.Color.foreground
            dim: root.auth.secondsRemaining >= 60
            font.pixelSize: Style.font.caption
          }

          UiText {
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            text: "Waiting for you to authenticate in the browser…"
            dim: true
            font.pixelSize: Style.font.caption
          }
        }

        // error
        ColumnLayout {
          Layout.fillWidth: true
          visible: root.auth.errorMessage !== "" && !root.auth.needsEmail
          spacing: Style.spacing.lg

          UiText {
            Layout.fillWidth: true
            text: root.auth.errorMessage
            foreground: Commons.Color.urgent
            wrapMode: Text.Wrap
            horizontalAlignment: Text.AlignHCenter
          }

          Button {
            Layout.fillWidth: true
            text: "Try again"
            iconText: "\uf021"
            bordered: true
            onClicked: root.auth.startLogin(root.auth.loginProvider)
          }
        }

        Button {
          Layout.fillWidth: true
          text: "Cancel"
          bordered: true
          onClicked: root.auth.cancel()
        }
      }
    }
  }
}
