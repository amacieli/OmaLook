import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../../common"

// Rules OmaLook runs itself (OL-040 phase 4d). They are exactly what a mail provider can store, nothing
// more: ALL conditions must match, any ONE value inside a condition matches, and any ONE exception skips
// the rule. No groups, no "or" between different conditions. OmaLook tags new mail with them on accounts
// that have no server rules (Gmail); on Exchange the server rules above do that, and these only tag
// existing mail when you ask.
Column {
  id: sec

  property var app: null
  // true while one of the inputs owns the keyboard (the settings shell then leaves j/k/h/l alone)
  readonly property bool editing: nameField.activeFocus || tagField.activeFocus || sec._valueFocus

  property bool _valueFocus: false
  property var data_: ({ rules: [], accounts: [], tags: [], limit: 1000 })
  property string notice: ""
  property string armedId: ""

  // ---- editor state -------------------------------------------------------------------------------
  property bool open: false
  property string editId: ""
  property string editName: ""
  property string editTags: ""
  property string editAccount: ""        // "" = every account
  property bool editStop: true
  property bool editEnabled: true
  property var conds: []                 // [{ kind, text }]  text = comma separated values
  property var excs: []
  property var preview: null
  property string previewError: ""

  // ---- apply to existing mail ----------------------------------------------------------------------
  property string applyId: ""            // rule being applied, "" = none
  property var applyPreview: null

  readonly property var kinds: [
    { value: "from_addresses", label: "From exactly" },
    { value: "sender_contains", label: "Sender contains" },
    { value: "recipient_contains", label: "To or Cc contains" },
    { value: "subject_contains", label: "Subject contains" },
    { value: "body_contains", label: "Body contains" },
    { value: "body_or_subject_contains", label: "Subject or body contains" },
    { value: "categories", label: "Already has tag" },
    { value: "has_attachments", label: "Has attachments" },
    { value: "importance", label: "Importance is" }
  ]
  function kindLabel(k) { for (var i = 0; i < kinds.length; i++) if (kinds[i].value === k) return kinds[i].label; return k }
  function isText(k) { return k !== "has_attachments" && k !== "importance" }

  width: parent ? parent.width : 0
  spacing: Style.space(6)

  function request(method, path, done) {
    if (!app) return
    app.request(method, path, function(xhr) {
      var r = null
      try { r = JSON.parse(xhr.responseText) } catch (e) { r = null }
      done(xhr.status === 200 && r ? r : { ok: false, error: "the backend did not answer" })
    })
  }

  function load() {
    request("GET", "/local-rules", function(r) { if (r.ok) sec.data_ = r })
  }
  Component.onCompleted: load()

  // ---- rule <-> JSON ---------------------------------------------------------------------------------
  function split(text) {
    return String(text).split(",").map(function(s) { return s.trim() }).filter(function(s) { return s !== "" })
  }
  function preds(list) {
    var out = []
    for (var i = 0; i < list.length; i++) {
      var k = list[i].kind
      if (k === "has_attachments") out.push({ kind: k, value: true })
      else if (k === "importance") out.push({ kind: k, value: list[i].text || "high" })
      else out.push({ kind: k, value: split(list[i].text) })
    }
    return out
  }
  function ruleJson() {
    return JSON.stringify({
      id: editId, name: editName.trim(), enabled: editEnabled,
      applies_to: editAccount === "" ? [] : [editAccount],
      conditions: preds(conds), exceptions: preds(excs),
      actions: { tags: split(editTags), stop: editStop }
    })
  }
  function textOf(p) {
    if (p.kind === "has_attachments") return ""
    if (p.kind === "importance") return String(p.value)
    return (p.value || []).join(", ")
  }
  function fromRule(list) {
    return (list || []).map(function(p) { return { kind: p.kind, text: textOf(p) } })
  }

  function summary(r) {
    function one(p) {
      if (p.kind === "has_attachments") return "has attachments"
      if (p.kind === "importance") return "importance is " + p.value
      return sec.kindLabel(p.kind).toLowerCase() + " " + (p.value || []).map(function(v) { return "\u201c" + v + "\u201d" }).join(" or ")
    }
    var s = r.conditions.map(one).join(", and ")
    if (r.exceptions.length > 0) s += ", except " + r.exceptions.map(one).join(" or ")
    s += "  \u2192  tag " + r.actions.tags.map(function(t) { return "\u201c" + t + "\u201d" }).join(", ")
    if (r.actions.stop) s += ", stop"
    return s
  }
  function accountLine(r) {
    if (!r.applies_to || r.applies_to.length === 0) return "all accounts"
    var names = []
    for (var i = 0; i < r.applies_to.length; i++) {
      var a = sec.data_.accounts.filter(function(x) { return x.id === r.applies_to[i] })
      names.push(a.length ? a[0].email : r.applies_to[i])
    }
    return names.join(", ")
  }

  // ---- editor actions ----------------------------------------------------------------------------
  function begin(r) {
    previewTimer.stop(); sec.preview = null; sec.previewError = ""; sec.applyId = ""; sec.applyPreview = null
    if (r) {
      editId = r.id; editName = r.name; editEnabled = r.enabled; editStop = r.actions.stop
      editTags = r.actions.tags.join(", "); editAccount = (r.applies_to && r.applies_to.length) ? r.applies_to[0] : ""
      conds = fromRule(r.conditions); excs = fromRule(r.exceptions)
    } else {
      editId = ""; editName = ""; editEnabled = true; editStop = true; editTags = ""; editAccount = ""
      conds = [{ kind: "sender_contains", text: "" }]; excs = []
    }
    open = true
    previewTimer.restart()
  }
  function setPred(listName, i, key, val) {
    var l = (listName === "c" ? conds : excs).slice()
    var p = { kind: l[i].kind, text: l[i].text }
    p[key] = val
    if (key === "kind") p.text = p.kind === "importance" ? "high" : ""
    l[i] = p
    if (listName === "c") conds = l; else excs = l
    previewTimer.restart()
  }
  function addPred(listName) {
    var l = (listName === "c" ? conds : excs).slice()
    var used = l.map(function(p) { return p.kind })
    var next = ""
    for (var i = 0; i < kinds.length; i++) if (used.indexOf(kinds[i].value) < 0) { next = kinds[i].value; break }
    if (next === "") return
    l.push({ kind: next, text: next === "importance" ? "high" : "" })
    if (listName === "c") conds = l; else excs = l
    previewTimer.restart()
  }
  function removePred(listName, i) {
    var l = (listName === "c" ? conds : excs).slice()
    l.splice(i, 1)
    if (listName === "c") conds = l; else excs = l
    previewTimer.restart()
  }
  // kinds still free in a list (a rule can hold each condition once: that is what the server stores)
  function freeKinds(listName, i) {
    var l = listName === "c" ? conds : excs
    var used = l.map(function(p) { return p.kind })
    return kinds.filter(function(k) { return k.value === l[i].kind || used.indexOf(k.value) < 0 })
  }
  function canAdd(listName) { return (listName === "c" ? conds : excs).length < kinds.length }

  function runPreview() {
    request("GET", "/local-rules/preview?rule=" + encodeURIComponent(ruleJson()), function(r) {
      if (r.ok) { sec.preview = r.preview; sec.previewError = "" }
      else { sec.preview = null; sec.previewError = r.error || "" }
    })
  }
  Timer { id: previewTimer; interval: 500; onTriggered: sec.runPreview() }

  function save() {
    request("POST", "/local-rules/save?rule=" + encodeURIComponent(ruleJson()), function(r) {
      if (r.ok) { sec.notice = "Rule saved."; sec.open = false; sec.load() }
      else sec.previewError = r.error || "Could not save"
    })
  }
  function nearTag() {
    var mine = split(editTags)
    var known = data_.tags || []
    var msgs = []
    function sq(s) { return s.toLowerCase().replace(/[^a-z0-9]/g, "") }
    for (var i = 0; i < mine.length; i++) {
      var exact = known.some(function(k) { return k.toLowerCase() === mine[i].toLowerCase() })
      if (exact) continue
      var near = known.filter(function(k) { return sq(k) === sq(mine[i]) || (sq(mine[i]).length >= 5 && (sq(k).indexOf(sq(mine[i])) >= 0 || sq(mine[i]).indexOf(sq(k)) >= 0) && sq(k).length >= 5) })
      msgs.push(near.length ? "\u201c" + mine[i] + "\u201d is new; did you mean \u201c" + near[0] + "\u201d?" : "\u201c" + mine[i] + "\u201d is a new tag.")
    }
    return msgs.join("  ")
  }

  // ---- list actions ----------------------------------------------------------------------------------
  function post(path, text) {
    request("POST", path, function(r) { sec.notice = r.ok ? text : "Could not do that: " + (r.error || "no answer"); sec.load() })
  }
  function startApply(r) {
    sec.applyId = r.id; sec.applyPreview = null
    request("GET", "/local-rules/preview?rule=" + encodeURIComponent(JSON.stringify(r)), function(p) {
      sec.applyPreview = p.ok ? p.preview : { error: p.error || "no answer" }
    })
  }
  function confirmApply() {
    var over = sec.applyPreview && sec.applyPreview.messages > sec.data_.limit
    request("POST", "/local-rules/apply?id=" + encodeURIComponent(sec.applyId) + (over ? "&confirm=yes" : ""), function(r) {
      var o = r.outcome
      if (!r.ok) sec.notice = "Could not apply: " + (r.error || "no answer")
      else if (o && o.result === "refused") sec.notice = "Not applied: it would change " + o.would_change + " messages, over the limit of " + o.limit + "."
      else sec.notice = "Tagged " + (o ? o.messages : 0) + " existing messages. The tags go to the server in the background."
      sec.applyId = ""; sec.applyPreview = null; sec.load()
    })
  }

  // =====================================================================================================
  PanelSectionHeader { text: "RULES OMALOOK RUNS (" + sec.data_.rules.length + ")" }

  UiText {
    width: parent.width
    wrapMode: Text.WordWrap
    dim: true
    font.pixelSize: Style.font.caption
    text: "Same shape as a server rule: every condition must match, any one value in a condition is enough, and any one exception skips the rule. "
        + "On accounts without server rules (Gmail) OmaLook tags new mail with these as it arrives. Exchange tags arriving mail with its own rules above, "
        + "so there these only tag mail already in the Inbox, when you press Apply. Rules only add tags; they never remove one."
  }
  UiText {
    visible: sec.notice !== ""
    width: parent.width
    wrapMode: Text.WordWrap
    font.pixelSize: Style.font.caption
    foreground: sec.notice.indexOf("Could not") === 0 || sec.notice.indexOf("Not applied") === 0 ? Commons.Color.urgent : Commons.Color.foreground
    text: sec.notice
  }

  Repeater {
    model: sec.data_.rules
    delegate: Column {
      id: row
      required property var modelData
      width: sec.width
      spacing: Style.space(4)
      RowLayout {
        width: parent.width
        spacing: Style.spacing.md
        UiText {
          Layout.fillWidth: true
          wrapMode: Text.WordWrap
          opacity: row.modelData.enabled ? 1 : 0.5
          font.pixelSize: Style.font.caption
          text: (row.modelData.enabled ? "" : "(off)  ") + row.modelData.name + ": " + sec.summary(row.modelData)
                + "   [" + sec.accountLine(row.modelData) + (row.modelData.applied > 0 ? "; tagged " + row.modelData.applied : "") + "]"
        }
        Button { Layout.preferredWidth: Style.space(32); bordered: true; iconText: "\uf077"; onClicked: sec.post("/local-rules/move?id=" + encodeURIComponent(row.modelData.id) + "&dir=up", "Rule moved up.") }
        Button { Layout.preferredWidth: Style.space(32); bordered: true; iconText: "\uf078"; onClicked: sec.post("/local-rules/move?id=" + encodeURIComponent(row.modelData.id) + "&dir=down", "Rule moved down.") }
        Button {
          Layout.preferredWidth: Style.space(72); bordered: true; iconText: "\uf011"
          text: row.modelData.enabled ? "Off" : "On"
          onClicked: sec.post("/local-rules/enable?id=" + encodeURIComponent(row.modelData.id) + "&on=" + (row.modelData.enabled ? "false" : "true"), row.modelData.enabled ? "Rule turned off." : "Rule turned on.")
        }
        Button { Layout.preferredWidth: Style.space(72); bordered: true; iconText: "\uf044"; text: "Edit"; onClicked: sec.begin(row.modelData) }
        Button { Layout.preferredWidth: Style.space(96); bordered: true; text: "Apply"; enabled: row.modelData.enabled; onClicked: sec.startApply(row.modelData) }
        Button {
          Layout.preferredWidth: Style.space(sec.armedId === row.modelData.id ? 96 : 32)
          bordered: true; iconText: "\uf1f8"
          text: sec.armedId === row.modelData.id ? "Sure?" : ""
          foreground: sec.armedId === row.modelData.id ? Commons.Color.urgent : Commons.Color.foreground
          onClicked: {
            if (sec.armedId !== row.modelData.id) { sec.armedId = row.modelData.id; disarm.restart(); return }
            sec.armedId = ""
            sec.request("POST", "/local-rules/delete?id=" + encodeURIComponent(row.modelData.id) + "&confirm=yes", function(r) { sec.notice = r.ok ? "Rule deleted." : "Could not do that: " + (r.error || ""); sec.load() })
          }
        }
      }
      // count first, then the explicit apply
      Column {
        visible: sec.applyId === row.modelData.id
        width: parent.width
        spacing: Style.space(4)
        UiText {
          width: parent.width
          wrapMode: Text.WordWrap
          font.pixelSize: Style.font.caption
          foreground: sec.applyPreview && sec.applyPreview.error ? Commons.Color.urgent : (sec.applyPreview && sec.applyPreview.messages > sec.data_.limit ? Commons.Color.urgent : Commons.Color.foreground)
          text: !sec.applyPreview ? "Counting\u2026"
              : sec.applyPreview.error ? sec.applyPreview.error
              : "Inbox mail this rule matches: " + sec.applyPreview.matched + ". It would tag " + sec.applyPreview.messages + " message" + (sec.applyPreview.messages === 1 ? "" : "s")
                + " that do not have the tag yet" + (sec.applyPreview.messages > sec.data_.limit ? " \u2014 more than your limit of " + sec.data_.limit + ", so it needs your explicit go-ahead." : ".")
        }
        RowLayout {
          width: parent.width
          Item { Layout.fillWidth: true }
          Button { Layout.preferredWidth: Style.space(96); bordered: true; text: "Cancel"; onClicked: { sec.applyId = ""; sec.applyPreview = null } }
          Button {
            Layout.preferredWidth: Style.space(180); bordered: true
            enabled: !!sec.applyPreview && !sec.applyPreview.error && sec.applyPreview.messages > 0
            text: sec.applyPreview && sec.applyPreview.messages > sec.data_.limit ? "Apply anyway" : "Apply to " + (sec.applyPreview ? sec.applyPreview.messages : 0)
            onClicked: sec.confirmApply()
          }
        }
      }
    }
  }
  Timer { id: disarm; interval: 4000; onTriggered: sec.armedId = "" }

  UiText {
    visible: sec.data_.rules.length === 0 && !sec.open
    width: parent.width
    dim: true
    font.pixelSize: Style.font.caption
    text: "No rules of your own yet."
  }

  Button {
    visible: !sec.open
    width: Style.space(160)
    bordered: true
    iconText: "\uf067"
    text: "New rule"
    onClicked: sec.begin(null)
  }

  // ---- editor ------------------------------------------------------------------------------------------
  Column {
    id: editor
    visible: sec.open
    width: parent.width
    spacing: Style.space(6)

    UiText { text: sec.editId === "" ? "New rule" : "Edit rule"; dim: true; font.pixelSize: Style.font.caption }
    RowLayout {
      width: parent.width
      spacing: Style.spacing.lg
      TextField { id: nameField; Layout.preferredWidth: parent.width * 0.4; placeholderText: "Name"; text: sec.editName; onTextEdited: { sec.editName = text } }
      TextField { id: tagField; Layout.fillWidth: true; placeholderText: "Tag (several: separate with commas)"; text: sec.editTags; onTextEdited: { sec.editTags = text; previewTimer.restart() } }
    }
    UiText { visible: sec.nearTag() !== ""; width: parent.width; wrapMode: Text.WordWrap; font.pixelSize: Style.font.caption; dim: true; text: sec.nearTag() }

    Dropdown {
      width: parent.width
      label: "Applies to"
      value: sec.editAccount
      options: {
        var o = [{ value: "", label: "All accounts" }]
        var a = sec.data_.accounts
        for (var i = 0; i < a.length; i++) o.push({ value: a[i].id, label: a[i].email + (a[i].on_arrival ? "  (tags new mail)" : "  (existing mail only)") })
        return o
      }
      onChanged: function(v) { sec.editAccount = v; previewTimer.restart() }
    }

    // two lists with the same row shape: conditions (all must match) and exceptions (any one skips the rule)
    Repeater {
      model: ["c", "e"]
      delegate: Column {
        id: lst
        required property string modelData
        width: editor.width
        spacing: Style.space(4)
        readonly property var items: modelData === "c" ? sec.conds : sec.excs
        UiText {
          font.pixelSize: Style.font.caption
          dim: true
          text: lst.modelData === "c" ? "When ALL of these match (each value inside one is \u201cany of\u201d, separate values with commas):"
                                      : "Unless ANY of these match:"
        }
        Repeater {
          model: lst.items.length
          delegate: RowLayout {
            id: prow
            required property int index
            width: lst.width
            spacing: Style.spacing.md
            Dropdown {
              Layout.preferredWidth: Style.space(220)
              showLabel: false
              value: lst.items[prow.index].kind
              options: sec.freeKinds(lst.modelData, prow.index)
              onChanged: function(v) { sec.setPred(lst.modelData, prow.index, "kind", v) }
            }
            TextField {
              visible: sec.isText(lst.items[prow.index].kind)
              Layout.fillWidth: true
              placeholderText: lst.items[prow.index].kind === "from_addresses" ? "name@example.org, other@example.org" : "text, more text"
              text: lst.items[prow.index].text
              onActiveFocusChanged: sec._valueFocus = activeFocus
              onTextEdited: sec.setPred(lst.modelData, prow.index, "text", text)
            }
            Dropdown {
              visible: lst.items[prow.index].kind === "importance"
              Layout.fillWidth: true
              showLabel: false
              value: lst.items[prow.index].text
              options: [{ value: "low", label: "low" }, { value: "normal", label: "normal" }, { value: "high", label: "high" }]
              onChanged: function(v) { sec.setPred(lst.modelData, prow.index, "text", v) }
            }
            Item { visible: lst.items[prow.index].kind === "has_attachments"; Layout.fillWidth: true }
            Button { Layout.preferredWidth: Style.space(32); bordered: true; iconText: "\uf00d"; onClicked: sec.removePred(lst.modelData, prow.index) }
          }
        }
        Button {
          width: Style.space(200)
          visible: sec.canAdd(lst.modelData)
          bordered: true
          iconText: "\uf067"
          text: lst.modelData === "c" ? "Add condition" : "Add exception"
          onClicked: sec.addPred(lst.modelData)
        }
      }
    }

    Toggle {
      width: parent.width
      label: "Stop after this rule"
      description: "Later rules do not run on a message this rule tagged (like \u201cstop processing more rules\u201d)."
      checked: sec.editStop
      onClicked: sec.editStop = !sec.editStop
    }

    UiText {
      width: parent.width
      wrapMode: Text.WordWrap
      font.pixelSize: Style.font.caption
      foreground: sec.previewError !== "" ? Commons.Color.urgent : Commons.Color.foreground
      text: sec.previewError !== "" ? sec.previewError
          : sec.preview ? "Matches " + sec.preview.matched + " of " + sec.preview.scanned + " cached Inbox messages; "
                          + sec.preview.messages + " would gain the tag."
          : "Counting\u2026"
    }

    RowLayout {
      width: parent.width
      Item { Layout.fillWidth: true }
      Button { Layout.preferredWidth: Style.space(96); bordered: true; text: "Cancel"; onClicked: sec.open = false }
      Button { Layout.preferredWidth: Style.space(140); bordered: true; text: "Save rule"; enabled: sec.editName.trim() !== "" && sec.previewError === ""; onClicked: sec.save() }
    }
  }
}
