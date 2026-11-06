import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../../common"
import ".."

// Rules: the automatic rules your mail server runs on arriving mail, as plain sentences. Exchange's are
// read from the server (Inbox rules), tag rules grouped by tag, everything else listed on its own so a
// forward or delete rule can never hide among the tag rules. Read-only for now: edit them in Outlook.
// Gmail filters need a permission OmaLook does not ask for yet.
SettingsPage {
  id: page

  property var app: null

  title: "Rules"
  description: "What your mail server does with arriving mail. Shown as read from the server; changing rules here comes later, so keep editing them in Outlook."

  readonly property var accounts: app ? app.rulesData.accounts : []

  Component.onCompleted: { if (app) app.loadRules() }
  Timer { interval: 10000; repeat: true; running: !!page.app; onTriggered: page.app.loadRules() }

  function when(secs) {
    if (!secs) return "not read yet"
    var d = new Date(secs * 1000)
    return "read " + Qt.formatDateTime(d, "yyyy-MM-dd hh:mm")
  }

  function line(r) {
    var parts = []
    parts.push(r.conditions.length > 0 ? r.conditions.join(", and ") : "every message")
    if (r.exceptions.length > 0) parts.push("except " + r.exceptions.join(", ") )
    return parts.join(", ")
  }

  function otherLine(r) {
    var acts = r.actions.slice()
    if (r.tags.length > 0) acts.unshift("tag " + r.tags.join(", "))
    if (r.stops) acts.push("stop")
    return page.line(r) + "  \u2192  " + acts.join(", ")
  }

  function risky(r) {
    for (var i = 0; i < r.actions.length; i++) if (/DELETE|Deleted|forward|redirect/.test(r.actions[i])) return true
    return false
  }

  Repeater {
    model: page.accounts
    delegate: Column {
      id: acct
      required property var modelData
      width: parent.width
      spacing: Style.space(6)

      PanelSectionHeader { text: acct.modelData.email.toUpperCase() + "  (" + acct.modelData.provider + ")" }

      UiText {
        width: parent.width
        wrapMode: Text.WordWrap
        dim: true
        font.pixelSize: Style.font.caption
        text: {
          var m = acct.modelData
          if (m.provider !== "exchange") return "Gmail filters are not read yet: that needs a Gmail settings permission, asked for together with editing them."
          if (m.state === "needs_signin") return "The server refused the rule list. Sign in to this account again (Settings \u203a Accounts) to allow it." + (m.total > 0 ? " Showing the last copy." : "")
          if (m.state === "error") return "Could not read the rules: " + m.error + (m.total > 0 ? " Showing the last copy." : "")
          if (m.state === "none") return "Not read yet."
          return m.total + (m.total === 1 ? " rule, " : " rules, ") + page.when(m.fetched_at) + ". Rules run on the server, in this order; \u201cstop\u201d means a later rule does not see the message."
        }
      }

      // tag rules, grouped by tag
      Repeater {
        model: acct.modelData.groups
        delegate: Column {
          id: grp
          required property var modelData
          width: parent.width
          spacing: 2
          RowLayout {
            width: parent.width
            spacing: Style.spacing.lg
            Rectangle { Layout.preferredWidth: Style.space(12); Layout.preferredHeight: Style.space(12); radius: 3; color: grp.modelData.color }
            UiText { Layout.fillWidth: true; text: grp.modelData.tag; font.bold: true; elide: Text.ElideRight }
            UiText { text: grp.modelData.rules.length + (grp.modelData.rules.length === 1 ? " rule" : " rules"); dim: true; font.pixelSize: Style.font.caption }
          }
          Repeater {
            model: grp.modelData.rules
            delegate: UiText {
              required property var modelData
              x: Style.space(24)
              width: parent.width - Style.space(24)
              wrapMode: Text.WordWrap
              opacity: modelData.enabled ? 1 : 0.5
              font.pixelSize: Style.font.caption
              text: (modelData.enabled ? "" : "(off)  ") + page.line(modelData) + (modelData.stops ? "" : "   (keeps going)")
            }
          }
        }
      }

      // everything that is not a plain tag rule
      PanelSectionHeader { visible: acct.modelData.other.length > 0; text: "OTHER RULES (" + acct.modelData.other.length + ")" }
      Repeater {
        model: acct.modelData.other
        delegate: UiText {
          required property var modelData
          width: parent.width
          wrapMode: Text.WordWrap
          opacity: modelData.enabled ? 1 : 0.5
          foreground: page.risky(modelData) ? Commons.Color.urgent : Commons.Color.foreground
          text: (modelData.enabled ? "" : "(off)  ") + page.otherLine(modelData) + (modelData.unknown.length > 0 ? "   [also: " + modelData.unknown.join(", ") + "]" : "")
        }
      }

      UiText {
        visible: acct.modelData.system.length > 0
        width: parent.width
        wrapMode: Text.WordWrap
        dim: true
        font.pixelSize: Style.font.caption
        text: acct.modelData.system.length + " read-only rule" + (acct.modelData.system.length === 1 ? "" : "s") + " made by Outlook itself ("
              + acct.modelData.system.map(function(r) { return r.name || r.id }).join(", ") + "). They are left alone."
      }

      PanelSeparator { width: parent.width }
    }
  }

  UiText {
    visible: page.accounts.length === 0
    width: parent.width
    text: "No accounts yet."
    dim: true
  }
}
