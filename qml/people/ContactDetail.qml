import QtQuick
import QtQuick.Layouts
import QtQuick.Controls
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"
import "fields.js" as PF

// The detail pane of the People view: every field the contact has, section by section
// (docs/PLAN-contacts.md section 5). Read only; the keys that act on a field live in the registry
// (`w` mail, `S` text, `O` open, `y` copy, `e` edit) and work on the contact under the cursor.
Item {
    id: root

    // GET /contacts/get answer: { ok, contact, folder_name, account_email, sync_state, problem, ... }
    property var detail: null
    property bool loading: false
    property string photoPath: ""      // file path of the contact's picture ("" = none: initials are shown)
    readonly property var c: detail && detail.ok === true ? detail.contact : null
    readonly property var secs: c ? PF.sections(c) : []
    readonly property color dimC: Qt.darker(Commons.Color.foreground, 1.4)

    function scrollBy(px) {
        var max = Math.max(0, flick.contentHeight - flick.height)
        flick.contentY = Math.max(0, Math.min(max, flick.contentY + px))
    }

    Rectangle { anchors.fill: parent; color: Qt.rgba(Commons.Color.background.r, Commons.Color.background.g, Commons.Color.background.b, 0.0) }

    UiText {
        anchors.centerIn: parent
        visible: !root.c
        text: root.loading ? "loading\u2026" : (root.detail && root.detail.ok === false ? root.detail.error : "no contact selected")
        dim: true
    }

    Flickable {
        id: flick
        anchors.fill: parent
        anchors.margins: Style.spacing.lg
        visible: !!root.c
        clip: true
        contentWidth: width
        contentHeight: col.implicitHeight
        boundsBehavior: Flickable.StopAtBounds
        ScrollBar.vertical: ThemedScrollBar {}

        ColumnLayout {
            id: col
            width: flick.width - Style.spacing.lg
            spacing: Style.spacing.md

            // ---- header: initials, name, role
            RowLayout {
                Layout.fillWidth: true
                spacing: Style.spacing.lg
                Rectangle {
                    Layout.preferredWidth: Style.space(52); Layout.preferredHeight: Style.space(52)
                    radius: width / 2
                    color: Qt.rgba(Commons.Color.accent.r, Commons.Color.accent.g, Commons.Color.accent.b, 0.25)
                    clip: true
                    Image {
                        anchors.fill: parent
                        visible: root.photoPath !== "" && status === Image.Ready
                        source: root.photoPath !== "" ? "file://" + root.photoPath : ""
                        fillMode: Image.PreserveAspectCrop
                        asynchronous: true
                        cache: false
                        sourceSize.width: 128; sourceSize.height: 128
                    }
                    UiText {
                        visible: root.photoPath === ""
                        anchors.centerIn: parent
                        text: {
                            var n = root.c ? PF.displayName(root.c) : ""
                            var p = n.split(/\s+/).filter(Boolean)
                            return p.length === 0 ? "?" : (p[0][0] + (p.length > 1 ? p[p.length - 1][0] : "")).toUpperCase()
                        }
                        font.bold: true; font.pixelSize: Style.font.subtitle
                        foreground: Commons.Color.accent
                    }
                }
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 2
                    UiText {
                        Layout.fillWidth: true; elide: Text.ElideRight
                        text: root.c ? PF.displayName(root.c) : ""
                        font.bold: true; font.pixelSize: Style.font.subtitle
                    }
                    UiText {
                        Layout.fillWidth: true; elide: Text.ElideRight; dim: true
                        text: root.c ? [root.c.jobTitle, root.c.companyName].filter(function(x) { return x }).join(" \u00b7 ") : ""
                        visible: text !== ""
                    }
                }
            }

            // ---- a write that did not go through
            UiText {
                Layout.fillWidth: true; wrapMode: Text.WordWrap
                visible: !!root.detail && root.detail.sync_state !== "ok"
                foreground: root.detail && (root.detail.sync_state === "failed" || root.detail.sync_state === "conflict") ? Commons.Color.urgent : Hues.yellow
                text: {
                    if (!root.detail) return ""
                    var st = root.detail.sync_state, p = root.detail.problem
                    if (st === "pending") return "\u27f3 saving to the server\u2026"
                    if (st === "conflict") return "\u26a0 changed on the server since you started: " + (p && p.error ? p.error : "nothing was written") + "   (e to edit again, Ctrl-Z to drop your change)"
                    if (st === "failed") return "\u2715 not saved: " + (p && p.error ? p.error : "the server refused it") + "   (e to edit again, Ctrl-Z to drop your change)"
                    return ""
                }
            }

            Repeater {
                model: root.secs
                ColumnLayout {
                    id: sec
                    required property var modelData
                    Layout.fillWidth: true
                    spacing: 2
                    UiText { text: String(sec.modelData.title).toUpperCase(); dim: true; font.pixelSize: Style.font.caption; font.letterSpacing: 1; Layout.topMargin: Style.spacing.sm }
                    PanelSeparator { Layout.fillWidth: true }
                    Repeater {
                        model: sec.modelData.rows
                        RowLayout {
                            id: rowItem
                            required property var modelData
                            Layout.fillWidth: true
                            spacing: Style.spacing.lg
                            UiText { Layout.preferredWidth: Style.space(92); Layout.alignment: Qt.AlignTop; text: rowItem.modelData.label; dim: true; font.pixelSize: Style.font.bodySmall }
                            SelectableText {
                                Layout.fillWidth: true
                                text: rowItem.modelData.value
                                wrapMode: TextEdit.Wrap
                                foreground: rowItem.modelData.kind === "email" || rowItem.modelData.kind === "url" ? Hues.cyan : Commons.Color.foreground
                            }
                        }
                    }
                }
            }

            // ---- where it lives
            UiText { text: "FILED"; dim: true; font.pixelSize: Style.font.caption; font.letterSpacing: 1; Layout.topMargin: Style.spacing.sm }
            PanelSeparator { Layout.fillWidth: true }
            UiText {
                Layout.fillWidth: true; wrapMode: Text.WordWrap; dim: true; font.pixelSize: Style.font.bodySmall
                text: root.detail ? [root.detail.folder_name, root.detail.account_email].filter(function(x) { return x }).join(" \u00b7 ")
                                  + (root.c && root.c.lastModifiedDateTime ? "\nchanged " + String(root.c.lastModifiedDateTime).replace("T", " ").replace(/:\d\dZ$/, " UTC") : "") : ""
            }
        }
    }
}
