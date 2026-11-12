import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../../common"
import ".."

// Meeting accounts block of Settings -> Accounts (docs/PLAN-calendar-events.md 7.5, OL-021 track B / OL-010).
// Teams and Meet come with the calendar account (read-only rows); Zoom and Webex are stubs until an OAuth app
// exists; the personal link and Jitsi need no sign-in. Saved to settings.toml `[meetings]` by the backend.
Column {
  id: sec

  property var app: null
  readonly property var ms: app ? app.meetingSettings : ({})
  property string newProvider: "zoom"
  property string addNotice: ""

  readonly property bool editing: addDropdown.popupOpen || labelField.activeFocus || linkField.activeFocus || serverField.activeFocus
    || defaultsRepeater.anyOpen

  width: parent ? parent.width : 400
  spacing: Style.space(6)

  // calendar accounts that can host a meeting: [{id, email, provider}]
  readonly property var calendarAccounts: {
    var out = []
    var list = app ? app.accounts : []
    for (var i = 0; i < list.length; i++)
      if ((list[i].provider === "exchange" || list[i].provider === "gmail") && list[i].email) out.push(list[i])
    return out
  }

  function addMeetingAccount() {
    if (newProvider === "zoom" || newProvider === "webex")
      addNotice = (newProvider === "zoom" ? "Zoom" : "Webex") + " sign-in is not built yet (needs setup). Your personal Zoom or Webex link works below."
    else if (newProvider === "jitsi")
      addNotice = "Jitsi needs no sign-in: it is on below. Change the server there if you run your own."
    else
      addNotice = "Type your link below."
  }

  function defaultOptions(provider) {
    var o = [{ value: "none", label: "None" }]
    o.push(provider === "gmail" ? { value: "googleMeet", label: "Google Meet" } : { value: "teamsForBusiness", label: "Microsoft Teams" })
    o.push({ value: "personalLink", label: ms.personal_label ? ms.personal_label : "Personal meeting link" })
    o.push({ value: "jitsi", label: "Jitsi" })
    return o
  }

  PanelSeparator { width: parent.width }
  PanelSectionHeader { text: "ADD MEETING ACCOUNT" }

  RowLayout {
    width: parent.width
    spacing: Style.spacing.lg
    Dropdown {
      id: addDropdown
      Layout.fillWidth: true
      Layout.alignment: Qt.AlignVCenter
      showLabel: false
      value: sec.newProvider
      options: [{ value: "zoom", label: "Zoom" }, { value: "webex", label: "Webex" },
                { value: "jitsi", label: "Jitsi" }, { value: "personal", label: "Personal meeting link" }]
      onChanged: function(v) { sec.newProvider = v; sec.addNotice = "" }
    }
    Button {
      Layout.alignment: Qt.AlignVCenter
      bordered: true
      iconText: "\uf067"
      text: "Add"
      onClicked: sec.addMeetingAccount()
    }
  }
  UiText { visible: sec.addNotice !== ""; width: parent.width; text: sec.addNotice; dim: true; wrapMode: Text.WordWrap }

  PanelSeparator { width: parent.width }
  PanelSectionHeader { text: "MEETING ACCOUNTS" }

  Repeater {
    model: sec.calendarAccounts
    delegate: UiText {
      required property var modelData
      width: sec.width
      text: (modelData.provider === "gmail" ? "Google Meet" : "Microsoft Teams") + "   included with " + modelData.email
      elide: Text.ElideRight
    }
  }
  UiText { visible: sec.calendarAccounts.length === 0; width: parent.width; text: "No Microsoft or Google calendar account signed in."; dim: true }
  UiText { width: parent.width; text: "Zoom   needs setup (sign-in not built yet)"; dim: true }
  UiText { width: parent.width; text: "Webex   needs setup (sign-in not built yet)"; dim: true }

  Toggle {
    width: parent.width
    label: "Offer Microsoft Teams"
    checked: sec.ms.teams_enabled !== false
    onClicked: if (sec.app) sec.app.saveMeetingSettings({ teams_enabled: !(sec.ms.teams_enabled !== false) })
  }
  Toggle {
    width: parent.width
    label: "Offer Google Meet"
    checked: sec.ms.meet_enabled !== false
    onClicked: if (sec.app) sec.app.saveMeetingSettings({ meet_enabled: !(sec.ms.meet_enabled !== false) })
  }
  Toggle {
    width: parent.width
    label: "Offer Jitsi"
    description: "A fresh https://" + (sec.ms.jitsi_server || "meet.jit.si") + "/ room for each event, no account needed"
    checked: sec.ms.jitsi_enabled !== false
    onClicked: if (sec.app) sec.app.saveMeetingSettings({ jitsi_enabled: !(sec.ms.jitsi_enabled !== false) })
  }
  TextField {
    id: serverField
    width: parent.width
    placeholderText: "Jitsi server (default meet.jit.si)"
    text: sec.ms.jitsi_server && sec.ms.jitsi_server !== "meet.jit.si" ? sec.ms.jitsi_server : ""
    onEditingFinished: if (sec.app) sec.app.saveMeetingSettings({ jitsi_server: text.trim() })
  }

  PanelSeparator { width: parent.width }
  PanelSectionHeader { text: "PERSONAL MEETING LINK" }

  TextField {
    id: labelField
    width: parent.width
    placeholderText: "Name, e.g. My Zoom room"
    text: sec.ms.personal_label || ""
    onEditingFinished: if (sec.app) sec.app.saveMeetingSettings({ personal_label: text })
  }
  RowLayout {
    width: parent.width
    spacing: Style.spacing.lg
    TextField {
      id: linkField
      Layout.fillWidth: true
      placeholderText: "https://zoom.us/j/..."
      text: sec.ms.personal_link || ""
      onEditingFinished: if (sec.app) sec.app.saveMeetingSettings({ personal_link: text.trim() })
    }
    Button {
      Layout.alignment: Qt.AlignVCenter
      bordered: true
      iconText: "\uf1f8"
      text: "Remove"
      visible: !!sec.ms.personal_link
      onClicked: if (sec.app) { linkField.text = ""; labelField.text = ""; sec.app.saveMeetingSettings({ personal_link: "", personal_label: "" }) }
    }
  }
  Toggle {
    width: parent.width
    label: "Offer the personal link"
    checked: sec.ms.personal_enabled !== false
    onClicked: if (sec.app) sec.app.saveMeetingSettings({ personal_enabled: !(sec.ms.personal_enabled !== false) })
  }
  UiText {
    visible: !!sec.app && sec.app.meetingNotice !== ""
    width: parent.width
    text: sec.app ? sec.app.meetingNotice : ""
    foreground: Commons.Color.urgent
    wrapMode: Text.WordWrap
  }

  PanelSeparator { width: parent.width }
  PanelSectionHeader { text: "DEFAULT ONLINE MEETING FOR NEW EVENTS" }

  Repeater {
    id: defaultsRepeater
    property bool anyOpen: false
    model: sec.calendarAccounts
    delegate: Dropdown {
      required property var modelData
      width: sec.width
      label: modelData.email
      value: (sec.ms.default_provider && sec.ms.default_provider[modelData.id]) || "none"
      options: sec.defaultOptions(modelData.provider)
      onPopupOpenChanged: defaultsRepeater.anyOpen = popupOpen
      onChanged: function(v) { if (sec.app) sec.app.setMeetingDefault(modelData.id, v) }
    }
  }
  UiText {
    width: parent.width
    text: "Preselected in the event editor for that calendar; you can change it per event."
    dim: true; wrapMode: Text.WordWrap; font.pixelSize: Style.font.caption
  }
}
