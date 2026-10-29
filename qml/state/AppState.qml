import QtQuick
import Quickshell
import "../mail/format.js" as Fmt

// All non-visual state: navigation + focus model, folder/message data, and the
// backend (Rust daemon, local HTTP) calls. Components read properties and call
// the intent functions below; none of them talk to the backend themselves.
Item {
  id: root

  // ------------------------------------------------------------- backend
  // OMALOOK_PORT lets a second (test) instance's UI talk to its own backend; the default is the real one.
  readonly property string backendUrl: "http://127.0.0.1:" + (Quickshell.env("OMALOOK_PORT") || "27182")
  readonly property string configDir: {
    var d = Quickshell.env("CONFIG_DIR")
    return (d && d.length > 0) ? d : Quickshell.env("HOME") + "/.config/omalook"
  }
  property bool backendOnline: false
  onBackendOnlineChanged: {
    if (backendOnline) { perfMark("backend first answered"); loadCalendarSettings(); loadUiSettings(); loadSenders(); loadMailSettings(); loadBackgroundSettings(); loadMcp(); loadSearchHistory(); loadViewState() }
  }

  // ---- launch timing (see src/perf.rs): marks are queued until the backend answers, then
  // posted to POST /perf so they land in the same ~/.config/omalook/perf.log timeline.
  // Each carries its own QML-side age (ms since AppState was created).
  property double _perfT0: Date.now()
  property var _perfQueue: []
  function perfMark(label) {
    var text = label + " (qml +" + (Date.now() - _perfT0) + "ms)"
    if (!backendOnline && label !== "backend first answered") { _perfQueue.push(text); return }
    var q = _perfQueue; _perfQueue = []
    q.push(text)
    for (var i = 0; i < q.length; i++) {
      var xhr = new XMLHttpRequest()
      xhr.open("POST", backendUrl + "/perf?m=" + encodeURIComponent(q[i]), true)
      xhr.send()
    }
  }
  Component.onCompleted: perfMark("AppState created")
  // Calendar view chosen in the calendar dropdown: day | workweek | week | month
  property string calendarMode: "month"
  // People view dropdowns: all | favorites | lists   and   first | last | company | recent
  property string peopleView: "all"
  property string peopleSort: "first"
  // Tasks list filter: all | active | done
  property string tasksFilter: "all"
  readonly property alias auth: authController

  // ---------------------------------------------------------- navigation
  // Module list for `g` chords and the palette.
  readonly property var navItems: [
    { icon: "\uf0e0", label: "Mail",     view: "mail"     },
    { icon: "\uf073", label: "Calendar", view: "calendar" },
    { icon: "\uf0c0", label: "People",   view: "contacts" },
    { icon: "\uf0ae", label: "Tasks",    view: "tasks"    },
    { icon: "\uf27a", label: "SMS",      view: "sms"      },
    { icon: "\uf013", label: "Settings", view: "settings", pinned: true }
  ]

  readonly property var settingsCategories: [
    { id: "account",       icon: "\uf007", label: "Accounts"      },
    { id: "general",       icon: "\uf013", label: "General"       },
    { id: "appearance",    icon: "\uf1fc", label: "Appearance"    },
    { id: "mail",          icon: "\uf0e0", label: "Mail"          },
    { id: "senders",       icon: "\uf2bd", label: "Senders"       },
    { id: "calendar",      icon: "\uf073", label: "Calendar"      },
    { id: "sms",           icon: "\uf27a", label: "SMS"           },
    { id: "mcp",           icon: "\uf1e6", label: "MCP"           },
    { id: "notifications", icon: "\uf0f3", label: "Notifications" },
    { id: "about",         icon: "\uf05a", label: "About"         }
  ]

  property string currentView: "mail"
  // "folder" = folder bar, "msg" = message list (or the view's own list /
  // category list), "reader" = reading pane. There is no menu bar: modules are
  // reached with `g` chords or the command palette.
  property string focusPane: "msg"
  property int navIndex: 0
  property bool sidebarExpanded: true   // legacy persisted setting; no UI uses it
  property int folderIndex: 0
  property string selectedFolderId: ""
  property int msgIndex: 0
  property int settingsCategoryIndex: 0

  readonly property string currentViewLabel: navItems[navIndex].label
  readonly property alias folderModel: folderModelObj
  readonly property alias messageModel: messageModelObj

  property string messagesStatus: ""

  // Active search pattern ("from:stripe is:unread"); "" = the plain folder view. The scope is
  // whatever the list shows (one folder, or an "all accounts" folder); switching folders clears it.
  property string searchQuery: ""
  property bool searchPartial: false  // a fullbody: search hit the provider cap: newest matches only
  property var searchHistory: []      // recent patterns, newest first (kept by the backend)

  // _readRev is bumped whenever a row's read flag changes, so the reading pane's
  // button label re-evaluates (ListModel.get() hands back a snapshot).
  property int _readRev: 0
  readonly property var currentMessage: {
    _readRev
    return messageModelObj.count > msgIndex && msgIndex >= 0 ? messageModelObj.get(msgIndex) : null
  }

  readonly property string selectedFolderName: {
    var n = folderModelObj.count
    for (var i = 0; i < n; i++)
      if (folderModelObj.get(i).id === selectedFolderId) return folderModelObj.get(i).display_name
    return "Inbox"
  }

  // Counts come from the folder row (Graph's totalItemCount / unreadItemCount):
  // the message list is paged, so counting the loaded rows would stop at the
  // first page, and the cached per-message read flag is not reliable.
  readonly property var selectedFolder: {
    _readRev
    for (var i = 0; i < folderModelObj.count; i++)
      if (folderModelObj.get(i).id === selectedFolderId) return folderModelObj.get(i)
    return null
  }
  readonly property int messageTotal: selectedFolder && selectedFolder.total_item_count > 0
    ? selectedFolder.total_item_count : messageModelObj.count
  readonly property int unreadCount: selectedFolder ? (selectedFolder.unread_item_count || 0) : 0

  readonly property int messagePageSize: 200
  property bool _hasMoreMessages: false
  property bool _loadingMessages: false
  property int _messageGeneration: 0

  // ---- message body (reading pane) -----------------------------------------
  // The list carries only previews; the full body is fetched when a message is
  // opened (the backend caches it on disk, and the last few are kept here too).
  // Which view a message opens in (Settings → Mail → Message rendering):
  //   "html"          every message as HTML
  //   "system"        every message in the system font
  //   "system_sender" system font, unless the sender is listed with "always HTML"
  // The HTML/System button (and the "view as HTML" bar) override it per message,
  // for as long as the app is running.
  property string messageRendering: "system_sender"
  property var _htmlOverride: ({})
  property int _htmlRev: 0

  readonly property bool currentSenderAlwaysHtml: {
    _imagesRev
    for (var i = 0; i < senderModelObj.count; i++) {
      var s = senderModelObj.get(i)
      if (s.email === currentSenderEmail) return s.always_html === true
    }
    return false
  }

  readonly property bool currentHtmlOverridden: {
    _htmlRev
    return currentMessageId !== "" && _htmlOverride[currentMessageId] !== undefined
  }

  readonly property bool currentHtmlMode: {
    _htmlRev
    if (currentMessageId === "") return false
    var o = _htmlOverride[currentMessageId]
    if (o !== undefined) return o
    if (messageRendering === "html") return true
    if (messageRendering === "system") return false
    return currentSenderAlwaysHtml
  }

  // Offer "view as HTML" (this message / always for this sender) only where it can
  // matter: an HTML message, shown in the system font, in the mode that honours
  // sender preferences, that the user has not already chosen a view for.
  readonly property bool currentHtmlOffer:
    !currentHtmlMode && !currentHtmlOverridden && messageRendering === "system_sender"
    && currentBody.state === "ready" && currentBody.type === "html"

  // Which level decides the view now: "message" (this message's override), "sender" (the
  // sender's always-HTML rule, system_sender mode only) or "default" (the global setting).
  readonly property string currentRenderLevel: {
    _htmlRev
    if (currentMessageId === "") return "default"
    if (_htmlOverride[currentMessageId] !== undefined) return "message"
    if (messageRendering === "system_sender" && currentSenderAlwaysHtml) return "sender"
    return "default"
  }
  readonly property string currentRenderLabel:
    (currentHtmlMode ? "HTML" : "System") + " \u00b7 " + currentRenderLevel

  // A reply or forward of the message on screen is still inside its undo window.
  readonly property bool currentRenderLocked: {
    var id = currentMessageId
    if (id === "") return false
    for (var i = 0; i < sends.length; i++) {
      var m = sends[i].message
      if (sends[i].state === "pending" && m && String(m.in_reply_to || "") === id) return true
    }
    return false
  }

  // `H`: step this message through the levels. In system_sender mode the cycle is
  //   HTML/sender -> System/message -> System/default (rule removed) -> HTML/message -> HTML/sender
  // and in html / system mode it is just default <-> message (there is no sender level).
  // Returns the text to tell the user.
  function cycleRendering() {
    var id = currentMessageId
    if (id === "") return ""
    if (currentRenderLocked) return "Sending this message's reply \u2014 view stays as it is until the undo window ends"
    var email = currentSenderEmail
    var o = _htmlOverride[id]
    if (o === undefined) {
      _htmlOverride[id] = !currentHtmlMode
      _htmlRev++
      return currentHtmlMode ? "HTML for this message" : "System font for this message"
    }
    if (messageRendering !== "system_sender" || email === "") {
      delete _htmlOverride[id]
      _htmlRev++
      return "Back to the default view"
    }
    var rule = currentSenderAlwaysHtml
    if (rule !== o) {
      delete _htmlOverride[id]
      _htmlRev++
      if (o) _listSenderWith(email, "html")
      else setSenderPref(email, "html", false)
      return o ? "HTML always for " + email : "System font for " + email + " (always-HTML rule removed)"
    }
    delete _htmlOverride[id]
    _htmlRev++
    return "Back to the default view"
  }

  function toggleHtml() { var t = cycleRendering(); if (t !== "") notify(t) }

  function setMessageRendering(mode) {
    if (mode !== "html" && mode !== "system" && mode !== "system_sender") return
    var before = messageRendering
    messageRendering = mode
    request("POST", "/settings/message_rendering?value=" + mode, function(xhr) {
      var ok = false
      try { ok = JSON.parse(xhr.responseText).ok === true } catch (e) { ok = false }
      if (!ok) root.messageRendering = before
    })
  }
  property var currentBody: ({ id: "", state: "idle", type: "", content: "" })
  property var _bodyCache: ({})
  property var _bodyOrder: []

  readonly property string currentMessageId: currentMessage ? String(currentMessage.id || "") : ""
  // Debounced: holding j/k (or gg/G) changes the selection many times a second, and each
  // uncached message would start a fetch plus a rich-text layout (OL-001). Cached bodies
  // show at once; everything else waits until the cursor has rested for 120 ms.
  // Assigning currentBody always fires the reading pane's onBodyChanged, which throws away the
  // rendered result and renders again (WebKit helper included). So only assign when the body is
  // actually different: a re-selection of the same message after a list reload or a periodic
  // refresh must leave the finished render alone.
  function _setBody(b) {
    var c = currentBody
    if (c && c.id === b.id && c.state === b.state && c.type === b.type && c.content === b.content
        && c.reason === b.reason && c.preview === b.preview) return
    currentBody = b
  }
  // true while loadMessages() swaps the whole model: the selection is momentarily "no message".
  property bool _reloadingModel: false

  onCurrentMessageIdChanged: {
    var id = currentMessageId
    // Same message, already shown (list reloaded under the cursor): nothing to fetch or redraw.
    if (id === "" && _reloadingModel) return
    if (id !== "" && currentBody.id === id && currentBody.state === "ready"
        && invite.id === id && attachments.id === id) { _bodyDebounce.stop(); return }
    if (id === "" || _bodyCache[id]) { _bodyDebounce.stop(); loadBody(id); loadInvite(id); loadAttachments(id); return }
    _setBody({ id: id, state: "loading", type: "", content: "" })
    invite = _emptyInvite(id, "idle")
    attachments = _emptyAttachments(id, "idle")
    _bodyDebounce.restart()
  }
  // The invite lookup waits the same 120 ms as the body, so holding j/k never queues Graph calls.
  Timer { id: _bodyDebounce; interval: 120; onTriggered: { root.loadBody(root.currentMessageId); root.loadInvite(root.currentMessageId); root.loadAttachments(root.currentMessageId) } }

  // ---- attachments -----------------------------------------------------------
  // attachments: { id, state: "idle"|"loading"|"ready"|"error", error, dir, items: [{ id, name, size,
  // content_type, inline, kind }] } for the selected message. Inline items (pictures that are part
  // of the HTML) come back too; everything the UI shows goes through realAttachments().
  function _emptyAttachments(id, state, error) { return { id: id, state: state, error: error || "", dir: "", items: [] } }
  property var attachments: _emptyAttachments("", "idle")
  property var _attCache: ({})
  // "messageId|attachmentId" -> true once saved this session (shown as a check mark).
  property var savedAttachments: ({})
  property bool savingAttachments: false

  // What the user would call attachments of the selected message (inline pictures excluded).
  function realAttachments() {
    return attachments.id === currentMessageId ? attachments.items.filter(function(i) { return !i.inline }) : []
  }
  function attachmentSaved(attId) { return savedAttachments[attachments.id + "|" + attId] === true }

  // Only messages the list says have attachments are looked up. Like the invite lookup it runs
  // after the selection has rested, so holding j/k never queues Graph calls.
  function loadAttachments(id) {
    if (id === "" || !currentMessage || currentMessage.has_attachments !== true) { attachments = _emptyAttachments(id, "idle"); return }
    var hit = _attCache[id]
    if (hit) { attachments = hit; return }
    attachments = _emptyAttachments(id, "loading")
    request("GET", "/messages/attachments?id=" + encodeURIComponent(id), function(xhr) {
      var r = null
      try { r = JSON.parse(xhr.responseText) } catch (e) { r = null }
      var o
      if (!r || r.error !== undefined || r.attachments === undefined)
        o = _emptyAttachments(id, "error", r && r.error ? String(r.error) : "could not reach the backend")
      else { o = _emptyAttachments(id, "ready"); o.items = r.attachments; o.dir = r.dir || ""; _attCache[id] = o }
      if (id === currentMessageId) attachments = o
    })
  }

  // Save one attachment (attId) or every non-inline one ("") into the download folder, as
  // individual files. The result is one line in the status bar.
  function saveAttachments(attId) {
    var id = attachments.id
    if (id === "" || id !== currentMessageId || savingAttachments || attachments.state !== "ready") return
    savingAttachments = true
    var q = "/messages/attachments/save?id=" + encodeURIComponent(id) + (attId !== "" ? "&att=" + encodeURIComponent(attId) : "")
    request("POST", q, function(xhr) {
      savingAttachments = false
      var r = null
      try { r = JSON.parse(xhr.responseText) } catch (e) { r = null }
      if (!r || r.error !== undefined) { notify("Could not save: " + (r && r.error ? r.error : "backend did not answer")); return }
      var marks = savedAttachments
      r.saved.forEach(function(s) { marks[id + "|" + s.id] = true })
      savedAttachments = Object.assign({}, marks)
      var n = r.saved.length, f = r.failed.length
      if (f === 0) notify(n === 1 ? "Saved " + r.saved[0].name + " to " + r.dir : "Saved " + n + " files to " + r.dir)
      else notify((n > 0 ? "Saved " + n + ", " : "") + f + " failed: " + r.failed[0].name + " (" + r.failed[0].error + ")")
    })
  }

  // ---- meeting invitations ---------------------------------------------------
  // invite: { id, state: "idle"|"loading"|"ready"|"sending"|"error", error, type, actionable,
  //           response, organizer, isOrganizer, cancelled, hasEvent, when, location }.
  // `type` is the Graph meetingMessageType ("meetingRequest", "meetingCancelled", ...) or "none".
  // `response` is the user's own answer on the event ("notResponded", "accepted", ...).
  function _emptyInvite(id, state, error) {
    return { id: id, state: state, error: error || "", type: "", actionable: false, response: "",
             organizer: "", isOrganizer: false, cancelled: false, hasEvent: false, when: "", location: "" }
  }
  property var invite: _emptyInvite("", "idle")
  property var _inviteCache: ({})
  // Bumped after an answer so the calendar view reloads its events.
  property int calendarRev: 0

  // Everything the backend said about the message, as the reader shows it.
  function _inviteFrom(id, r, state) {
    var o = _emptyInvite(id, state)
    o.type = r.type || ""; o.actionable = !!r.actionable; o.response = r.response || ""
    o.organizer = r.organizer || ""; o.isOrganizer = !!r.isOrganizer; o.cancelled = !!r.cancelled
    o.hasEvent = !!r.hasEvent; o.when = r.when || ""; o.location = r.location || ""
    return o
  }

  function _setInvite(id, o) {
    if (id !== currentMessageId) return
    invite = o
  }

  // Ordinary mail ("none") never changes; invitations can be answered or cancelled elsewhere,
  // so they are looked up again after a minute (the backend keeps its own five-minute cache).
  function loadInvite(id) {
    if (id === "") { invite = _emptyInvite("", "idle"); return }
    var hit = _inviteCache[id]
    if (hit && (hit.r.type === "none" || Date.now() - hit.at < 60000)) { _setInvite(id, _inviteFrom(id, hit.r, "ready")); return }
    _setInvite(id, _emptyInvite(id, "loading"))
    request("GET", "/messages/invite?id=" + encodeURIComponent(id), function(xhr) {
      var r = null
      try { r = JSON.parse(xhr.responseText) } catch (e) { r = null }
      if (!r || r.error !== undefined || r.type === undefined) {
        _setInvite(id, _emptyInvite(id, "error", r && r.error ? String(r.error) : ""))
        return
      }
      _inviteCache[id] = { r: r, at: Date.now() }
      _setInvite(id, _inviteFrom(id, r, "ready"))
    })
  }

  // choice: "accept" | "tentative" | "decline". Sent silently to the organizer.
  function respondInvite(choice) {
    var id = invite.id
    if (!invite.actionable || invite.state === "sending" || id !== currentMessageId) return
    var before = invite
    var sending = _inviteFrom(id, { type: before.type, actionable: true, response: before.response, organizer: before.organizer,
                                    isOrganizer: before.isOrganizer, cancelled: before.cancelled, hasEvent: true,
                                    when: before.when, location: before.location }, "sending")
    _setInvite(id, sending)
    request("POST", "/messages/invite?id=" + encodeURIComponent(id) + "&choice=" + choice, function(xhr) {
      var r = null
      try { r = JSON.parse(xhr.responseText) } catch (e) { r = null }
      if (!r || r.error !== undefined || r.ok !== true) {
        var msg = r && r.error ? String(r.error) : "could not reach the backend"
        var back = _inviteFrom(id, { type: before.type, actionable: true, response: before.response, organizer: before.organizer,
                                     isOrganizer: before.isOrganizer, cancelled: before.cancelled, hasEvent: true,
                                     when: before.when, location: before.location }, "error")
        back.error = msg
        _setInvite(id, back)
        notify(msg)
        return
      }
      _inviteCache[id] = { r: r, at: Date.now() }
      _setInvite(id, _inviteFrom(id, r, "ready"))
      if (r.calendar === "refreshed") calendarRev++
      markRead(id)   // answering an invitation deals with it: it no longer counts as unread
      var label = choice === "accept" ? "Accepted" : choice === "tentative" ? "Tentatively accepted" : "Declined"
      notify(label + (r.calendar !== "refreshed" ? " (calendar not refreshed: " + (r.calendar || "") + ")" : ""))
    })
  }

  function loadBody(id) {
    if (id === "") { _setBody({ id: "", state: "idle", type: "", content: "" }); return }
    var hit = _bodyCache[id]
    if (hit) { _setBody({ id: id, state: "ready", type: hit.type, content: hit.content }); return }
    _setBody({ id: id, state: "loading", type: "", content: "" })
    request("GET", "/messages/body?id=" + encodeURIComponent(id), function(xhr) {
      var result = null
      if (xhr.status === 200) {
        try { result = JSON.parse(xhr.responseText) } catch (e) { result = null }
      }
      var ok = result && result.error === undefined && result.type !== undefined
      if (ok) {
        _bodyCache[id] = { type: result.type, content: result.content }
        _bodyOrder.push(id)
        if (_bodyOrder.length > 30) delete _bodyCache[_bodyOrder.shift()]
      }
      if (root.currentMessageId !== id) return       // moved on while it loaded
      root._setBody(ok
        ? { id: id, state: "ready", type: result.type, content: result.content }
        : { id: id, state: "error", type: "", content: "",
            reason: (result && result.error) ? String(result.error) : "could not load the message body",
            preview: (result && result.preview) ? String(result.preview) : "" })
    })
  }

  // ---- per-sender preferences (Settings → Senders) --------------------------
  // [{ email, always_html, always_images }] from /settings/senders. Stored only;
  // nothing in the reading pane consults them yet.
  readonly property alias senderModel: senderModelObj
  ListModel { id: senderModelObj }
  property string senderNotice: ""

  property string _sendersJson: ""

  function loadSenders() {
    request("GET", "/settings/senders", function(xhr) {
      if (xhr.status !== 200) return
      try {
        var rows = JSON.parse(xhr.responseText)
        if (!Array.isArray(rows)) return
        // Polled every 20 s. clear() empties the model before append() refills it, and the
        // reading pane's "always HTML" / "always load images" bindings read it in between: for a
        // listed sender they flipped off and on again, and each flip re-rendered the open message.
        // Unchanged data must not touch the model.
        var json = xhr.responseText
        if (json === root._sendersJson) return
        root._sendersJson = json
        senderModelObj.clear()
        for (var i = 0; i < rows.length; i++) senderModelObj.append(rows[i])
        root._imagesRev++
      } catch (e) { console.log("[Senders] parse error:", e) }
    })
  }

  function addSender(email, done) {
    var e = String(email || "").trim()
    if (e === "") { senderNotice = "Enter an email address."; if (done) done(false); return }
    request("POST", "/settings/senders/add?email=" + encodeURIComponent(e), function(xhr) {
      var r = null
      try { r = JSON.parse(xhr.responseText) } catch (x) { r = null }
      var status = r && r.status ? r.status : "error"
      senderNotice = status === "added" ? ""
        : (status === "exists" ? e.toLowerCase() + " is already listed."
        : (status === "invalid" ? "That doesn't look like an email address."
        : "Could not add the address."))
      if (status === "added" || status === "exists") loadSenders()
      if (done) done(status === "added")
    })
  }

  // field: "html" | "images". Optimistic; put back if the backend refuses.
  function setSenderPref(email, field, value) {
    var key = field === "html" ? "always_html" : "always_images"
    function apply(v) {
      for (var i = 0; i < senderModelObj.count; i++)
        if (senderModelObj.get(i).email === email) { senderModelObj.setProperty(i, key, v); return }
    }
    apply(value)
    root._imagesRev++
    request("POST", "/settings/senders/set?email=" + encodeURIComponent(email)
            + "&field=" + field + "&value=" + value, function(xhr) {
      var ok = false
      try { ok = JSON.parse(xhr.responseText).ok === true } catch (x) { ok = false }
      if (!ok) { apply(!value); root._imagesRev++ }
    })
  }

  function removeSender(email) {
    request("POST", "/settings/senders/remove?email=" + encodeURIComponent(email), function(xhr) {
      loadSenders()
    })
  }

  // ---- remote images in the HTML view ------------------------------------------
  // Blocked unless this message was unblocked (kept for the session) or its sender
  // is listed with "always load images" (Settings → Senders).
  property var _imagesUnblocked: ({})
  property int _imagesRev: 0

  readonly property string currentSenderEmail:
    currentMessage ? String(currentMessage.from_email || "").trim().toLowerCase() : ""

  readonly property bool currentImagesAllowed: {
    _imagesRev
    if (currentMessageId === "") return false
    if (_imagesUnblocked[currentMessageId] === true) return true
    for (var i = 0; i < senderModelObj.count; i++) {
      var s = senderModelObj.get(i)
      if (s.email === currentSenderEmail) return s.always_images === true
    }
    return false
  }

  readonly property bool currentSenderAlwaysImages: {
    _imagesRev
    for (var i = 0; i < senderModelObj.count; i++) {
      var s = senderModelObj.get(i)
      if (s.email === currentSenderEmail) return s.always_images === true
    }
    return false
  }

  // Which level lets the images load now: "sender" (always-load-images rule), "message"
  // (unblocked for this message, session only) or "blocked".
  readonly property string currentImagesLevel: {
    _imagesRev
    if (currentMessageId === "") return "blocked"
    if (currentSenderAlwaysImages) return "sender"
    if (_imagesUnblocked[currentMessageId] === true) return "message"
    return "blocked"
  }

  // `I`: blocked -> this message -> this sender (always-load-images rule) -> blocked
  // (the rule is removed). Returns the text to tell the user.
  function cycleImages() {
    var id = currentMessageId, email = currentSenderEmail
    if (id === "") return ""
    var level = currentImagesLevel
    if (level === "blocked") {
      _imagesUnblocked[id] = true
      _imagesRev++
      return "Remote images loaded for this message"
    }
    if (level === "message" && email !== "") {
      delete _imagesUnblocked[id]
      _imagesRev++
      _listSenderWith(email, "images")
      return "Remote images always loaded for " + email
    }
    delete _imagesUnblocked[id]
    _imagesRev++
    if (level === "sender") setSenderPref(email, "images", false)
    return "Remote images blocked again" + (level === "sender" ? " (always-load rule for " + email + " removed)" : "")
  }

  function toggleImages() { var t = cycleImages(); if (t !== "") notify(t) }

  // Make sure `email` is in the senders list with `field` ("html" | "images") on.
  function _listSenderWith(email, field) {
    if (email === "") return
    request("POST", "/settings/senders/add?email=" + encodeURIComponent(email), function(xhr) {
      var status = ""
      try { status = JSON.parse(xhr.responseText).status } catch (e) { status = "" }
      if (status !== "added" && status !== "exists") return
      request("POST", "/settings/senders/set?email=" + encodeURIComponent(email)
              + "&field=" + field + "&value=true", function(x2) { root.loadSenders() })
    })
  }

  signal focusRequested()   // ask the shell to put keyboard focus back on the key catcher
  signal closeRequested()   // `Q` / `:q` — close the window

  ListModel { id: folderModelObj }
  ListModel { id: messageModelObj }

  AuthController {
    id: authController
    backendUrl: root.backendUrl
    configDir: root.configDir
    onAccountsChanged: root.loadAccounts()
    onIsAuthenticatedChanged: {
      if (isAuthenticated) { root.loadFolders(); root.loadMessages() }
      root.loadAccounts()
    }
  }

  // Settings → Calendar: how far recurring meetings are expanded (whole years either side of
  // today). Owned by the backend (settings.toml); the daemon applies changes on its next cycle.
  property int recurrenceYearsBack: 5
  property int recurrenceYearsAhead: 10
  property int maxYearsBack: 20
  property int maxYearsAhead: 30
  // New events (Settings → Calendar, docs/PLAN-calendar-events.md section 9).
  property int calendarDefaultMinutes: 30
  property int calendarDefaultReminder: 15
  property int calendarDefaultSeriesMonths: 12
  property string calendarDefaultAccount: ""        // "" = the account used last
  // The account the last saved calendar entry went to (`last_calendar_account` in view_state.json).
  property string lastCalendarAccount: ""
  onLastCalendarAccountChanged: _saveViewSoon()

  function applyCalendarSettings(data) {
    recurrenceYearsBack = data.recurrence_years_back
    recurrenceYearsAhead = data.recurrence_years_ahead
    if (typeof data.default_minutes === "number") calendarDefaultMinutes = data.default_minutes
    if (typeof data.default_reminder_minutes === "number") calendarDefaultReminder = data.default_reminder_minutes
    if (typeof data.default_series_months === "number") calendarDefaultSeriesMonths = data.default_series_months
    if (typeof data.default_account === "string") calendarDefaultAccount = data.default_account
    if (data.max_years_back) maxYearsBack = data.max_years_back
    if (data.max_years_ahead) maxYearsAhead = data.max_years_ahead
  }

  function loadCalendarSettings() {
    request("GET", "/settings/calendar", function(xhr) {
      if (xhr.status !== 200) return
      try { applyCalendarSettings(JSON.parse(xhr.responseText)) } catch (e) { console.log("[CalendarSettings] parse error:", e) }
    })
  }

  // Called on every stepper change; the actual save is debounced so rapid clicks send one request.
  function setRecurrenceWindow(back, ahead) {
    recurrenceYearsBack = back
    recurrenceYearsAhead = ahead
    calendarSettingsSave.restart()
  }

  // Settings → Calendar: defaults for new events (debounced like the window above).
  function setCalendarDefaults(minutes, reminder, seriesMonths, account) {
    calendarDefaultMinutes = minutes
    calendarDefaultReminder = reminder
    calendarDefaultSeriesMonths = seriesMonths
    calendarDefaultAccount = account
    calendarSettingsSave.restart()
  }

  Timer {
    id: calendarSettingsSave
    interval: 600
    onTriggered: root.request("POST",
      "/settings/calendar?back=" + root.recurrenceYearsBack + "&ahead=" + root.recurrenceYearsAhead
        + "&minutes=" + root.calendarDefaultMinutes + "&reminder=" + root.calendarDefaultReminder
        + "&series_months=" + root.calendarDefaultSeriesMonths + "&account=" + encodeURIComponent(root.calendarDefaultAccount),
      function(xhr) {
        if (xhr.status !== 200) return
        // the backend clamps; show what was actually stored
        try { root.applyCalendarSettings(JSON.parse(xhr.responseText)) } catch (e) {}
      })
  }

  // Signed-in accounts for Settings → Accounts: [{ id, provider, email, signed_in }]
  property var accounts: []

  function loadAccounts() {
    request("GET", "/accounts", function(xhr) {
      if (xhr.status !== 200) return
      try {
        var list = JSON.parse(xhr.responseText)
        root.accounts = list
        // The backend resolves the address asynchronously — re-poll until it's there
        accountRetry.running = list.some(function(a) { return !a.email })
      } catch (e) { console.log("[Accounts] parse error:", e) }
    })
  }

  Timer {
    id: accountRetry
    interval: 2000
    repeat: true
    onTriggered: root.loadAccounts()
  }

  // --------------------------------------------------------- backend calls
  function request(method, path, onDone, body) {
    var xhr = new XMLHttpRequest()
    xhr.onreadystatechange = function() {
      if (xhr.readyState !== XMLHttpRequest.DONE) return
      root.backendOnline = xhr.status !== 0
      if (onDone) onDone(xhr)
    }
    xhr.open(method, root.backendUrl + path, true)
    xhr.send(body)
  }

  // Refresh (button, F5): ask the backend to poll the provider now. Show the cache at once;
  // whatever the poll stores reaches the list through the sync serial (checkSyncSerial).
  function syncMail() {
    request("POST", "/mail/sync", function(xhr) {
      if (xhr.status !== 200) notify("Refresh failed: backend not reachable")
    })
    loadFolders()
    loadMessages()
  }

  function loadFolders() {
    request("GET", "/folders", function(xhr) {
      if (xhr.status !== 200) return
      try {
        var folders = JSON.parse(xhr.responseText)
        folderModelObj.clear()
        for (var i = 0; i < folders.length; i++) folderModelObj.append(folders[i])
        root._applyWantedFolder()
        if (root.selectedFolderId === "") {
          for (var k = 0; k < folderModelObj.count; k++)
            if (!folderModelObj.get(k).all_accounts) { root.selectedFolderId = folderModelObj.get(k).id; break }
        }
      } catch (e) { console.log("[Folders] parse error:", e) }
    })
  }

  // Folder rows carry the unread count the status bar shows; nudge it locally so it
  // follows a toggle straight away (the next folder sync replaces it with the provider's).
  // Bumps the folder's own row and, when it has one, the "<Kind> - all accounts" row that
  // sums it, so the count is right whichever of the two is on screen.
  function _bumpFolderUnread(folderId, delta) {
    var kind = "", aggId = ""
    for (var i = 0; i < folderModelObj.count; i++) {
      var f = folderModelObj.get(i)
      if (f.id === folderId && !f.all_accounts) {
        folderModelObj.setProperty(i, "unread_item_count", Math.max(0, (f.unread_item_count || 0) + delta))
        kind = String(f.well_known_name || "")
        break
      }
    }
    if (kind === "") return
    aggId = "all:" + kind
    for (var j = 0; j < folderModelObj.count; j++) {
      var g = folderModelObj.get(j)
      if (g.id === aggId) {
        folderModelObj.setProperty(j, "unread_item_count", Math.max(0, (g.unread_item_count || 0) + delta))
        return
      }
    }
  }

  function _setRead(index, isRead) {
    messageModelObj.setProperty(index, "is_read", isRead)
    root._readRev++
  }

  // ---- compose ---------------------------------------------------------------------------
  // The compose pane takes the reading pane's slot while `composing`. This is state + intents
  // only; sending is not wired to the backend yet (PLAN-compose.md Phase A), so submitCompose
  // reports that instead of pretending.
  property bool composing: false
  property var composeInitial: ({ kind: "new" })
  property string composeStatus: ""
  property string composeDefaultMode: "system"   // [mail.compose] default_format, once settings carry it
  property var composeContacts: []               // [{ name, email }] for recipient suggestions

  function loadComposeContacts() {
    request("GET", "/contacts?view=all", function(xhr) {
      if (xhr.status !== 200) return
      try {
        var list = JSON.parse(xhr.responseText), out = []
        for (var i = 0; i < list.length; i++)
          for (var j = 0; j < (list[i].emails || []).length; j++)
            out.push({ name: list[i].display_name || "", email: list[i].emails[j] })
        root.composeContacts = out
      } catch (e) { console.log("[Compose] contacts parse error:", e) }
    })
  }

  function _addr(m) {
    var e = m && m.from_email ? String(m.from_email) : ""
    var n = m && m.from_name ? String(m.from_name) : ""
    return n !== "" && e !== "" ? n + " <" + e + ">" : e
  }

  // kind: "new" | "reply" | "replyAll" | "forward". Default source: the message under the cursor.
  function openCompose(kind) {
    var m = currentMessage
    var init = { kind: kind, accountId: "", to: "", cc: "", bcc: "", subject: "", quote: null, inReplyTo: "" }
    var acct = accounts.filter(function(a) { return a.signed_in && a.email })
    if (acct.length > 0) init.accountId = acct[0].id
    if (kind !== "new" && m) {
      if (m.account_id) init.accountId = String(m.account_id)
      var subj = String(m.subject || "")
      var plain = currentBody && currentBody.state === "ready"
        ? (currentBody.type === "html" ? Fmt.htmlToText(currentBody.content) : currentBody.content)
        : String(m.body_preview || "")
      init.quote = { from: _addr(m), date: Fmt.fullDate(m.received_at), text: plain }
      init.inReplyTo = String(m.id || "")
      if (kind === "forward") {
        init.subject = /^fw(d)?:/i.test(subj) ? subj : "Fwd: " + subj
      } else {
        init.subject = /^re:/i.test(subj) ? subj : "Re: " + subj
        init.to = _addr(m)
        if (kind === "replyAll") {
          var r = Fmt.replyAllRecipients(m, accounts.map(function(a) { return a.email }).concat([m.account_email]))
          init.to = r.to; init.cc = r.cc; init.bcc = r.bcc
        }
      }
    }
    composeStatus = ""
    composeInitial = init
    composing = true
    if (composeContacts.length === 0) loadComposeContacts()
  }

  // File browser for attachments: the daemon lists one folder at a time; the last folder used is
  // where the browser opens next time (this session).
  property string attachBrowseDir: ""

  function listDir(path, hidden, cb) {
    request("GET", "/fs/list?path=" + encodeURIComponent(path) + (hidden ? "&hidden=1" : ""), function(xhr) {
      var r = null
      try { r = JSON.parse(xhr.responseText) } catch (e) {}
      cb(xhr.status === 200 && r ? r : { error: "could not reach the backend" })
    })
  }

  function closeCompose() { composing = false; composeStatus = ""; focusRequested() }

  // ---- sending ---------------------------------------------------------------------------
  // Send hands the message to the backend's outbox, which holds it for `sendDelaySecs` (Settings
  // -> Mail) so it can still be taken back, then sends it at once from its own worker. The
  // compose pane closes straight away; SendToast shows each message's progress and offers Undo.
  property int sendDelaySecs: 3
  property int maxSendDelaySecs: 60

  function applyMailSettings(d) {
    if (d.send_delay_secs !== undefined) sendDelaySecs = d.send_delay_secs
    if (d.max_send_delay_secs) maxSendDelaySecs = d.max_send_delay_secs
  }

  function loadMailSettings() {
    request("GET", "/settings/mail", function(xhr) {
      if (xhr.status !== 200) return
      try { applyMailSettings(JSON.parse(xhr.responseText)) } catch (e) { console.log("[MailSettings] parse error:", e) }
    })
  }

  // Called on every stepper change; the save is debounced so rapid clicks send one request.
  function setSendDelay(secs) {
    sendDelaySecs = secs
    mailSettingsSave.restart()
  }

  Timer {
    id: mailSettingsSave
    interval: 600
    onTriggered: root.request("POST", "/settings/mail?send_delay=" + root.sendDelaySecs, function(xhr) {
      if (xhr.status !== 200) return
      try { root.applyMailSettings(JSON.parse(xhr.responseText)) } catch (e) {}   // the backend clamps
    })
  }

  // ---- background running ----------------------------------------------------------------
  // Settings -> General -> "Keep running in background": the backend (sync, SMS mirror) stays up
  // after this window closes and the next launch reattaches to it. Persisted in settings.toml.
  property bool runInBackground: false

  function loadBackgroundSettings() {
    request("GET", "/settings/background", function(xhr) {
      if (xhr.status !== 200) return
      try { runInBackground = JSON.parse(xhr.responseText).run_in_background === true } catch (e) { console.log("[Background] parse error:", e) }
    })
  }

  function setRunInBackground(on) {
    var before = runInBackground
    runInBackground = on
    request("POST", "/settings/background?enabled=" + (on ? "true" : "false"), function(xhr) {
      if (xhr.status !== 200) { runInBackground = before; return }
      try { runInBackground = JSON.parse(xhr.responseText).run_in_background === true } catch (e) { runInBackground = before }
    })
  }

  // ---- MCP server (Settings -> MCP) --------------------------------------------------------
  // State of the MCP permissions as the backend reports it (src/mcp/admin.rs): master switch,
  // whether it is active (needs "Keep running in background"), preset, domain levels, write
  // tools with their modes, allowed account ids. Every change goes to the backend, which
  // validates and stores it in settings.toml [mcp] and answers with the new state.
  property var mcp: ({ settings_readable: true, run_in_background: false, enabled: false, active: false, listening: false,
                       preset: "locked", domains: ({}), read_tools: ({}), tools: [], accounts: [], http_url: "", command: "" })
  property string mcpError: ""

  function applyMcp(text) {
    try {
      var d = JSON.parse(text)
      mcpError = d.error || ""
      delete d.error
      mcp = d
    } catch (e) { console.log("[MCP] parse error:", e) }
  }

  function loadMcp() {
    request("GET", "/settings/mcp", function(xhr) { if (xhr.status === 200) applyMcp(xhr.responseText) })
  }

  // path is the part after /settings/mcp, query included, e.g. "/domain?domain=mail&level=read".
  function mcpPost(path, done) {
    request("POST", "/settings/mcp" + path, function(xhr) {
      if (xhr.status === 200) applyMcp(xhr.responseText)
      if (done) done(xhr.status === 200 && mcpError === "")
    })
  }

  // Messages handed to the backend and not yet dismissed:
  // [{ id, message, sendAt (ms), state: pending|sending|sent|failed, error, doneAt }]
  property var sends: []
  property real sendNow: Date.now()      // ticks while anything is in flight, for the countdown
  readonly property bool sending: sends.length > 0

  function _sendIndex(id) {
    for (var i = 0; i < sends.length; i++) if (sends[i].id === id) return i
    return -1
  }

  function _patchSend(id, patch) {
    var i = _sendIndex(id)
    if (i < 0) return
    var next = sends.slice(), cur = next[i], merged = {}
    for (var k in cur) merged[k] = cur[k]
    for (var p in patch) merged[p] = patch[p]
    next[i] = merged
    sends = next
  }

  function dismissSend(id) { sends = sends.filter(function(s) { return s.id !== id }) }

  function submitCompose(message) {
    composeStatus = "sending…"
    request("POST", "/compose/send", function(xhr) {
      var r = null
      try { r = JSON.parse(xhr.responseText) } catch (e) {}
      if (xhr.status !== 200 || !r || !r.ok) {
        // Stay in the editor: nothing was queued, nothing is lost.
        composeStatus = r && r.error ? String(r.error) : "Could not queue the message (is the backend running?)"
        return
      }
      sendNow = Date.now()
      sends = sends.concat([{
        id: r.id, message: message, sendAt: sendNow + r.delay_secs * 1000, delaySecs: r.delay_secs,
        state: r.delay_secs > 0 ? "pending" : "sending", error: "", doneAt: 0, polling: false
      }])
      closeCompose()
    }, JSON.stringify(message))
  }

  function _pollSend(id) {
    var i = _sendIndex(id)
    if (i < 0 || sends[i].polling) return
    _patchSend(id, { polling: true })
    request("GET", "/compose/status?id=" + encodeURIComponent(id), function(xhr) {
      var r = null
      try { r = JSON.parse(xhr.responseText) } catch (e) {}
      if (_sendIndex(id) < 0) return
      if (!r || !r.state) { _patchSend(id, { polling: false }); return }
      if (r.state === "sent") _patchSend(id, { state: "sent", doneAt: Date.now(), polling: false })
      else if (r.state === "failed") _patchSend(id, { state: "failed", error: String(r.error || "The message could not be sent."), polling: false })
      else if (r.state === "cancelled") dismissSend(id)
      else _patchSend(id, { state: Date.now() >= sends[_sendIndex(id)].sendAt ? "sending" : "pending", polling: false })
    })
  }

  Timer {
    id: sendTick
    interval: 250
    repeat: true
    running: root.sends.length > 0
    onTriggered: {
      root.sendNow = Date.now()
      var keep = [], changed = false
      for (var i = 0; i < root.sends.length; i++) {
        var s = root.sends[i]
        if (s.state === "sent" && root.sendNow - s.doneAt > 2500) { changed = true; continue }   // fade out
        keep.push(s)
        // Once the delay is over, ask how it went (about once a second).
        if ((s.state === "pending" || s.state === "sending") && root.sendNow >= s.sendAt - 100
            && Math.floor(root.sendNow / 1000) !== Math.floor((root.sendNow - 250) / 1000))
          root._pollSend(s.id)
      }
      if (changed) root.sends = keep
    }
  }

  // Take a queued message back and return to drafting. Only works while it is still waiting;
  // once the worker has started sending it is too late, and the toast says so.
  function undoSend(id) {
    if (composing) return          // one compose at a time: finish or discard the open one first
    var i = _sendIndex(id)
    if (i < 0) return
    var msg = sends[i].message
    request("POST", "/compose/cancel?id=" + encodeURIComponent(id), function(xhr) {
      var r = null
      try { r = JSON.parse(xhr.responseText) } catch (e) {}
      if (r && r.cancelled) { dismissSend(id); reopenCompose(msg) }
      else _patchSend(id, { state: r && r.state === "sent" ? "sent" : "sending", doneAt: Date.now() })
    })
  }

  function reopenFailedSend(id) {
    if (composing) return
    var i = _sendIndex(id)
    if (i < 0) return
    var msg = sends[i].message
    dismissSend(id)
    reopenCompose(msg)
  }

  function _addrText(list) {
    return (list || []).map(function(a) { return a.name ? a.name + " <" + a.email + ">" : a.email }).join(", ")
  }

  // Back into the editor exactly as it was sent: recipients, subject, body and mode.
  function reopenCompose(m) {
    composeStatus = ""
    composeInitial = {
      kind: m.kind, accountId: m.account_id, inReplyTo: m.in_reply_to || "",
      to: _addrText(m.to), cc: _addrText(m.cc), bcc: _addrText(m.bcc),
      subject: m.subject, quote: null, restore: m.body, mode: m.mode, attachments: m.attachments || []
    }
    composing = true
    if (composeContacts.length === 0) loadComposeContacts()
  }

  // The newest message still inside its delay (what `u` takes back).
  function undoLatest() {
    for (var i = sends.length - 1; i >= 0; i--)
      if (sends[i].state === "pending") { undoSend(sends[i].id); return true }
    return false
  }

  // Mark the message at `index` read/unread (default: the one under the cursor).
  // Optimistic: the row, the reading pane and the folder count change at once; the
  // backend stores it and the daemon pushes it to the provider. A refused request
  // puts everything back.
  function toggleRead(index) {
    var i = index === undefined ? msgIndex : index
    if (i < 0 || i >= messageModelObj.count) return
    var m = messageModelObj.get(i)
    var id = m.id
    var wasRead = !!m.is_read
    var delta = wasRead ? 1 : -1
    var fid = m.folder_id || selectedFolderId
    _setRead(i, !wasRead)
    _bumpFolderUnread(fid, delta)
    request("POST", "/messages/read?id=" + encodeURIComponent(id) + "&read=" + (!wasRead), function(xhr) {
      if (xhr.status === 200 && xhr.responseText === "ok") return
      // Roll back — the list may have been reloaded meanwhile, so find the row by id.
      for (var j = 0; j < messageModelObj.count; j++) {
        if (messageModelObj.get(j).id === id) { root._setRead(j, wasRead); break }
      }
      root._bumpFolderUnread(fid, -delta)
    })
  }

  // Mark the message with this id read (no-op when it already is). Same optimistic update and
  // rollback as toggleRead, but addressed by id: used after answering an invitation, when the
  // cursor may have moved on by the time the backend replies.
  function markRead(id) {
    var row = -1, fid = selectedFolderId
    for (var j = 0; j < messageModelObj.count; j++) {
      if (messageModelObj.get(j).id === id) { row = j; break }
    }
    if (row >= 0) {
      var m = messageModelObj.get(row)
      if (m.is_read) return
      fid = m.folder_id || selectedFolderId
      _setRead(row, true)
      _bumpFolderUnread(fid, -1)
    }
    request("POST", "/messages/read?id=" + encodeURIComponent(id) + "&read=true", function(xhr) {
      if (xhr.status === 200 && xhr.responseText === "ok") return
      if (row < 0) return
      for (var k = 0; k < messageModelObj.count; k++) {
        if (messageModelObj.get(k).id === id) { root._setRead(k, false); break }
      }
      root._bumpFolderUnread(fid, 1)
    })
  }

  // Background refresh: the daemon changes read flags and folder counts on its own
  // (sync, reads done in other clients), so pick those up without disturbing the
  // cursor or scroll position. Rows are patched in place when the loaded ids still
  // line up; if mail arrived or left, the first page is reloaded instead.
  function refreshFolderCounts() {
    request("GET", "/folders", function(xhr) {
      if (xhr.status !== 200) return
      try {
        var folders = JSON.parse(xhr.responseText)
        var same = folders.length === folderModelObj.count
        for (var i = 0; same && i < folders.length; i++)
          if (folders[i].id !== folderModelObj.get(i).id) same = false
        if (!same) { loadFolders(); return }
        var countsChanged = false
        for (var j = 0; j < folders.length; j++) {
          var f = folderModelObj.get(j)
          if (f.unread_item_count !== folders[j].unread_item_count) {
            folderModelObj.setProperty(j, "unread_item_count", folders[j].unread_item_count); countsChanged = true
          }
          if (f.total_item_count !== folders[j].total_item_count) {
            folderModelObj.setProperty(j, "total_item_count", folders[j].total_item_count); countsChanged = true
          }
        }
        if (countsChanged) root._readRev++
      } catch (e) { console.log("[Folders] refresh parse error:", e) }
    })
  }

  function refreshMessages() {
    if (_loadingMessages || messageModelObj.count === 0) return
    var gen = root._messageGeneration
    var n = Math.min(messageModelObj.count, 1000)
    var path = "/messages?limit=" + n + "&offset=0"
    if (root.selectedFolderId !== "") path += "&folder_id=" + encodeURIComponent(root.selectedFolderId)
    if (root.searchQuery !== "") path += "&q=" + encodeURIComponent(root.searchQuery)
    request("GET", path, function(xhr) {
      if (xhr.status !== 200 || gen !== root._messageGeneration || root._loadingMessages) return
      try {
        var rows = JSON.parse(xhr.responseText)
        var aligned = rows.length === n
        for (var i = 0; aligned && i < n; i++)
          if (rows[i].id !== messageModelObj.get(i).id) aligned = false
        if (!aligned) { root.perfMark("refreshMessages: list changed on disk (new mail) -> reload"); if (messageModelObj.count <= root.messagePageSize) loadMessages(); return }
        var changed = false
        for (var j = 0; j < n; j++) {
          if (!!messageModelObj.get(j).is_read !== !!rows[j].is_read) {
            messageModelObj.setProperty(j, "is_read", rows[j].is_read)
            changed = true
          }
        }
        if (changed) root._readRev++
      } catch (e) { console.log("[Messages] refresh parse error:", e) }
    })
  }

  Timer {
    interval: 20000
    running: root.backendOnline
    repeat: true
    onTriggered: { root.refreshFolderCounts(); root.refreshMessages(); root.loadSenders() }
  }

  // ---- backend change signal: the daemon bumps a counter whenever a sync stage changed what
  // the mail UI shows (recent-50 stored, read flags, folder counts, new/older mail). Polling
  // this tiny endpoint (no DB access) makes fresh mail appear within one tick of being
  // stored. Fast (500 ms) for the first 90 s after launch while the stages land, then 3 s.
  property int _syncSerial: -1
  // SMS: bumped when the backend's sms serial moves; the SMS view reloads on it.
  property int smsRev: 0
  property int _smsSerial: -1
  property string smsSearch: ""       // active SMS search (name, number or message text); "" = none
  property bool smsHasThread: false   // the SMS view has a conversation open (the reader pane exists)
  property bool _syncFast: true
  Timer { interval: 90000; running: root.backendOnline; onTriggered: root._syncFast = false }
  Timer {
    interval: root._syncFast ? 500 : 3000
    running: root.backendOnline
    repeat: true
    onTriggered: root.checkSyncSerial()
  }
  function checkSyncSerial() {
    request("GET", "/sync/serial", function(xhr) {
      if (xhr.status !== 200) return
      var n = -1, sms = -1
      try { var j = JSON.parse(xhr.responseText); n = j.mail; sms = (j.sms === undefined ? -1 : j.sms) } catch (e) { return }
      if (sms !== root._smsSerial) { root._smsSerial = sms; root.smsRev++ }
      if (root._syncSerial === -1) { root._syncSerial = n; if (n === 0) return }
      else if (n === root._syncSerial) return
      root._syncSerial = n
      root.perfMark("sync serial " + n + " seen -> refreshing mail UI")
      root.refreshFolderCounts()
      if (messageModelObj.count === 0) root.loadMessages()
      else root.refreshMessages()
    })
  }

  function _messagesPath(offset) {
    var path = "/messages?limit=" + messagePageSize + "&offset=" + offset
    if (root.selectedFolderId !== "") path += "&folder_id=" + encodeURIComponent(root.selectedFolderId)
    if (root.searchQuery !== "") path += "&q=" + encodeURIComponent(root.searchQuery)
    return path
  }

  // Run `text` as a search over the open folder. A pattern the backend rejects leaves the list
  // and the previous search untouched and reports the reason through onError(message).
  function applySearch(text, onError) {
    var q = (text || "").trim()
    if (q === "") { clearSearch(); return }
    var prev = root.searchQuery
    root.searchQuery = q
    loadMessages(function(err) { root.searchQuery = prev; if (onError) onError(err) },
                 function() { root.recordSearch(q) })
  }

  function loadSearchHistory() {
    request("GET", "/search/history", function(xhr) {
      if (xhr.status !== 200) return
      try { root.searchHistory = JSON.parse(xhr.responseText) } catch (e) {}
    })
  }

  // A pattern that ran without error goes to the front of the history (the backend persists it).
  function recordSearch(q) {
    var h = root.searchHistory.filter(function(p) { return p !== q })
    h.unshift(q)
    root.searchHistory = h.slice(0, 50)
    request("POST", "/search/history?q=" + encodeURIComponent(q), null)
  }

  // Back to the plain folder view.
  function clearSearch() {
    if (root.searchQuery === "") return
    root.searchQuery = ""
    root.msgIndex = 0
    loadMessages()
  }

  property bool _perfFirstMessages: true
  function loadMessages(onRejected, onAccepted) {
    if (_perfFirstMessages) perfMark("first loadMessages request sent")
    var priorStatus = root.messagesStatus
    root.messagesStatus = "…"
    var gen = ++root._messageGeneration
    root._loadingMessages = true
    request("GET", _messagesPath(0), function(xhr) {
      if (gen !== root._messageGeneration) return   // folder changed meanwhile
      root._loadingMessages = false
      if (xhr.status === 400 && onRejected) {   // bad search pattern: keep what is on screen
        var why = "bad pattern"
        try { why = JSON.parse(xhr.responseText).error || why } catch (e) {}
        root.messagesStatus = priorStatus
        onRejected(why)
        return
      }
      if (xhr.status === 200) {
        try {
          var messages = JSON.parse(xhr.responseText)
          root._reloadingModel = true
          messageModelObj.clear()
          for (var i = 0; i < messages.length; i++) messageModelObj.append(messages[i])
          root._reloadingModel = false
          if (root.currentMessageId === "") root.loadBody("")
          if (root._perfFirstMessages && messages.length > 0) { root._perfFirstMessages = false; perfMark("EMAILS VISIBLE: model populated with " + messages.length + " rows") }
          else if (messages.length > 0) perfMark("mail list reloaded: " + messages.length + " rows")
          root._hasMoreMessages = messages.length >= root.messagePageSize
          root.searchPartial = root.searchQuery !== "" && xhr.getResponseHeader("X-Search-Truncated") === "1"
          root.messagesStatus = root.searchQuery !== ""
            ? messages.length.toLocaleString(Qt.locale("en_US"), "f", 0) + (root._hasMoreMessages ? "+" : "") + " found"
            : root.messageTotal.toLocaleString(Qt.locale("en_US"), "f", 0)
          if (root.msgIndex >= messageModelObj.count) root.msgIndex = Math.max(0, messageModelObj.count - 1)
          if (onAccepted) onAccepted()
        } catch (e) { root.messagesStatus = "err" }
      } else {
        root.messagesStatus = xhr.status === 0 ? "" : "e" + xhr.status
      }
    })
  }

  // Next page of the open folder, appended below what is already loaded.
  function loadMoreMessages() {
    if (!_hasMoreMessages || _loadingMessages) return
    var gen = root._messageGeneration
    root._loadingMessages = true
    request("GET", _messagesPath(messageModelObj.count), function(xhr) {
      if (gen !== root._messageGeneration) return
      root._loadingMessages = false
      if (xhr.status !== 200) return
      try {
        var messages = JSON.parse(xhr.responseText)
        for (var i = 0; i < messages.length; i++) messageModelObj.append(messages[i])
        root._hasMoreMessages = messages.length >= root.messagePageSize
      } catch (e) { console.log("[Messages] page parse error:", e) }
    })
  }

  property bool _applyingSettings: false

  // Whole-UI scale (Settings -> Appearance). shell.qml scales the window content by this, so
  // a change takes effect at once; the backend clamps and stores it in settings.toml [font].
  property real uiScale: 1.0
  readonly property real minUiScale: 0.5
  readonly property real maxUiScale: 3.0
  function clampUiScale(v) {
    var n = Number(v)
    if (!isFinite(n)) return 1.0
    return Math.round(Math.max(minUiScale, Math.min(maxUiScale, n)) * 100) / 100
  }
  function setUiScale(v) {
    var next = clampUiScale(v)
    if (next === uiScale) return
    var before = uiScale
    uiScale = next
    request("POST", "/settings/scale_factor?value=" + next, function(xhr) {
      var ok = false
      try { var r = JSON.parse(xhr.responseText); ok = r.ok === true; if (ok) root.uiScale = clampUiScale(r.scale_factor) } catch (e) { ok = false }
      if (!ok) root.uiScale = before
    })
  }

  function loadUiSettings() {
    request("GET", "/settings/ui", function(xhr) {
      if (xhr.status !== 200) return
      try {
        var s = JSON.parse(xhr.responseText)
        if (s.message_rendering === "html" || s.message_rendering === "system" || s.message_rendering === "system_sender")
          root.messageRendering = s.message_rendering
        if (typeof s.scale_factor === "number") root.uiScale = root.clampUiScale(s.scale_factor)
        if (typeof s.folder_pane === "boolean") {
          root._applyingSettings = true
          root.showFolderPane = s.folder_pane
          root._applyingSettings = false
        }
        if (typeof s.list_width_offset === "number") {
          root._applyingSettings = true
          root.listWidthOffset = Math.round(s.list_width_offset)
          root._applyingSettings = false
        }
        if (typeof s.calendar_pane_offset === "number") {
          root._applyingSettings = true
          root.calendarPaneOffset = Math.round(s.calendar_pane_offset)
          root._applyingSettings = false
        }
        if (typeof s.sidebar_expanded === "boolean") {
          root._applyingSettings = true
          root.sidebarExpanded = s.sidebar_expanded
          root._applyingSettings = false
        }
      } catch (e) { console.log("[Settings] parse error:", e) }
    })
  }

  onSidebarExpandedChanged: {
    if (!_applyingSettings)
      request("POST", "/settings/sidebar_expanded", null, sidebarExpanded ? "true" : "false")
  }

  onSelectedFolderIdChanged: { searchQuery = ""; clearMarks(); loadMessages(); _saveViewSoon() }

  // ---- remembered view (OL-014) ------------------------------------------------------------
  // Where the user was is kept in ~/.config/omalook/view_state.json (backend: /settings/view_state)
  // and put back on the next start: the module, the mail folder (an "all accounts" row too), the
  // calendar view, the people view and sort, and the task filter. Anything stored that no longer
  // exists (a removed account's folder, an unknown value) is ignored and the default stays.
  // Nothing is saved until the stored state has been read, so the defaults chosen during start-up
  // never overwrite it.
  property bool _viewRestored: false
  property string _wantFolderId: ""
  property string _lastModule: "mail"   // last module that was not Settings (a start in Settings would surprise)
  onCurrentViewChanged: { if (currentView !== "settings") _lastModule = currentView; _saveViewSoon() }
  onCalendarModeChanged: _saveViewSoon()
  onPeopleViewChanged: _saveViewSoon()
  onPeopleSortChanged: _saveViewSoon()
  onTasksFilterChanged: _saveViewSoon()

  Timer { id: viewSaveTimer; interval: 300; onTriggered: root._saveViewState() }
  function _saveViewSoon() { if (_viewRestored) viewSaveTimer.restart() }
  function _saveViewState() {
    request("POST", "/settings/view_state", null, JSON.stringify({
      view: _lastModule, folder: selectedFolderId, calendar_mode: calendarMode, last_calendar_account: lastCalendarAccount,
      people_view: peopleView, people_sort: peopleSort, tasks_filter: tasksFilter }))
  }

  // Select the remembered folder once the folder list has it. Returns true when it was dealt with
  // (selected, or not there at all); false while the list is still empty.
  function _applyWantedFolder() {
    if (_wantFolderId === "") return true
    if (folderModelObj.count === 0) return false
    for (var i = 0; i < folderModelObj.count; i++) {
      if (folderModelObj.get(i).id === _wantFolderId) {
        folderIndex = i
        selectedFolderId = _wantFolderId
        break
      }
    }
    _wantFolderId = ""
    return true
  }

  function loadViewState() {
    request("GET", "/settings/view_state", function(xhr) {
      if (xhr.status === 200) {
        try {
          var s = JSON.parse(xhr.responseText)
          function pick(v, allowed, cur) { return allowed.indexOf(v) >= 0 ? v : cur }
          root.calendarMode = pick(s.calendar_mode, ["day", "workweek", "week", "month"], root.calendarMode)
          if (typeof s.last_calendar_account === "string") root.lastCalendarAccount = s.last_calendar_account
          root.peopleView = pick(s.people_view, ["all", "favorites", "lists"], root.peopleView)
          root.peopleSort = pick(s.people_sort, ["first", "last", "company", "recent"], root.peopleSort)
          root.tasksFilter = pick(s.tasks_filter, ["all", "active", "done"], root.tasksFilter)
          if (typeof s.folder === "string" && s.folder !== "") { root._wantFolderId = s.folder; root._applyWantedFolder() }
          // Only when the user has not moved on in the meantime.
          if (root.currentView === "mail" && typeof s.view === "string" && s.view !== "mail" && s.view !== "settings") {
            for (var i = 0; i < root.navItems.length; i++)
              if (root.navItems[i].view === s.view) { root.setNavIndex(i); break }
          }
        } catch (e) { console.log("[ViewState] parse error:", e) }
      }
      root._viewRestored = true
    })
  }

  // ---- keyboard bindings ([keys] in settings.toml) ---------------------------------------
  // Polled: the backend serves the table with the file's mtime as `serial`, so editing
  // settings.toml rebinds keys within a couple of seconds, no restart.
  property var keyConfig: ({ preset: "", bindings: ({}), error: "" })
  property string _keySerial: ""
  function loadKeyConfig() {
    request("GET", "/settings/keys", function(xhr) {
      if (xhr.status !== 200) return
      try {
        var d = JSON.parse(xhr.responseText)
        if (d.serial === root._keySerial) return
        root._keySerial = d.serial
        root.keyConfig = { preset: d.preset || "", bindings: d.bindings || ({}), error: d.error || "" }
      } catch (e) { console.log("[Keys] parse error:", e) }
    })
  }
  Timer {
    interval: 2000; repeat: true; running: root.backendOnline; triggeredOnStart: true
    onTriggered: root.loadKeyConfig()
  }

  // ---- folder pane ------------------------------------------------------------------------
  // Off by default: folders are reached with the folder picker (`g f`), and the message list
  // titles itself with the folder (and account). `F` brings the pane back.
  property bool showFolderPane: false
  onShowFolderPaneChanged: {
    if (!showFolderPane && focusPane === "folder") focusPane = "msg"
    if (!_applyingSettings) request("POST", "/settings/folder_pane", null, showFolderPane ? "true" : "false")
  }
  function toggleFolderPane() { showFolderPane = !showFolderPane }

  // ---- list / reading pane divider --------------------------------------------------------
  // Message list width in characters, relative to its default (`ui.list_width_offset` in
  // settings.toml). Ctrl-Right / Ctrl-Left move the divider one character. MailView publishes
  // the bounds that keep both panes usable.
  property int listWidthOffset: 0
  property int listWidthMinOffset: -40
  property int listWidthMaxOffset: 40
  // Set false when the reading pane can be switched off: a lone list has no divider to move.
  property bool showReaderPane: true
  readonly property bool canResizeList: currentView === "mail" && showReaderPane
  onListWidthOffsetChanged: {
    if (!_applyingSettings) request("POST", "/settings/list_width_offset", null, String(listWidthOffset))
  }
  function resizeList(dCols) {
    if (!canResizeList) return
    var n = Math.max(listWidthMinOffset, Math.min(listWidthMaxOffset, listWidthOffset + dCols))
    if (n !== listWidthOffset) listWidthOffset = n
  }

  // Calendar sidebar width (OL-031), same idea: characters relative to its default
  // (`ui.calendar_pane_offset` in settings.toml). CalendarView clamps it to what fits and asks for the change.
  property int calendarPaneOffset: 0
  onCalendarPaneOffsetChanged: {
    if (!_applyingSettings) request("POST", "/settings/calendar_pane_offset", null, String(calendarPaneOffset))
  }

  // "Inbox" or, with several accounts, "Inbox · adam@example.com".
  readonly property string selectedFolderLabel: {
    var f = selectedFolder
    if (!f || f.all_accounts) return selectedFolderName
    var accounts = {}, n = 0
    for (var i = 0; i < folderModelObj.count; i++) {
      var e = folderModelObj.get(i).account_email || ""
      if (e !== "" && !accounts[e]) { accounts[e] = true; n++ }
    }
    return n > 1 && f.account_email ? selectedFolderName + " · " + f.account_email : selectedFolderName
  }

  // ---- categories (Exchange) / tags (Gmail) --------------------------------------------
  // `t` opens a picker for the message under the cursor (or the marked ones): toggle an
  // existing category, or make a new one with a colour. Names sync with the provider; the
  // account's category list comes from /categories.
  property var catDefs: []             // [{ name, color, pending }] of catAccountId
  property var catPalette: []          // [{ key, label, hex }]
  property bool catMasterList: true    // false: Exchange will not show/colour categories until the account signs in again
  property string catAccountId: ""
  property string pendingCatName: ""   // new category waiting for its colour

  function targetAccountId() { var m = currentMessage; return m ? String(m.account_id || "") : "" }

  function accountProvider(id) {
    for (var i = 0; i < accounts.length; i++) if (accounts[i].id === id) return accounts[i].provider
    return ""
  }

  // What the provider calls them: Exchange categories, Gmail labels (shown as tags), others labels.
  function tagNoun(id) {
    var p = accountProvider(id !== undefined ? id : targetAccountId())
    return p === "gmail" ? "tag" : (p === "exchange" || p === "" ? "category" : "label")
  }

  function loadCategories(accountId, done) {
    catAccountId = accountId
    request("GET", "/categories?account=" + encodeURIComponent(accountId), function(xhr) {
      if (xhr.status !== 200) return
      try {
        var d = JSON.parse(xhr.responseText)
        catDefs = d.categories || []
        catPalette = d.palette || []
        catMasterList = d.master_list !== false
      } catch (e) { console.log("[Categories] parse error:", e) }
      if (done) done()
    })
  }

  function _rowsFor(ids) {
    var set = {}, out = []
    ids.forEach(function(id) { set[id] = true })
    for (var i = 0; i < messageModelObj.count; i++) {
      var r = messageModelObj.get(i)
      if (set[r.id]) out.push({ index: i, row: r })
    }
    return out
  }

  function _catsOf(row) { return Fmt.categories(row) }

  // "all" | "some" | "none": how many of the target messages carry `name`.
  function tagState(name) {
    var rows = _rowsFor(actionTargets())
    if (rows.length === 0) return "none"
    var n = 0, lower = name.toLowerCase()
    rows.forEach(function(x) { if (_catsOf(x.row).some(function(c) { return String(c[0]).toLowerCase() === lower })) n++ })
    return n === rows.length ? "all" : (n === 0 ? "none" : "some")
  }

  function _catColor(name) {
    var lower = name.toLowerCase()
    for (var i = 0; i < catDefs.length; i++) if (String(catDefs[i].name).toLowerCase() === lower) return catDefs[i].color
    return "#8a8886"
  }

  // Everything has it -> take it off all; otherwise put it on all.
  function toggleTag(name) {
    var ids = actionTargets()
    if (ids.length === 0) return
    var rows = _rowsFor(ids)
    var remove = tagState(name) === "all"
    var lower = name.toLowerCase(), color = _catColor(name)
    var noun = tagNoun()
    rows.forEach(function(x) {
      var list = _catsOf(x.row).filter(function(c) { return String(c[0]).toLowerCase() !== lower })
      if (!remove) list.push([name, color])
      messageModelObj.setProperty(x.index, "cats", JSON.stringify(list))
    })
    request("POST", "/messages/category?ids=" + encodeURIComponent(ids.join(",")) + "&op=" + (remove ? "remove" : "add")
            + "&name=" + encodeURIComponent(name), function(xhr) {
      var res = null
      try { res = JSON.parse(xhr.responseText) } catch (e) {}
      if (xhr.status !== 200 || !res || res.error !== undefined) {
        notify("Could not change " + noun + ": " + ((res && res.error) ? res.error : "backend did not answer"))
        loadMessages()
      }
    })
    notify((remove ? "Removed " : "Added ") + noun + " \u201c" + name + "\u201d" + (ids.length > 1 ? " on " + ids.length + " messages" : ""))
  }

  // A new category: no colour step when Exchange cannot take colours yet.
  function beginNewCategory(name) {
    pendingCatName = name
    if (!catMasterList && accountProvider(targetAccountId()) === "exchange") createCategory(name, "blue")
    else focusPaletteRequested("tagcolor ")
  }
  signal focusPaletteRequested(string prefill)

  function createCategory(name, colorKey) {
    var acct = targetAccountId()
    pendingCatName = ""
    request("POST", "/categories/create?account=" + encodeURIComponent(acct) + "&name=" + encodeURIComponent(name)
            + "&color=" + encodeURIComponent(colorKey), function(xhr) {
      var res = null
      try { res = JSON.parse(xhr.responseText) } catch (e) {}
      if (xhr.status !== 200 || !res || res.error !== undefined) {
        notify("Could not create " + tagNoun(acct) + ": " + ((res && res.error) ? res.error : "backend did not answer"))
        return
      }
      var hex = "#8a8886"
      for (var i = 0; i < catPalette.length; i++) if (catPalette[i].key === colorKey) hex = catPalette[i].hex
      var defs = catDefs.slice()
      defs.push({ name: name, color: hex, pending: true })
      catDefs = defs
      toggleTag(name)
    })
  }

  // ---- one-time repopulate of message details (progress) ---------------------------------
  property bool metaActive: false
  property int metaRemaining: 0
  Timer {
    interval: 4000; repeat: true; running: root.backendOnline; triggeredOnStart: true
    onTriggered: root.request("GET", "/messages/meta_progress", function(xhr) {
      if (xhr.status !== 200) return
      try {
        var d = JSON.parse(xhr.responseText)
        var wasActive = root.metaActive
        root.metaActive = d.active === true && d.remaining > 0
        root.metaRemaining = d.remaining
        if (wasActive && !root.metaActive) root.loadMessages()   // the details are in: show them
      } catch (e) {}
    })
  }

  // ---- marks and mailbox actions (archive / delete / move) -------------------------------
  // `v` marks messages; actions apply to the marked ones, or to the message under the cursor
  // when nothing is marked. The rows vanish at once; the backend holds the change for the mail
  // "send delay" (Settings -> Mail) so `u` can take it back, then pushes it to the provider.
  signal notify(string text)
  property var marks: ({})
  property int markCount: 0
  property int _markAnchor: -1

  function _setMarks(m) { marks = m; markCount = Object.keys(m).length }
  function clearMarks() { if (markCount > 0) _setMarks({}); _markAnchor = -1 }
  function _copyMarks() { var m = {}; for (var k in marks) m[k] = true; return m }

  function toggleMark(index) {
    var i = index === undefined ? msgIndex : index
    if (i < 0 || i >= messageModelObj.count) return
    var id = messageModelObj.get(i).id
    var m = _copyMarks()
    if (m[id]) delete m[id]; else m[id] = true
    _setMarks(m)
    _markAnchor = i
  }

  // Mark everything between the last marked row and the cursor.
  function markRange() {
    var a = _markAnchor >= 0 ? _markAnchor : msgIndex
    var lo = Math.min(a, msgIndex), hi = Math.max(a, msgIndex)
    var m = _copyMarks()
    for (var i = lo; i <= hi && i < messageModelObj.count; i++) m[messageModelObj.get(i).id] = true
    _setMarks(m)
  }

  function markAll() {
    var m = {}
    for (var i = 0; i < messageModelObj.count; i++) m[messageModelObj.get(i).id] = true
    _setMarks(m)
  }

  // Ids an action applies to, in list order.
  function actionTargets() {
    var ids = []
    if (markCount > 0) {
      for (var i = 0; i < messageModelObj.count; i++) {
        var id = messageModelObj.get(i).id
        if (marks[id]) ids.push(id)
      }
    } else if (currentMessageId !== "") ids.push(currentMessageId)
    return ids
  }

  readonly property string selectedFolderKind: selectedFolder ? String(selectedFolder.well_known_name || "").toLowerCase() : ""
  readonly property bool inTrashFolder: selectedFolderKind === "deleteditems"

  // The undo window of the latest action: { ids, label, unread, until (ms) }, or null.
  property var lastAction: null
  property real actionNow: Date.now()
  Timer {
    interval: 250; repeat: true; running: root.lastAction !== null
    onTriggered: {
      root.actionNow = Date.now()
      if (root.lastAction && root.actionNow >= root.lastAction.until) root.lastAction = null
    }
  }

  // Pending yes/no question (permanent delete): { message, confirmText, run }, or null.
  property var pendingConfirm: null

  function _noun(n) { return n === 1 ? "1 message" : n + " messages" }

  function archiveSelected() {
    if (selectedFolderKind === "archive") { notify("Already in Archive"); return }
    runAction("archive", "", "")
  }

  // `x`: Deleted Items / Trash -> ask, then delete for good; anywhere else -> move to Trash.
  function deleteSelected() {
    var ids = actionTargets()
    if (ids.length === 0) return
    if (inTrashFolder) {
      pendingConfirm = {
        message: "Delete " + _noun(ids.length) + " permanently? This cannot be undone.",
        confirmText: "Delete",
        run: function() { runAction("delete", "", "") }
      }
      return
    }
    runAction("trash", "", "")
  }

  function moveSelected(destId, destName) { runAction("move", destId, destName) }

  function runAction(op, dest, destName) {
    var ids = actionTargets()
    if (ids.length === 0) return
    var verb = op === "archive" ? "Archived" : op === "trash" ? "Deleted" : op === "delete" ? "Permanently deleted"
             : "Moved to " + destName
    var set = {}
    ids.forEach(function(id) { set[id] = true })

    // Hide the rows now; the cursor keeps its place so the next message takes over.
    var unread = 0, unreadBy = {}
    for (var i = messageModelObj.count - 1; i >= 0; i--) {
      var row = messageModelObj.get(i)
      if (!set[row.id]) continue
      if (!row.is_read) {
        unread++
        var rf = row.folder_id || selectedFolderId
        unreadBy[rf] = (unreadBy[rf] || 0) + 1
      }
      messageModelObj.remove(i)
    }
    for (var uf in unreadBy) _bumpFolderUnread(uf, -unreadBy[uf])
    if (msgIndex >= messageModelObj.count) msgIndex = Math.max(0, messageModelObj.count - 1)
    clearMarks()

    var path = "/messages/action?ids=" + encodeURIComponent(ids.join(",")) + "&op=" + op
             + (op === "move" ? "&dest=" + encodeURIComponent(dest) : "")
    request("POST", path, function(xhr) {
      var res = null
      try { res = JSON.parse(xhr.responseText) } catch (e) {}
      if (xhr.status !== 200 || !res || res.error !== undefined) {
        notify("Could not " + op + ": " + ((res && res.error) ? res.error : "backend did not answer"))
        loadMessages()
        loadFolders()
        return
      }
      if (res.delay_secs > 0)
        root.lastAction = { ids: ids, label: verb + " " + _noun(ids.length), unread: unread, unreadBy: unreadBy, until: Date.now() + res.delay_secs * 1000 }
      else root.lastAction = null
      root.actionNow = Date.now()
    })
  }

  // `u`: take back the latest action while its window is open. True when there was one.
  function undoAction() {
    var a = lastAction
    if (!a) return false
    lastAction = null
    request("POST", "/messages/action/undo?ids=" + encodeURIComponent(a.ids.join(",")), function(xhr) {
      for (var uf in a.unreadBy) _bumpFolderUnread(uf, a.unreadBy[uf])
      loadMessages()
      notify("Restored " + _noun(a.ids.length))
    })
    return true
  }

  // Backend may still be starting: retry until it answers, then load once.
  Timer {
    interval: 1000
    running: true
    repeat: true
    onTriggered: {
      if (root.backendOnline) { stop(); return }
      root.loadUiSettings()
      root.loadFolders()
      root.loadMessages()
      root.loadSenders()
    }
  }

  // ------------------------------------------------------- navigation intents
  function setNavIndex(i) {
    navIndex = i
    currentView = navItems[i].view
  }

  function drillIn() {
    if (currentView === "mail" && showFolderPane) { focusPane = "folder"; folderIndex = Math.max(0, folderIndex) }
    else if (currentView === "mail") focusPane = "msg"
    else { focusPane = "msg"; msgIndex = 0 }
  }

  function selectFolderAt(i) {
    if (i >= 0 && i < folderModelObj.count) selectedFolderId = folderModelObj.get(i).id
  }

  function openSettingsCategory(id) {
    for (var i = 0; i < settingsCategories.length; i++)
      if (settingsCategories[i].id === id) settingsCategoryIndex = i
    for (var j = 0; j < navItems.length; j++)
      if (navItems[j].view === "settings") setNavIndex(j)
    focusPane = "msg"
    focusRequested()
  }

  // Mouse intents ------------------------------------------------------
  function clickFolder(i)     { folderIndex = i; selectFolderAt(i); focusPane = "msg"; msgIndex = 0; focusRequested() }
  function clickMessage(i)    { focusPane = "msg"; msgIndex = i; focusRequested() }
  function clickCategory(i)   { settingsCategoryIndex = i; focusPane = "msg"; focusRequested() }

  // Keyboard intents (called from PanelKeyCatcher) ---------------------
  function moveCursor(dx, dy) {
    if (dy !== 0) moveVertical(dy)
    else if (dx > 0) moveInto()
    else if (dx < 0) back(true)
  }

  function moveVertical(dy) {
    if (focusPane === "folder") {
      folderIndex = Math.max(0, Math.min(folderIndex + dy, folderModelObj.count - 1))
      selectFolderAt(folderIndex)
    } else if (focusPane === "msg") {
      if (currentView === "settings")
        settingsCategoryIndex = Math.max(0, Math.min(settingsCategoryIndex + dy, settingsCategories.length - 1))
      else if (currentView === "mail")
        msgIndex = Math.max(0, Math.min(msgIndex + dy, messageModelObj.count - 1))
    }
  }

  function moveInto() {
    if (focusPane === "folder") {
      selectFolderAt(folderIndex)
      focusPane = "msg"
      msgIndex = 0
    } else if (focusPane === "msg" && currentView === "mail" && currentMessage) {
      focusPane = "reader"
    } else if (focusPane === "msg" && currentView === "sms" && smsHasThread) {
      focusPane = "reader"
    }
  }

  // Does the view have something to show in its reader pane?
  readonly property bool readerAvailable: currentView === "sms" ? smsHasThread : !!currentMessage

  function activate() { moveInto() }

  // `h` / Esc: one pane to the left; stops at the leftmost pane. Never closes the window (that is `Q`).
  function back() {
    if (focusPane === "reader") focusPane = "msg"
    else if (focusPane === "msg" && currentView === "mail" && showFolderPane) focusPane = "folder"
  }

  // Panes that exist in the current view, left to right.
  function panesInView() {
    if (currentView === "sms") return ["msg", "reader"]
    if (currentView !== "mail") return ["msg"]
    return showFolderPane ? ["folder", "msg", "reader"] : ["msg", "reader"]
  }

  // Put the cursor in `pane` (no-op if the view has no such pane). Entering the
  // message list from outside resets nothing, so the cursor stays where it was.
  function focusPaneNamed(pane) {
    if (panesInView().indexOf(pane) < 0) return
    if (pane === "folder") folderIndex = Math.max(0, folderIndex)
    if (pane === "reader" && !readerAvailable) return
    focusPane = pane
  }

  // Tab / Shift-Tab.
  function cyclePane(dir) {
    var panes = panesInView()
    var i = panes.indexOf(focusPane)
    if (i < 0) i = 0
    for (var n = 1; n <= panes.length; n++) {
      var next = panes[(i + dir * n + panes.length * n) % panes.length]
      if (next === "reader" && !readerAvailable) continue
      focusPaneNamed(next)
      return
    }
  }

  // Jump to folder `i` of the folder model and land in its message list.
  function gotoFolder(i) {
    gotoView("mail")
    clickFolder(i)
  }

  // `g` + letter: go to a module and land in its main pane.
  function gotoView(view) {
    for (var i = 0; i < navItems.length; i++) {
      if (navItems[i].view !== view) continue
      setNavIndex(i)
      if (view !== "mail") msgIndex = 0
      focusPane = "msg"
      return
    }
  }

  // `s` (old key): folder -> list, otherwise same as Tab.
  function cycleFocus() {
    if (focusPane === "folder") { selectFolderAt(folderIndex); focusPane = "msg"; msgIndex = 0 }
    else cyclePane(1)
  }
}
