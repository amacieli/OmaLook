import QtQuick
import QtQuick.Controls
import qs.Commons
import qs.Ui
import "../../common"
import ".."

// "Message rendering", "Delay sending email", "Ask before sending" and "Signature" are wired to the
// backend (settings.toml); the other values are placeholders and are not persisted.
SettingsPage {
  id: page

  property var app: null

  property string syncInterval: "5"
  property int markReadDelay: 2
  property bool conversationView: true

  editing: sigText.activeFocus || syncDropdown.popupOpen || delay.field.activeFocus || renderingDropdown.popupOpen || sendDelay.field.activeFocus || audience.field.activeFocus || renderCache.field.activeFocus

  title: "Mail"
  description: "Sync behaviour and reading preferences."
  placeholder: true

  Column {
    width: parent.width
    spacing: Style.space(6)

    PanelSectionHeader { text: "SYNC" }
    Dropdown {
      id: syncDropdown
      width: parent.width
      label: "Check for new mail"
      value: page.syncInterval
      options: [
        { value: "1",  label: "Every minute" },
        { value: "5",  label: "Every 5 minutes" },
        { value: "15", label: "Every 15 minutes" },
        { value: "0",  label: "Manually" }
      ]
      onChanged: function(v) { page.syncInterval = v }
    }
  }

  PanelSeparator { width: parent.width }

  Column {
    width: parent.width
    spacing: Style.space(6)

    PanelSectionHeader { text: "MESSAGE RENDERING" }
    Dropdown {
      id: renderingDropdown
      width: parent.width
      label: "Show messages as"
      value: page.app ? page.app.messageRendering : "system_sender"
      options: [
        { value: "html",          label: "Always HTML" },
        { value: "system",        label: "Always System" },
        { value: "system_sender", label: "Always System with per-sender override" }
      ]
      onChanged: function(v) { if (page.app) page.app.setMessageRendering(v) }
    }
    UiText {
      width: parent.width
      text: page.app && page.app.messageRendering === "system_sender"
        ? "System font, except for senders marked \"Always HTML\" under Senders."
        : "The HTML / System button on a message still overrides this for that message."
      dim: true
      wrapMode: Text.WordWrap
      font.pixelSize: Style.font.caption
    }
  }

  PanelSeparator { width: parent.width }

  Toggle {
    width: parent.width
    label: "Conversation view"
    description: page.conversationView ? "Messages grouped by thread" : "Flat message list"
    checked: page.conversationView
    onClicked: page.conversationView = !page.conversationView
  }

  NumberField {
    id: delay
    label: "Mark as read after (seconds)"
    value: page.markReadDelay
    from: 0
    to: 60
    onModified: function(v) { page.markReadDelay = v }
  }

  Column {
    width: parent.width
    spacing: Style.space(6)

    PanelSectionHeader { text: "SENDING" }
    NumberField {
      id: sendDelay
      label: "Delay sending email for (seconds)"
      value: page.app ? page.app.sendDelaySecs : 3
      from: 0
      to: page.app ? page.app.maxSendDelaySecs : 60
      onModified: function(v) { if (page.app) page.app.setSendDelay(v) }
    }
    UiText {
      width: parent.width
      text: sendDelay.value === 0
        ? "Messages are sent the moment you press Send."
        : "After Send, a message waits this long so you can take it back (Undo, or u) and keep drafting. Set to 0 to send at once."
      dim: true
      wrapMode: Text.WordWrap
      font.pixelSize: Style.font.caption
    }
  }

  PanelSeparator { width: parent.width }

  Column {
    width: parent.width
    spacing: Style.space(6)

    PanelSectionHeader { text: "DISTRIBUTION LISTS" }
    NumberField {
      id: audience
      label: "Ask before sending to more than (people)"
      value: page.app ? page.app.largeAudienceConfirm : 20
      from: 0
      to: 1000
      onModified: function(v) { if (page.app) page.app.setLargeAudienceConfirm(v) }
    }
    UiText {
      width: parent.width
      text: audience.value === 0
        ? "Send never asks about the size of the audience."
        : "A message to more people than this (lists counted by their members) asks \u201cSend to about N people?\u201d first. It is a question, not a block. 0 turns it off."
      dim: true
      wrapMode: Text.WordWrap
      font.pixelSize: Style.font.caption
    }
  }

  PanelSeparator { width: parent.width }

  Column {
    width: parent.width
    spacing: Style.space(6)

    PanelSectionHeader { text: "RENDERED MAIL CACHE" }
    NumberField {
      id: renderCache
      label: "Size limit for pre-rendered HTML mail (MB)"
      value: page.app ? page.app.renderCacheMb : 512
      from: page.app ? page.app.renderCacheMinMb : 64
      to: page.app ? page.app.renderCacheMaxMb : 16384
      stepSize: 64
      onModified: function(v) { if (page.app) page.app.setRenderCacheMb(v) }
    }
    UiText {
      width: parent.width
      text: "Pictures of HTML messages, kept so they open at once. Using " + (page.app ? Math.round(page.app.renderCacheUsedBytes / 1048576) : 0)
        + " MB now. When a new picture does not fit, the pictures of the oldest messages are removed first. Lowering the limit trims the cache right away."
      dim: true
      wrapMode: Text.WordWrap
      font.pixelSize: Style.font.caption
    }
  }

  PanelSeparator { width: parent.width }

  // OL-050: one signature per account, saved to settings.toml. Inserted above the quoted text of
  // replies and forwards, and below the cursor in new messages.
  Column {
    id: sigSection
    width: parent.width
    spacing: Style.space(6)
    readonly property var signedIn: page.app ? page.app.accounts.filter(function(a) { return a.signed_in && a.email }) : []
    property string accountId: signedIn.length > 0 ? signedIn[0].id : ""
    onAccountIdChanged: sigText.text = page.app && page.app.signatures[accountId] ? page.app.signatures[accountId] : ""

    PanelSectionHeader { text: "SIGNATURE" }
    Dropdown {
      id: sigAccount
      width: parent.width
      label: "Account"
      value: sigSection.accountId
      options: sigSection.signedIn.map(function(a) { return { value: a.id, label: a.email } })
      onChanged: function(v) { sigSection.accountId = v }
    }
    TextArea {
      id: sigText
      width: parent.width
      height: Math.max(implicitHeight, Style.space(110))
      wrapMode: TextEdit.Wrap
      font.family: Style.font.family
      font.pixelSize: Style.font.body
      color: Commons.Color.foreground
      placeholderTextColor: Qt.darker(Commons.Color.foreground, 1.8)
      placeholderText: "Blank for no signature"
      enabled: sigSection.accountId !== ""
    }
    Button {
      bordered: true
      text: "Save signature"
      enabled: sigSection.accountId !== ""
      onClicked: { if (page.app) page.app.setSignature(sigSection.accountId, sigText.text) }
    }
    UiText {
      width: parent.width
      text: "Plain text. Sent with a \"-- \" line above it in system-font mail; HTML mail gets the text alone. "
        + "In compose, Ctrl+G turns it off for that one message."
      dim: true
      wrapMode: Text.WordWrap
      font.pixelSize: Style.font.caption
    }
  }
}
