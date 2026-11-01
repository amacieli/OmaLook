import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import OmaLook.Compose
import "../common"
import "modes.js" as Modes
import "attach.js" as Att
import "../mail/format.js" as Fmt

// Compose pane. Takes the reading pane's slot in MailView (PLAN-compose.md decision Q3).
//
// Pure view: it owns the fields and the editor, and reports intent through signals; the
// owner (AppState) does the sending. Loaded through a Loader so a missing native plugin
// cannot take the mail view down with it.
//
//   initial: { kind: "new"|"reply"|"replyAll"|"forward", accountId, to, cc, bcc, subject,
//              quote: { from, date, text } | null }
FocusScope {
  id: root

  property var initial: ({ kind: "new", accountId: "", to: "", cc: "", bcc: "", subject: "", quote: null })
  property var accounts: []
  property var contacts: []
  property string defaultMode: "system"
  // Result line from the owner: "" | "sending…" | an error.
  property string status: ""
  // Messages from this editor itself (bad address, no recipient...). Kept apart from `status` because the
  // owner binds `status` to its send result, and assigning to a bound property in QML silently removes
  // the binding: after the first send attempt no backend error (e.g. "account is not signed in") ever showed.
  property string localStatus: ""
  readonly property string shownStatus: localStatus !== "" ? localStatus : status
  onStatusChanged: if (status !== "") localStatus = ""
  // Attachments: [{ path, name, size }]. `listDir(path, hidden, cb)` and `browseDir` come from the
  // owner (the daemon lists folders for the file browser; the last folder used is remembered).
  property var attachments: []
  property var listDir: null
  property string browseDir: ""

  signal browsed(string dir)
  signal sendRequested(var message)
  signal discardRequested()
  signal focusRequested()

  property string mode: defaultMode
  property string accountId: initial.accountId || ""
  // "" | "discard" | "nosubject" | "flatten"
  property string confirming: ""
  property bool linkOpen: false

  // For tests and the owner.
  readonly property alias editorItem: editor
  readonly property alias formatter: handler
  readonly property alias headerItem: header

  readonly property bool editing: true      // the shell's letter keys must never fire while composing
  readonly property string kindLabel: ({ "new": "New message", reply: "Reply", replyAll: "Reply all", forward: "Forward" })[initial.kind] || "New message"

  // What the editor holds before the user types (the quoted original), to tell "edited" from "untouched".
  property string _seedText: ""
  readonly property bool dirty: header.to.recipients.length > 0 || header.cc.recipients.length > 0
    || header.bcc.recipients.length > 0 || header.subject.text !== (initial.subject || "")
    || attachments.length > 0 || editor.text.length > 0 && handler.plainText().trim() !== _seedText.trim()

  function _quoteHtml(q) {
    if (!q) return ""
    var lines = Fmt.escapeHtml(q.text || "").split("\n").join("<br>")
    return "<p></p><p>On " + Fmt.escapeHtml(q.date || "") + ", " + Fmt.escapeHtml(q.from || "") + " wrote:</p><blockquote>" + lines + "</blockquote>"
  }

  function _recipientsOf(f) { return f.recipients.map(function(r) { return { email: r.email, name: r.name } }) }

  function _body() {
    // Plain text only when nothing was formatted (decision Q1); otherwise HTML.
    if (root.mode === Modes.SYSTEM && !handler.hasFormatting) return { format: "text", content: handler.plainText() }
    return { format: "html", content: handler.html() }
  }

  readonly property var _account: {
    var a = accounts.filter(function(x) { return x.id === root.accountId })
    return a.length > 0 ? a[0] : null
  }
  // What the From account's provider accepts. Starts from the built-in figures so the meter is right
  // at once; the daemon's answer (`GET /attach/limits`) replaces it, and a From switch asks again.
  property var fetchLimits: null              // function(accountId, cb(limits)), from the owner
  property var limits: Att.defaultLimits("")
  function _loadLimits() {
    var id = root.accountId
    root.limits = Att.defaultLimits(root._account ? root._account.provider : "")
    if (root.fetchLimits && id !== "") root.fetchLimits(id, function(l) { if (l && l.hard && root.accountId === id) root.limits = l })
  }
  onAccountIdChanged: _loadLimits()
  onAccountsChanged: _loadLimits()
  onFetchLimitsChanged: _loadLimits()

  // Forward only: the files attached to the original, ticked by default and removable like any chip.
  property var fetchOriginals: null           // function(messageId, cb({originals}|{error})), from the owner
  property var originals: []                  // kept: [{ id, name, size }]
  property string originalsState: "idle"      // idle | loading | ready | error
  function _loadOriginals() {
    if (initial.kind !== "forward" || !initial.inReplyTo || !root.fetchOriginals) return
    root.originalsState = "loading"
    root.fetchOriginals(initial.inReplyTo, function(r) {
      if (r && r.originals) {
        var keep = initial.forwardedKeep
        root.originals = keep ? r.originals.filter(function(o) { return keep.indexOf(o.id) >= 0 }) : r.originals
        root.originalsState = "ready"
      } else {
        root.originals = []
        root.originalsState = "error"
        root.localStatus = "Could not read the original's attachments; they will not be forwarded."
      }
    })
  }
  onFetchOriginalsChanged: if (originalsState === "idle") _loadOriginals()

  // Everything the strip and the list show: the forward's originals first, then picked files.
  readonly property var rows: Att.combine(originals, attachments)
  readonly property real attachedBytes: Att.rawTotal(rows)

  function openAttach() {
    root.localStatus = ""
    browser.open()
  }

  function openList() {
    if (root.rows.length === 0) { root.localStatus = "Nothing is attached."; return }
    root.localStatus = ""
    attachList.open()
  }

  // Files picked in the browser. Adding past the limit is allowed: the strip turns red and says
  // why, and Send refuses with the numbers (a From switch can push a fine set over the limit too).
  function addAttachments(files) {
    var have = root.attachments.map(function(a) { return a.path })
    var next = root.attachments.slice()
    for (var i = 0; i < files.length; i++) {
      if (have.indexOf(files[i].path) >= 0) continue
      next.push({ path: files[i].path, name: files[i].name, size: files[i].size || 0 })
    }
    root.attachments = next
    editor.forceActiveFocus()
  }

  function removeAttachment(i) {
    var next = root.attachments.slice()
    next.splice(i, 1)
    root.attachments = next
  }

  // `row` indexes `rows`.
  function removeRow(row) {
    var r = root.rows[row]
    if (!r) return
    if (r.origin === "original") {
      var next = root.originals.slice()
      next.splice(r.ref, 1)
      root.originals = next
    } else {
      root.removeAttachment(r.ref)
    }
  }

  // The palette twins of the keys (`:attach`, `:attachments`, `:detach`, `:detach-all`).
  function runCommand(name) {
    if (name === "attach") openAttach()
    else if (name === "attachments") openList()
    else if (name === "detach") removeRow(root.rows.length - 1)
    else if (name === "detach-all") { root.attachments = []; root.originals = [] }
  }

  function buildMessage() {
    return {
      kind: initial.kind,
      account_id: root.accountId,
      in_reply_to: initial.inReplyTo || "",
      to: _recipientsOf(header.to), cc: _recipientsOf(header.cc), bcc: _recipientsOf(header.bcc),
      subject: header.subject.text,
      mode: root.mode,
      body: _body(),
      attachments: root.attachments.map(function(a) { return { path: a.path, name: a.name, size: a.size } }),
      // Which of the original's files go along (forward only); left out until the list is known.
      forwarded_keep: initial.kind === "forward" && root.originalsState !== "idle" ? root.originals.map(function(o) { return o.id }) : undefined
    }
  }

  function trySend() {
    root.localStatus = ""
    // Text still sitting in an address box counts: it becomes a chip (and is checked) first.
    header.to.commit(true); header.cc.commit(true); header.bcc.commit(true)
    var all = header.to.recipients.concat(header.cc.recipients, header.bcc.recipients)
    if (all.length === 0) { root.localStatus = "Add at least one recipient."; header.to.input.forceActiveFocus(); return }
    var bad = all.filter(function(r) { return !r.valid })
    if (bad.length > 0) { root.localStatus = "Not a valid address: " + bad[0].email; return }
    if (root.originalsState === "loading") { root.localStatus = "Still reading the original's attachments\u2026"; return }
    var m = Att.meter(root.limits, root.rows)
    if (m.level === "over") {
      var v = Att.states(root.limits, root.rows), big = v.state.indexOf("big")
      root.localStatus = "The attachments are too big for " + Att.accountName(root.limits) + " (" + Fmt.bytes(m.raw) + " of files; about " + Fmt.bytes(m.capacity) + " is the most)"
        + (big >= 0 ? ". Remove " + root.rows[big].name + " or change From." : ".")
      return
    }
    if (header.subject.text.trim() === "") { root.confirming = "nosubject"; return }
    root.sendRequested(root.buildMessage())
  }

  function requestClose() {
    if (root.dirty) root.confirming = "discard"
    else root.discardRequested()
  }

  function setMode(m) {
    if (m === root.mode) return
    if (m === Modes.SYSTEM && handler.hasRichAttributes) { root.confirming = "flatten"; return }
    if (m === Modes.SYSTEM) handler.flattenToSystem()
    root.mode = m
  }

  Component.onCompleted: {
    header.to.setFromText(initial.to || "")
    header.cc.setFromText(initial.cc || "")
    header.bcc.setFromText(initial.bcc || "")
    header.showCc = header.cc.recipients.length > 0
    header.showBcc = header.bcc.recipients.length > 0
    header.subject.text = initial.subject || ""
    if (initial.mode) root.mode = initial.mode
    if (initial.attachments) root.attachments = initial.attachments
    _loadLimits()
    _loadOriginals()
    if (initial.quote) handler.setHtml(_quoteHtml(initial.quote))
    // A message taken back (Undo) or that failed to send comes back exactly as written.
    if (initial.restore) {
      if (initial.restore.format === "text") handler.setPlainText(initial.restore.content)
      else handler.setHtml(initial.restore.content)
    }
    _seedText = initial.restore ? "" : handler.plainText()
    // Replies start in the body, new messages in To.
    if (initial.restore) { editor.forceActiveFocus(); editor.cursorPosition = editor.length }
    else if (initial.kind === "new" || initial.kind === "forward") header.to.input.forceActiveFocus()
    else { editor.forceActiveFocus(); editor.cursorPosition = 0 }
  }

  Keys.onPressed: function(e) {
    if (browser.opened || attachList.opened) return
    if (root.confirming !== "") {
      if (confirmDialog.handleKey(e)) e.accepted = true
      return
    }
    var ctrl = e.modifiers & Qt.ControlModifier
    var shift = e.modifiers & Qt.ShiftModifier
    if (e.key === Qt.Key_Escape) {
      if (root.linkOpen) root.linkOpen = false
      else root.requestClose()
      e.accepted = true
    } else if (ctrl && (e.key === Qt.Key_Return || e.key === Qt.Key_Enter)) { root.trySend(); e.accepted = true }
    else if (ctrl && e.key === Qt.Key_B) { handler.toggleBold(); e.accepted = true }
    else if (ctrl && e.key === Qt.Key_I) { handler.toggleItalic(); e.accepted = true }
    else if (ctrl && e.key === Qt.Key_U) { handler.toggleUnderline(); e.accepted = true }
    else if (ctrl && shift && e.key === Qt.Key_A) { root.openAttach(); e.accepted = true }
    else if (ctrl && shift && e.key === Qt.Key_L) { root.openList(); e.accepted = true }
    else if (ctrl && shift && e.key === Qt.Key_Backspace && root.rows.length > 0) { root.removeRow(root.rows.length - 1); e.accepted = true }
    else if (ctrl && e.key === Qt.Key_K) { root.linkOpen = true; linkUrl.forceActiveFocus(); e.accepted = true }
    else if (ctrl && shift && (e.key === Qt.Key_7 || e.key === Qt.Key_Ampersand)) { handler.toggleNumberedList(); e.accepted = true }
    else if (ctrl && shift && (e.key === Qt.Key_8 || e.key === Qt.Key_Asterisk)) { handler.toggleBulletList(); e.accepted = true }
  }

  ColumnLayout {
    anchors.fill: parent
    spacing: 0

    // ---- title row
    RowLayout {
      Layout.fillWidth: true
      Layout.margins: Style.spacing.lg
      spacing: Style.spacing.md

      UiText {
        text: root.kindLabel
        font.pixelSize: Style.font.heading
        font.bold: true
      }

      Item { Layout.fillWidth: true }

      UiText {
        text: root.shownStatus
        visible: text !== ""
        foreground: root.shownStatus.indexOf("…") >= 0 ? Commons.Color.foreground : Commons.Color.urgent
        dim: root.shownStatus.indexOf("…") >= 0
        font.pixelSize: Style.font.caption
        elide: Text.ElideRight
        Layout.maximumWidth: Style.space(360)
      }

      // One button: it names the mode you would switch to, like the reading pane's toggle.
      Button {
        bordered: true
        text: root.mode === Modes.HTML ? "System" : "HTML"
        iconText: root.mode === Modes.HTML ? "\uf031" : "\uf121"
        tooltipText: root.mode === Modes.HTML
          ? "Switch to system font: sent in the recipient's own font"
          : "Switch to HTML: fonts, sizes, colours, alignment"
        onClicked: root.setMode(root.mode === Modes.HTML ? Modes.SYSTEM : Modes.HTML)
      }

      Item { Layout.preferredWidth: Style.spacing.lg }

      Button {
        bordered: true
        text: root.rows.length > 0 ? "Attach (" + root.rows.length + ")" : "Attach"
        iconText: "\uf0c6"
        tooltipText: "Ctrl+Shift+A"
        onClicked: root.openAttach()
      }

      Button {
        bordered: true
        text: "Discard"
        iconText: "\uf1f8"
        tooltipText: "Esc"
        onClicked: root.requestClose()
      }
      Button {
        bordered: true
        selected: true
        text: "Send"
        iconText: "\uf1d8"
        tooltipText: "Ctrl+Enter"
        onClicked: root.trySend()
      }
    }

    PanelSeparator { Layout.fillWidth: true }

    // ---- header fields
    ComposeHeader {
      id: header
      z: 10      // the address suggestion list overhangs the format bar and editor below
      Layout.fillWidth: true
      Layout.margins: Style.spacing.lg
      accounts: root.accounts
      accountId: root.accountId
      nextFocus: editor
      contacts: root.contacts
      onAccountPicked: function(id) { root.accountId = id }
    }

    PanelSeparator { Layout.fillWidth: true }

    // ---- format bar
    FormatBar {
      Layout.fillWidth: true
      Layout.margins: Style.spacing.md
      fmt: handler
      mode: root.mode
      canUndo: editor.canUndo
      canRedo: editor.canRedo
      onUndoRequested: editor.undo()
      onRedoRequested: editor.redo()
      onLinkRequested: { root.linkOpen = true; linkUrl.forceActiveFocus() }
    }

    // ---- link bar
    RowLayout {
      visible: root.linkOpen
      Layout.fillWidth: true
      Layout.leftMargin: Style.spacing.lg
      Layout.rightMargin: Style.spacing.lg
      Layout.bottomMargin: Style.spacing.md
      spacing: Style.spacing.md
      UiText { text: "Link"; dim: true; font.pixelSize: Style.font.caption }
      TextField {
        id: linkUrl
        Layout.fillWidth: true
        placeholderText: "https://…"
        onAccepted: linkApply.clicked()
      }
      Button {
        id: linkApply
        bordered: true
        text: "Apply"
        onClicked: {
          var u = linkUrl.text.trim()
          if (u !== "" && !/^[a-z]+:/i.test(u)) u = "https://" + u
          if (u !== "") handler.insertLink(u, "")
          linkUrl.text = ""; root.linkOpen = false; editor.forceActiveFocus()
        }
      }
      Button { bordered: true; text: "Cancel"; onClicked: { root.linkOpen = false; editor.forceActiveFocus() } }
    }

    // ---- attachments: one line of constant height, absent when nothing is attached
    AttachStrip {
      id: attachStrip
      Layout.fillWidth: true
      items: root.rows
      limits: root.limits
      onRemoveRequested: function(row) { root.removeRow(row) }
      onListRequested: root.openList()
    }

    PanelSeparator { Layout.fillWidth: true }

    // ---- editor. System mode sits on the theme; HTML mode on a white page, the way the
    // reading pane shows HTML mail.
    ScrollView {
      id: scroll
      Layout.fillWidth: true
      Layout.fillHeight: true
      clip: true
      implicitWidth: 0
      implicitHeight: 0
      ScrollBar.horizontal.policy: ScrollBar.AlwaysOff
      ScrollBar.vertical: ThemedScrollBar {
        parent: scroll
        tint: root.mode === Modes.HTML ? "#000000" : Commons.Color.foreground
        x: scroll.width - width
        y: 0
        height: scroll.height
      }
      contentWidth: availableWidth

      Rectangle {
        width: scroll.availableWidth
        height: Math.max(scroll.height, editor.implicitHeight + Style.spacing.huge * 2)
        color: root.mode === Modes.HTML ? "#ffffff" : "transparent"

        MouseArea { anchors.fill: parent; onClicked: { editor.forceActiveFocus(); editor.cursorPosition = editor.length } }

        TextEdit {
          id: editor
          x: Style.spacing.huge
          y: Style.spacing.huge
          width: parent.width - Style.spacing.huge * 2
          textFormat: TextEdit.RichText
          wrapMode: TextEdit.Wrap
          selectByMouse: true
          persistentSelection: true
          color: root.mode === Modes.HTML ? "#000000" : Commons.Color.foreground
          selectionColor: root.mode === Modes.HTML ? "#b3d4fc" : Style.selectionFill
          selectedTextColor: root.mode === Modes.HTML ? "#000000" : Commons.Color.foreground
          font.family: root.mode === Modes.HTML ? "Calibri" : Style.font.family
          font.pixelSize: root.mode === Modes.HTML ? 15 : Style.font.body
          onLinkActivated: function(l) { Qt.openUrlExternally(l) }
          KeyNavigation.backtab: header.subject

          DocumentHandler {
            id: handler
            document: editor.textDocument
            cursorPosition: editor.cursorPosition
            selectionStart: editor.selectionStart
            selectionEnd: editor.selectionEnd
          }
        }
      }
    }

    // ---- footer hint
    PanelSeparator { Layout.fillWidth: true }
    RowLayout {
      Layout.fillWidth: true
      Layout.margins: Style.spacing.md
      UiText {
        Layout.fillWidth: true
        dim: true
        font.pixelSize: Style.font.caption
        elide: Text.ElideRight
        text: root.mode === Modes.SYSTEM
          ? "System font — sent without fonts, sizes or colours; plain text if nothing is formatted"
          : "HTML — sent with the fonts, sizes and colours you chose"
      }
      UiText {
        dim: true
        font.pixelSize: Style.font.caption
        text: "Ctrl+Enter send · Esc discard · Ctrl+K link · Ctrl+Shift+A attach"
      }
    }
  }

  AttachList {
    id: attachList
    x: Style.spacing.lg
    width: root.width - Style.spacing.lg * 2
    y: attachStrip.y + attachStrip.height
    z: 800
    items: root.rows
    limits: root.limits
    onRemoveRequested: function(row) { root.removeRow(row) }
    onAddRequested: { attachList.close(); root.openAttach() }
    onClosed: editor.forceActiveFocus()
  }

  AttachBrowser {
    id: browser
    anchors.fill: parent
    z: 900
    listDir: root.listDir
    startDir: root.browseDir
    attached: root.attachments.map(function(a) { return a.path })
    onPicked: function(files) { root.addAttachments(files) }
    onCanceled: editor.forceActiveFocus()
    onNavigated: function(d) { root.browsed(d) }
  }

  // While a confirm dialog is up, keys go to it, not to the editor/header
  // field that had focus (those consume Enter/Left/Right/Tab themselves).
  property Item _focusBeforeConfirm: null
  onConfirmingChanged: {
    if (confirming !== "") {
      _focusBeforeConfirm = Window.activeFocusItem
      confirmDialog.selectedIndex = 1
      dialogKeys.forceActiveFocus()
    } else {
      if (_focusBeforeConfirm) _focusBeforeConfirm.forceActiveFocus()
      _focusBeforeConfirm = null
    }
  }
  Item {
    id: dialogKeys
    Keys.onPressed: function(e) { if (confirmDialog.handleKey(e)) e.accepted = true }
  }

  ConfirmDialog {
    id: confirmDialog
    anchors.fill: parent
    z: 1000
    opened: root.confirming !== ""
    message: root.confirming === "discard" ? "Discard this message?"
      : root.confirming === "nosubject" ? "Send without a subject?"
      : "Switching to system font removes fonts, sizes, colours and alignment. Continue?"
    cancelText: root.confirming === "discard" ? "Keep editing" : "Cancel"
    confirmText: root.confirming === "discard" ? "Discard" : root.confirming === "nosubject" ? "Send" : "Switch"
    onCanceled: root.confirming = ""
    onConfirmed: {
      var what = root.confirming
      root.confirming = ""
      if (what === "discard") root.discardRequested()
      else if (what === "nosubject") root.sendRequested(root.buildMessage())
      else if (what === "flatten") { handler.flattenToSystem(); root.mode = Modes.SYSTEM }
    }
  }
}
