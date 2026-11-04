import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../../common"
import "../../compose/modes.js" as Modes
import ".."

// Lists: distribution lists kept by OmaLook. Pick a list in the drop-down to see its members; add or
// remove members, rename or delete the list, or make a new one. While typing an address, the
// address book and every address seen in mail (From, To, Cc, Bcc) are suggested. In a message, a
// list name is suggested in To / Cc / Bcc and becomes one chip that expands when the message is
// queued (so To, Cc and Bcc each keep their own audience: the field decides who sees whom).
SettingsPage {
  id: page

  property var app: null
  property int selectedId: 0

  editing: newField.activeFocus || renameField.activeFocus || addField.activeFocus || listDropdown.popupOpen

  title: "Lists"
  description: "Distribution lists you can type into To, Cc or Bcc. They belong to OmaLook, not to one account, so any From account can use them."

  readonly property var lists: app ? app.dlLists : []
  readonly property var current: {
    for (var i = 0; i < lists.length; i++) if (lists[i].id === selectedId) return lists[i]
    return null
  }

  function ensureSelection() {
    if (current) return
    selectedId = lists.length > 0 ? lists[0].id : 0
  }
  onListsChanged: ensureSelection()

  Component.onCompleted: {
    if (!page.app) return
    page.app.listNotice = ""
    page.app.loadLists(function() { page.ensureSelection() })
    page.app.loadListTargets()
    if (page.app.composeContacts.length === 0) page.app.loadComposeContacts()
  }

  // ---- add-member suggestions
  property var matches: []
  property int pick: -1

  function refreshMatches() {
    var q = addField.text.trim().toLowerCase()
    if (q.length < 1 || !app || !current) { matches = []; pick = -1; return }
    var have = {}
    for (var i = 0; i < current.members.length; i++) have[current.members[i].email.toLowerCase()] = true
    var out = [], c = app.composeContacts
    for (var j = 0; j < c.length && out.length < 6; j++) {
      var e = String(c[j].email).toLowerCase()
      if (have[e]) continue
      if (e.indexOf(q) >= 0 || String(c[j].name || "").toLowerCase().indexOf(q) >= 0) out.push(c[j])
    }
    matches = out
    pick = out.length > 0 ? 0 : -1
  }

  // Add what is picked in the suggestions, or whatever was typed or pasted ("a@b.c, Name <d@e.f>").
  function submitMember(raw) {
    if (!app || !current) return
    var items = []
    if (raw !== true && pick >= 0 && pick < matches.length) items = [{ email: matches[pick].email, name: matches[pick].name || "" }]
    else items = Modes.parseRecipients(addField.text).map(function(r) { return { email: r.email, name: r.name } })
    if (items.length === 0) { app.listNotice = "Enter an email address."; return }
    var id = current.id, i = 0
    function next(r, st) {
      if (st === "invalid" && i > 0) { /* keep the notice of the bad one */ }
      if (i >= items.length) { addField.text = ""; page.matches = []; page.pick = -1; return }
      var it = items[i++]
      app.addListMember(id, it.email, it.name, next)
    }
    next(null, "")
  }

  Column {
    width: parent.width
    spacing: Style.space(6)

    PanelSectionHeader { text: "LIST" }

    Dropdown {
      id: listDropdown
      width: parent.width
      showLabel: false
      visible: page.lists.length > 0
      value: String(page.selectedId)
      options: page.lists.map(function(l) { return { value: String(l.id), label: l.name + "  (" + l.members.length + (l.members.length === 1 ? " member)" : " members)") } })
      onChanged: function(v) { page.selectedId = parseInt(v); addField.text = "" }
    }

    UiText {
      visible: page.lists.length === 0
      width: parent.width
      text: "No lists yet. Name one below."
      dim: true
    }

    RowLayout {
      width: parent.width
      spacing: Style.spacing.lg

      TextField {
        id: newField
        Layout.fillWidth: true
        placeholderText: "New list name"
        onAccepted: page.createNew()
        onTextChanged: if (page.app && page.app.listNotice !== "") page.app.listNotice = ""
      }
      Button {
        Layout.alignment: Qt.AlignVCenter
        bordered: true
        iconText: "\uf067"
        text: "New list"
        onClicked: page.createNew()
      }
    }

    UiText {
      visible: !!page.app && page.app.listNotice !== ""
      width: parent.width
      text: page.app ? page.app.listNotice : ""
      foreground: Commons.Color.urgent
      wrapMode: Text.WordWrap
    }
  }

  function createNew() {
    if (!app) return
    app.createList(newField.text, function(r, st) {
      if (st === "done" && r && r.id) { page.selectedId = r.id; newField.text = "" }
    })
  }

  PanelSeparator { width: parent.width }

  // ---- where the lists are kept
  Column {
    width: parent.width
    spacing: Style.space(6)

    PanelSectionHeader { text: "KEEP ALL LISTS ON" }

    UiText {
      width: parent.width
      text: "Every account can always use every list, wherever they are kept. This only chooses which places also hold a copy."
      dim: true
      wrapMode: Text.WordWrap
      font.pixelSize: Style.font.caption
    }

    RowLayout {
      width: parent.width
      spacing: Style.spacing.lg
      UiText { Layout.fillWidth: true; text: "This computer" }
      UiText { text: "always"; dim: true; font.pixelSize: Style.font.caption }
    }

    Repeater {
      model: page.app ? page.app.accounts.filter(function(a) { return a.email }) : []
      delegate: ColumnLayout {
        id: acct
        required property var modelData
        readonly property bool canSync: modelData.provider === "gmail"
        readonly property bool on: !!page.app && page.app.listSyncTargets.indexOf(modelData.id) >= 0
        width: parent.width
        spacing: 2

        RowLayout {
          Layout.fillWidth: true
          spacing: Style.spacing.lg
          UiText {
            Layout.fillWidth: true
            text: acct.modelData.email + "   (" + acct.modelData.provider + ")"
            elide: Text.ElideRight
          }
          ToggleSwitch {
            enabled: acct.canSync
            opacity: acct.canSync ? 1 : 0.4
            checked: acct.on
            onToggled: page.app.setListTarget(acct.modelData.id, !acct.on)
          }
        }
        UiText {
          Layout.fillWidth: true
          wrapMode: Text.WordWrap
          dim: true
          font.pixelSize: Style.font.caption
          text: !acct.canSync
            ? "Outlook cannot store distribution lists. They stay on this computer and still work when sending from this account."
            : (acct.on ? "Chosen. Copying lists to this account is not built yet: for now they stay on this computer, and still work from this account."
                       : "Off.")
        }
      }
    }
  }

  PanelSeparator { width: parent.width; visible: !!page.current }

  // ---- the selected list: rename / delete
  Column {
    visible: !!page.current
    width: parent.width
    spacing: Style.space(6)

    PanelSectionHeader { text: "NAME" }

    RowLayout {
      width: parent.width
      spacing: Style.spacing.lg

      TextField {
        id: renameField
        Layout.fillWidth: true
        text: page.current ? page.current.name : ""
        onAccepted: page.renameCurrent()
        onTextChanged: if (page.app && page.app.listNotice !== "") page.app.listNotice = ""
      }
      Button {
        Layout.alignment: Qt.AlignVCenter
        bordered: true
        iconText: "\uf044"
        text: "Rename"
        enabled: !!page.current && renameField.text.trim() !== "" && renameField.text.trim() !== page.current.name
        onClicked: page.renameCurrent()
      }
      // Two-step, like removing an account.
      Button {
        id: deleteButton
        property bool confirming: false
        Layout.alignment: Qt.AlignVCenter
        Layout.preferredWidth: Style.space(110)
        bordered: true
        iconText: "\uf1f8"
        text: confirming ? "Sure?" : "Delete list"
        foreground: confirming ? Commons.Color.urgent : Commons.Color.foreground
        onClicked: {
          if (confirming) { confirming = false; if (page.current) page.app.deleteList(page.current.id) }
          else confirming = true
        }
        Timer { interval: 4000; running: deleteButton.confirming; onTriggered: deleteButton.confirming = false }
      }
    }
  }

  function renameCurrent() {
    if (app && current) app.renameList(current.id, renameField.text)
  }

  PanelSeparator { width: parent.width; visible: !!page.current }

  // ---- members
  Column {
    visible: !!page.current
    width: parent.width
    spacing: Style.space(6)

    PanelSectionHeader { text: page.current ? "ADD A MEMBER TO " + page.current.name.toUpperCase() : "" }

    RowLayout {
      width: parent.width
      spacing: Style.spacing.lg

      TextField {
        id: addField
        Layout.fillWidth: true
        placeholderText: "Name or address (suggested from your contacts and mail)"
        onTextChanged: { if (page.app && page.app.listNotice !== "") page.app.listNotice = ""; page.refreshMatches() }
        onAccepted: page.submitMember(false)
        Keys.onPressed: function(e) {
          if (e.key === Qt.Key_Down && page.matches.length > 0) { page.pick = Math.min(page.pick + 1, page.matches.length - 1); e.accepted = true }
          else if (e.key === Qt.Key_Up && page.matches.length > 0) { page.pick = Math.max(page.pick - 1, 0); e.accepted = true }
          else if (e.key === Qt.Key_Escape && page.matches.length > 0) { page.matches = []; page.pick = -1; e.accepted = true }
        }
      }
      Button {
        Layout.alignment: Qt.AlignVCenter
        bordered: true
        iconText: "\uf067"
        text: "Add"
        onClicked: page.submitMember(true)
      }
    }

    // Suggestions sit in the flow (not floating) so the settings scroll view never clips them.
    Repeater {
      model: page.matches
      delegate: Rectangle {
        id: sug
        required property var modelData
        required property int index
        width: parent.width
        height: Style.spacing.popupRowHeight
        radius: Style.cornerRadius
        color: index === page.pick ? Style.selectedFill : "transparent"
        UiText {
          anchors.verticalCenter: parent.verticalCenter
          x: Style.spacing.lg
          width: parent.width - Style.spacing.lg * 2
          elide: Text.ElideRight
          text: sug.modelData.name ? sug.modelData.name + "  <" + sug.modelData.email + ">" : sug.modelData.email
        }
        MouseArea {
          anchors.fill: parent
          hoverEnabled: true
          onEntered: page.pick = sug.index
          onClicked: { page.pick = sug.index; page.submitMember(false); addField.forceActiveFocus() }
        }
      }
    }
  }

  Column {
    visible: !!page.current
    width: parent.width
    spacing: Style.space(4)

    PanelSectionHeader {
      text: page.current ? "MEMBERS (" + page.current.members.length + ")" : ""
    }

    UiText {
      visible: !!page.current && page.current.members.length === 0
      width: parent.width
      text: "Nobody in this list yet. A list with no members cannot be sent to."
      dim: true
      wrapMode: Text.WordWrap
    }

    Repeater {
      model: page.current ? page.current.members : []

      delegate: RowLayout {
        id: row
        required property var modelData
        width: parent.width
        spacing: Style.spacing.lg

        UiText {
          Layout.fillWidth: true
          text: row.modelData.name ? row.modelData.name + "   <" + row.modelData.email + ">" : row.modelData.email
          elide: Text.ElideRight
        }
        Button {
          Layout.preferredWidth: Style.space(96)
          bordered: true
          iconText: "\uf1f8"
          text: "Remove"
          onClicked: if (page.current) page.app.removeListMember(page.current.id, row.modelData.email)
        }
      }
    }
  }
}
