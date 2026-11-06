import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../../common"
import "../../keys/TagNames.js" as TagNames
import ".."

// Tags: every category (Exchange) and label (Gmail) of every account in one list, merged by name, with
// how many messages carry it on each account. Click a tag to recolour it, delete it, or add it to an
// account that does not have it yet. Changes show here at once and are pushed to the mail servers by
// the background sync. Renaming and merging are not offered (near-duplicates are cleaned up by hand).
SettingsPage {
  id: page

  property var app: null
  property string selectedKey: ""
  property bool confirmingDelete: false

  editing: filterField.activeFocus

  title: "Tags"
  description: "Categories (Exchange) and labels (Gmail) of all your accounts, merged by name. In the mail list, `tag:lucia` finds every tag containing \u201clucia\u201d, `tag:=Family` finds exactly that tag, and `tag:none` finds untagged mail."

  readonly property var rows: app ? app.tagRows : []
  readonly property var shown: {
    var q = filterField.text.trim().toLowerCase()
    if (q === "") return rows
    return rows.filter(function(t) { return t.name.toLowerCase().indexOf(q) >= 0 })
  }
  readonly property var current: {
    for (var i = 0; i < rows.length; i++) if (TagNames.key(rows[i].name) === selectedKey) return rows[i]
    return null
  }

  readonly property bool hasList: !!current && current.accounts.some(function(a) { return a.defined })

  function counts(t) {
    var parts = []
    for (var i = 0; i < t.accounts.length; i++) {
      var a = t.accounts[i]
      if (!a.defined && a.messages === 0) continue
      parts.push(a.provider + " " + a.messages)
    }
    return parts.join("  \u00b7  ")
  }

  function missingOn(t) {
    return t.accounts.filter(function(a) { return !a.defined && a.email !== "" })
  }

  function n(v) { return String(v).replace(/\B(?=(\d{3})+(?!\d))/g, ",") }

  // server rules that assign this tag (deleting the tag does not touch them: they would keep tagging new mail)
  function ruleCount(name) {
    var n = 0, k = String(name).toLowerCase().replace(/\s+/g, "")
    var accts = page.app ? (page.app.rulesData.accounts || []) : []
    accts.forEach(function(a) { (a.groups || []).forEach(function(g) {
      if (String(g.tag).toLowerCase().replace(/\s+/g, "") === k) n += g.rules.length
    }) })
    return n
  }

  Component.onCompleted: { if (app) { app.tagNotice = ""; app.loadTags(); app.loadRules() } }

  // Pending changes land on the server in the background: keep the page current while it is open.
  Timer {
    interval: 4000; repeat: true; running: !!page.app
    onTriggered: page.app.loadTags()
  }

  Column {
    width: parent.width
    spacing: Style.space(6)

    PanelSectionHeader { text: "TAGS (" + page.rows.length + ")" }

    TextField {
      id: filterField
      width: parent.width
      placeholderText: "Filter tags"
    }

    UiText {
      visible: !!page.app && page.app.tagNotice !== ""
      width: parent.width
      text: page.app ? page.app.tagNotice : ""
      wrapMode: Text.WordWrap
      foreground: text.indexOf("Could not") === 0 ? Commons.Color.urgent : Commons.Color.foreground
    }

  }

  // ---- the selected tag
  Column {
    visible: !!page.current
    width: parent.width
    spacing: Style.space(6)

    PanelSectionHeader { text: page.current ? page.current.name.toUpperCase() : "" }

    UiText {
      width: parent.width
      dim: true
      font.pixelSize: Style.font.caption
      wrapMode: Text.WordWrap
      text: page.current ? (page.n(page.current.total) + (page.current.total === 1 ? " message" : " messages") + " carry it in total.") : ""
    }

    // colour (a tag no account lists has none to change: add it to an account first)
    UiText { visible: !page.hasList; text: "Not in any account's list (Exchange lets a message carry a name that is not in its category list). Add it to an account below to give it a colour."; dim: true; font.pixelSize: Style.font.caption; width: parent.width; wrapMode: Text.WordWrap }
    UiText { visible: page.hasList; text: "Colour"; dim: true; font.pixelSize: Style.font.caption }
    Flow {
      visible: page.hasList
      width: parent.width
      spacing: Style.space(6)
      Repeater {
        model: page.app ? page.app.tagPalette : []
        delegate: Rectangle {
          id: sw
          required property var modelData
          width: Style.space(24); height: Style.space(24); radius: 4
          color: modelData.hex
          border.width: page.current && page.current.color === modelData.hex ? 2 : 0
          border.color: Commons.Color.foreground
          MouseArea {
            anchors.fill: parent
            hoverEnabled: true
            onClicked: if (page.current) page.app.recolourTag(page.current.name, sw.modelData.key)
          }
        }
      }
    }
    UiText {
      visible: !!page.current && page.current.accounts.some(function(a) { return a.color_pending })
      width: parent.width
      dim: true
      font.pixelSize: Style.font.caption
      text: "Colour change waiting to reach the server."
    }

    // per account
    UiText { text: "Accounts"; dim: true; font.pixelSize: Style.font.caption }
    Repeater {
      model: page.current ? page.current.accounts.filter(function(a) { return a.email !== "" }) : []
      delegate: RowLayout {
        id: acct
        required property var modelData
        width: parent.width
        spacing: Style.spacing.lg
        UiText {
          Layout.fillWidth: true
          elide: Text.ElideRight
          text: acct.modelData.email + "   (" + acct.modelData.provider + ")"
        }
        UiText {
          dim: true
          font.pixelSize: Style.font.caption
          text: acct.modelData.defined
            ? (acct.modelData.pending ? "being created, " : "") + page.n(acct.modelData.messages) + (acct.modelData.messages === 1 ? " message" : " messages")
            : (acct.modelData.messages > 0 ? page.n(acct.modelData.messages) + " messages, not in its list" : "not on this account")
        }
        Button {
          visible: !acct.modelData.defined
          Layout.preferredWidth: Style.space(96)
          bordered: true
          iconText: "\uf067"
          text: "Add"
          onClicked: if (page.current) page.app.adoptTag(page.current.name, acct.modelData.account_id)
        }
      }
    }

    // delete (two steps; says what it will touch)
    UiText {
      visible: page.confirmingDelete
      width: parent.width
      wrapMode: Text.WordWrap
      foreground: Commons.Color.urgent
      text: page.current ? ("This takes \u201c" + page.current.name + "\u201d off " + page.n(page.current.total) + (page.current.total === 1 ? " message" : " messages") + " and removes it from every account that has it. Mail is not deleted." + (page.ruleCount(page.current.name) > 0 ? " " + page.ruleCount(page.current.name) + (page.ruleCount(page.current.name) === 1 ? " rule" : " rules") + " on the server assign it and will keep tagging new mail: delete them on the Rules page first." : "")) : ""
    }
    RowLayout {
      width: parent.width
      spacing: Style.spacing.lg
      Item { Layout.fillWidth: true }
      Button {
        visible: page.confirmingDelete
        Layout.preferredWidth: Style.space(96)
        bordered: true
        text: "Cancel"
        onClicked: page.confirmingDelete = false
      }
      Button {
        Layout.preferredWidth: Style.space(140)
        bordered: true
        iconText: "\uf1f8"
        text: page.confirmingDelete ? "Yes, delete" : "Delete tag"
        foreground: page.confirmingDelete ? Commons.Color.urgent : Commons.Color.foreground
        onClicked: {
          if (!page.confirmingDelete) { page.confirmingDelete = true; return }
          page.confirmingDelete = false
          if (page.current) { var nm = page.current.name; page.selectedKey = ""; page.app.deleteTag(nm) }
        }
      }
    }

    PanelSeparator { width: parent.width }
  }

  // ---- every tag (the selected tag's panel is above, so it is never off screen)
  Column {
    width: parent.width
    spacing: Style.space(6)

    UiText {
      visible: page.rows.length === 0
      width: parent.width
      text: "No tags yet. Press t on a message to tag it."
      dim: true
    }

    Repeater {
      model: page.shown
      delegate: Rectangle {
        id: row
        required property var modelData
        readonly property bool on: TagNames.key(modelData.name) === page.selectedKey
        width: parent.width
        height: Style.spacing.popupRowHeight
        radius: Style.cornerRadius
        color: on ? Style.selectedFill : "transparent"

        RowLayout {
          anchors.fill: parent
          anchors.leftMargin: Style.spacing.lg
          anchors.rightMargin: Style.spacing.lg
          spacing: Style.spacing.lg
          Rectangle { Layout.preferredWidth: Style.space(12); Layout.preferredHeight: Style.space(12); radius: 3; color: row.modelData.color }
          UiText { Layout.fillWidth: true; text: row.modelData.name; elide: Text.ElideRight }
          UiText { text: page.counts(row.modelData); dim: true; font.pixelSize: Style.font.caption }
        }
        MouseArea {
          anchors.fill: parent
          onClicked: { page.confirmingDelete = false; page.selectedKey = row.on ? "" : TagNames.key(row.modelData.name) }
        }
      }
    }
  }
}
