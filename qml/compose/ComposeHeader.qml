import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Ui
import "../common"

// From / To / Cc / Bcc / Subject. Pure view: the owner reads recipients and subject back
// from the aliased fields.
ColumnLayout {
  id: root

  property var accounts: []          // [{ id, provider, email, signed_in }]
  property string accountId: ""
  property var contacts: []
  property var lists: []
  property bool showCc: false
  property bool showBcc: false
  // Where Tab goes from Subject (the body editor; wired by ComposeView).
  property Item nextFocus: null

  readonly property alias to: toField
  readonly property alias cc: ccField
  readonly property alias bcc: bccField
  readonly property alias subject: subjectField

  signal accountPicked(string id)
  signal edited()

  spacing: Style.spacing.md

  readonly property var _signedIn: accounts.filter(function(a) { return a.signed_in && a.email })

  RowLayout {
    Layout.fillWidth: true
    visible: root._signedIn.length > 1
    spacing: Style.spacing.lg
    UiText { Layout.preferredWidth: Style.space(52); text: "From"; dim: true; font.pixelSize: Style.font.caption }
    Dropdown {
      Layout.preferredWidth: Style.space(320)
      showLabel: false
      value: root.accountId
      options: root._signedIn.map(function(a) { return { value: a.id, label: a.email } })
      onChanged: function(v) { root.accountPicked(v) }
    }
    Item { Layout.fillWidth: true }
  }

  RowLayout {
    Layout.fillWidth: true
    spacing: Style.spacing.md
    z: toField.activeFocus ? 50 : 0     // keep the To suggestion list above the rows below

    RecipientField {
      id: toField
      Layout.fillWidth: true
      label: "To"
      contacts: root.contacts
      lists: root.lists
      onChanged: root.edited()
      KeyNavigation.tab: root.showCc ? ccField.input : (root.showBcc ? bccField.input : subjectField)
    }

    Button {
      visible: !root.showCc
      Layout.alignment: Qt.AlignTop
      text: "Cc"
      bordered: true
      onClicked: { root.showCc = true; ccField.input.forceActiveFocus() }
    }
    Button {
      visible: !root.showBcc
      Layout.alignment: Qt.AlignTop
      text: "Bcc"
      bordered: true
      onClicked: { root.showBcc = true; bccField.input.forceActiveFocus() }
    }
  }

  RecipientField {
    id: ccField
    visible: root.showCc
    Layout.fillWidth: true
    label: "Cc"
    contacts: root.contacts
    lists: root.lists
    onChanged: root.edited()
    KeyNavigation.tab: root.showBcc ? bccField.input : subjectField
  }

  RecipientField {
    id: bccField
    visible: root.showBcc
    Layout.fillWidth: true
    label: "Bcc"
    contacts: root.contacts
    lists: root.lists
    onChanged: root.edited()
    KeyNavigation.tab: subjectField
  }

  RowLayout {
    Layout.fillWidth: true
    spacing: Style.spacing.lg
    UiText { Layout.preferredWidth: Style.space(52); text: "Subject"; dim: true; font.pixelSize: Style.font.caption }
    TextField {
      id: subjectField
      Layout.fillWidth: true
      placeholderText: "Subject"
      KeyNavigation.tab: root.nextFocus
      onTextChanged: root.edited()
    }
  }
}
