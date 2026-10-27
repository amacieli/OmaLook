import QtQuick
import QtQuick.Layouts
import QtQuick.Controls
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"
import "CalendarUtil.js" as CU

// Calendar sidebar (modelled on Clockenstein): a mini month with the cursor's week shaded, the
// signed-in calendar accounts with their colour, and an agenda (Today / Tomorrow / coming up).
// Every colour and size comes from the Omarchy theme (Commons.Color, Hues, Style).
ColumnLayout {
    id: root

    property date   anchorDate: new Date()
    property date   today: new Date()
    property var    events: []          // normalised events (CU.normalise) around today and the mini month
    property var    accounts: []        // [{id,email}]
    property string accountFilter: ""

    signal dayPicked(date day)
    signal monthStep(int dir)
    signal accountPicked(string email)  // "" = all

    readonly property color fg: Commons.Color.foreground
    readonly property color dimFg: Qt.darker(Commons.Color.foreground, 1.4)
    readonly property date gridStart: CU.monthGridStart(anchorDate)
    readonly property var agendaRows: CU.agenda(events, today, 30, 40)

    spacing: Style.spacing.sm

    // ---- mini month header
    RowLayout {
        Layout.fillWidth: true
        spacing: Style.spacing.sm
        UiText {
            Layout.fillWidth: true
            elide: Text.ElideRight
            text: CU.MONTHS[root.anchorDate.getMonth()] + " " + root.anchorDate.getFullYear()
            foreground: Commons.Color.accent
            font.bold: true
            font.pixelSize: Style.font.subtitle
        }
        PanelActionButton { iconText: "\uf053"; tooltipText: "Previous month"; onClicked: root.monthStep(-1) }
        PanelActionButton { iconText: "\uf054"; tooltipText: "Next month"; onClicked: root.monthStep(1) }
    }

    // ---- mini month grid
    Item {
        id: mini
        Layout.fillWidth: true
        readonly property real cellW: width / 7
        readonly property real cellH: Math.max(Style.font.caption + 8, Math.min(cellW, Style.font.body * 2.2))
        implicitHeight: cellH * 7

        Row {
            Repeater {
                model: ["M", "T", "W", "T", "F", "S", "S"]
                delegate: UiText {
                    width: mini.cellW; height: mini.cellH
                    horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter
                    text: modelData; dim: true
                    font.pixelSize: Style.font.caption
                }
            }
        }
        Grid {
            y: mini.cellH
            columns: 7
            Repeater {
                model: 42
                delegate: Item {
                    id: mc
                    property date day: CU.addDays(root.gridStart, index)
                    property bool inMonth: day.getMonth() === root.anchorDate.getMonth()
                    property bool isToday: CU.sameDay(day, root.today)
                    property bool inWeek: CU.sameDay(CU.weekStart(day), CU.weekStart(root.anchorDate))
                    property bool isCursor: CU.sameDay(day, root.anchorDate)
                    property bool hasEvents: CU.onDay(root.events, day).length > 0
                    width: mini.cellW; height: mini.cellH

                    Rectangle {   // the cursor's week, shaded like Clockenstein
                        anchors.fill: parent
                        visible: mc.inWeek
                        color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.10)
                    }
                    Rectangle {   // today: filled accent disc
                        anchors.centerIn: parent
                        width: Math.min(parent.width, parent.height) - 2; height: width; radius: width / 2
                        visible: mc.isToday
                        color: Commons.Color.accent
                    }
                    Rectangle {   // keyboard cursor day
                        anchors.centerIn: parent
                        width: Math.min(parent.width, parent.height) - 2; height: width; radius: width / 2
                        visible: mc.isCursor && !mc.isToday
                        color: "transparent"; border.width: 1; border.color: Commons.Color.accent
                    }
                    UiText {
                        anchors.centerIn: parent
                        anchors.verticalCenterOffset: -1
                        text: mc.day.getDate()
                        font.pixelSize: Style.font.caption
                        font.bold: mc.isToday
                        foreground: mc.isToday ? Commons.Color.background : root.fg
                        dim: !mc.inMonth && !mc.isToday
                        opacity: mc.inMonth || mc.isToday ? 1 : 0.55
                    }
                    Rectangle {   // event dot
                        visible: mc.hasEvents
                        anchors.horizontalCenter: parent.horizontalCenter
                        anchors.bottom: parent.bottom; anchors.bottomMargin: 2
                        width: 3; height: 3; radius: 1.5
                        color: mc.isToday ? Commons.Color.background : Commons.Color.accent
                    }
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: root.dayPicked(mc.day)
                    }
                }
            }
        }
    }

    PanelSeparator { Layout.fillWidth: true }

    // ---- calendars (accounts)
    UiText { text: "CALENDARS"; dim: true; font.pixelSize: Style.font.caption; font.letterSpacing: 1; visible: root.accounts.length > 0 }
    Repeater {
        model: root.accounts
        delegate: Item {
            id: acRow
            Layout.fillWidth: true
            implicitHeight: Style.font.body + Style.spacing.sm * 2
            property bool active: root.accountFilter === "" || root.accountFilter === modelData.email
            opacity: active ? 1 : 0.45
            Rectangle {
                anchors.fill: parent
                visible: acMouse.containsMouse
                color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.08)
            }
            Rectangle {
                id: dot
                anchors.verticalCenter: parent.verticalCenter
                x: Style.spacing.sm
                width: Style.font.caption; height: width; radius: width / 2
                color: Hues.accountColor(root.accounts, modelData.email)
            }
            UiText {
                anchors.left: dot.right; anchors.leftMargin: Style.spacing.md
                anchors.right: parent.right; anchors.rightMargin: Style.spacing.sm
                anchors.verticalCenter: parent.verticalCenter
                text: modelData.email
                elide: Text.ElideRight
                font.pixelSize: Style.font.bodySmall
            }
            MouseArea {
                id: acMouse
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: root.accountPicked(root.accountFilter === modelData.email ? "" : modelData.email)
            }
        }
    }

    PanelSeparator { Layout.fillWidth: true }

    // ---- agenda
    ListView {
        id: agenda
        Layout.fillWidth: true
        Layout.fillHeight: true
        clip: true
        model: root.agendaRows
        boundsBehavior: Flickable.StopAtBounds
        spacing: 0
        ScrollBar.vertical: ThemedScrollBar {}

        delegate: Item {
            id: ag
            width: agenda.width
            property var row: modelData
            property bool isHead: row.kind === "head"
            height: isHead ? Style.font.caption + Style.spacing.md * 2 + 2 : Style.font.bodySmall * 2 + Style.spacing.sm * 2

            UiText {
                visible: ag.isHead
                anchors.left: parent.left; anchors.leftMargin: Style.spacing.sm
                anchors.bottom: parent.bottom; anchors.bottomMargin: 2
                text: ag.isHead ? ag.row.text : ""
                dim: ag.row.text !== "Today" && ag.row.text !== "Tomorrow"
                foreground: ag.row.text === "Today" || ag.row.text === "Tomorrow" ? Commons.Color.accent : root.fg
                font.pixelSize: Style.font.caption
                font.bold: true
            }
            Item {
                visible: !ag.isHead
                anchors.fill: parent
                Rectangle {
                    anchors.fill: parent
                    visible: evMouse.containsMouse
                    color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.08)
                }
                Rectangle {
                    id: bar
                    x: 0; width: 3; height: parent.height - 4; anchors.verticalCenter: parent.verticalCenter
                    color: ag.isHead ? "transparent" : Hues.accountColor(root.accounts, ag.row.ev.raw.account_email)
                }
                Column {
                    anchors.left: bar.right; anchors.leftMargin: Style.spacing.md
                    anchors.right: parent.right; anchors.rightMargin: Style.spacing.sm
                    anchors.verticalCenter: parent.verticalCenter
                    UiText {
                        width: parent.width; elide: Text.ElideRight
                        text: ag.isHead ? "" : ag.row.time
                        dim: true; font.pixelSize: Style.font.caption
                    }
                    UiText {
                        width: parent.width; elide: Text.ElideRight
                        text: ag.isHead ? "" : ag.row.ev.title
                        font.pixelSize: Style.font.bodySmall; font.bold: true
                    }
                }
                MouseArea {
                    id: evMouse
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: root.dayPicked(ag.row.date)
                }
            }
        }
    }
    UiText {
        visible: root.agendaRows.length === 0
        Layout.fillWidth: true
        text: "Nothing coming up"
        dim: true; font.pixelSize: Style.font.bodySmall
    }
}
