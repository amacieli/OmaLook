import QtQuick
import qs.Commons
import qs.Commons as Commons
import "../common"
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

    property string monoFont:     Style.font.family
    property color  accentColor:  Commons.Color.accent
    property color  textColor:    Commons.Color.foreground
    property color  dimColor:     Qt.darker(Commons.Color.foreground, 1.4)
    property color  veryDimColor: Qt.rgba(Commons.Color.foreground.r, Commons.Color.foreground.g, Commons.Color.foreground.b, 0.3)
    property color  lineColor:    Qt.rgba(Commons.Color.foreground.r, Commons.Color.foreground.g, Commons.Color.foreground.b, 0.12)

    signal dayClicked(date day)
    signal dayActivated(date day)

    readonly property date gridStart: CU.monthGridStart(anchorDate)
    readonly property int  headerH: Style.font.caption + 12
    readonly property int  chipH: Style.font.caption + 6
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
                    font.family: root.monoFont; font.pixelSize: Style.font.caption; font.letterSpacing: 1
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
                property int  maxChips: Math.max(0, Math.floor((height - Style.font.body - 12) / (root.chipH + 2)))

                width: root.cellW; height: root.cellH
                // Clockenstein: today's cell is tinted; the keyboard cursor cell gets an accent outline.
                color: isToday ? Qt.rgba(root.accentColor.r, root.accentColor.g, root.accentColor.b, 0.12)
                     : isSel   ? Qt.rgba(root.accentColor.r, root.accentColor.g, root.accentColor.b, 0.05) : "transparent"

                Rectangle { anchors.right: parent.right; width: 1; height: parent.height; color: root.lineColor }
                Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: root.lineColor }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: root.dayClicked(cell.day)
                    onDoubleClicked: root.dayActivated(cell.day)
                }

                // date number, top-right-ish like Clockenstein: "OCT 1" on the first of a month, today in accent
                Text {
                    id: numBox
                    anchors.right: parent.right; anchors.top: parent.top
                    anchors.rightMargin: 6; anchors.topMargin: 4
                    height: Style.font.body + 4
                    text: (cell.day.getDate() === 1 ? CU.MONTHS[cell.day.getMonth()].substring(0, 3).toUpperCase() + " " : "") + cell.day.getDate()
                    font.family: root.monoFont; font.pixelSize: Style.font.body
                    font.bold: cell.isToday || cell.day.getDate() === 1
                    color: cell.isToday ? root.accentColor
                         : !cell.inMonth ? root.veryDimColor
                         : cell.day.getDate() === 1 ? root.accentColor
                         : CU.isWeekend(cell.day) ? root.dimColor : root.textColor
                }
                Rectangle {   // keyboard cursor outline
                    anchors.fill: parent; anchors.margins: 0
                    visible: cell.isSel
                    color: "transparent"; border.width: 1; border.color: root.accentColor
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
                            id: chip
                            width: parent.width; height: root.chipH
                            property var cev: cell.evs[index]
                            property bool sel: root.selectedId !== "" && cev.raw.id === root.selectedId && cell.isSel
                            property color hue: root.accounts.length > 1 ? Hues.accountColor(root.accounts, cev.raw.account_email) : root.accentColor
                            // all-day events are filled bars; timed ones are a dot and a title (Clockenstein)
                            color: cev.allDay ? Qt.rgba(hue.r, hue.g, hue.b, sel ? 0.45 : 0.25)
                                 : sel ? Qt.rgba(hue.r, hue.g, hue.b, 0.25) : "transparent"
                            border.width: sel ? 1 : 0
                            border.color: hue
                            Rectangle { visible: chip.cev.allDay; width: 2; height: parent.height; color: chip.hue }
                            Rectangle { visible: !chip.cev.allDay; x: 4; anchors.verticalCenter: parent.verticalCenter
                                width: 6; height: 6; radius: 3; color: chip.hue }
                            Text {
                                anchors.fill: parent; anchors.leftMargin: chip.cev.allDay ? 6 : 14; anchors.rightMargin: 2
                                verticalAlignment: Text.AlignVCenter
                                text: (chip.cev.allDay ? "" : CU.timeLabel(chip.cev.startMin) + " ") + chip.cev.title
                                elide: Text.ElideRight
                                font.family: root.monoFont; font.pixelSize: Style.font.caption
                                color: root.textColor
                            }
                        }
                    }
                    Text {
                        visible: cell.evs.length > 0 && cell.maxChips < cell.evs.length
                        text: "+" + (cell.evs.length - Math.max(0, cell.maxChips - 1)) + " more"
                        font.family: root.monoFont; font.pixelSize: Style.font.caption
                        color: root.dimColor
                    }
                }
            }
        }
    }
}
