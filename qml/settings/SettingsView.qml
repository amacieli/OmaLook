import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import qs.Commons
import qs.Ui
import "../common"
import "pages"

// Settings: category list on the left, one sub-page on the right. Add a
// category by (1) adding it to AppState.settingsCategories and (2) adding a
// case to `pageFor` below.
RowLayout {
  id: root

  required property var app

  signal loginRequested(string provider)
  signal accountLoginRequested(string accountId, string provider)
  signal logoutRequested(string accountId)
  signal removeRequested(string accountId)

  // True while the active page has an input that owns the keyboard.
  readonly property bool editing: pageLoader.item ? pageLoader.item.editing === true : false

  // Put the keyboard into the active page's first control, if it has one (l / Right / Enter).
  function enterPage() {
    var it = pageLoader.item
    return (it && it.focusFirst) ? it.focusFirst() === true : false
  }

  spacing: 0

  // ---- category list
  Item {
    Layout.preferredWidth: Style.space(200)
    Layout.fillHeight: true

    ColumnLayout {
      anchors.fill: parent
      anchors.margins: Style.spacing.md
      spacing: Style.spacing.sm

      PanelSectionHeader {
        text: "SETTINGS"
        Layout.leftMargin: Style.spacing.sm
      }

      Repeater {
        model: root.app.settingsCategories

        Button {
          required property var modelData
          required property int index
          Layout.fillWidth: true
          leftAlign: true
          iconText: modelData.icon
          text: modelData.label
          selected: root.app.settingsCategoryIndex === index
          hasCursor: root.app.focusPane === "msg" && root.app.settingsCategoryIndex === index
          onClicked: root.app.clickCategory(index)
        }
      }

      Item { Layout.fillHeight: true }
    }
  }

  VSeparator { Layout.preferredWidth: 1; Layout.fillHeight: true }

  // ---- active page
  ScrollView {
    id: scroll
    Layout.fillWidth: true
    Layout.fillHeight: true
    clip: true
    ScrollBar.horizontal.policy: ScrollBar.AlwaysOff

    Loader {
      id: pageLoader
      x: Style.space(20)
      y: Style.space(16)
      width: scroll.availableWidth - Style.space(40)
      sourceComponent: {
        switch (root.app.settingsCategories[root.app.settingsCategoryIndex].id) {
          case "general":       return generalPage
          case "account":       return accountPage
          case "appearance":    return appearancePage
          case "mail":          return mailPage
          case "senders":       return sendersPage
          case "calendar":      return calendarPage
          case "sms":           return smsPage
          case "notifications": return notificationsPage
          default:              return aboutPage
        }
      }
    }
  }

  Component {
    id: accountPage
    AccountPage {
      accounts: root.app.accounts
      isAuthenticated: root.app.auth.isAuthenticated
      onLoginRequested: function(provider) { root.loginRequested(provider) }
      onAccountLoginRequested: function(accountId, provider) { root.accountLoginRequested(accountId, provider) }
      onLogoutRequested: function(accountId) { root.logoutRequested(accountId) }
      onRemoveRequested: function(accountId) { root.removeRequested(accountId) }
    }
  }
  Component {
    id: appearancePage
    AppearancePage { app: root.app }
  }
  Component { id: generalPage;       GeneralPage { app: root.app } }
  Component { id: mailPage;          MailPage { app: root.app } }
  Component { id: sendersPage;       SendersPage { app: root.app } }
  Component { id: calendarPage;      CalendarPage { app: root.app } }
  Component { id: smsPage;           SmsPage {} }
  Component { id: notificationsPage; NotificationsPage {} }
  Component {
    id: aboutPage
    AboutPage {
      backendUrl: root.app.backendUrl
      configDir: root.app.configDir
      backendOnline: root.app.backendOnline
    }
  }
}
