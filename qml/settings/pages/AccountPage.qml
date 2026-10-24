import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../../common"
import ".."

// Accounts: add a mail account of any provider, then manage every account below.
// Exchange/Outlook (Microsoft) sign-in works, any number of them; the other
// providers are listed so the flow is in place for them.
// "Log out" stops syncing but keeps the account, its token and its cached data
// (shown as signed out; "Log in" resumes instantly). "Remove" deletes the account,
// its keyring token and its cached data.
SettingsPage {
  id: page

  // [{ id, provider, email, signed_in }] from GET /accounts
  property var accounts: []
  property bool isAuthenticated: false

  property string newProvider: "exchange"
  property string notice: ""

  signal loginRequested(string provider)
  signal accountLoginRequested(string accountId, string provider)
  signal logoutRequested(string accountId)
  signal removeRequested(string accountId)

  editing: providerDropdown.popupOpen

  readonly property var providerOptions: [
    { value: "exchange", label: "Exchange" },
    { value: "outlook",  label: "Outlook" },
    { value: "gmail",    label: "Gmail" },
    { value: "yahoo",    label: "Yahoo Mail" },
    { value: "icloud",   label: "iCloud Mail" },
    { value: "aol",      label: "AOL Mail" },
    { value: "zoho",     label: "Zoho Mail" },
    { value: "fastmail", label: "Fastmail" },
    { value: "gmx",      label: "GMX Mail" },
    { value: "proton",   label: "Proton Mail (Bridge)" },
    { value: "imap",     label: "IMAP account" },
    { value: "smtp",     label: "SMTP account" }
  ]

  function providerLabel(id) {
    for (var i = 0; i < providerOptions.length; i++)
      if (providerOptions[i].value === id) return providerOptions[i].label
    return id
  }

  function addAccount() {
    var supported = page.newProvider === "exchange" || page.newProvider === "outlook" || page.newProvider === "gmail"
    if (!supported) {
      page.notice = page.providerLabel(page.newProvider) + " sign-in isn't supported by the backend yet."
    } else {
      page.notice = ""
      page.loginRequested(page.newProvider)
    }
  }

  title: "Accounts"
  description: "Add mail accounts and manage the ones you're signed in to."

  Column {
    width: parent.width
    spacing: Style.space(8)

    PanelSectionHeader { text: "ADD ACCOUNT" }

    RowLayout {
      width: parent.width
      spacing: Style.spacing.lg

      Dropdown {
        id: providerDropdown
        Layout.fillWidth: true
        Layout.alignment: Qt.AlignVCenter
        showLabel: false
        value: page.newProvider
        options: page.providerOptions
        onChanged: function(v) { page.newProvider = v; page.notice = "" }
      }

      Button {
        Layout.alignment: Qt.AlignVCenter
        bordered: true
        iconText: "\uf067"
        text: "Add"
        onClicked: page.addAccount()
      }
    }

    UiText {
      visible: page.notice !== ""
      width: parent.width
      text: page.notice
      dim: true
      wrapMode: Text.WordWrap
    }

    PanelSeparator { width: parent.width }

    UiText {
      visible: page.accounts.length === 0
      width: parent.width
      text: "No accounts signed in."
      dim: true
    }

    Repeater {
      model: page.accounts
      delegate: Column {
        required property var modelData
        width: parent.width
        spacing: Style.space(4)

        PanelSectionHeader { text: page.providerLabel(modelData.provider).toUpperCase() + " ACCOUNT" }

        RowLayout {
          width: parent.width
          spacing: Style.spacing.lg

          UiText {
            Layout.fillWidth: true
            text: modelData.email !== "" ? modelData.email : "loading…"
            dim: modelData.email === ""
            elide: Text.ElideRight
          }
          UiText {
            text: modelData.signed_in ? "signed in" : "signed out — cached data kept"
            foreground: modelData.signed_in ? Commons.Color.accent : Commons.Color.foreground
            dim: !modelData.signed_in
          }
          // Signed out → "Log in": the kept token is reused (no sign-in prompt). Only if it is
          // gone or was revoked does the backend ask for a device-flow login, and signing in to
          // the same mailbox then re-attaches to this account (cached data kept, no re-sync).
          Button {
            bordered: true
            iconText: modelData.signed_in ? "\uf2f5" : "\uf090"
            text: modelData.signed_in ? "Log out" : "Log in"
            onClicked: modelData.signed_in ? page.logoutRequested(modelData.id)
                                           : page.accountLoginRequested(modelData.id, modelData.provider)
          }
          // Two-step remove: it deletes the cached mail/calendar data too.
          Button {
            id: removeButton
            property bool confirming: false
            bordered: true
            iconText: "\uf1f8"
            text: confirming ? "Really remove?" : "Remove"
            foreground: confirming ? Commons.Color.urgent : Commons.Color.foreground
            onClicked: {
              if (confirming) { confirming = false; page.removeRequested(modelData.id) }
              else confirming = true
            }
            Timer {
              interval: 4000
              running: removeButton.confirming
              onTriggered: removeButton.confirming = false
            }
          }
        }
      }
    }
  }
}
