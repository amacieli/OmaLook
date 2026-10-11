import QtQuick
import QtQuick.Layouts
import qs.Commons
import "../common"
// Makes Quickshell scan the compose files (they are only loaded on demand, through a Loader).
import "../compose"

// Composes the message list and the reading pane. Glue only: data + intents
// come from the shared AppState.
RowLayout {
  id: root

  required property var app

  readonly property int pageRows: messages.pageRows
  readonly property bool editing: preview.editing || root.app.composing

  spacing: Style.spacing.sm

  // Reader pane keyboard scrolling (called by the key registry).
  function scrollReader(px) { preview.scrollBy(px) }
  function pageReader(frac) { preview.scrollPages(frac) }
  function readerEdge(bottom) { preview.scrollEdge(bottom) }

  MessageList {
    id: messages
    Layout.preferredWidth: implicitWidth
    Layout.fillHeight: true
    model: root.app.messageModel
    folderName: root.app.selectedFolderName
    status: root.app.messagesStatus
    currentIndex: root.app.msgIndex
    paneFocused: root.app.focusPane === "msg"
    marks: root.app.marks
    frameTitle: root.app.selectedFolderLabel
    hotkey: root.app.showFolderPane ? "\u00b3" : "\u00b2"
    onRowClicked: function(i) { root.app.clickMessage(i) }
    onEndReached: root.app.loadMoreMessages()
    onRefreshRequested: { root.app.loadMessages(); root.app.focusRequested() }
  }

  // Compose takes the reading pane's slot. Loaded on demand: the editor needs the native
  // OmarchyLook.Compose plugin, and a Loader keeps a missing plugin from breaking the list.
  Loader {
    id: composeLoader
    Layout.fillWidth: true
    Layout.fillHeight: true
    active: root.app.composing
    visible: active
    PaneFrame { tint: Hues.yellow; focused: true; title: "compose"; visible: composeLoader.active }
    // The message to edit (a reply's recipients and quote, or one taken back with Undo) must be
    // in place when the pane is created, because it fills its fields as it completes. setSource
    // hands the values over at creation; assigning them in onLoaded would be too late.
    Connections {
      target: root.app
      function onComposingChanged() {
        if (root.app.composing)
          composeLoader.setSource("../compose/ComposeView.qml",
            { initial: root.app.composeInitial, defaultMode: root.app.composeDefaultMode })
      }
    }
    onLoaded: {
      item.accounts = Qt.binding(function() { return root.app.accounts })
      item.contacts = Qt.binding(function() { return root.app.composeContacts })
      item.status = Qt.binding(function() { return root.app.composeStatus })
      item.forceActiveFocus()
    }
    Connections {
      target: composeLoader.item
      ignoreUnknownSignals: true
      function onSendRequested(message) { root.app.submitCompose(message) }
      function onDiscardRequested() { root.app.closeCompose() }
    }
    onStatusChanged: if (status === Loader.Error) {
      root.app.composeStatus = "Could not load the editor (is the OmarchyLook.Compose plugin on QML_IMPORT_PATH?)"
      root.app.composing = false
    }
  }

  MessagePreview {
    id: preview
    visible: !root.app.composing
    paneFocused: root.app.focusPane === "reader"
    hotkey: root.app.showFolderPane ? "\u2074" : "\u00b3"
    Layout.fillWidth: true
    Layout.fillHeight: true
    onReplyRequested: root.app.openCompose("reply")
    onReplyAllRequested: root.app.openCompose("replyAll")
    onForwardRequested: root.app.openCompose("forward")
    message: root.app.currentMessage
    invite: root.app.invite
    onToggleReadRequested: root.app.toggleRead()
    onToggleHtmlRequested: root.app.toggleHtml()
    htmlMode: root.app.currentHtmlMode
    htmlOffer: root.app.currentHtmlOffer
    onViewAsHtmlRequested: function(scope) { root.app.viewAsHtml(scope); root.app.focusRequested() }
    imagesAllowed: root.app.currentImagesAllowed
    onUnblockRequested: function(scope) { root.app.unblockImages(scope); root.app.focusRequested() }
    onFocusRequested: root.app.focusRequested()
    body: root.app.currentBody
  }
}
