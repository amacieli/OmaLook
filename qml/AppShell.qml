import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "mail"
import "chrome"
import "auth"
import "settings"
import "views"
import "state"
import "common"
import "keys"

// Window content: wires the state object to the presentational components and
// owns keyboard dispatch. No styling or backend logic lives here.
FocusScope {
  id: root

  signal closeRequested()

  function focusKeys() { keyCatcher.forceActiveFocus() }

  // UI scale factor (Settings -> Appearance); shell.qml scales this whole item by it.
  readonly property real uiScale: appState.uiScale

  AppState {
    id: appState
    onFocusRequested: root.focusKeys()
    onCloseRequested: root.closeRequested()
  }

  Component.onCompleted: root.focusKeys()

  // ---- keyboard: registry (what each key does) + router (which key, which scope)
  AppCommands {
    id: commands
    app: appState
    config: appState.keyConfig
    content: contentLoader.item
    folderPageRows: folderBar.pageRows
    onPaletteRequested: function(prefill) { palette.show(prefill) }
    onHelpRequested: help.show()
    onQuitRequested: root.closeRequested()
  }

  CommandPalette {
    id: palette
    anchors.fill: parent
    z: 950
    commands: commands.effective
    scopes: keyCatcher.scopes
    onClosed: root.focusKeys()
  }

  HelpOverlay {
    id: help
    anchors.fill: parent
    z: 960
    commands: commands.effective
    problems: commands.problems
    preset: appState.keyConfig.preset
    scopes: keyCatcher.scopes
    onClosed: root.focusKeys()
  }

  KeyRouter {
    id: keyCatcher
    anchors.fill: parent
    commands: commands.effective

    // Innermost first: the pane, "<view>/<pane>", the view, then everything.
    readonly property string paneScope: appState.focusPane === "msg" ? "list" : appState.focusPane
    scopes: [paneScope, appState.currentView + "/" + paneScope, appState.currentView, "global"]

    // A yes/no question (permanent delete) takes every key until it is answered.
    interceptor: function(e) {
      // Link hint labels (f in the reading pane) take every key until a label is typed or Esc.
      var ci = contentLoader.item
      if (ci && ci.hintMode === true && !appState.pendingConfirm) return ci.hintInput(e.key, e.text) === true
      if (!appState.pendingConfirm) return false
      confirmDialog.handleKey(e)
      return true
    }

    // Hand keys to text inputs / popups while they own focus.
    blocked: settingsEditing || appState.auth.showModal || palette.open || help.open

    // A page (settings fields, the mail view's image dropdown, compose) owns the keyboard.
    readonly property bool settingsEditing: contentLoader.item && contentLoader.item.editing === true

    // Esc must still dismiss the auth modal even though `blocked` is set.
    Keys.onEscapePressed: function(event) {
      event.accepted = appState.auth.showModal
      if (event.accepted) appState.auth.cancel()
    }

    ColumnLayout {
      anchors.fill: parent
      spacing: 0

      RowLayout {
        Layout.fillWidth: true
        Layout.fillHeight: true
        Layout.margins: Style.spacing.sm
        spacing: Style.spacing.sm

        FolderBar {
          id: folderBar
          visible: appState.currentView === "mail" && appState.showFolderPane
          Layout.preferredWidth: visible ? implicitWidth : 0
          Layout.fillHeight: true
          model: appState.folderModel
          currentIndex: appState.folderIndex
          selectedId: appState.selectedFolderId
          paneFocused: appState.focusPane === "folder"
          onFolderClicked: function(i) { appState.clickFolder(i) }
          onRefreshRequested: { appState.loadFolders(); appState.focusRequested() }
        }

        // Mail draws its own two frames (list, reader); every other view is one pane.
        Item {
          id: viewArea
          Layout.fillWidth: true
          Layout.fillHeight: true

          // Views that draw their own pane frames (mail, SMS) vs. one framed pane.
          readonly property bool ownsFrames: appState.currentView === "mail" || appState.currentView === "sms"

          Loader {
            id: contentLoader
            anchors.fill: parent
            // Non-mail views sit inside the frame; mail's panes draw their own.
            anchors.margins: viewArea.ownsFrames ? 0 : 2
            anchors.topMargin: viewArea.ownsFrames ? 0 : Style.spacing.xl
            sourceComponent: {
              switch (appState.currentView) {
                case "settings": return settingsComponent
                case "calendar": return calendarComponent
                case "contacts": return peopleComponent
                case "tasks":    return tasksComponent
                case "sms":      return smsComponent
                default:         return mailComponent
              }
            }
          }

          PaneFrame {
            tint: Hues.blue
            visible: !viewArea.ownsFrames
            focused: appState.focusPane === "msg"
            hotkey: "\u00b9"
            title: appState.currentViewLabel
          }
        }
      }

      PanelSeparator { Layout.fillWidth: true }

      BottomStatusBar {
        Layout.fillWidth: true
        backendOnline: appState.backendOnline
        isAuthenticated: appState.auth.isAuthenticated
        viewLabel: appState.currentViewLabel
        folderName: appState.selectedFolderName
        messageCount: appState.messageTotal
        searchQuery: appState.searchQuery
        searchCount: appState.messageModel.count
        searchMore: appState._hasMoreMessages
        searchPartial: appState.searchPartial
        unreadCount: appState.unreadCount
        focusPane: appState.focusPane
        currentView: appState.currentView
        pendingText: keyCatcher.pendingText
        pendingOptions: keyCatcher.pendingOptions
        notice: commands.notice
        markCount: appState.markCount
        metaActive: appState.metaActive
        metaRemaining: appState.metaRemaining
        keyOf: commands.keyOf
        linkCount: contentLoader.item && contentLoader.item.linkCount !== undefined ? contentLoader.item.linkCount : 0
        linkIndex: contentLoader.item && contentLoader.item.linkIndex !== undefined ? contentLoader.item.linkIndex : -1
        linkHref: contentLoader.item && contentLoader.item.linkHref !== undefined ? contentLoader.item.linkHref : ""
        hintMode: !!contentLoader.item && contentLoader.item.hintMode === true
      }
    }

    Component { id: mailComponent;     MailView { app: appState } }
    Component { id: calendarComponent; CalendarView { mode: appState.calendarMode; dataRev: appState.calendarRev; onModeChanged: appState.calendarMode = mode; monoFont: Style.font.family; accentColor: Commons.Color.accent; successColor: Commons.Color.accent; dangerColor: Commons.Color.urgent; textColor: Commons.Color.foreground } }
    Component { id: peopleComponent;   PeopleView   { viewMode: appState.peopleView; onViewModeChanged: appState.peopleView = viewMode; sortKey: appState.peopleSort; onSortKeyChanged: appState.peopleSort = sortKey; monoFont: Style.font.family; accentColor: Commons.Color.accent; successColor: Commons.Color.accent; dangerColor: Commons.Color.urgent; textColor: Commons.Color.foreground } }
    Component { id: tasksComponent;    TasksView    { filter: appState.tasksFilter; onFilterChanged: appState.tasksFilter = filter; monoFont: Style.font.family; accentColor: Commons.Color.accent; successColor: Commons.Color.accent; dangerColor: Commons.Color.urgent; textColor: Commons.Color.foreground } }
    Component { id: smsComponent;      SmsView { app: appState } }
    Component {
      id: settingsComponent
      SettingsView {
        app: appState
        onLoginRequested: function(provider) { appState.auth.startLogin(provider) }
        onAccountLoginRequested: function(accountId, provider) { appState.auth.loginAccount(accountId, provider) }
        onLogoutRequested: function(accountId) { appState.auth.logout(accountId) }
        onRemoveRequested: function(accountId) { appState.auth.removeAccount(accountId) }
      }
    }
  }

  // Pops up after a short pause on a half-typed chord such as `g`.
  WhichKey {
    anchors.left: parent.left
    anchors.bottom: parent.bottom
    anchors.leftMargin: Style.spacing.huge
    anchors.bottomMargin: Style.space(40)
    z: 900
    pendingText: keyCatcher.pendingText
    options: keyCatcher.pendingOptions
  }

  ConfirmDialog {
    id: confirmDialog
    anchors.fill: parent
    z: 1100
    opened: appState.pendingConfirm !== null
    message: appState.pendingConfirm ? appState.pendingConfirm.message : ""
    confirmText: appState.pendingConfirm ? appState.pendingConfirm.confirmText : "Confirm"
    // Enter must never destroy mail by accident: the dialog opens on Cancel.
    onOpenedChanged: if (opened) selectedIndex = 0
    onCanceled: appState.pendingConfirm = null
    onConfirmed: { var c = appState.pendingConfirm; appState.pendingConfirm = null; if (c) c.run() }
  }

  Connections {
    target: appState
    function onNotify(text) { commands.say(text) }
    function onFocusPaletteRequested(prefill) { palette.show(prefill) }
  }

  ActionToast {
    id: actionToast
    app: appState
    anchors.right: parent.right
    anchors.bottom: parent.bottom
    anchors.rightMargin: Style.spacing.huge
    anchors.bottomMargin: Style.space(40) + (sendToast.visible ? sendToast.height + Style.spacing.md : 0)
    z: 900
  }

  SendToast {
    id: sendToast
    app: appState
    anchors.right: parent.right
    anchors.bottom: parent.bottom
    anchors.rightMargin: Style.spacing.huge
    anchors.bottomMargin: Style.space(40)      // clear of the bottom status bar
    z: 900
  }

  AuthModal {
    anchors.fill: parent
    auth: appState.auth
    z: 1000
  }

  ReauthConfirm {
    anchors.fill: parent
    auth: appState.auth
    z: 1001
  }
}
