import QtQuick
import QtQuick.Layouts
import qs.Commons
import "../common"
import "sms.js" as Sms

// SMS: senders list (left) + conversation pane (right), read from the local SMS cache
// (`GET /sms/*`, filled from the paired phone by the backend's KDE Connect daemon).
//
// Keys: j/k, PageUp/PageDown, g/G move through senders; l / Enter opens the conversation
// pane, where j/k and paging scroll the history and h / Esc go back.
//   z  mark the whole conversation read / unread (like mail: opening a thread does not read it)
//   r  (or i) reply: the composer under the history; Ctrl+Enter sends, Esc leaves
//   c / Ctrl+N  new conversation (name or number, then the text)
//   /  search conversations by name, number or message text; Esc clears
//   R  retry texts that failed to send
RowLayout {
  id: root

  required property var app

  spacing: Style.spacing.sm

  readonly property int pageSize: 100

  property string currentKey: ""      // thread under the cursor in the senders list
  property int threadIndex: -1
  property string openKey: ""         // thread whose history the conversation pane shows
  property var openThread: null       // its row in the senders model (title, device_id, thread_id)
  property bool loadingMsgs: false
  property bool hasMore: false
  property int unreadTotal: 0
  property string listStatus: "…"
  property int _msgSerial: 0
  property var drafts: ({})           // thread key -> unsent text, kept while you look at other threads
  property string pendingSelect: ""   // thread key to open once the list has it (after a new conversation)

  // A composer (reply line or new conversation) owns the keyboard while it has focus.
  readonly property bool editing: conv.composerFocused || newThread.opened

  // ---- key registry hooks (AppCommands) ---------------------------------
  function keyMove(dx, dy) {
    if (dy === 0) return false
    select(threadIndex + dy)
    return true
  }
  function keyPage(frac) { select(threadIndex + (frac < 0 ? -1 : 1) * Math.max(1, Math.round(threads.pageRows * Math.abs(frac)))) }
  function keyEdge(bottom) { select(bottom ? threadModel.count - 1 : 0) }
  function scrollReader(px) { conv.scrollBy(px) }
  function pageReader(frac) { conv.scrollPages(frac) }
  function readerEdge(bottom) { conv.scrollEdge(bottom) }

  // The conversation a thread-level key (z, r, R) acts on: the open one while the cursor is in it,
  // else the one under the cursor in the list.
  function targetThread() {
    if (app.focusPane === "reader" && openThread) return openThread
    return threadIndex >= 0 ? threadModel.get(threadIndex) : null
  }

  function toggleRead() {
    var t = targetThread()
    if (!t) return
    var read = t.unread_count > 0
    var idx = indexOfKey(Sms.threadKey(t))
    var before = t.unread_count
    // Immediately, like mail; reloaded from the backend right after.
    if (idx >= 0) {
      threadModel.setProperty(idx, "unread_count", read ? 0 : 1)
      unreadTotal = Math.max(0, unreadTotal + (read ? -before : 1))
    }
    app.request("POST", "/sms/read?thread=" + t.thread_id + "&device=" + encodeURIComponent(t.device_id) + "&read=" + read, function(xhr) {
      var ok = xhr.status === 200
      try { ok = ok && !JSON.parse(xhr.responseText).error } catch (e) { ok = false }
      if (!ok) app.notify("Could not change the read state")
      loadThreads()
    })
  }

  // r / i: write in the open conversation (opening the one under the cursor first).
  function replyHere() {
    if (threadIndex < 0 && !openThread) return
    if (app.focusPane !== "reader" || !openThread) { openTimer.stop(); openConversation() }
    if (!openThread) return
    app.smsHasThread = true
    app.focusPane = "reader"
    conv.focusComposer()
  }

  function newConversation() {
    app.smsHasThread = true
    app.focusPane = "reader"
    newThread.open()
  }

  function retryFailed() {
    var t = targetThread()
    if (!t) return
    app.request("POST", "/sms/retry?thread=" + t.thread_id + "&device=" + encodeURIComponent(t.device_id), function(xhr) {
      var r = null
      try { r = JSON.parse(xhr.responseText) } catch (e) {}
      if (r && r.sent === 0 && r.failed === 0) app.notify("Nothing to retry here")
      else if (r && r.failed > 0) app.notify("Send failure: the phone did not take the text")
      else app.notify("Sending again\u2026")
      loadThreads(); if (openKey === Sms.threadKey(t)) loadNewest(false)
    })
  }

  // `/`: search by name, number or message text ("" clears).
  function applySearch(q) {
    app.smsSearch = q.trim()
    currentKey = ""; threadIndex = -1
    loadThreads()
  }

  // ---- senders list ------------------------------------------------------
  ListModel { id: threadModel }
  ListModel { id: msgModel }

  function select(i) {
    if (threadModel.count === 0) return
    i = Math.max(0, Math.min(i, threadModel.count - 1))
    threadIndex = i
    var t = threadModel.get(i)
    currentKey = Sms.threadKey(t)
    openTimer.restart()      // j/k through many threads must not fetch each one
  }

  function indexOfKey(key) {
    for (var i = 0; i < threadModel.count; i++) if (Sms.threadKey(threadModel.get(i)) === key) return i
    return -1
  }

  // Bring the model in line with `rows` using inserts/moves/updates, so scroll position
  // and delegates survive a refresh (a thread that gets a new message just moves to the top).
  function applyThreads(rows) {
    var unread = 0
    for (var i = 0; i < rows.length; i++) {
      var r = rows[i]
      unread += r.unread_count
      var row = { device_id: r.device_id, thread_id: r.thread_id, title: r.title || "", last_body: r.last_body,
                  last_direction: r.last_direction, last_date_ms: r.last_date_ms,
                  unread_count: r.unread_count, message_count: r.message_count,
                  snippet: r.snippet || "", match: r.match || "" }
      var key = Sms.threadKey(row)
      if (i < threadModel.count && Sms.threadKey(threadModel.get(i)) === key) {
        threadModel.set(i, row)
        continue
      }
      var j = -1
      for (var k = i + 1; k < threadModel.count; k++) if (Sms.threadKey(threadModel.get(k)) === key) { j = k; break }
      if (j >= 0) { threadModel.move(j, i, 1); threadModel.set(i, row) }
      else threadModel.insert(i, row)
    }
    if (threadModel.count > rows.length) threadModel.remove(rows.length, threadModel.count - rows.length)
    unreadTotal = unread
    listStatus = rows.length === 0 && app.smsSearch !== "" ? "No conversation matches \u201c" + app.smsSearch + "\u201d."
      : rows.length === 0 ? "No text messages cached yet.\nPair your phone with KDE Connect (Settings \u203a SMS); history is pulled automatically." : ""

    if (pendingSelect !== "") {
      var pi = indexOfKey(pendingSelect)
      if (pi >= 0) { pendingSelect = ""; select(pi); openTimer.stop(); openConversation(); return }
    }
    if (currentKey === "" && threadModel.count > 0) { select(0); return }
    // A conversation started from here lives under a stand-in id until the phone reports the real one
    // (the same title): follow it, so the open conversation does not go blank.
    if (openThread && openThread.thread_id < 0 && indexOfKey(openKey) < 0) {
      for (var ri = 0; ri < threadModel.count; ri++) {
        var rt = threadModel.get(ri)
        if (rt.thread_id > 0 && rt.title === openThread.title && rt.device_id === openThread.device_id) {
          currentKey = Sms.threadKey(rt); threadIndex = ri; openKey = ""; openConversation(); return
        }
      }
    }
    var idx = indexOfKey(currentKey)
    if (idx >= 0) {
      threadIndex = idx
      var t = threadModel.get(idx)
      if (openKey === currentKey) openThread = t
    } else if (threadModel.count > 0) select(Math.min(Math.max(threadIndex, 0), threadModel.count - 1))
  }

  function loadThreads() {
    var q = app.smsSearch
    app.request("GET", "/sms/threads?limit=2000" + (q !== "" ? "&q=" + encodeURIComponent(q) : ""), function(xhr) {
      if (q !== app.smsSearch) return          // an older search's answer
      if (xhr.status !== 200) {
        if (threadModel.count === 0) listStatus = app.backendOnline ? "could not load messages" : "backend offline"
        return
      }
      try {
        var rows = JSON.parse(xhr.responseText)
        if (rows.error) { listStatus = rows.error; return }
        applyThreads(rows)
      } catch (e) { console.log("[SMS] threads parse error:", e) }
    })
  }

  // ---- conversation ------------------------------------------------------
  function msgPath(before) {
    var p = "/sms/messages?thread=" + openThread.thread_id + "&device=" + encodeURIComponent(openThread.device_id) + "&limit=" + pageSize
    if (before) p += "&before=" + before
    return p
  }

  function rowFor(m) {
    return { device_id: m.device_id, uid: m.uid, mms: m.mms, body: m.body, date_ms: m.date_ms,
             direction: m.direction, is_read: m.is_read, att_text: Sms.attachmentText(m.attachments) }
  }

  function openConversation() {
    if (currentKey === openKey) return
    if (openKey !== "") drafts[openKey] = conv.draft
    openKey = currentKey
    conv.setDraft(drafts[openKey] || "")
    conv.sendError = ""
    openThread = threadIndex >= 0 ? threadModel.get(threadIndex) : null
    app.smsHasThread = openThread !== null || newThread.opened
    msgModel.clear()
    hasMore = false
    _msgSerial++
    if (openThread) loadNewest(true)
  }

  // Newest page. `fresh`: a just-opened thread (jump to the end); otherwise merge into what is shown.
  function loadNewest(fresh) {
    if (!openThread) return
    var serial = _msgSerial, key = openKey
    if (fresh) loadingMsgs = true
    app.request("GET", msgPath(0), function(xhr) {
      if (serial !== _msgSerial || key !== openKey) return
      loadingMsgs = false
      if (xhr.status !== 200) return
      var rows
      try { rows = JSON.parse(xhr.responseText) } catch (e) { return }
      if (!Array.isArray(rows)) return
      rows.reverse()                         // API is newest first; the pane is oldest first
      if (fresh || msgModel.count === 0) {
        msgModel.clear()
        for (var i = 0; i < rows.length; i++) msgModel.append(rowFor(rows[i]))
        hasMore = rows.length >= pageSize
        stickTimer.restart()
      } else {
        var wasAtEnd = conv.atBottom
        // A text of mine that was waiting for the phone (negative uid) is replaced by the phone's copy
        // (or has gone): drop it before merging so the real one lands below the others.
        var onServer = {}
        for (var q = 0; q < rows.length; q++) onServer[Sms.messageKey(rows[q])] = true
        for (var p = msgModel.count - 1; p >= 0; p--)
          if (msgModel.get(p).uid < 0 && !onServer[Sms.messageKey(msgModel.get(p))]) msgModel.remove(p)
        var have = {}
        for (var k = 0; k < msgModel.count; k++) have[Sms.messageKey(msgModel.get(k))] = k
        var added = false
        for (var j = 0; j < rows.length; j++) {
          var idx = have[Sms.messageKey(rows[j])]
          if (idx !== undefined) {
            if (msgModel.get(idx).is_read !== rows[j].is_read) msgModel.setProperty(idx, "is_read", rows[j].is_read)
            if (msgModel.get(idx).direction !== rows[j].direction) msgModel.setProperty(idx, "direction", rows[j].direction)
            if (msgModel.get(idx).body !== rows[j].body) msgModel.setProperty(idx, "body", rows[j].body)
          } else if (msgModel.count === 0 || rows[j].date_ms >= msgModel.get(msgModel.count - 1).date_ms) {
            msgModel.append(rowFor(rows[j])); added = true
          }
        }
        if (added && wasAtEnd) stickTimer.restart()
      }
    })
  }

  function loadOlder() {
    if (!openThread || loadingMsgs || !hasMore || msgModel.count === 0) return
    var serial = _msgSerial, key = openKey
    loadingMsgs = true
    app.request("GET", msgPath(msgModel.get(0).date_ms), function(xhr) {
      if (serial !== _msgSerial || key !== openKey) return
      loadingMsgs = false
      if (xhr.status !== 200) return
      var rows
      try { rows = JSON.parse(xhr.responseText) } catch (e) { return }
      if (!Array.isArray(rows)) return
      hasMore = rows.length >= pageSize
      var have = {}
      for (var k = 0; k < msgModel.count; k++) have[Sms.messageKey(msgModel.get(k))] = true
      rows = rows.filter(function(m) { return !have[Sms.messageKey(m)] })   // newest first
      // Inserting newest-first at index 0 leaves the block oldest-first.
      for (var i = 0; i < rows.length; i++) msgModel.insert(0, rowFor(rows[i]))
      if (rows.length > 0) Qt.callLater(function() { conv.keepAfterPrepend(rows.length) })
    })
  }

  // Like mail, opening a conversation does not read it: `z` does (toggleRead).

  function sendReply(text) {
    if (!openThread) return
    var key = openKey
    conv.sending = true; conv.sendError = ""
    app.request("POST", "/sms/send", function(xhr) {
      conv.sending = false
      var r = null
      try { r = JSON.parse(xhr.responseText) } catch (e) {}
      if (!r) { conv.sendError = "the backend did not answer"; return }
      if (key === openKey) {
        if (r.ok) { conv.clearDraft(); drafts[key] = "" }
        else conv.sendError = r.error || "unknown error"
      }
      loadThreads(); if (key === openKey) { loadNewest(false); stickTimer.restart() }
    }, JSON.stringify({ device: openThread.device_id, thread: openThread.thread_id, text: text }))
  }

  Timer { id: openTimer; interval: 120; onTriggered: root.openConversation() }
  Timer { id: stickTimer; interval: 60; onTriggered: conv.toBottom() }
  // The backend's change counter normally drives refreshes (smsRev); this is only a safety net.
  Timer { interval: 30000; running: true; repeat: true; onTriggered: { root.loadThreads(); if (root.openKey !== "") root.loadNewest(false) } }

  Connections {
    target: root.app
    function onSmsRevChanged() { root.loadThreads(); if (root.openKey !== "") root.loadNewest(false) }
  }

  Component.onCompleted: loadThreads()
  Component.onDestruction: root.app.smsHasThread = false

  SmsThreadList {
    id: threads
    Layout.preferredWidth: implicitWidth
    Layout.fillHeight: true
    model: threadModel
    currentIndex: root.threadIndex
    paneFocused: root.app.focusPane === "msg"
    status: root.listStatus
    unreadTotal: root.unreadTotal
    searchText: root.app.smsSearch
    hotkey: "\u00b9"
    onRowClicked: function(i) { root.select(i); openTimer.stop(); root.openConversation(); root.app.focusPane = "msg"; root.app.focusRequested() }
    onRefreshRequested: { root.loadThreads(); root.app.focusRequested() }
  }

  SmsConversation {
    id: conv
    Layout.fillWidth: true
    Layout.fillHeight: true
    model: msgModel
    hasThread: root.openThread !== null
    title: root.openThread ? root.openThread.title : ""
    subtitle: root.openThread ? root.openThread.message_count + (root.openThread.message_count === 1 ? " message" : " messages") : ""
    paneFocused: root.app.focusPane === "reader"
    loading: root.loadingMsgs
    hasMore: root.hasMore
    hotkey: "\u00b2"
    visible: !newThread.opened
    onOlderRequested: root.loadOlder()
    onSendRequested: function(text) { root.sendReply(text) }
    onLeaveRequested: root.app.focusRequested()
  }

  SmsNewThread {
    id: newThread
    app: root.app
    Layout.fillWidth: true
    Layout.fillHeight: true
    onSent: function(r) {
      root.pendingSelect = r.device + ":" + r.thread
      root.app.focusRequested()
      root.loadThreads()
    }
    onClosed: { root.app.smsHasThread = root.openThread !== null; if (!root.openThread) root.app.focusPane = "msg" }
  }
}
