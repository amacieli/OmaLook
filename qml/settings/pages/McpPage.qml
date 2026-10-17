import QtQuick
import qs.Commons
import qs.Ui
import "../../common"
import ".."

// Settings -> MCP: what an AI client may do with OmaLook over MCP. Everything here lives in
// settings.toml [mcp] and is applied by the backend (src/mcp/admin.rs); the page only shows the
// state the backend reports and asks it to change things. Changes take effect on the client's next
// call. MCP runs inside the background process, so with "Keep running in background" off the whole
// page is read-only and says why.
//
// Keyboard: Tab / Shift-Tab between controls, Enter or Space to press / toggle, h / l between the
// chips of a group.
SettingsPage {
  id: page

  property var app: null

  readonly property var st: app ? app.mcp : ({})
  readonly property bool bg: app ? app.runInBackground : false
  // Controls work only while the daemon is kept running in the background.
  readonly property bool usable: bg && st.settings_readable !== false

  property string copied: ""        // what was last copied, for the confirmation line
  property bool confirmRegen: false

  editing: false

  title: "MCP"
  description: "Let AI apps use OmaLook through the Model Context Protocol. Everything is off until you turn it on here, and you choose exactly what each app may read or change."

  Component.onCompleted: if (app) app.loadMcp()

  function focusFirst() {
    if (page.usable) master.forceActiveFocus()
    else if (goGeneral.visible) goGeneral.forceActiveFocus()
    else return false
    return true
  }

  // Scroll the surrounding flickable so `item` is on screen (Tab can land far down this page).
  function reveal(item) {
    var f = page.parent
    while (f && f.contentY === undefined) f = f.parent
    if (!f) return
    var y = item.mapToItem(f.contentItem, 0, 0).y
    if (y - Style.space(20) < f.contentY) f.contentY = Math.max(0, y - Style.space(20))
    else if (y + item.height + Style.space(20) > f.contentY + f.height) f.contentY = y + item.height + Style.space(20) - f.height
  }

  function levelOf(domain) { return st.domains && st.domains[domain] ? st.domains[domain] : "off" }
  function toolsOf(domain) {
    var out = []
    var all = st.tools || []
    for (var i = 0; i < all.length; i++) if (all[i].domain === domain) out.push(all[i])
    return out
  }
  function accountAllowed(id) { return (st.accounts || []).indexOf(id) >= 0 }
  function copy(what) {
    if (!app) return
    page.copied = ""
    app.mcpPost("/copy?what=" + what, function(ok) { page.copied = ok ? what : "" })
  }

  readonly property var domainInfo: [
    { id: "mail",     label: "Mail",     note: "Folders, categories, message headers and the short preview stored in the cache. Previews are text from senders and can contain instructions aimed at an AI." },
    { id: "calendar", label: "Calendar", note: "Events. Invitation text can come from anyone." },
    { id: "contacts", label: "Contacts", note: "Names, companies, email addresses and phone numbers." },
    { id: "sms",      label: "SMS",      note: "Text messages mirrored from your phone. Not tied to an account, so the account list below does not apply." },
    { id: "accounts", label: "Accounts", note: "Which accounts exist and whether they are signed in. Nothing more." }
  ]

  // ---- why it is unavailable
  BorderSurface {
    visible: !page.bg
    width: parent.width
    implicitHeight: warn.implicitHeight + Style.spacing.lg * 2
    color: "transparent"
    radius: Style.cornerRadius
    borderSpec: Border.controlSpec("normal", Color.foreground, Color.accent)

    Column {
      id: warn
      anchors.left: parent.left; anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter
      anchors.margins: Style.spacing.lg
      spacing: Style.spacing.md
      UiText {
        width: parent.width
        wrapMode: Text.WordWrap
        font.pixelSize: Style.font.bodySmall
        text: "MCP runs inside OmaLook's background process, so it needs \"Keep running in background\" (Settings > General). Until that is on, MCP is off and these controls are locked."
      }
      Button {
        id: goGeneral
        text: "Open General settings"
        bordered: true
        focusable: true
        visible: !page.bg
        onClicked: if (page.app) page.app.openSettingsCategory("general")
        onActiveFocusChanged: if (activeFocus) page.reveal(goGeneral)
      }
    }
  }

  UiText {
    visible: page.st.settings_readable === false
    width: parent.width
    wrapMode: Text.WordWrap
    color: Color.urgent
    font.pixelSize: Style.font.bodySmall
    text: "settings.toml could not be read, so MCP stays locked and cannot be changed here. Fix the file first."
  }

  UiText {
    visible: page.app && page.app.mcpError !== ""
    width: parent.width
    wrapMode: Text.WordWrap
    color: Color.urgent
    font.pixelSize: Style.font.bodySmall
    text: page.app ? "Not changed: " + page.app.mcpError : ""
  }

  // ---- master switch
  Column {
    width: parent.width
    spacing: Style.space(6)
    opacity: page.usable ? 1 : 0.45

    PanelSectionHeader { text: "MCP SERVER" }

    Toggle {
      id: master
      width: parent.width
      enabled: page.usable
      label: "Enable MCP server"
      description: !page.bg ? "Off: needs Keep running in background."
        : !page.st.enabled ? "Off. Nothing is listening and no AI app can reach OmaLook."
        : page.st.listening ? "On. Listening on " + page.st.http_url + " and through the omalook mcp command."
        : "On. Starting…"
      checked: page.st.enabled === true
      onClicked: page.app.mcpPost("/enabled?value=" + (page.st.enabled ? "false" : "true"))
      onActiveFocusChanged: if (activeFocus) page.reveal(master)
    }
  }

  PanelSeparator { width: parent.width }

  // ---- connecting a client
  Column {
    width: parent.width
    spacing: Style.space(6)
    opacity: page.usable ? 1 : 0.45

    PanelSectionHeader { text: "CONNECT AN APP" }

    UiText {
      width: parent.width
      wrapMode: Text.WordWrap
      dim: true
      font.pixelSize: Style.font.caption
      text: "Most apps start a command: " + (page.st.command || "omalook mcp") + ". It needs no password; it reads OmaLook's token file itself. Apps that connect by URL need the token (HTTP: " + (page.st.http_url || "") + ")."
    }

    Flow {
      width: parent.width
      spacing: Style.spacing.md
      Button {
        id: copyStdio
        text: "Copy client config"
        bordered: true; focusable: true; enabled: page.usable
        onClicked: page.copy("stdio")
        onActiveFocusChanged: if (activeFocus) page.reveal(copyStdio)
      }
      Button {
        id: copyHermes
        text: "Copy Hermes config"
        bordered: true; focusable: true; enabled: page.usable
        onClicked: page.copy("hermes")
        onActiveFocusChanged: if (activeFocus) page.reveal(copyHermes)
      }
      Button {
        id: copyToken
        text: "Copy HTTP token"
        bordered: true; focusable: true; enabled: page.usable
        onClicked: page.copy("token")
        onActiveFocusChanged: if (activeFocus) page.reveal(copyToken)
      }
      Button {
        id: regen
        text: page.confirmRegen ? "Press again to confirm" : "Regenerate token"
        bordered: true; focusable: true; enabled: page.usable
        onClicked: {
          if (!page.confirmRegen) { page.confirmRegen = true; regenReset.restart(); return }
          page.confirmRegen = false
          page.app.mcpPost("/token/regenerate")
        }
        onActiveFocusChanged: { if (activeFocus) page.reveal(regen); else page.confirmRegen = false }
        Timer { id: regenReset; interval: 5000; onTriggered: page.confirmRegen = false }
      }
    }

    UiText {
      visible: page.copied !== ""
      width: parent.width
      dim: true
      font.pixelSize: Style.font.caption
      text: page.copied === "token" ? "Token copied to the clipboard." : "Copied to the clipboard."
    }
    UiText {
      visible: page.confirmRegen
      width: parent.width
      wrapMode: Text.WordWrap
      dim: true
      font.pixelSize: Style.font.caption
      text: "Regenerating cuts off every app that uses the URL and token. Apps using the omalook mcp command pick the new token up by themselves."
    }
  }

  PanelSeparator { width: parent.width }

  // ---- which accounts
  Column {
    width: parent.width
    spacing: Style.space(6)
    opacity: page.usable ? 1 : 0.45

    PanelSectionHeader { text: "ACCOUNTS AI APPS MAY USE" }

    UiText {
      width: parent.width
      wrapMode: Text.WordWrap
      dim: true
      font.pixelSize: Style.font.caption
      text: (page.st.accounts || []).length === 0
        ? "None selected, so mail, calendar and contacts return nothing."
        : "Mail, calendar and contacts of other accounts stay invisible to AI apps."
    }

    Repeater {
      model: page.app ? page.app.accounts : []
      delegate: Toggle {
        required property var modelData
        width: parent.width
        enabled: page.usable
        label: (modelData.email || modelData.id)
        description: modelData.provider + (modelData.signed_in ? "" : " (signed out)")
        checked: page.accountAllowed(modelData.id)
        onClicked: page.app.mcpPost("/account?id=" + encodeURIComponent(modelData.id) + "&value=" + (checked ? "false" : "true"))
        onActiveFocusChanged: if (activeFocus) page.reveal(this)
      }
    }
  }

  PanelSeparator { width: parent.width }

  // ---- what they may do
  Column {
    width: parent.width
    spacing: Style.space(6)
    opacity: page.usable ? 1 : 0.45

    PanelSectionHeader { text: "WHAT THEY MAY DO" }

    ButtonGroup {
      id: presets
      focusable: page.usable
      options: [{ value: "locked", label: "Locked" }, { value: "read_only", label: "Read only" }, { value: "approval", label: "Read + write, asking first" }]
      value: page.st.preset || "locked"
      onChanged: function(v) { page.app.mcpPost("/preset?name=" + v) }
      onActiveFocusChanged: if (activeFocus) page.reveal(presets)
    }
    UiText {
      width: parent.width
      wrapMode: Text.WordWrap
      dim: true
      font.pixelSize: Style.font.caption
      text: page.st.preset === "custom"
        ? "Custom settings. Pick a preset above to start over."
        : page.st.preset === "locked" ? "Locked: AI apps can use nothing."
        : page.st.preset === "read_only" ? "Read only: AI apps can look but not change anything."
        : "Read + write: reversible changes are allowed, anything that sends or deletes needs your approval (approval prompts arrive in a later update; until then those actions are refused)."
    }

    Repeater {
      model: page.domainInfo
      delegate: Column {
        id: dom
        required property var modelData
        readonly property string level: page.levelOf(modelData.id)
        readonly property var tools: page.toolsOf(modelData.id)
        width: parent.width
        spacing: Style.space(4)

        Item { width: 1; height: Style.space(6) }

        Row {
          width: parent.width
          spacing: Style.spacing.lg
          UiText {
            id: domLabel
            width: Style.space(90)
            anchors.verticalCenter: parent.verticalCenter
            text: dom.modelData.label
            font.pixelSize: Style.font.subtitle
            font.bold: true
          }
          ButtonGroup {
            id: domChips
            focusable: page.usable
            anchors.verticalCenter: parent.verticalCenter
            options: dom.modelData.id === "accounts"
              ? [{ value: "off", label: "Off" }, { value: "read", label: "Read" }]
              : [{ value: "off", label: "Off" }, { value: "read", label: "Read only" }, { value: "write", label: "Read + write" }]
            value: dom.level
            onChanged: function(v) { page.app.mcpPost("/domain?domain=" + dom.modelData.id + "&level=" + v) }
            onActiveFocusChanged: if (activeFocus) page.reveal(domChips)
          }
        }

        UiText {
          width: parent.width
          wrapMode: Text.WordWrap
          dim: true
          font.pixelSize: Style.font.caption
          text: dom.modelData.note + (dom.level === "off" ? "" : "  " + (page.st.read_tools ? page.st.read_tools[dom.modelData.id] : 0) + " read tool(s) available.")
        }

        // Write tools: only for a domain set to "Read + write".
        Column {
          visible: dom.level === "write" && dom.tools.length > 0
          width: parent.width
          spacing: 0

          UiText {
            width: parent.width
            wrapMode: Text.WordWrap
            font.pixelSize: Style.font.caption
            font.bold: true
            text: "Changes this app may make   (Off: not offered.  Ask: you approve each time.  Allow: no prompt.)"
          }

          Repeater {
            model: dom.tools
            delegate: McpToolRow {
              required property var modelData
              width: parent.width
              toolName: modelData.name
              description: modelData.description
              mode: modelData.mode
              defaultMode: modelData.default
              implemented: modelData.implemented
              enabled: page.usable
              onModeChosen: function(m) { page.app.mcpPost("/tool?name=" + modelData.name + "&mode=" + (m === modelData.default ? "default" : m)) }
              onFocusReached: function(it) { page.reveal(it) }
            }
          }
        }
      }
    }
  }
}
