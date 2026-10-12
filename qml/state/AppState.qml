import QtQuick
import Quickshell
import "../mail/format.js" as Fmt

// All non-visual state: navigation + focus model, folder/message data, and the
// backend (Rust daemon, local HTTP) calls. Components read properties and call
// the intent functions below; none of them talk to the backend themselves.
Item {
  id: root

  // ------------------------------------------------------------- backend
  readonly property string backendUrl: "http://127.0.0.1:27182"
  readonly property string configDir: {
    var d = Quickshell.env("CONFIG_DIR")
    return (d && d.length > 0) ? d : Quickshell.env("HOME") + "/.config/omalook"
  }
  property bool backendOnline: false
  onBackendOnlineChanged: {
    if (backendOnline) { perfMark("backend first answered"); loadCalendarSettings(); loadUiSettings(); loadSenders(); loadMailSettings(); loadSearchHistory() }
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
    { id: "appearance",    icon: "\uf1fc", label: "Appearance"    },
    { id: "mail",          icon: "\uf0e0", label: "Mail"          },
    { id: "senders",       icon: "\uf2bd", label: "Senders"       },
    { id: "calendar",      icon: "\uf073", label: "Calendar"      },
    { id: "sms",           icon: "\uf27a", label: "SMS"           },
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

  function toggleHtml() {
    if (currentMessageId === "") return
    _htmlOverride[currentMessageId] = !currentHtmlMode
    _htmlRev++
  }

  // scope: "message" | "sender" (list the sender with always-HTML on).
  function viewAsHtml(scope) {
    var id = currentMessageId
    if (id === "") return
    _htmlOverride[id] = true
    _htmlRev++
    if (scope === "sender") _listSenderWith(currentSenderEmail, "html")
  }

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
  onCurrentMessageIdChanged: {
    var id = currentMessageId
    if (id === "" || _bodyCache[id]) { _bodyDebounce.stop(); loadBody(id); loadInvite(id); loadAttachments(id); return }
    currentBody = { id: id, state: "loading", type: "", content: "" }
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
    if (id === "") { currentBody = { id: "", state: "idle", type: "", content: "" }; return }
    var hit = _bodyCache[id]
    if (hit) { currentBody = { id: id, state: "ready", type: hit.type, content: hit.content }; return }
    currentBody = { id: id, state: "loading", type: "", content: "" }
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
      root.currentBody = ok
        ? { id: id, state: "ready", type: result.type, content: result.content }
        : { id: id, state: "error", type: "", content: "",
            reason: (result && result.error) ? String(result.error) : "could not load the message body",
            preview: (result && result.preview) ? String(result.preview) : "" }
    })
  }

  // ---- per-sender preferences (Settings → Senders) --------------------------
  // [{ email, always_html, always_images }] from /settings/senders. Stored only;
  // nothing in the reading pane consults them yet.
  readonly property alias senderModel: senderModelObj
  ListModel { id: senderModelObj }
  property string senderNotice: ""

  function loadSenders() {
    request("GET", "/settings/senders", function(xhr) {
      if (xhr.status !== 200) return
      try {
        var rows = JSON.parse(xhr.responseText)
        if (!Array.isArray(rows)) return
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

  // scope: "message" = just this one (session only); "sender" = also list the sender
  // with always-load-images on.
  function unblockImages(scope) {
    var id = currentMessageId
    if (id === "") return
    _imagesUnblocked[id] = true     // either way, load them now
    _imagesRev++
    if (scope === "sender") _listSenderWith(currentSenderEmail, "images")
  }

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

  function applyCalendarSettings(data) {
    recurrenceYearsBack = data.recurrence_years_back
    recurrenceYearsAhead = data.recurrence_years_ahead
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

  Timer {
    id: calendarSettingsSave
    interval: 600
    onTriggered: root.request("POST",
      "/settings/calendar?back=" + root.recurrenceYearsBack + "&ahead=" + root.recurrenceYearsAhead,
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

  function loadFolders() {
    request("GET", "/folders", function(xhr) {
      if (xhr.status !== 200) return
      try {
        var folders = JSON.parse(xhr.responseText)
        folderModelObj.clear()
        for (var i = 0; i < folders.length; i++) folderModelObj.append(folders[i])
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
        // Reply-all keeps the original To list; the backend phase will drop our own address.
        if (kind === "replyAll") init.cc = String(m.to_recipients || m.to || "")
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
        for (var j = 0; j < folders.length; j++) {
          var f = folderModelObj.get(j)
          if (f.unread_item_count !== folders[j].unread_item_count)
            folderModelObj.setProperty(j, "unread_item_count", folders[j].unread_item_count)
          if (f.total_item_count !== folders[j].total_item_count)
            folderModelObj.setProperty(j, "total_item_count", folders[j].total_item_count)
        }
        root._readRev++
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
      var n = -1
      try { n = JSON.parse(xhr.responseText).mail } catch (e) { return }
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
          messageModelObj.clear()
          for (var i = 0; i < messages.length; i++) messageModelObj.append(messages[i])
          if (root._perfFirstMessages && messages.length > 0) { root._perfFirstMessages = false; perfMark("EMAILS VISIBLE: model populated with " + messages.length + " rows") }
          else if (messages.length > 0) perfMark("mail list reloaded: " + messages.length + " rows")
          root._hasMoreMessages = messages.length >= root.messagePageSize
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

  function loadUiSettings() {
    request("GET", "/settings/ui", function(xhr) {
      if (xhr.status !== 200) return
      try {
        var s = JSON.parse(xhr.responseText)
        if (s.message_rendering === "html" || s.message_rendering === "system" || s.message_rendering === "system_sender")
          root.messageRendering = s.message_rendering
        if (typeof s.folder_pane === "boolean") {
          root._applyingSettings = true
          root.showFolderPane = s.folder_pane
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

  onSelectedFolderIdChanged: { searchQuery = ""; clearMarks(); loadMessages() }

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
    }
  }

  function activate() { moveInto() }

  // `h` / Esc: one pane to the left; stops at the leftmost pane. Never closes the window (that is `Q`).
  function back() {
    if (focusPane === "reader") focusPane = "msg"
    else if (focusPane === "msg" && currentView === "mail" && showFolderPane) focusPane = "folder"
  }

  // Panes that exist in the current view, left to right.
  function panesInView() {
    if (currentView !== "mail") return ["msg"]
    return showFolderPane ? ["folder", "msg", "reader"] : ["msg", "reader"]
  }

  // Put the cursor in `pane` (no-op if the view has no such pane). Entering the
  // message list from outside resets nothing, so the cursor stays where it was.
  function focusPaneNamed(pane) {
    if (panesInView().indexOf(pane) < 0) return
    if (pane === "folder") folderIndex = Math.max(0, folderIndex)
    if (pane === "reader" && !currentMessage) return
    focusPane = pane
  }

  // Tab / Shift-Tab.
  function cyclePane(dir) {
    var panes = panesInView()
    var i = panes.indexOf(focusPane)
    if (i < 0) i = 0
    for (var n = 1; n <= panes.length; n++) {
      var next = panes[(i + dir * n + panes.length * n) % panes.length]
      if (next === "reader" && !currentMessage) continue
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
