import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../../common"
import ".."
import "gmailfilter.js" as GF

// Gmail filters of one account (OL-040 phase 5): the filters Gmail itself runs on arriving mail, listed in
// plain words, with an add / edit form limited to what Gmail can hold exactly (an address or a domain, an
// optional "unless", one tag). Gmail matches whole words and has no "stop", so anything else is refused
// before it is queued, with the reason. An edit creates the new filter first and removes the old one after.
// Filters OmaLook did not write (archive, forward, subject text ...) are listed read-only with the reason.
Column {
  id: root

  property var app: null
  property var page: null
  property var account: null            // one entry of app.rulesData.accounts (provider "gmail")

  spacing: Style.space(6)

  property string formTag: ""
  property string formFrom: ""
  property string formUnless: ""
  property string editId: ""            // "" = a new filter; else the filter being replaced
  property var preview: null
  property string armedId: ""
  Timer { id: disarm; interval: 4000; onTriggered: root.armedId = "" }

  function ruleJson() { return GF.ruleJson(formTag, formFrom, formUnless) }
  readonly property bool formReady: formTag.trim() !== "" && GF.pred(formFrom) !== null
  function check() { if (formReady) app.filterCheck(account.account_id, ruleJson(), function(r) { root.preview = r }) }
  function reset() { formTag = ""; formFrom = ""; formUnless = ""; editId = ""; preview = null }
  function submit() {
    var json = ruleJson(), tag = formTag.trim(), who = formFrom.trim()
    if (editId !== "") app.filterEdit(account.account_id, editId, json, "Filter changed: " + who + " \u2192 \u201c" + tag + "\u201d.")
    else app.filterAdd(account.account_id, json, "Filter: mail from " + who + " \u2192 tag \u201c" + tag + "\u201d.")
    reset()
  }

  function editable(f) { return GF.editable(f) }
  function load(f) {
    var v = GF.fields(f)
    editId = f.id; preview = null
    formTag = v.tag; formFrom = v.from; formUnless = v.unless
  }

  UiText {
    width: parent.width
    wrapMode: Text.WordWrap
    dim: true
    font.pixelSize: Style.font.caption
    text: {
      var m = root.account
      if (!m) return ""
      if (m.state === "needs_signin") return "Gmail did not let OmaLook read this account's filters. Sign in to this account again (Settings \u203a Accounts) to give it the filters permission." + (m.total > 0 ? " Showing the last copy." : "")
      if (m.state === "error") return "Could not read the filters: " + m.error + (m.total > 0 ? " Showing the last copy." : "")
      if (m.state === "none") return "Not read yet."
      return m.total + (m.total === 1 ? " filter, " : " filters, ") + page.when(m.fetched_at) + ". Gmail runs every filter that matches, in no particular order: there is no \u201cstop\u201d. A filter only tags mail that arrives from then on."
    }
  }

  Column {
    visible: !!root.account && root.account.can_write
    width: parent.width
    spacing: Style.space(6)
    UiText { text: root.editId !== "" ? "Change this filter" : "Add a filter"; dim: true; font.pixelSize: Style.font.caption }
    RowLayout {
      width: parent.width
      spacing: Style.spacing.lg
      TextField {
        Layout.preferredWidth: parent.width * 0.25
        placeholderText: "Tag"
        text: root.formTag
        onTextEdited: { root.formTag = text; root.preview = null }
      }
      TextField {
        Layout.fillWidth: true
        placeholderText: "From: addresses or domains, separated by commas"
        text: root.formFrom
        onTextEdited: { root.formFrom = text; root.preview = null }
        onAccepted: root.check()
      }
    }
    RowLayout {
      width: parent.width
      spacing: Style.spacing.lg
      TextField {
        Layout.fillWidth: true
        placeholderText: "Unless from (optional)"
        text: root.formUnless
        onTextEdited: { root.formUnless = text; root.preview = null }
        onAccepted: root.check()
      }
      Button {
        Layout.preferredWidth: Style.space(96)
        bordered: true
        text: "Check"
        enabled: root.formReady
        onClicked: root.check()
      }
      Button {
        visible: root.editId !== "" || root.formTag !== "" || root.formFrom !== ""
        Layout.preferredWidth: Style.space(96)
        bordered: true
        text: "Clear"
        onClicked: root.reset()
      }
    }
    UiText {
      visible: root.preview !== null && root.preview.ok !== true
      width: parent.width
      wrapMode: Text.WordWrap
      foreground: Commons.Color.urgent
      text: root.preview && root.preview.error ? root.preview.error : ""
    }
    Column {
      visible: root.preview !== null && root.preview.ok === true
      width: parent.width
      spacing: Style.space(4)
      UiText {
        visible: !!root.preview && root.preview.writable === false
        width: parent.width
        wrapMode: Text.WordWrap
        foreground: Commons.Color.urgent
        text: root.preview ? "Gmail cannot hold this: " + root.preview.reason : ""
      }
      UiText {
        visible: !!root.preview && root.preview.writable === true
        width: parent.width
        wrapMode: Text.WordWrap
        text: root.preview && root.preview.preview
              ? "Mail from " + root.formFrom.trim() + (root.formUnless.trim() !== "" ? ", unless from " + root.formUnless.trim() : "") + " \u2192 tag \u201c" + root.formTag.trim() + "\u201d. "
                + root.preview.preview.matched + " message" + (root.preview.preview.matched === 1 ? "" : "s") + " in the cache match now (the filter itself only tags mail that arrives from now on)."
                + (root.editId !== "" ? " The old filter is removed after the new one is in place." : "")
              : ""
      }
      RowLayout {
        width: parent.width
        Item { Layout.fillWidth: true }
        Button { Layout.preferredWidth: Style.space(96); bordered: true; text: "Cancel"; onClicked: root.preview = null }
        Button {
          Layout.preferredWidth: Style.space(160)
          bordered: true
          iconText: root.editId !== "" ? "\uf0c7" : "\uf067"
          text: root.editId !== "" ? "Save changes" : "Create filter"
          enabled: !!root.preview && root.preview.writable === true
          onClicked: root.submit()
        }
      }
    }
  }

  Repeater {
    model: root.account ? root.account.filters : []
    delegate: Column {
      id: frow
      required property var modelData
      width: root.width
      spacing: 2
      RowLayout {
        width: parent.width
        spacing: Style.spacing.md
        Rectangle {
          Layout.preferredWidth: Style.space(12); Layout.preferredHeight: Style.space(12); radius: 3
          color: frow.modelData.tags.length > 0 ? Commons.Color.accent : Commons.Color.foreground; opacity: 0.6
        }
        UiText {
          Layout.fillWidth: true
          wrapMode: Text.WordWrap
          font.pixelSize: Style.font.caption
          text: (frow.modelData.tags.length > 0 ? frow.modelData.tags.join(", ") : "(no tag)") + "  \u2014  " + frow.modelData.text
        }
        Button {
          visible: !!root.account && root.account.can_write && root.editable(frow.modelData)
          Layout.preferredWidth: Style.space(72)
          bordered: true
          iconText: "\uf044"
          text: "Edit"
          onClicked: root.load(frow.modelData)
        }
        Button {
          visible: !!root.account && root.account.can_write && frow.modelData.editable
          Layout.preferredWidth: Style.space(root.armedId === frow.modelData.id ? 96 : 32)
          bordered: true
          iconText: "\uf1f8"
          text: root.armedId === frow.modelData.id ? "Sure?" : ""
          foreground: root.armedId === frow.modelData.id ? Commons.Color.urgent : Commons.Color.foreground
          onClicked: {
            if (root.armedId !== frow.modelData.id) { root.armedId = frow.modelData.id; disarm.restart(); return }
            root.armedId = ""
            root.app.filterDelete(root.account.account_id, frow.modelData.id, "Filter deleted.")
          }
        }
      }
      UiText {
        visible: !frow.modelData.editable
        x: Style.space(24)
        width: parent.width - Style.space(24)
        wrapMode: Text.WordWrap
        dim: true
        font.pixelSize: Style.font.caption
        text: "Read-only here: this filter " + frow.modelData.reason + "."
      }
    }
  }
}
