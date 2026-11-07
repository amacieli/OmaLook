import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../../common"
import ".."

// Rules: the automatic rules your mail server runs on arriving mail, as plain sentences. Exchange's are
// read from the server (Inbox rules), tag rules grouped by tag, everything else listed on its own so a
// forward or delete rule can never hide among the tag rules. Plain tag rules can be added, turned on and
// off, moved and deleted here; every change is checked against the server before and after it is made.
// Forward / delete / importance rules and Outlook's own read-only rules are shown but never changed here.
// Gmail filters need a permission OmaLook does not ask for yet.
SettingsPage {
  id: page

  property var app: null

  title: "Rules"
  description: "What your mail server does with arriving mail. Tag rules can be added, switched off, moved and deleted here; other rules are shown but are changed in Outlook. A new rule only tags mail that arrives from then on."

  readonly property var accounts: app ? app.rulesData.accounts : []

  Component.onCompleted: { if (app) app.loadRules() }
  Timer { interval: 10000; repeat: true; running: !!page.app; onTriggered: page.app.loadRules() }

  // the rule whose delete button is armed (second click deletes), disarmed after a few seconds
  property string armedId: ""
  Timer { id: disarm; interval: 4000; onTriggered: page.armedId = "" }

  // Manual runs, per account: kept here (not in the rows) because the list is rebuilt whenever the data changes.
  //   { running, id ("*" = all tag rules, else a rule id), phase: "tagging" | "sending", tagged, started, notice }
  // A run is "running" from the click until every tag it made has gone to the server (the account's
  // tag_pushes_pending is back to 0), or two minutes, after which it carries on in the background.
  property var runStates: ({})
  function runState(acct) { return runStates[acct] || { running: false, id: "", phase: "", tagged: 0, started: 0, notice: "" } }
  function setRun(acct, patch) {
    var m = {}
    for (var k in runStates) m[k] = runStates[k]
    var cur = runState(acct), n = {}
    for (var c in cur) n[c] = cur[c]
    for (var q in patch) n[q] = patch[q]
    m[acct] = n
    runStates = m
  }
  function pendingFor(acct) {
    for (var i = 0; i < accounts.length; i++) if (accounts[i].account_id === acct) return accounts[i].tag_pushes_pending || 0
    return 0
  }
  function runText(acct) {
    var r = runState(acct)
    if (r.running && r.phase === "tagging") return "Running\u2026"
    if (r.running) return "Tagged " + r.tagged + " message" + (r.tagged === 1 ? "" : "s") + ". Sending the tags to the server, " + pendingFor(acct) + " left\u2026"
    return r.notice
  }
  function checkRuns() {
    var now = Date.now()
    for (var acct in runStates) {
      var r = runStates[acct]
      if (!r.running || r.phase !== "sending") continue
      if (pendingFor(acct) === 0) setRun(acct, { running: false, notice: "Finished: " + r.tagged + " message" + (r.tagged === 1 ? "" : "s") + " tagged and sent to the server." })
      else if (now - r.started > 120000) setRun(acct, { running: false, notice: "Tagged " + r.tagged + ". " + pendingFor(acct) + " are still going to the server in the background." })
    }
  }
  function anyRunning() { for (var a in runStates) if (runStates[a].running) return true; return false }
  Timer { interval: 1500; repeat: true; running: !!page.app && page.anyRunning(); onTriggered: page.app.loadRules(function() { page.checkRuns() }) }

  function position(a, id) {
    for (var i = 0; i < a.order.length; i++) if (a.order[i].id === id) return i + 1
    return 0
  }
  function canMove(a, id, up) {
    for (var i = 0; i < a.order.length; i++) {
      if (a.order[i].id !== id) continue
      var j = up ? i - 1 : i + 1
      return j >= 0 && j < a.order.length && !a.order[j].locked
    }
    return false
  }
  function waiting(a) {
    for (var i = 0; i < a.ops.length; i++) if (a.ops[i].state === "pending") return true
    return false
  }
  // changes still waiting, plus the ones that did not go through in the last ten minutes
  function opsToShow(a) {
    var now = Date.now() / 1000
    return a.ops.filter(function(o) { return o.state === "pending" || (o.state !== "done" && now - o.created_at < 600) })
  }
  function opText(o) {
    var p = o.payload || {}
    var what = o.kind === "add" ? "new rule \u201c" + ((p.body && p.body.displayName) || "") + "\u201d"
             : o.kind === "enable" ? (p.on ? "turn a rule on" : "turn a rule off")
             : o.kind === "delete" ? "delete a rule" : "move a rule"
    if (o.state === "pending") return "Waiting for the server: " + what
    return "Not done (" + what + "): " + o.message
  }

  function when(secs) {
    if (!secs) return "not read yet"
    var d = new Date(secs * 1000)
    return "read " + Qt.formatDateTime(d, "yyyy-MM-dd hh:mm")
  }

  function line(r) {
    var parts = []
    parts.push(r.conditions.length > 0 ? r.conditions.join(", and ") : "every message")
    if (r.exceptions.length > 0) parts.push("except " + r.exceptions.join(", ") )
    return parts.join(", ")
  }

  function otherLine(r) {
    var acts = r.actions.slice()
    if (r.tags.length > 0) acts.unshift("tag " + r.tags.join(", "))
    if (r.stops) acts.push("stop")
    return page.line(r) + "  \u2192  " + acts.join(", ")
  }

  function risky(r) {
    for (var i = 0; i < r.actions.length; i++) if (/DELETE|Deleted|forward|redirect/.test(r.actions[i])) return true
    return false
  }

  Repeater {
    model: page.accounts
    delegate: Column {
      id: acct
      required property var modelData
      width: parent.width
      spacing: Style.space(6)

      property string formTag: ""
      property string formWho: ""
      property var preview: null
      readonly property string previewKind: formWho.indexOf("@") > 0 ? "address" : "domain"
      readonly property string previewWho: previewKind === "address" ? formWho.trim().toLowerCase() : "@" + formWho.trim().replace(/^@/, "").toLowerCase()
      function check() {
        if (formTag.trim() === "" || formWho.trim() === "") return
        var tag = formTag.trim(), kind = previewKind, who = formWho.trim().replace(kind === "domain" ? /^@/ : /^$/, "")
        page.app.ruleCheck(modelData.account_id, tag, kind, who, function(r) { acct.preview = r })
      }

      PanelSectionHeader { text: acct.modelData.email.toUpperCase() + "  (" + acct.modelData.provider + ")" }

      UiText {
        width: parent.width
        wrapMode: Text.WordWrap
        dim: true
        font.pixelSize: Style.font.caption
        text: {
          var m = acct.modelData
          if (m.provider !== "exchange") return "Gmail filters are not read yet: that needs a Gmail settings permission, asked for together with editing them."
          if (m.state === "needs_signin") return "The server refused the rule list. Sign in to this account again (Settings \u203a Accounts) to allow it." + (m.total > 0 ? " Showing the last copy." : "")
          if (m.state === "error") return "Could not read the rules: " + m.error + (m.total > 0 ? " Showing the last copy." : "")
          if (m.state === "none") return "Not read yet."
          return m.total + (m.total === 1 ? " rule, " : " rules, ") + page.when(m.fetched_at) + ". Rules run on the server, in this order; \u201cstop\u201d means a later rule does not see the message."
        }
      }

      // what is waiting for the server / did not go through
      Repeater {
        model: page.opsToShow(acct.modelData)
        delegate: UiText {
          required property var modelData
          width: acct.width
          wrapMode: Text.WordWrap
          font.pixelSize: Style.font.caption
          foreground: modelData.state === "pending" ? Commons.Color.foreground : Commons.Color.urgent
          text: page.opText(modelData)
        }
      }
      UiText {
        visible: !!page.app && page.app.ruleNotice !== "" && acct.modelData.can_write
        width: parent.width
        wrapMode: Text.WordWrap
        font.pixelSize: Style.font.caption
        foreground: !!page.app && page.app.ruleNotice.indexOf("Could not") === 0 ? Commons.Color.urgent : Commons.Color.foreground
        text: page.app ? page.app.ruleNotice : ""
      }

      // run the tag rules now over mail already in the Inbox: count first, then an explicit go-ahead
      property string runTarget: ""        // "" none, "*" every enabled tag rule, else one rule id
      property var runPreview: null
      function startRun(target) {
        if (!page.runState(modelData.account_id).running) page.setRun(modelData.account_id, { notice: "" })
        runTarget = target; runPreview = null
        page.app.ruleRunPreview(modelData.account_id, target === "*" ? "" : target, function(r) { acct.runPreview = r })
      }
      function confirmRun() {
        var p = runPreview && runPreview.preview ? runPreview : null
        var over = p && p.preview.messages > p.limit
        var target = runTarget, account = modelData.account_id
        runTarget = ""; runPreview = null
        page.setRun(account, { running: true, id: target, phase: "tagging", tagged: 0, started: Date.now(), notice: "" })
        page.app.ruleRun(account, target === "*" ? "" : target, over, function(r) {
          var o = r.outcome
          if (!r.ok) page.setRun(account, { running: false, notice: "Could not run: " + (r.error || "no answer") })
          else if (o && o.result === "refused") page.setRun(account, { running: false, notice: "Not run: it would tag " + o.would_change + " messages, over the limit of " + o.limit + "." })
          else if (!o || o.messages === 0) page.setRun(account, { running: false, notice: "Nothing to tag: every matching message already has its tag." })
          else { page.setRun(account, { phase: "sending", tagged: o.messages, started: Date.now() }); page.app.loadRules(function() { page.checkRuns() }) }
        })
      }
      RowLayout {
        visible: acct.modelData.can_write
        width: parent.width
        UiText { Layout.fillWidth: true; dim: true; font.pixelSize: Style.font.caption; text: "Tag mail that is already in the Inbox and missed a rule (the server only runs rules on arriving mail)." }
        Button {
          Layout.preferredWidth: Style.space(220); bordered: true
          enabled: !page.runState(acct.modelData.account_id).running
          iconText: page.runState(acct.modelData.account_id).running && page.runState(acct.modelData.account_id).id === "*" ? "\uf04c" : "\uf04b"
          text: page.runState(acct.modelData.account_id).running && page.runState(acct.modelData.account_id).id === "*" ? "Running\u2026" : "Run all tag rules"
          onClicked: acct.startRun("*")
        }
      }
      RunConfirm { owner: acct; page: page; target: "*"; width: parent.width }

      // add a rule: a sender address or a whole domain -> a tag; checked first, then created last in the list
      Column {
        visible: acct.modelData.can_write
        width: parent.width
        spacing: Style.space(6)
        UiText { text: "Add a rule"; dim: true; font.pixelSize: Style.font.caption }
        RowLayout {
          width: parent.width
          spacing: Style.spacing.lg
          TextField {
            Layout.preferredWidth: parent.width * 0.3
            placeholderText: "Tag"
            text: acct.formTag
            onTextEdited: { acct.formTag = text; acct.preview = null }
          }
          TextField {
            id: senderField
            Layout.fillWidth: true
            placeholderText: "Sender (name@example.org) or domain (example.org)"
            text: acct.formWho
            onTextEdited: { acct.formWho = text; acct.preview = null }
            onAccepted: acct.check()
          }
          Button {
            Layout.preferredWidth: Style.space(96)
            bordered: true
            text: "Check"
            enabled: acct.formTag.trim() !== "" && acct.formWho.trim() !== ""
            onClicked: acct.check()
          }
        }
        UiText {
          visible: acct.preview !== null && acct.preview.ok !== true
          width: parent.width
          wrapMode: Text.WordWrap
          foreground: Commons.Color.urgent
          text: acct.preview && acct.preview.error ? acct.preview.error : ""
        }
        Column {
          visible: acct.preview !== null && acct.preview.ok === true
          width: parent.width
          spacing: Style.space(4)
          UiText {
            width: parent.width
            wrapMode: Text.WordWrap
            text: acct.preview && acct.preview.preview
                  ? "Mail from " + acct.previewWho + " \u2192 tag \u201c" + acct.formTag.trim() + "\u201d. " + acct.preview.preview.matches + " cached message" + (acct.preview.preview.matches === 1 ? "" : "s")
                    + " from there (" + acct.preview.preview.already_tagged + " already tagged). The rule only tags mail that arrives from now on, and goes last in the list."
                  : ""
          }
          Repeater {
            model: acct.preview && acct.preview.preview ? acct.preview.preview.shadowed_by : []
            delegate: UiText {
              required property var modelData
              width: acct.width
              wrapMode: Text.WordWrap
              foreground: Commons.Color.urgent
              font.pixelSize: Style.font.caption
              text: "\u26a0 " + modelData + ". The new rule would never run until you move it above that one."
            }
          }
          UiText {
            visible: !!acct.preview && !!acct.preview.preview && acct.preview.preview.exists
            width: parent.width
            wrapMode: Text.WordWrap
            foreground: Commons.Color.urgent
            text: "An identical rule is already on the server."
          }
          RowLayout {
            width: parent.width
            Item { Layout.fillWidth: true }
            Button { Layout.preferredWidth: Style.space(96); bordered: true; text: "Cancel"; onClicked: acct.preview = null }
            Button {
              Layout.preferredWidth: Style.space(140)
              bordered: true
              iconText: "\uf067"
              text: "Create rule"
              enabled: !!acct.preview && !!acct.preview.preview && !acct.preview.preview.exists
              onClicked: {
                page.app.ruleAdd(acct.modelData.account_id, acct.formTag.trim(), acct.previewKind, acct.previewWho.replace(/^@/, ""))
                acct.preview = null; acct.formWho = ""
              }
            }
          }
        }
      }

      // tag rules, grouped by tag
      Repeater {
        model: acct.modelData.groups
        delegate: Column {
          id: grp
          required property var modelData
          width: parent.width
          spacing: 2
          RowLayout {
            width: parent.width
            spacing: Style.spacing.lg
            Rectangle { Layout.preferredWidth: Style.space(12); Layout.preferredHeight: Style.space(12); radius: 3; color: grp.modelData.color }
            UiText { Layout.fillWidth: true; text: grp.modelData.tag; font.bold: true; elide: Text.ElideRight }
            UiText { text: grp.modelData.rules.length + (grp.modelData.rules.length === 1 ? " rule" : " rules"); dim: true; font.pixelSize: Style.font.caption }
            Button {
              visible: acct.modelData.can_write
              Layout.preferredWidth: Style.space(36)
              bordered: true
              iconText: "\uf067"
              onClicked: { acct.formTag = grp.modelData.tag; senderField.forceActiveFocus() }
            }
          }
          Repeater {
            model: grp.modelData.rules
            delegate: Column {
              id: rwrap
              required property var modelData
              x: Style.space(24)
              width: parent.width - Style.space(24)
              spacing: Style.space(4)
              RowLayout {
              id: rrow
              readonly property var modelData: rwrap.modelData
              width: parent.width
              spacing: Style.spacing.md
              UiText {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                opacity: rrow.modelData.enabled ? 1 : 0.5
                font.pixelSize: Style.font.caption
                text: (page.position(acct.modelData, rrow.modelData.id) > 0 ? "#" + page.position(acct.modelData, rrow.modelData.id) + "  " : "")
                      + (rrow.modelData.enabled ? "" : "(off)  ") + page.line(rrow.modelData) + (rrow.modelData.stops ? "" : "   (keeps going)")
              }
              Button {
                visible: acct.modelData.can_write && rrow.modelData.enabled
                Layout.preferredWidth: Style.space(32)
                bordered: true
                enabled: !page.runState(acct.modelData.account_id).running
                iconText: page.runState(acct.modelData.account_id).running && page.runState(acct.modelData.account_id).id === rrow.modelData.id ? "\uf04c" : "\uf04b"
                onClicked: acct.startRun(rrow.modelData.id)
              }
              Button {
                visible: acct.modelData.can_write
                enabled: page.canMove(acct.modelData, rrow.modelData.id, true)
                Layout.preferredWidth: Style.space(32)
                bordered: true
                iconText: "\uf077"
                onClicked: page.app.ruleMove(acct.modelData.account_id, rrow.modelData.id, true)
              }
              Button {
                visible: acct.modelData.can_write
                enabled: page.canMove(acct.modelData, rrow.modelData.id, false)
                Layout.preferredWidth: Style.space(32)
                bordered: true
                iconText: "\uf078"
                onClicked: page.app.ruleMove(acct.modelData.account_id, rrow.modelData.id, false)
              }
              Button {
                visible: acct.modelData.can_write
                Layout.preferredWidth: Style.space(72)
                bordered: true
                iconText: "\uf011"
                text: rrow.modelData.enabled ? "Off" : "On"
                onClicked: page.app.ruleEnable(acct.modelData.account_id, rrow.modelData.id, !rrow.modelData.enabled)
              }
              Button {
                visible: acct.modelData.can_write
                Layout.preferredWidth: Style.space(page.armedId === rrow.modelData.id ? 96 : 32)
                bordered: true
                iconText: "\uf1f8"
                text: page.armedId === rrow.modelData.id ? "Sure?" : ""
                foreground: page.armedId === rrow.modelData.id ? Commons.Color.urgent : Commons.Color.foreground
                onClicked: {
                  if (page.armedId !== rrow.modelData.id) { page.armedId = rrow.modelData.id; disarm.restart(); return }
                  page.armedId = ""
                  page.app.ruleDelete(acct.modelData.account_id, rrow.modelData.id)
                }
              }
              }
              RunConfirm { owner: acct; page: page; target: rwrap.modelData.id; width: parent.width }
            }
          }
        }
      }

      // everything that is not a plain tag rule
      PanelSectionHeader { visible: acct.modelData.other.length > 0; text: "OTHER RULES (" + acct.modelData.other.length + ")" }
      Repeater {
        model: acct.modelData.other
        delegate: UiText {
          required property var modelData
          width: parent.width
          wrapMode: Text.WordWrap
          opacity: modelData.enabled ? 1 : 0.5
          foreground: page.risky(modelData) ? Commons.Color.urgent : Commons.Color.foreground
          text: (modelData.enabled ? "" : "(off)  ") + page.otherLine(modelData) + (modelData.unknown.length > 0 ? "   [also: " + modelData.unknown.join(", ") + "]" : "")
        }
      }

      UiText {
        visible: acct.modelData.system.length > 0
        width: parent.width
        wrapMode: Text.WordWrap
        dim: true
        font.pixelSize: Style.font.caption
        text: acct.modelData.system.length + " read-only rule" + (acct.modelData.system.length === 1 ? "" : "s") + " made by Outlook itself ("
              + acct.modelData.system.map(function(r) { return r.name || r.id }).join(", ") + "). They are left alone."
      }

      PanelSeparator { width: parent.width }
    }
  }

  UiText {
    visible: page.accounts.length === 0
    width: parent.width
    text: "No accounts yet."
    dim: true
  }

  LocalRulesSection { id: localRules; app: page.app; width: parent.width }
  editing: localRules.editing
}
