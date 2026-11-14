import QtQuick
import qs.Commons
import qs.Ui
import "../../common"
import ".."

// "Keep running in background" is wired to the backend (settings.toml [sync] run_in_background).
SettingsPage {
  id: page

  property var app: null

  title: "General"
  description: "How OmaLook runs on this computer."

  Column {
    width: parent.width
    spacing: Style.space(6)

    PanelSectionHeader { text: "BACKGROUND" }

    Toggle {
      width: parent.width
      label: "Keep running in background"
      description: page.app && page.app.runInBackground
        ? "Mail, calendar and SMS keep syncing after the window is closed. Opening OmaLook again reattaches to it."
        : "Syncing stops when the window is closed."
      checked: page.app ? page.app.runInBackground : false
      onClicked: if (page.app) page.app.setRunInBackground(!page.app.runInBackground)
    }

    UiText {
      visible: page.app && page.app.runInBackground
      width: parent.width
      wrapMode: Text.WordWrap
      dim: true
      font.pixelSize: Style.font.caption
      text: "To stop it completely: turn this off and close the window (the background process exits at once if no window is open)."
    }

    UiText {
      visible: page.app && page.app.mcp.enabled
      width: parent.width
      wrapMode: Text.WordWrap
      dim: true
      font.pixelSize: Style.font.caption
      text: page.app && page.app.runInBackground
        ? "The MCP server (Settings > MCP) runs inside this background process; turning this off stops it."
        : "The MCP server is switched on in Settings > MCP, but stays off while this is off."
    }

    PanelSectionHeader { text: "DATABASE" }

    UiText {
      width: parent.width
      wrapMode: Text.WordWrap
      font.pixelSize: Style.font.caption
      color: page.dbState === "damaged" ? Commons.Color.urgent : Commons.Color.foreground
      text: page.dbStateText
    }

    UiText {
      visible: page.dbState === "damaged"
      width: parent.width
      wrapMode: Text.WordWrap
      dim: true
      font.pixelSize: Style.font.caption
      text: "Drafts, rules and settings are kept; mail downloads again after the rebuild."
    }

    Row {
      spacing: Style.space(3)

      Button {
        visible: page.dbState === "damaged"
        bordered: true
        iconText: "\uf021"
        text: "Rebuild on next start"
        onClicked: page.app.requestDbRebuild(false, null)
      }

      Button {
        visible: page.dbState === "rebuild_requested"
        bordered: true
        iconText: "\uf00d"
        text: "Cancel rebuild"
        onClicked: page.app.requestDbRebuild(true, null)
      }
    }
  }

  // The state of the database as GET /sync/health reports it (app.dbHealth).
  readonly property string dbState: (app && app.dbHealth && app.dbHealth.state) || ""
  readonly property string dbStateText: {
    var db = app ? app.dbHealth : null
    var lost = (db && db.lost && db.lost.length) ? " (lost: " + db.lost.join(", ") + ")" : ""
    if (dbState === "damaged") return "Damaged: " + ((db && db.detail) || "the file cannot be read")
    if (dbState === "rebuild_requested") return "Rebuild requested: it happens the next time OmaLook starts"
    if (dbState === "rebuilt") return "Rebuilt at this start" + lost
    if (dbState === "ok") return "Healthy"
    return "Not checked yet"
  }
}
