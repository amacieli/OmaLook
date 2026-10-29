import QtQuick
import QtQuick.Controls
import Quickshell
import Quickshell.Io
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"
import "format.js" as Fmt

// Reading pane. `message` is the model row (or null for the empty state).
Item {
  id: root

  PaneFrame { tint: Hues.green; focused: root.paneFocused; hotkey: root.hotkey; title: "message" }

  property var message: null
  property bool paneFocused: false
  // Attachment list of this message from AppState.attachments ({ id, state, items, dir, ... }).
  property var attachments: ({ id: "", state: "idle", items: [], dir: "" })
  signal attachmentsRequested()
  readonly property var realAttachments: (!!root.message && root.attachments.id === String(root.message.id || "") && root.attachments.state === "ready")
    ? root.attachments.items.filter(function(i) { return !i.inline }) : null
  // One line however many there are: the name when there is one, else the count. Hidden when the
  // list said "attachments" but they all turned out to be pictures inside the message.
  readonly property bool showAttach: !!root.message && root.message.has_attachments === true
    && (root.realAttachments === null || root.realAttachments.length > 0)
  readonly property bool showImportance: !!root.message && root.message.importance === "high"
  readonly property string attachText: {
    var l = root.realAttachments
    if (l === null) return "\uf0c6  attachments  \u00b7  A"
    if (l.length === 1) return "\uf0c6  " + l[0].name + "  (" + Fmt.bytes(l[0].size) + ")  \u00b7  A to save"
    return "\uf0c6  " + l.length + " attachments  \u00b7  A to choose, S to save all"
  }
  // Meeting invitation state from AppState.invite (see AppState: type, response, when, ...).
  property var invite: ({ id: "", state: "idle", type: "none", actionable: false, response: "", error: "" })
  readonly property bool inviteForThis: !!root.message && root.invite.id === String(root.message.id || "")
  // Shown for every calendar message (request, cancellation, someone's answer), not just answerable ones.
  readonly property bool inviteShown: root.inviteForThis && root.invite.type !== "none" && root.invite.type !== ""
  readonly property string inviteKeys: "y accept · m tentative · n decline"
  readonly property color inviteHue: {
    var i = root.invite
    if (i.cancelled || i.response === "declined" || i.type === "meetingDeclined") return Hues.red
    if (i.response === "accepted" || i.type === "meetingAccepted") return Hues.green
    if (i.response === "tentativelyAccepted" || i.type === "meetingTentativelyAccepted") return Hues.yellow
    return Hues.cyan
  }
  // Two lines: what it is (and when/where), then what you can do or have done.
  readonly property string inviteTitle: {
    var i = root.invite
    var what = i.cancelled ? "Meeting cancelled"
      : i.type === "meetingRequest" ? "Meeting invitation"
      : i.type === "meetingAccepted" ? "Meeting update: " + (i.organizer || "attendee") + " accepted"
      : i.type === "meetingTentativelyAccepted" ? "Meeting update: attendee tentatively accepted"
      : i.type === "meetingDeclined" ? "Meeting update: attendee declined"
      : "Calendar message"
    var extra = []
    if (i.when) extra.push(i.when)
    if (i.location) extra.push(i.location)
    if (i.type === "meetingRequest" && i.organizer && !i.isOrganizer) extra.push("from " + i.organizer)
    return "\u25A6 " + what + (extra.length ? "   " + extra.join(" · ") : "")
  }
  readonly property string inviteStatus: {
    var i = root.invite
    if (i.state === "sending") return "sending response…"
    if (i.state === "loading") return "checking invitation…"
    if (i.state === "error" && i.error !== "") return i.error
    if (i.cancelled) return i.hasEvent ? "No response needed" : "Removed from your calendar"
    if (i.type !== "meetingRequest") return "Informational, no response needed"
    if (i.isOrganizer) return "You are the organizer"
    if (!i.hasEvent) return "This meeting is no longer on your calendar"
    var now = i.response === "accepted" ? "You accepted"
      : i.response === "tentativelyAccepted" ? "You tentatively accepted"
      : i.response === "declined" ? "You declined" : "Not answered yet"
    return now + "   " + root.inviteKeys
  }
  property string hotkey: "\u00b3"

  // Keyboard scrolling of the body (the reader pane). Whichever view is showing.
  function _flick() { return htmlScroll.visible ? htmlScroll.contentItem : scroll.contentItem }
  function _clamp(f, y) { return Math.max(0, Math.min(Math.max(0, f.contentHeight - f.height), y)) }
  function scrollBy(px) { var f = _flick(); f.contentY = _clamp(f, f.contentY + px) }
  function scrollPages(frac) { var f = _flick(); f.contentY = _clamp(f, f.contentY + frac * f.height * 0.9) }
  function scrollEdge(bottom) { var f = _flick(); f.contentY = bottom ? _clamp(f, 1e9) : 0 }

  signal replyRequested()
  signal replyAllRequested()
  signal forwardRequested()
  signal toggleReadRequested()
  signal toggleHtmlRequested()
  signal unblockRequested(string scope)   // "message" | "sender"
  signal focusRequested()                 // give the keyboard back to the shell

  // Body of `message`: { state: idle|loading|ready|error, type: html|text, content }.
  property var body: ({ state: "idle", type: "", content: "" })
  property bool htmlMode: false
  property string renderLabel: ""     // e.g. "HTML \u00b7 sender": the view and the level that decides it
  property bool renderLocked: false   // a reply to this message is inside its undo window
  // True when this message (or its sender) may load remote images.
  property bool imagesAllowed: false
  // Offer "view as HTML" above the system-font body.
  property bool htmlOffer: false
  // The image dropdown owns the keyboard while its popup is open.
  readonly property bool editing: imageChoice.popupOpen

  // A different message starts at the top of both views.
  readonly property string bodyId: root.body ? String(root.body.id || "") : ""
  onBodyIdChanged: {
    imageChoice.value = "message"
    scroll.ScrollBar.vertical.position = 0
    htmlScroll.ScrollBar.vertical.position = 0
  }

  readonly property bool bodyReady: !!root.body && root.body.state === "ready"
  // Both views are produced off the GUI thread (RenderWorker.js) and only after the selection has
  // rested for a moment, so holding a key down in the list never waits on a message body.
  // Qt's rich-text layout of the finished HTML is the one step that must stay on the GUI thread.
  readonly property string statusText: !root.body ? ""
    : (root.body.state === "loading" ? "loading message…"
      : (root.body.state === "error" ? String(root.body.reason || "could not load the message body") : ""))
  // What is shown under a load error: the cached preview, if there is one.
  readonly property string fallbackText: !!root.body && root.body.state === "error" ? String(root.body.preview || "") : ""

  property bool renderPending: false
  property string plainText: ""
  // Colours of the current link (inverse video) and of the hint labels, for the system view.
  readonly property var hl: ({ link: String(Hues.cyan), curBg: String(Commons.Color.accent), curFg: String(Commons.Color.background),
                               hintBg: String(Hues.yellow), hintFg: String(Commons.Color.background) })
  property string plainHtml: ""          // linkedHtml() template of the system view
  property var sysLinks: []              // [{href, label}] in the system view
  property var sysOffsets: []            // their character positions, to scroll to them
  property var rendered: ({ html: "", blocked: 0 })
  property int _seq: 0
  readonly property real _htmlWidth: Math.max(0, htmlScroll.width - Style.spacing.huge * 2)

  // ---- keyboard links -------------------------------------------------------------------
  // Tab / ] and Shift-Tab / [ step through the links of whichever view shows, Enter / o opens the
  // current one, y copies it, f labels them all (type the label to open). The key commands live in
  // keys/AppCommands.qml; the shell routes hint-label keys to hintInput().
  readonly property var navLinks: htmlMode ? (shotShown ? shotLinks : []) : sysLinks
  readonly property int linkCount: navLinks.length
  property int linkIndex: -1
  property bool hintMode: false
  property string hintTyped: ""
  readonly property var hintList: hintMode ? Fmt.hintLabels(linkCount) : []
  readonly property string currentHref: (linkIndex >= 0 && linkIndex < linkCount) ? String(navLinks[linkIndex].href) : ""
  // The highlighted link only shows while the reading pane has the keyboard.
  readonly property int shownLink: paneFocused ? linkIndex : -1
  signal notice(string text)

  // Where link i sits in the scrolled content: { y, h }.
  function _linkRect(i) {
    if (htmlMode) {
      var l = shotLinks[i], k = root.shotScale * shot.k
      return { y: l.y * k, h: Math.max(1, l.h * k) }
    }
    var ys = _sysRects()
    return ys[i] || { y: -1, h: 1 }
  }
  // Where each system-view link sits, found by probing the laid-out text (Text cannot report the
  // position of a character). Done once per layout, only when a link is first stepped to.
  property var _sysY: null
  function _sysRects() {
    if (_sysY !== null) return _sysY
    var ys = [], found = 0
    var step = Math.max(4, Math.round(sysText.font.pixelSize * 0.6))
    var x0 = sysText.leftPadding, x1 = sysText.width - sysText.rightPadding
    for (var y = sysText.topPadding; y < sysText.height && found < linkCount; y += step) {
      for (var x = x0; x < x1; x += 5) {
        var l = sysText.linkAt(x, y)
        if (l.indexOf("omalook-link:") !== 0) continue
        var n = Number(l.slice(13))
        if (ys[n] === undefined) { ys[n] = { y: y - step / 2, h: step * 2 }; found++ }
      }
    }
    _sysY = ys
    return ys
  }
  function _showLink(i) {
    var f = _flick(), r = _linkRect(i)
    if (r.y < 0) return
    var m = Math.min(40, f.height / 4)
    if (r.y < f.contentY + m || r.y + r.h > f.contentY + f.height - m) f.contentY = _clamp(f, r.y - f.height * 0.3)
  }
  // The first link on screen (going forward) or the last one (going back), for the first Tab.
  function _edgeLink(dir) {
    var f = _flick(), top = f.contentY, bot = top + f.height
    if (dir > 0) { for (var i = 0; i < linkCount; i++) if (_linkRect(i).y >= top - 1) return i; return linkCount - 1 }
    for (var j = linkCount - 1; j >= 0; j--) { var r = _linkRect(j); if (r.y + r.h <= bot + 1) return j }
    return 0
  }
  function linkStep(dir) {
    if (linkCount === 0) return
    linkIndex = (linkIndex < 0 || linkIndex >= linkCount) ? _edgeLink(dir) : (linkIndex + dir + linkCount) % linkCount
    _showLink(linkIndex)
  }
  function linkClear() { linkIndex = -1; hintMode = false; hintTyped = "" }
  function linkOpen() { if (currentHref !== "") Qt.openUrlExternally(currentHref) }
  function linkCopy() {
    if (currentHref === "") return
    Quickshell.execDetached(["wl-copy", currentHref])
    root.notice("copied " + currentHref)
  }
  function linkHints() { if (linkCount > 0) { hintTyped = ""; hintMode = true } }
  // Keys while the hint labels show. Returns true when consumed (all of them: the mode is modal).
  function hintInput(key, text) {
    if (key === Qt.Key_Escape) { linkClear(); return true }
    if (key === Qt.Key_Backspace) { hintTyped = hintTyped.slice(0, -1); return true }
    var c = String(text || "").toLowerCase()
    if (c.length !== 1 || "asdfghjkl".indexOf(c) < 0) return true
    var typed = hintTyped + c
    var matches = hintList.filter(function(h) { return h.indexOf(typed) === 0 })
    if (matches.length === 0) return true
    hintTyped = typed
    var hit = hintList.indexOf(typed)
    if (hit >= 0 && matches.length === 1) {
      hintMode = false; hintTyped = ""
      linkIndex = hit
      _showLink(hit)
      linkOpen()
    }
    return true
  }
  onPaneFocusedChanged: if (!paneFocused) { hintMode = false; hintTyped = "" }

  // ---- WebKit view -------------------------------------------------------------------------
  // HTML mail is drawn by omalook-render (WebKitGTK, its own process, see tools/render): full
  // CSS, rounded cards, remote images when allowed. It returns a PNG plus the link boxes. If the
  // helper is missing or fails, the message falls back to Qt's rich-text renderer below.
  readonly property string helperPath: Quickshell.env("QML_DIR") + "/../bin/omalook-render"
  property bool webkitOk: false          // the helper exists
  property int webkitFailures: 0
  readonly property bool webkitBroken: webkitFailures >= 3   // stop trying this session
  property bool shotFailed: false        // this message fell back to Qt
  property bool shotPending: false
  property string shotSource: ""
  property var shotLinks: []
  property real shotScale: 1
  property real shotPixelWidth: 0
  property real shotPixelHeight: 0
  property color shotBg: "#ffffff"
  property var _shotProc: null
  readonly property bool shotShown: shotSource !== "" && !shotFailed
  readonly property bool htmlBusy: renderPending || shotPending

  FileView {
    path: root.helperPath
    printErrors: false
    onLoaded: root.webkitOk = true
    onLoadFailed: root.webkitOk = false
  }

  function _wantShot() {
    return htmlMode && webkitOk && !webkitBroken && !shotFailed
      && !!root.body && root.body.state === "ready" && root.body.type === "html"
  }

  function _stopShot() {
    if (_shotProc) { var p = _shotProc; _shotProc = null; p.running = false; p.destroy(500) }
  }

  Component {
    id: shotProcess
    Process {
      id: proc
      property int seq: 0
      property string payload: ""
      property string outFile: ""
      property bool gotOutput: false
      stdinEnabled: true
      onStarted: { write(payload); stdinEnabled = false }
      stdout: StdioCollector {
        onStreamFinished: {
          if (proc.seq !== root._seq || root._shotProc !== proc) return
          var r = null
          try { r = JSON.parse(text) } catch (e) { r = null }
          if (!r || !r.width) return   // onExited handles the failure
          proc.gotOutput = true
          root.shotScale = r.scale
          root.shotPixelWidth = r.width
          root.shotPixelHeight = r.height
          root.shotLinks = r.links || []
          root.shotBg = r.bg || "#ffffff"
          root.shotSource = "file://" + proc.outFile
          root.shotPending = false
        }
      }
      onExited: function(code, status) {
        proc.destroy(500)
        if (proc.seq !== root._seq || root._shotProc !== proc) return
        if (code === 0 && proc.gotOutput) return
        // Failed (no display, timeout, crash): show this message with Qt's renderer instead.
        root.webkitFailures++
        root.shotFailed = true
        root.shotPending = false
        root._shotProc = null
        worker.sendMessage({ seq: root._seq, type: String(root.body.type || "text"), content: String(root.body.content || ""),
                             wantHtml: true, maxWidth: root._htmlWidth, allowRemote: root.imagesAllowed })
      }
    }
  }

  function _startShot() {
    _stopShot()
    var run = Quickshell.env("XDG_RUNTIME_DIR") || "/tmp"
    var out = run + "/omalook-render-" + (_seq % 6) + ".png"
    var args = [helperPath, "--width", String(Math.max(300, Math.floor(htmlScroll.availableWidth))),
                "--scale", String(Math.max(1, Screen.devicePixelRatio)), "--out", out]
    if (imagesAllowed) args.splice(1, 0, "--images")
    var p = shotProcess.createObject(root, { seq: _seq, payload: String(root.body.content || ""), outFile: out, command: args })
    _shotProc = p
    p.running = true
  }

  // Anything that changes what is shown drops the old result at once and starts the settle timer;
  // results of superseded requests are discarded when they arrive.
  function _invalidate() {
    _seq++
    _stopShot()
    plainText = ""
    plainHtml = ""
    sysLinks = []
    sysOffsets = []
    _sysY = null
    linkClear()
    rendered = { html: "", blocked: 0 }
    shotSource = ""
    shotLinks = []
    shotFailed = false
    // Read the body itself: a change handler can run before `bodyReady` has re-evaluated.
    var ready = !!root.body && root.body.state === "ready"
    renderPending = ready
    shotPending = _wantShot()
    if (ready) settle.restart(); else settle.stop()
  }
  onBodyChanged: _invalidate()
  onHtmlModeChanged: _invalidate()
  onImagesAllowedChanged: _invalidate()
  onWebkitOkChanged: if (htmlMode && !!root.body && root.body.state === "ready") _invalidate()
  on_HtmlWidthChanged: if (htmlMode && !!root.body && root.body.state === "ready") _invalidate()

  Timer {
    id: settle
    interval: 150
    onTriggered: {
      // The worker always runs: plain text for the system view, the Qt HTML (or at least the
      // blocked-image count) for the HTML view.
      worker.sendMessage({
        seq: root._seq, type: String(root.body.type || "text"), content: String(root.body.content || ""),
        wantHtml: root.htmlMode, maxWidth: root._htmlWidth, allowRemote: root.imagesAllowed
      })
      if (root._wantShot()) root._startShot()
    }
  }
  WorkerScript {
    id: worker
    source: "RenderWorker.js"
    onMessage: function(r) {
      if (r.seq !== root._seq) return   // scrolled on: this message is no longer the one shown
      root.plainText = r.plain
      root.plainHtml = r.plainHtml
      root.sysLinks = r.plainLinks || []
      root.sysOffsets = r.plainOffsets || []
      // With the WebKit view showing (or about to), Qt never lays the HTML out: only the count of
      // blocked images is kept.
      var qtHtml = (root.shotFailed || !root._wantShotIgnoringFailure()) ? r.html : ""
      root.rendered = { html: qtHtml, blocked: r.blocked }
      root.renderPending = false
    }
  }
  function _wantShotIgnoringFailure() {
    return htmlMode && webkitOk && !webkitBroken && !!root.body && root.body.type === "html"
  }

  // ---- empty state
  ColumnLayout {
    anchors.centerIn: parent
    visible: !root.message
    spacing: Style.spacing.md

    UiText {
      Layout.alignment: Qt.AlignHCenter
      text: "\uf0e0"
      dim: true
      opacity: 0.5
      font.pixelSize: Style.font.displayLarge
    }
    UiText {
      Layout.alignment: Qt.AlignHCenter
      text: "select a message"
      dim: true
      opacity: 0.5
      font.pixelSize: Style.font.bodySmall
    }
  }

  // ---- message
  ColumnLayout {
    anchors.fill: parent
    anchors.margins: Style.spacing.sm
    anchors.topMargin: Style.spacing.xl
    visible: !!root.message
    spacing: 0

    // header
    ColumnLayout {
      Layout.fillWidth: true
      Layout.margins: Style.spacing.huge
      spacing: Style.spacing.md

      UiText {
        text: (root.message && root.message.subject) || "(no subject)"
        Layout.fillWidth: true
        foreground: Hues.brightForeground
        wrapMode: Text.WordWrap
        font.pixelSize: Style.font.heading
        font.bold: true
      }

      BorderSurface {
        visible: root.inviteShown
        Layout.fillWidth: true
        implicitHeight: inviteCol.implicitHeight + Style.spacing.md * 2
        radius: Style.cornerRadius
        color: Style.selectedFillFor(root.inviteHue, root.inviteHue)
        borderSpec: Border.flat(root.inviteHue, Style.normalBorderWidth)

        ColumnLayout {
          id: inviteCol
          anchors.fill: parent
          anchors.margins: Style.spacing.md
          spacing: 2
          UiText {
            text: root.inviteTitle
            Layout.fillWidth: true
            foreground: root.inviteHue
            wrapMode: Text.WordWrap
            font.pixelSize: Style.font.bodySmall
            font.bold: true
          }
          UiText {
            text: root.inviteStatus
            Layout.fillWidth: true
            foreground: Hues.brightForeground
            wrapMode: Text.WordWrap
            font.pixelSize: Style.font.bodySmall
          }
        }
      }

      RowLayout {
        Layout.fillWidth: true
        spacing: Style.spacing.md

        BorderSurface {
          implicitWidth: chip.implicitWidth + Style.spacing.lg * 2
          implicitHeight: chip.implicitHeight + Style.spacing.sm * 2
          radius: Style.cornerRadius
          readonly property color hue: Hues.hueFor(root.message ? root.message.from_email : "")
          color: Style.selectedFillFor(hue, hue)
          borderSpec: Border.flat(hue, Style.normalBorderWidth)

          UiText {
            id: chip
            anchors.centerIn: parent
            text: Fmt.senderName(root.message)
            foreground: parent.hue
            font.pixelSize: Style.font.caption
          }
        }

        UiText {
          text: root.message && root.message.from_email ? "<" + root.message.from_email + ">" : ""
          Layout.fillWidth: true
          elide: Text.ElideRight
          foreground: Hues.cyan
          font.pixelSize: Style.font.caption
        }

        UiText {
          text: Fmt.fullDate(root.message ? root.message.received_at : "")
          foreground: Hues.muted
          font.pixelSize: Style.font.caption
        }
      }

      // To / Cc / Bcc: one line each, only the ones that exist.
      Repeater {
        model: [
          { label: "to ",  key: "to_text" },
          { label: "cc ",  key: "cc_text" },
          { label: "bcc",  key: "bcc_text" }
        ]
        UiText {
          required property var modelData
          readonly property string line: root.message ? Fmt.recipientLine(modelData.label, root.message[modelData.key]) : ""
          visible: line.length > 0
          text: line
          Layout.fillWidth: true
          elide: Text.ElideRight
          dim: true
          font.pixelSize: Style.font.caption
        }
      }

      // Importance, attachments and categories.
      Flow {
        Layout.fillWidth: true
        spacing: Style.spacing.md
        visible: root.showImportance || root.showAttach || cats.count > 0

        UiText {
          id: importanceMark
          visible: root.showImportance
          text: "\uf06a  high importance"
          foreground: Commons.Color.urgent
          font.pixelSize: Style.font.caption
        }
        UiText {
          id: attachMark
          visible: root.showAttach
          text: root.attachText
          dim: true
          font.pixelSize: Style.font.caption
          MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: root.attachmentsRequested() }
        }
        Repeater {
          id: cats
          model: root.message ? Fmt.categories(root.message) : []
          CategoryChip {
            required property var modelData
            label: modelData[0]
            tint: modelData[1]
          }
        }
      }
    }

    PanelSeparator { Layout.fillWidth: true }

    // actions
    RowLayout {
      Layout.fillWidth: true
      Layout.margins: Style.spacing.lg
      spacing: Style.spacing.md

      Button {
        text: "reply"
        iconText: "\uf112"
        bordered: true
        onClicked: root.replyRequested()
      }
      Button {
        text: "reply all"
        iconText: "\uf122"
        bordered: true
        onClicked: root.replyAllRequested()
      }
      Button {
        text: "forward"
        iconText: "\uf064"
        bordered: true
        onClicked: root.forwardRequested()
      }

      Item { Layout.fillWidth: true }

      Button {
        bordered: true
        text: root.renderLabel !== "" ? root.renderLabel : (root.htmlMode ? "System" : "HTML")
        iconText: root.htmlMode ? "\uf121" : "\uf031"
        enabled: !root.renderLocked
        onClicked: root.toggleHtmlRequested()
      }
      Button {
        bordered: true
        text: root.message && root.message.is_read ? "mark unread" : "mark read"
        iconText: root.message && root.message.is_read ? "\uf2b6" : "\uf2b7"
        onClicked: root.toggleReadRequested()
      }
    }

    PanelSeparator { Layout.fillWidth: true }

    // body: two sub-views, one shown at a time.
    StackLayout {
      Layout.fillWidth: true
      Layout.fillHeight: true
      currentIndex: root.htmlMode ? 1 : 0

      // System view — an offer to view the message as HTML, then plain text in the
      // shell font.
      ColumnLayout {
        spacing: 0

        RowLayout {
          visible: root.htmlOffer
          Layout.fillWidth: true
          Layout.margins: Style.spacing.lg
          spacing: Style.spacing.md

          UiText {
            Layout.fillWidth: true
            text: "This message has HTML formatting \u00b7 H cycles the view (message, then sender)"
            dim: true
            elide: Text.ElideRight
          }
        }

        ScrollView {
          id: scroll
          clip: true
          Layout.fillWidth: true
          Layout.fillHeight: true
          // Sized by the StackLayout, never by its content (that made a binding loop).
          implicitWidth: 0
          implicitHeight: 0
          ScrollBar.horizontal.policy: ScrollBar.AlwaysOff
          // A ScrollView does not place a bar it is handed, so pin it to the right edge.
          ScrollBar.vertical: ThemedScrollBar {
            parent: scroll
            x: scroll.width - width
            y: 0
            height: scroll.height
          }

          UiText {
            id: sysText
            readonly property bool linked: root.statusText === "" && !root.renderPending && root.plainHtml !== ""
            width: scroll.availableWidth
            leftPadding: Style.spacing.huge
            rightPadding: Style.spacing.huge
            topPadding: Style.spacing.xxxl
            bottomPadding: Style.spacing.huge * 1.3
            textFormat: linked ? Text.RichText : Text.PlainText
            linkColor: Hues.cyan
            text: root.statusText !== ""
              ? root.statusText + (root.fallbackText !== "" ? "\n\n" + root.fallbackText : "")
              : (root.renderPending ? "rendering…" : (linked ? Fmt.markLinks(root.plainHtml, root.shownLink, root.hintMode ? root.hintList : null, root.hintTyped, root.hl) : root.plainText))
            dim: root.statusText !== "" || root.renderPending
            wrapMode: Text.WordWrap
            lineHeight: 1.4
            onLinkActivated: function(link) {
              var n = link.indexOf("omalook-link:") === 0 ? Number(link.slice(13)) : -1
              if (n >= 0 && n < root.sysLinks.length) Qt.openUrlExternally(root.sysLinks[n].href)
            }
            onWidthChanged: root._sysY = null
            // Hand cursor over a link (links are underlined by the rich-text renderer).
            MouseArea {
              anchors.fill: parent
              acceptedButtons: Qt.NoButton
              cursorShape: sysText.hoveredLink !== "" ? Qt.PointingHandCursor : Qt.ArrowCursor
            }
          }
        }
      }

      // HTML view — a bar for blocked images, then the message on a white page, as
      // Outlook's reading pane shows it.
      ColumnLayout {
        spacing: 0

        RowLayout {
          visible: root.bodyReady && !root.imagesAllowed && root.rendered.blocked > 0
          Layout.fillWidth: true
          Layout.margins: Style.spacing.lg
          spacing: Style.spacing.md

          UiText {
            Layout.fillWidth: true
            text: "Remote images were blocked (" + root.rendered.blocked + ")"
            dim: true
            elide: Text.ElideRight
          }

          Dropdown {
            id: imageChoice
            Layout.preferredWidth: Style.space(230)
            Layout.alignment: Qt.AlignVCenter
            showLabel: false
            value: "message"
            options: [
              { value: "message", label: "Unblock for this message" },
              { value: "sender",  label: "Unblock for this sender" }
            ]
            // The popup takes keyboard focus; hand it back when it closes.
            onPopupOpenChanged: if (!popupOpen) root.focusRequested()
          }

          Button {
            Layout.alignment: Qt.AlignVCenter
            bordered: true
            iconText: "\uf03e"
            text: "Unblock"
            onClicked: root.unblockRequested(imageChoice.value)
          }
        }

        ScrollView {
          id: htmlScroll
          clip: true
          Layout.fillWidth: true
          Layout.fillHeight: true
          // Sized by the StackLayout, never by its content (that made a binding loop).
          implicitWidth: 0
          implicitHeight: 0
          ScrollBar.horizontal.policy: ScrollBar.AlwaysOff
          // A ScrollView does not place a bar it is handed, so pin it to the right edge.
          ScrollBar.vertical: ThemedScrollBar {
            parent: htmlScroll
            tint: "#000000"
            x: htmlScroll.width - width
            y: 0
            height: htmlScroll.height
          }

          contentWidth: availableWidth
          contentHeight: page.height

          Rectangle {
            id: page
            width: htmlScroll.availableWidth
            height: root.shotShown ? Math.max(htmlScroll.height, shot.height) : Math.max(htmlScroll.height, pageColumn.implicitHeight + Style.spacing.huge * 2)
            color: root.shotShown ? root.shotBg : "#ffffff"

            // WebKit render: the PNG at pane width, with a click area over each link.
            Item {
              id: shot
              visible: root.shotShown
              width: page.width
              height: root.shotPixelWidth > 0 ? Math.round(width * root.shotPixelHeight / root.shotPixelWidth) : 0
              readonly property real k: root.shotPixelWidth > 0 ? width / root.shotPixelWidth : 1

              Image {
                anchors.fill: parent
                source: root.shotSource
                cache: false
                smooth: true
                fillMode: Image.Stretch
              }
              Repeater {
                model: root.shotLinks
                MouseArea {
                  id: linkBox
                  required property var modelData
                  required property int index
                  readonly property bool current: index === root.shownLink
                  readonly property string hint: root.hintMode && root.hintList[index] !== undefined && root.hintList[index].indexOf(root.hintTyped) === 0
                    ? root.hintList[index].slice(root.hintTyped.length) : ""
                  x: modelData.x * root.shotScale * shot.k
                  y: modelData.y * root.shotScale * shot.k
                  width: modelData.w * root.shotScale * shot.k
                  height: modelData.h * root.shotScale * shot.k
                  cursorShape: Qt.PointingHandCursor
                  onClicked: Qt.openUrlExternally(modelData.href)

                  // Keyboard cursor: an outlined, lightly filled box over the link.
                  Rectangle {
                    visible: linkBox.current
                    anchors.fill: parent
                    anchors.margins: -2
                    radius: 2
                    color: Qt.rgba(Commons.Color.accent.r, Commons.Color.accent.g, Commons.Color.accent.b, 0.22)
                    border.color: Commons.Color.accent
                    border.width: 2
                  }
                  // Hint label (f): typed letters are removed from it as they match.
                  Rectangle {
                    visible: linkBox.hint !== ""
                    x: 0
                    y: -height / 2
                    width: hintText.implicitWidth + 8
                    height: hintText.implicitHeight + 2
                    radius: 2
                    color: Hues.yellow
                    Text {
                      id: hintText
                      anchors.centerIn: parent
                      text: linkBox.hint
                      color: Commons.Color.background
                      font.family: Style.font.family
                      font.pixelSize: Style.font.caption
                      font.bold: true
                    }
                  }
                }
              }
            }

            ColumnLayout {
              id: pageColumn
              visible: !root.shotShown
              x: Style.spacing.huge
              y: Style.spacing.huge
              width: parent.width - Style.spacing.huge * 2
              spacing: Style.spacing.lg

              Text {
                id: htmlText
                Layout.fillWidth: true
                visible: root.statusText === "" && !root.htmlBusy
                textFormat: Text.RichText
                wrapMode: Text.Wrap
                color: "#000000"
                linkColor: "#0563c1"
                font.family: "Calibri"   // fontconfig maps it to Carlito if installed, else a sans
                font.pixelSize: 15
                text: root.rendered.html
                onLinkActivated: function(link) { Qt.openUrlExternally(link) }
                MouseArea {
                  anchors.fill: parent
                  acceptedButtons: Qt.NoButton
                  cursorShape: htmlText.hoveredLink !== "" ? Qt.PointingHandCursor : Qt.ArrowCursor
                }
              }

              Text {
                visible: root.statusText !== "" || root.htmlBusy
                Layout.fillWidth: true
                text: root.statusText !== "" ? root.statusText + (root.fallbackText !== "" ? "\n\n" + root.fallbackText : "") : "rendering…"
                color: "#777777"
                font.family: Style.font.family
                font.pixelSize: Style.font.body
              }
            }
          }
        }
      }
    }
  }
}
