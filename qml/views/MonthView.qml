import QtQuick
import QtQuick.Layouts
import "CalendarUtil.js" as CU

// Month view — 6×7 grid. Each cell has its date number top-left, followed
// by up to a few event chips and a "+N more" line.
Item {
    id: root

    property date   anchorDate: new Date()
    property var    events: []          // normalised (CU.normalise)
    property date   today: new Date()
    property string selectedId: ""      // raw id of the selected event
    property var    accounts: []        // [{id,email}]; chips take their account colour when > 1

    property string monoFont:     "JetBrainsMono Nerd Font"
    property color  accentColor:  "#7c6af7"
    property color  textColor:    "#e8e8e8"
    property color  dimColor:     "#555555"
    property color  veryDimColor: "#2a2a2a"
    property color  lineColor:    "#141414"

    signal dayClicked(date day)
    signal dayActivated(date day)

    readonly property date gridStart: CU.monthGridStart(anchorDate)
    readonly property int  headerH: 22
    readonly property real cellW: width / 7
    readonly property real cellH: Math.max(1, (height - headerH) / 6)

    // weekday header
    Row {
        id: hdr
        width: parent.width; height: root.headerH
        Repeater {
            model: CU.DAYS_SHORT
            delegate: Item {
                width: root.cellW; height: root.headerH
                Text {
                    anchors.left: parent.left; anchors.leftMargin: 8
                    anchors.verticalCenter: parent.verticalCenter
                    text: modelData.toUpperCase()
                    font.family: root.monoFont; font.pixelSize: 10; font.letterSpacing: 1
                    color: index >= 5 ? root.veryDimColor : root.dimColor
                }
            }
        }
    }

    Grid {
        anchors.top: hdr.bottom
        columns: 7
        Repeater {
            model: 42
            delegate: Rectangle {
                id: cell
                property date day: CU.addDays(root.gridStart, index)
                property bool inMonth: day.getMonth() === root.anchorDate.getMonth()
                property bool isToday: CU.sameDay(day, root.today)
                property bool isSel:   CU.sameDay(day, root.anchorDate)
                property var  evs:     CU.onDay(root.events, day)
                property int  maxChips: Math.max(0, Math.floor((height - 26) / 17))

                width: root.cellW; height: root.cellH
                color: isSel ? Qt.rgba(root.accentColor.r, root.accentColor.g, root.accentColor.b, 0.06) : "transparent"

                Rectangle { anchors.right: parent.right; width: 1; height: parent.height; color: root.lineColor }
                Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: root.lineColor }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: root.dayClicked(cell.day)
                    onDoubleClicked: root.dayActivated(cell.day)
                }

                // date number — top-left
                Item {
                    id: numBox
                    anchors.left: parent.left; anchors.top: parent.top
                    anchors.leftMargin: 4; anchors.topMargin: 3
                    width: Math.max(22, numText.implicitWidth + 10); height: 20
                    Rectangle {
                        anchors.fill: parent
                        visible: cell.isToday
                        color: root.accentColor
                    }
                    Text {
                        id: numText
                        anchors.centerIn: parent
                        text: cell.day.getDate()
                        font.family: root.monoFont; font.pixelSize: 12
                        font.bold: cell.isToday
                        color: cell.isToday ? "#000000"
                             : !cell.inMonth ? root.veryDimColor
                             : CU.isWeekend(cell.day) ? root.dimColor : root.textColor
                    }
                }

                // event chips
                Column {
                    anchors.left: parent.left; anchors.right: parent.right
                    anchors.top: numBox.bottom
                    anchors.leftMargin: 3; anchors.rightMargin: 3; anchors.topMargin: 2
                    spacing: 2
                    visible: cell.inMonth || cell.evs.length > 0
                    opacity: cell.inMonth ? 1.0 : 0.45

                    Repeater {
                        model: Math.min(cell.evs.length, cell.maxChips >= cell.evs.length ? cell.evs.length : Math.max(0, cell.maxChips - 1))
                        delegate: Rectangle {
                            width: parent.width; height: 15
                            property var cev: cell.evs[index]
                            property bool sel: root.selectedId !== "" && cev.raw.id === root.selectedId && cell.isSel
                            color: Qt.rgba(root.accentColor.r, root.accentColor.g, root.accentColor.b, sel ? 0.4 : 0.22)
                            border.width: sel ? 1 : 0
                            border.color: root.accentColor
                            Rectangle { width: 2; height: parent.height
                                color: root.accounts.length > 1 ? CU.accountColor(root.accounts, parent.cev.raw.account_email) : root.accentColor }
                            Text {
                                anchors.fill: parent; anchors.leftMargin: 6; anchors.rightMargin: 2
                                verticalAlignment: Text.AlignVCenter
                                property var ev: cell.evs[index]
                                text: (ev.allDay ? "" : CU.timeLabel(ev.startMin) + " ") + ev.title
                                elide: Text.ElideRight
                                font.family: root.monoFont; font.pixelSize: 9
                                color: root.textColor
                            }
                        }
                    }
                    Text {
                        visible: cell.evs.length > 0 && cell.maxChips < cell.evs.length
                        text: "+" + (cell.evs.length - Math.max(0, cell.maxChips - 1)) + " more"
                        font.family: root.monoFont; font.pixelSize: 9
                        color: root.dimColor
                    }
                }
            }
        }
    }
}
