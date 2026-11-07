import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../../common"

// The count-then-confirm panel and the progress line of a manual rule run, shown right under the thing that
// was pressed: under a rule's row for that rule, under the "Run all tag rules" button for the lot.
Column {
  id: root

  property var owner: null        // the account block (RulesPage delegate) that owns runTarget / runPreview
  property var page: null        // RulesPage: run state per account
  property string target: ""     // "*" or a rule id

  readonly property string account: owner ? owner.modelData.account_id : ""
  readonly property var rs: page ? page.runState(account) : ({ running: false, id: "" })
  readonly property string progress: (rs.id === target || (rs.running && rs.id === target)) ? page.runText(account) : ""
  readonly property var pv: owner ? owner.runPreview : null

  spacing: Style.space(4)

  // 1. count first
  Column {
    visible: !!root.owner && root.owner.runTarget === root.target
    width: parent.width
    spacing: Style.space(4)
    UiText {
      width: parent.width
      wrapMode: Text.WordWrap
      font.pixelSize: Style.font.caption
      foreground: root.pv && (root.pv.ok !== true || root.pv.preview.messages > root.pv.limit) ? Commons.Color.urgent : Commons.Color.foreground
      text: !root.pv ? "Counting\u2026"
          : root.pv.ok !== true ? root.pv.error
          : (root.target === "*" ? "Running " + root.pv.rules + " enabled tag rules, in order. " : "Running this rule. ")
            + "Inbox mail they match: " + root.pv.preview.matched + ". They would tag " + root.pv.preview.messages + " message" + (root.pv.preview.messages === 1 ? "" : "s")
            + " that lack the tag" + (root.pv.preview.messages > 0 ? " (" + Object.keys(root.pv.preview.by_tag).map(function(t) { return t + " " + root.pv.preview.by_tag[t] }).join(", ") + ")" : "")
            + (root.pv.preview.messages > root.pv.limit ? ". That is more than your limit of " + root.pv.limit + ", so it needs your explicit go-ahead." : ".")
            + (root.target === "*" && root.pv.left_out.length > 0 ? " Left out (not plain tag rules): " + root.pv.left_out.join(", ") + "." : "")
    }
    RowLayout {
      width: parent.width
      Item { Layout.fillWidth: true }
      Button { Layout.preferredWidth: Style.space(96); bordered: true; text: "Cancel"; onClicked: { root.owner.runTarget = ""; root.owner.runPreview = null } }
      Button {
        Layout.preferredWidth: Style.space(180); bordered: true
        enabled: !!root.pv && root.pv.ok === true && root.pv.preview.messages > 0
        text: root.pv && root.pv.ok === true && root.pv.preview.messages > root.pv.limit ? "Run anyway" : "Run now"
        onClicked: root.owner.confirmRun()
      }
    }
  }

  // 2. progress / result of the last run of this target
  UiText {
    visible: root.progress !== ""
    width: parent.width
    wrapMode: Text.WordWrap
    font.pixelSize: Style.font.caption
    foreground: root.progress.indexOf("Could not") === 0 || root.progress.indexOf("Not run") === 0 ? Commons.Color.urgent : Commons.Color.foreground
    text: root.progress
  }
}
