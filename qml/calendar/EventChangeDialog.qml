import QtQuick
import QtQuick.Layouts
import QtQuick.Controls
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"

// The question asked before an event change is queued (docs/PLAN-calendar-events.md 6, 12).
//   mode "scope":   a recurring event: this occurrence or the whole series. o / s pick, Esc backs out.
//   mode "confirm": what happens, how many people it reaches, and (cancel / decline) a message line.
//                   Enter sends, Esc backs out. Pure view: it reports intent; CalendarView posts the request.
FocusScope {
    id: root

    property string mode: "confirm"
    property string heading: ""
    property string detail: ""
    property string askLine: ""        // the scope question (scope mode)
    property bool   allowMessage: false
    property string status: ""         // a refusal from the owner, shown in red

    signal scopePicked(string scope)   // "occurrence" | "series"
    signal confirmed(string message)
    signal dismissed()
    onVisibleChanged: if (visible) focusFirst()

    readonly property color fg: Commons.Color.foreground

    function focusFirst() {
        if (mode === "confirm" && allowMessage) messageField.forceActiveFocus()
        else root.forceActiveFocus()
    }
    onModeChanged: focusFirst()

    Component.onCompleted: focusFirst()

    Keys.onPressed: function(e) {
        if (e.key === Qt.Key_Escape) { dismissed(); e.accepted = true; return }
        if (mode === "scope") {
            if (e.key === Qt.Key_O) { scopePicked("occurrence"); e.accepted = true }
            else if (e.key === Qt.Key_S) { scopePicked("series"); e.accepted = true }
            return
        }
        if (e.key === Qt.Key_Return || e.key === Qt.Key_Enter) { confirmed(messageField.text.trim()); e.accepted = true }
    }

    // The card sits over the grid, centred, like the editor it replaces the grid under.
    Rectangle {
        id: card
        anchors.centerIn: parent
        width: Math.min(Style.space(440), parent.width - Style.spacing.xl * 2)
        height: column.implicitHeight + Style.spacing.xl * 2
        radius: Style.cornerRadius
        color: Qt.rgba(Commons.Color.background.r, Commons.Color.background.g, Commons.Color.background.b, 1.0)
        border.width: 1
        border.color: root.status !== "" ? Commons.Color.urgent : Commons.Color.accent

        ColumnLayout {
            id: column
            anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
            anchors.margins: Style.spacing.xl
            spacing: Style.spacing.md

            UiText { Layout.fillWidth: true; text: root.heading; font.bold: true; font.pixelSize: Style.font.subtitle; foreground: Commons.Color.accent; wrapMode: Text.WordWrap }

            UiText {
                visible: root.mode === "scope"
                Layout.fillWidth: true; wrapMode: Text.WordWrap
                text: root.askLine
            }
            UiText {
                visible: root.mode === "scope"
                Layout.fillWidth: true
                text: "o   this occurrence only\ns   the whole series\nEsc   back"
                dim: true; font.pixelSize: Style.font.bodySmall
            }

            UiText {
                visible: root.mode === "confirm"
                Layout.fillWidth: true; wrapMode: Text.WordWrap
                text: root.detail
                foreground: root.fg
            }
            TextField {
                id: messageField
                visible: root.mode === "confirm" && root.allowMessage
                Layout.fillWidth: true
                placeholderText: "Message to attendees (optional)"
                activeFocusOnTab: true
            }

            UiText {
                visible: root.status !== ""
                Layout.fillWidth: true; wrapMode: Text.WordWrap
                text: root.status
                foreground: Commons.Color.urgent
                font.pixelSize: Style.font.bodySmall
            }

            UiText {
                visible: root.mode === "confirm"
                Layout.fillWidth: true
                text: "Enter confirm    Esc back    Ctrl-Z undoes it in the bar"
                dim: true; font.pixelSize: Style.font.caption
            }
        }
    }
}
