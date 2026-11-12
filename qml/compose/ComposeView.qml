import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell.Io
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import OmaLook.Compose
import "../common"
import "modes.js" as Modes
import "attach.js" as Att
import "../mail/format.js" as Fmt
import "external.js" as External
import "signature.js" as Sig
import "identity.js" as Ident

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
  // OL-050: every account's signature, {account_id: text}, from the owner (Settings -> Mail).
  property var signatures: ({})
  property var lists: []
  property int largeAudience: 20     // confirm above this many recipients (0 = never)
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
  // Attachments changed by the user: an autosave straight away (not after the 10 s timer).
  onAttachmentsChanged: if (root._ready) root.saveDraftNow("editing")
  property var missingNames: []              // draft attachments whose spool copy is gone (AttachStrip "!")
  property var listDir: null
  property string browseDir: ""
  property var recentDirs: []

  signal browsed(string dir)
  signal sendRequested(var message)
  signal discardRequested()
  signal focusRequested()
  // Status "saved to Drafts" once the worker has pushed the saved copy (polled while the pane is open).
  Timer {
    id: pushPoll
    interval: 3000
    repeat: true
    running: root.visible && root.draftId !== "" && root.draftStateFn !== null
    onTriggered: {
      if (root.dirty) return
      root.draftStateFn(root.draftId, function(d) {
        if (d && d.remote_id && !d.dirty && !root.dirty) root.localStatus = "saved to Drafts"
      })
    }
  }

  // Esc on a changed message, "Save draft": saved, then the owner closes the pane.
  signal saveAndCloseRequested()

  // Drafts (OL-048 Phase B2): the local draft id ("" until the first save; resumed drafts carry theirs),
  // the save function (AppState.saveDraft, injected like fetchLimits) and the autosave clock.
  property string draftId: initial.draftId || ""
  property var saveDraftFn: null
  property var draftStateFn: null            // (localId, done(draft|null)): asks the backend whether the provider has it
  property int autosaveSecs: 10

  property string mode: defaultMode
  property string accountId: initial.accountId || ""
  // OL-051: the send-as addresses per account (from the owner) and the From address. The user's pick wins; until then
  // a reply starts as the alias the original was addressed to, anything else as the account's default.
  property var identities: ({})
  property var rememberFrom: null              // function(accountId, email), from the owner: the new default for new mail
  property string pickedFrom: initial.from || ""      // an undone send reopens with the alias it was sent as
  readonly property string fromAddress: root.pickedFrom !== "" ? root.pickedFrom : Ident.autoFrom(root.identities, root._account, initial.kind, initial.origRecipients)
  // "" | "discard" | "nosubject" | "flatten"
  property string confirming: ""
  property int audience: 0            // recipients counted at the last Send, lists included
  property bool audienceOk: false     // the large-audience question was answered yes for this compose
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

  // ---- OL-050 signature. `_sigInserted` is the exact text put in the editor, so it can be found again.
  // Ctrl+G (and `:signature`) takes it out or puts it back for this message only.
  property bool signatureOn: true
  property string _sigInserted: ""
  property bool _ready: false
  readonly property string _sig: (root.signatures && root.accountId && root.signatures[root.accountId]) ? root.signatures[root.accountId] : ""
  function _quoteFirstLine() {
    return initial.quote ? "On " + (initial.quote.date || "") + ", " + (initial.quote.from || "") + " wrote:" : ""
  }
  // Returns false when the block was edited by hand (then it is left where it is).
  function _removeSignature() {
    if (root._sigInserted === "") return true
    var at = Sig.locate(handler.plainText(), root._sigInserted)
    if (at < 0) return false
    editor.remove(at, at + root._sigInserted.length)
    root._sigInserted = ""
    return true
  }
  function _insertSignature() {
    if (initial.restore || initial.kind === "bounce") return
    if (!_removeSignature()) return
    if (!root.signatureOn || root._sig === "") return
    var plain = handler.plainText()
    var ins = Sig.insertion(plain, root._sig, root.mode === Modes.SYSTEM, Sig.quoteStart(plain, _quoteFirstLine()))
    if (!ins) return
    editor.insert(ins.at, ins.text)
    root._sigInserted = ins.text
  }
  function toggleSignature() {
    root.localStatus = ""
    if (root._sig === "") { root.localStatus = "No signature for this account (Settings \u2192 Mail)."; return }
    root.signatureOn = !root.signatureOn
    if (root.signatureOn) _insertSignature()
    else if (!_removeSignature()) { root.localStatus = "The signature was edited; remove it by hand."; root.signatureOn = true }
    if (root.localStatus === "") root.localStatus = root.signatureOn ? "Signature on for this message." : "Signature off for this message."
  }
  function _recipientsOf(f) { return f.recipients.map(function(r) { return r.list ? { email: "", name: r.name, list: r.list } : { email: r.email, name: r.name } }) }

  function _body() {
    // Plain text only when nothing was formatted (decision Q1); otherwise HTML.
    if (root.mode === Modes.SYSTEM && !handler.hasFormatting) return { format: "text", content: handler.plainText() }
    return { format: "html", content: handler.html() }
  }

  readonly property var _account: {
    var a = accounts.filter(function(x) { return x.id === root.accountId })
    return a.length > 0 ? a[0] : null
  }
  // OL-023: recipients outside the From account's organisation (Exchange work/school accounts only).
  readonly property var _inside: External.bannerDomains(_account)
  readonly property var _externals: _inside ? External.externalRecipients(
      header.to.recipients.concat(header.cc.recipients, header.bcc.recipients),
      function(id) { var l = root.lists.filter(function(x) { return x.id === id }); return l.length > 0 ? (l[0].members || []) : [] },
      _inside) : []

  // What the From account's provider accepts. Starts from the built-in figures so the meter is right
  // at once; the daemon's answer (`GET /attach/limits`) replaces it, and a From switch asks again.
  property var fetchLimits: null              // function(accountId, cb(limits)), from the owner
  property var limits: Att.defaultLimits("")
  function _loadLimits() {
    var id = root.accountId
    root.limits = Att.defaultLimits(root._account ? root._account.provider : "")
    if (root.fetchLimits && id !== "") root.fetchLimits(id, function(l) { if (l && l.hard && root.accountId === id) root.limits = l })
  }
  // A From switch also swaps the signature for the new account's one.
  onAccountIdChanged: { _loadLimits(); if (_ready) _insertSignature() }
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

  // ---- Inline pictures (OL-054). In HTML mode a picture is an image `cid:<id>` in the document plus an
  // attachment entry with the same `inline_cid`. The entry is listed only while its image is in the
  // document, so deleting the picture from the text takes it out of the list and off the message.
  property int _pictureSeq: 0
  property bool _pictureMode: false   // the next browser pick goes into the text, not the attachments
  readonly property var liveAttachments: root.attachments.filter(function(a) {
    return !a.inline_cid || (handler.pictureCids || []).indexOf(a.inline_cid) >= 0
  })

  // Everything the strip and the list show: the forward's originals first, then picked files.
  readonly property var rows: Att.combine(originals, liveAttachments)
  readonly property real attachedBytes: Att.rawTotal(rows)

  function openAttach() {
    root.localStatus = ""
    root._pictureMode = false
    browser.title = "Attach files"
    browser.open()
  }

  function openPicture() {
    root.localStatus = ""
    root._pictureMode = true
    browser.title = "Insert pictures"
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
    var have = root.liveAttachments.map(function(a) { return a.path })
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
      var a = root.liveAttachments[r.ref]
      if (a && a.inline_cid) handler.removeImage(a.inline_cid)
      root.attachments = root.attachments.filter(function(x) { return x !== a })
    }
  }

  // The picked entry (same object as in `attachments`) behind `row`; null for the forward's originals.
  function _entryOf(row) {
    var r = root.rows[row]
    return r && r.origin === "picked" ? root.liveAttachments[r.ref] : null
  }

  // Pictures: a picture file goes into the text as an image when the mode can show it, else it is attached.
  function _addPicture(f) {
    var cid = root._newCid()
    if (!handler.insertImage(f.path, cid)) return false
    root.attachments = root.attachments.concat([{ path: f.path, name: f.name, size: f.size || 0, inline_cid: cid }])
    return true
  }
  function _newCid() {
    root._pictureSeq += 1
    return "omalook-" + Date.now().toString(36) + "-" + root._pictureSeq + "@omalook"
  }
  // Files from the browser (insert mode) or the clipboard: [{ path, name, size }].
  function insertPictures(files) {
    var pics = files.filter(function(f) { return Att.isPicture(f.name) })
    var others = files.filter(function(f) { return !Att.isPicture(f.name) })
    if (root.mode !== Modes.HTML) {
      root.addAttachments(files)
      if (pics.length > 0) root.localStatus = pics.length === 1
        ? "System-font mode can't show pictures: attached as a file."
        : "System-font mode can't show pictures: attached " + pics.length + " as files."
      return
    }
    var added = 0, failed = 0
    pics.forEach(function(f) { if (root._addPicture(f)) added++; else failed++ })
    if (others.length > 0) root.addAttachments(others)
    else editor.forceActiveFocus()
    var msg = added > 0 ? "Inserted " + added + (added === 1 ? " picture." : " pictures.") : ""
    if (failed > 0) msg += (msg ? " " : "") + failed + (failed === 1 ? " file" : " files") + " could not be read as a picture."
    if (others.length > 0) msg += (msg ? " " : "") + others.length + (others.length === 1 ? " file" : " files") + " not a picture: attached."
    root.localStatus = msg
  }
  // A picture still in the text (system-font switch) becomes an ordinary attachment. An entry whose picture
  // was already deleted from the text is dropped, not turned into a file, and a file listed twice is kept once.
  function _picturesToFiles() {
    var r = Att.pictureFiles(root.attachments, handler.pictureCids)
    root.attachments = r.files
    if (r.converted > 0) root.localStatus = r.converted === 1 ? "The picture is now an attached file: system-font mode cannot show it."
      : r.converted + " pictures are now attached files: system-font mode cannot show them."
    return r.converted
  }

  // The palette twins of the keys (`:attach`, `:attachments`, `:detach`, `:detach-all`).
  // ---- Ctrl+V with files on the clipboard (a file copied in the file manager) attaches them; anything else
  // pastes as text into whatever field has the focus. The decision is asynchronous (wl-paste), so the key is
  // always taken here and the text paste is replayed on the focused field when the clipboard holds no files.
  property var _pasteTarget: null
  property string _pasteType: ""
  // Picture data (PNG) on the clipboard: saved under the cache dir, then inserted (or attached in system mode).
  readonly property string _pasteScript: 'd="${XDG_CACHE_HOME:-$HOME/.cache}/omalook/paste"; mkdir -p "$d" || exit 1; '
    + 'f="$d/picture-$(date +%s%N).png"; if wl-paste --type image/png > "$f" && [ -s "$f" ]; then printf \'%s\' "$f"; else rm -f "$f"; fi'
  Process {
    id: pasteTypes
    command: ["wl-paste", "--list-types"]
    stdout: StdioCollector {
      onStreamFinished: {
        root._pasteType = Att.clipboardFileType(text)
        if (root._pasteType !== "") { pasteBody.command = ["wl-paste", "--no-newline", "--type", root._pasteType]; pasteBody.running = true }
        else if (Att.clipboardHasPng(text)) { pasteImage.command = ["sh", "-c", root._pasteScript]; pasteImage.running = true }
        else root._pasteText()
      }
    }
    onExited: function(code) { if (code !== 0) root._pasteText() }
  }
  Process {
    id: pasteImage
    stdout: StdioCollector { onStreamFinished: root._pastedPicture(text.trim()) }
  }
  function _pastedPicture(path) {
    if (path === "") { root.localStatus = "The clipboard picture could not be read."; return }
    var f = { path: path, name: "pasted-picture.png", size: 0 }
    var base = Att.baseName(path)
    if (!root.listDir) { root.insertPictures([f]); return }
    root.listDir(Att.dirName(path), true, function(r) {
      var hit = (r && r.entries ? r.entries : []).filter(function(e) { return e.name === base })
      f.size = hit.length > 0 ? (hit[0].size || 0) : 0
      root.insertPictures([f])
    })
  }
  Process {
    id: pasteBody
    stdout: StdioCollector { onStreamFinished: root._pasteFiles(Att.pathsFromClipboard(text)) }
  }
  function startPaste() { _pasteTarget = root.Window.activeFocusItem; pasteTypes.running = true }
  function _pasteText() {
    var t = _pasteTarget
    if (t && t.paste !== undefined) t.paste()
  }
  // Fill in each file's size from its folder's listing, then add; folders and unreadable paths are named, not added.
  function _pasteFiles(paths) {
    if (paths.length === 0) { _pasteText(); return }
    var dirs = {}, files = [], skipped = 0, waiting = 0
    paths.forEach(function(p) { var d = Att.dirName(p); (dirs[d] = dirs[d] || []).push(p) })
    var finish = function() {
      if (files.length > 0) root.addAttachments(files)
      if (skipped > 0) root.localStatus = skipped + (skipped === 1 ? " item" : " items") + " on the clipboard could not be attached (folders, or not readable)."
      else if (files.length > 0) root.localStatus = "Attached " + files.length + (files.length === 1 ? " file" : " files") + " from the clipboard."
    }
    var names = Object.keys(dirs)
    if (!root.listDir) { skipped = paths.length; finish(); return }
    waiting = names.length
    names.forEach(function(d) {
      root.listDir(d, true, function(r) {
        var by = {}
        ;(r && r.entries ? r.entries : []).forEach(function(e) { by[e.name] = e })
        dirs[d].forEach(function(p) {
          var e = by[Att.baseName(p)]
          if (e && !e.dir) files.push({ path: p, name: e.name, size: e.size || 0 })
          else skipped++
        })
        if (--waiting === 0) finish()
      })
    })
  }

  // Drag files from a file manager onto the pane. Not yet seen to work under Quickshell (needs a real drag):
  // if the compositor never delivers the drop, nothing happens and Ctrl+V / the browser still work.
  DropArea {
    anchors.fill: parent
    keys: ["text/uri-list"]
    onDropped: function(drop) {
      if (!drop.hasUrls) return
      var paths = drop.urls.map(function(u) { var m = /^file:\/\/(\/.*)$/.exec(String(u)); try { return m ? decodeURIComponent(m[1]) : "" } catch (e) { return "" } }).filter(function(p) { return p !== "" })
      root._pasteFiles(paths)
      drop.accept()
    }
  }

  signal paletteRequested()
  // Back from the command palette: the editor gets the keys again, unless the command that was run opened a panel.
  function focusAfterPalette() {
    if (browser.opened || attachList.opened || root.confirming !== "") return
    editor.forceActiveFocus()
  }
  function runCommand(name) {
    if (name === "attach") openAttach()
    else if (name === "attachments") openList()
    else if (name === "detach") removeRow(root.rows.length - 1)
    else if (name === "detach-all") { root.attachments = []; root.originals = [] }
    else if (name === "signature") root.toggleSignature()
  }

  function buildMessage() {
    return {
      kind: initial.kind,
      account_id: root.accountId,
      from: Ident.override(root.fromAddress, root._account),
      in_reply_to: initial.inReplyTo || "",
      to: _recipientsOf(header.to), cc: _recipientsOf(header.cc), bcc: _recipientsOf(header.bcc),
      subject: header.subject.text,
      draft_id: root.draftId,
      mode: root.mode,
      body: _body(),
      attachments: root.liveAttachments.map(function(a) {
        var o = { path: a.path, name: a.name, size: a.size }
        if (a.inline_cid) o.inline_cid = a.inline_cid
        return o
      }),
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
    var bad = null, audience = 0
    ;[header.to, header.cc, header.bcc].forEach(function(f) {
      f.recipients.forEach(function(r) {
        if (!f.isValid(r)) { if (!bad) bad = r; return }
        var l = r.list ? f.listInfo(r.list) : null
        audience += l ? l.count : 1
      })
    })
    if (bad) { root.localStatus = bad.list ? "The list \u201c" + bad.name + "\u201d is empty or no longer exists." : "Not a valid address: " + bad.email; return }
    root.audience = audience
    if (root.originalsState === "loading") { root.localStatus = "Still reading the original's attachments\u2026"; return }
    var m = Att.meter(root.limits, root.rows)
    if (m.level === "over") {
      var v = Att.states(root.limits, root.rows), big = v.state.indexOf("big")
      root.localStatus = "The attachments are too big for " + Att.accountName(root.limits) + " (" + Fmt.bytes(m.raw) + " of files; about " + Fmt.bytes(m.capacity) + " is the most)"
        + (big >= 0 ? ". Remove " + root.rows[big].name + " or change From." : ".")
      return
    }
    if (root.largeAudience > 0 && audience > root.largeAudience && !root.audienceOk) { root.confirming = "audience"; return }
    if (header.subject.text.trim() === "") { root.confirming = "nosubject"; return }
    root.sendRequested(root.buildMessage())
  }

  // Esc: an empty message closes silently; a changed one asks Save draft / Discard / Cancel.
  function requestClose() {
    if (root.dirty) root.confirming = "close"
    else root.discardRequested()
  }

  function setMode(m) {
    if (m === root.mode) return
    if (m === Modes.SYSTEM && handler.hasRichAttributes) { root.confirming = "flatten"; return }
    if (m === Modes.SYSTEM) { root._picturesToFiles(); handler.flattenToSystem() }
    root.mode = m
  }

  Component.onCompleted: {
    // A send taken back brings its entries as they were (list chips included); anything else is text.
    if (initial.toEntries) header.to.setRecipients(initial.toEntries); else header.to.setFromText(initial.to || "")
    if (initial.ccEntries) header.cc.setRecipients(initial.ccEntries); else header.cc.setFromText(initial.cc || "")
    if (initial.bccEntries) header.bcc.setRecipients(initial.bccEntries); else header.bcc.setFromText(initial.bcc || "")
    header.showCc = header.cc.recipients.length > 0
    header.showBcc = header.bcc.recipients.length > 0
    header.subject.text = initial.subject || ""
    if (initial.mode) root.mode = initial.mode
    if (initial.attachments) root.attachments = initial.attachments
    root.missingNames = initial.missing || []
    _loadLimits()
    _loadOriginals()
    if (initial.quote) handler.setHtml(_quoteHtml(initial.quote))
    // A message taken back (Undo) or that failed to send comes back exactly as written.
    if (initial.restore) {
      if (initial.restore.format === "text") handler.setPlainText(initial.restore.content)
      else handler.setHtml(initial.restore.content)
    }
    // A restored message's pictures come back from their files (the document only names them).
    if (initial.attachments) initial.attachments.forEach(function(a) { if (a.inline_cid) handler.registerImage(a.path, a.inline_cid) })
    // The signature goes in before `_seedText` is taken, so a message with only a signature is not "dirty".
    if (!initial.restore) _insertSignature()
    _seedText = initial.restore ? "" : handler.plainText()
    _ready = true
    // Replies start in the body, new messages in To.
    if (initial.restore) { editor.forceActiveFocus(); editor.cursorPosition = editor.length }
    else if (initial.kind === "new" || initial.kind === "forward") header.to.input.forceActiveFocus()
    else { editor.forceActiveFocus(); editor.cursorPosition = 0 }
  }

  // ---- drafts (OL-048 Phase B2) --------------------------------------------------------------
  // Autosave `autosaveSecs` after the last edit of the body or subject; also when the window loses
  // focus. Status line: "saving…" / "saved locally" / "save failed (retrying)" (a failed autosave retries).
  // "saved to Drafts" needs the provider's copy (set by the draft worker): not shown here yet.
  // `done(ok)` is optional (Save draft closes the pane only when the save landed).
  function saveDraftNow(state, done) {
    if (!_ready || saveDraftFn === null) { if (done) done(false); return }
    autosaveTimer.stop()
    if (!root.dirty && root.draftId === "") { if (done) done(true); return }
    root.localStatus = "saving\u2026"
    saveDraftFn(root.buildMessage(), state, function(r) {
      if (r && r.ok) {
        root.draftId = r.id
        root.localStatus = "saved locally"
        if (done) done(true)
      } else {
        root.localStatus = "save failed (retrying)"
        if (state === "editing") autosaveTimer.restart()
        if (done) done(false)
      }
    })
  }
  // Esc / Discard on a changed message: Save draft / Discard / Cancel (see closeDialog).
  function saveAndClose() {
    root.saveDraftNow("saved", function(ok) {
      if (ok) root.saveAndCloseRequested()
      else root.localStatus = "not saved: the message is still open"
    })
  }
  function _touchDraft() { if (_ready && root.dirty) autosaveTimer.restart() }
  Timer {
    id: autosaveTimer
    interval: root.autosaveSecs * 1000
    onTriggered: root.saveDraftNow("editing")
  }
  Connections { target: editor; ignoreUnknownSignals: true; function onTextChanged() { root._touchDraft() } }
  Connections { target: header.subject; ignoreUnknownSignals: true; function onTextChanged() { root._touchDraft() } }
  Connections {
    target: root.Window.window
    ignoreUnknownSignals: true
    function onActiveChanged() { if (root.Window.window && !root.Window.window.active) root.saveDraftNow("editing") }
  }

  Keys.onPressed: function(e) {
    if (browser.opened || attachList.opened) return
    if (root.confirming !== "") {
      if (root.confirming === "close" ? closeDialog.handleKey(e) : confirmDialog.handleKey(e)) e.accepted = true
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
    else if (ctrl && shift && e.key === Qt.Key_I) { root.openPicture(); e.accepted = true }
    else if (ctrl && e.key === Qt.Key_I) { handler.toggleItalic(); e.accepted = true }
    else if (ctrl && e.key === Qt.Key_U) { handler.toggleUnderline(); e.accepted = true }
else if (ctrl && !shift && e.key === Qt.Key_V) { root.startPaste(); e.accepted = true }
    else if (ctrl && !shift && e.key === Qt.Key_P) { root.paletteRequested(); e.accepted = true }   // the shell's keys are off while composing, so the palette (and its :attach twins) needs its own way in
        else if (ctrl && shift && e.key === Qt.Key_A) { root.openAttach(); e.accepted = true }
    else if (ctrl && shift && e.key === Qt.Key_L) { root.openList(); e.accepted = true }
    else if (ctrl && shift && e.key === Qt.Key_Backspace && root.rows.length > 0) { root.removeRow(root.rows.length - 1); e.accepted = true }
    else if (ctrl && e.key === Qt.Key_K) { root.linkOpen = true; linkUrl.forceActiveFocus(); e.accepted = true }
    else if (ctrl && shift && (e.key === Qt.Key_7 || e.key === Qt.Key_Ampersand)) { handler.toggleNumberedList(); e.accepted = true }
    else if (ctrl && shift && (e.key === Qt.Key_8 || e.key === Qt.Key_Asterisk)) { handler.toggleBulletList(); e.accepted = true }
    else if (ctrl && !shift && e.key === Qt.Key_G) { root.toggleSignature(); e.accepted = true }
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
        visible: root.mode === Modes.HTML
        text: "Picture"
        iconText: "\uf03e"
        tooltipText: "Ctrl+Shift+I"
        onClicked: root.openPicture()
      }

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
      identities: root.identities
      fromAddress: root.fromAddress
      nextFocus: editor
      contacts: root.contacts
      lists: root.lists
      onAccountPicked: function(id) { root.accountId = id }
      onFromPicked: function(email) {
        root.pickedFrom = email
        if (initial.kind === "new" && root.rememberFrom) root.rememberFrom(root.accountId, email)
      }
    }

    // ---- OL-023: external recipients. No close button: it stays while any external recipient is in
    // To, Cc or Bcc, and goes when the last one is removed. It never blocks or asks before sending.
    Rectangle {
      id: externalBanner
      visible: root._externals.length > 0
      Layout.fillWidth: true
      Layout.leftMargin: Style.spacing.lg
      Layout.rightMargin: Style.spacing.lg
      Layout.topMargin: Style.spacing.md
      implicitHeight: externalText.implicitHeight + Style.spacing.md * 2
      color: "transparent"
      border.color: Commons.Color.urgent
      border.width: 1
      UiText {
        id: externalText
        x: Style.spacing.md
        y: Style.spacing.md
        width: parent.width - Style.spacing.md * 2
        wrapMode: Text.WordWrap
        font.pixelSize: Style.font.caption
        foreground: Commons.Color.urgent
        text: root._externals.length > 0 ? External.bannerText(root._inside, root._externals) : ""
      }
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
      onPictureRequested: root.openPicture()
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
      missing: root.missingNames
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
        text: "Ctrl+Enter send · Esc discard · Ctrl+K link · Ctrl+Shift+A attach · Ctrl+Shift+I picture · Ctrl+G signature"
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
    onRenameRequested: function(row, name) {
      var a = root._entryOf(row)
      if (!a) return
      root.attachments = root.attachments.map(function(x) {
        if (x !== a) return x
        var o = { path: x.path, name: name, size: x.size }
        if (x.inline_cid) o.inline_cid = x.inline_cid
        return o
      })
    }
    onClosed: editor.forceActiveFocus()
  }

  AttachBrowser {
    id: browser
    anchors.fill: parent
    z: 900
    listDir: root.listDir
    startDir: root.browseDir
    recentDirs: root.recentDirs
    capacity: Att.capacity(root.limits)
    baseBytes: root.attachedBytes
    attached: root.liveAttachments.map(function(a) { return a.path })
    onPicked: function(files) {
      if (root._pictureMode) { root._pictureMode = false; root.insertPictures(files) }
      else root.addAttachments(files)
    }
    onCanceled: { root._pictureMode = false; editor.forceActiveFocus() }
    onNavigated: function(d) { root.browsed(d) }
  }

  // While a confirm dialog is up, keys go to it, not to the editor/header
  // field that had focus (those consume Enter/Left/Right/Tab themselves).
  property Item _focusBeforeConfirm: null
  onConfirmingChanged: {
    if (confirming !== "") {
      _focusBeforeConfirm = Window.activeFocusItem
      confirmDialog.selectedIndex = 1
      closeDialog.selectedIndex = 0
      dialogKeys.forceActiveFocus()
    } else {
      if (_focusBeforeConfirm) _focusBeforeConfirm.forceActiveFocus()
      _focusBeforeConfirm = null
    }
  }
  Item {
    id: dialogKeys
    Keys.onPressed: function(e) {
      if (root.confirming === "close" ? closeDialog.handleKey(e) : confirmDialog.handleKey(e)) e.accepted = true
    }
  }

  // Esc on a changed message: Save draft (the default, Enter) / Discard / Cancel (Esc keeps editing).
  ChoiceDialog {
    id: closeDialog
    anchors.fill: parent
    z: 1000
    opened: root.confirming === "close"
    message: "Save this message as a draft?"
    choices: [ { text: "Save draft" }, { text: "Discard", destructive: true }, { text: "Cancel" } ]
    escapeIndex: 2
    onChosen: function(i) {
      root.confirming = ""
      if (i === 0) root.saveAndClose()
      else if (i === 1) root.discardRequested()
    }
  }

  ConfirmDialog {
    id: confirmDialog
    anchors.fill: parent
    z: 1000
    opened: root.confirming !== ""
    message: root.confirming === "discard" ? "Discard this message?"
      : root.confirming === "nosubject" ? "Send without a subject?"
      : root.confirming === "audience" ? "Send to about " + root.audience + " people?"
      : "Switching to system font removes fonts, sizes, colours and alignment" + (handler.pictureCids.length > 0 ? ", and turns pictures into attached files" : "") + ". Continue?"
    cancelText: root.confirming === "discard" ? "Keep editing" : "Cancel"
    confirmText: root.confirming === "discard" ? "Discard" : (root.confirming === "nosubject" || root.confirming === "audience") ? "Send" : "Switch"
    onCanceled: root.confirming = ""
    onConfirmed: {
      var what = root.confirming
      root.confirming = ""
      if (what === "discard") root.discardRequested()
      else if (what === "nosubject") root.sendRequested(root.buildMessage())
      else if (what === "audience") { root.audienceOk = true; root.trySend() }
      else if (what === "flatten") { root._picturesToFiles(); handler.flattenToSystem(); root.mode = Modes.SYSTEM }
    }
  }
}
