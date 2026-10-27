import QtQuick
import qs.Commons
import qs.Commons as Commons
import "../common"
import QtQuick.Layouts
import "CalendarUtil.js" as CU

// Shared hour-by-hour grid used by DayView, WorkWeekView and FullWeekView.
// `days` is an array of Date objects; one column is drawn per day.
// Each column header is a cell with the date number in its top-left corner.
Item {
    id: root

    property var    days: []
    property var    events: []          // normalised (CU.normalise)
    property date   selectedDate: new Date()
    property date   today: new Date()
    // keyboard cursor (docs/PLAN-calendar-events.md 5.2): minutes from midnight on selectedDate, -1 = none
    property int    cursorMin: -1
    property string selectedId: ""      // raw id of the selected event
    property var    accounts: []        // [{id,email}]; the bar of each event takes its account colour when > 1

    function barColor(ev) {
        return accounts.length > 1 ? Hues.accountColor(accounts, ev.raw.account_email) : accentColor
    }
    function isSel(ev) { return selectedId !== "" && ev.raw.id === selectedId }
    onCursorMinChanged: ensureCursorVisible()
    onSelectedDateChanged: ensureCursorVisible()
    function ensureCursorVisible() {
        if (cursorMin < 0 || flick.height <= 0) return
        var y = cursorMin / 60 * hourHeight, h = hourHeight * CU.SLOT / 60
        flick._userScrolled = true
        if (y < flick.contentY) flick.contentY = y
        else if (y + h > flick.contentY + flick.height) flick.contentY = Math.min(y + h - flick.height, Math.max(0, flick.contentHeight - flick.height))
    }

    property string monoFont:     Style.font.family
    property color  accentColor:  Commons.Color.accent
    property color  textColor:    Commons.Color.foreground
    property color  dimColor:     Qt.darker(Commons.Color.foreground, 1.4)
    property color  veryDimColor: Qt.rgba(Commons.Color.foreground.r, Commons.Color.foreground.g, Commons.Color.foreground.b, 0.3)
    property color  lineColor:    Qt.rgba(Commons.Color.foreground.r, Commons.Color.foreground.g, Commons.Color.foreground.b, 0.12)

    property int hourHeight:  48
    property int gutterWidth: 60
    property int headerHeight: 56
    property int firstVisibleHour: 7

    signal dayClicked(date day)
    signal dayActivated(date day)       // double-click → open in day view

    readonly property real colWidth: days.length > 0 ? (width - gutterWidth) / days.length : 0

    // "All day" row: always present; collapsed it shows each day's first all-day event and a
    // "+N" badge, expanded it lists every one (scrolling past maxAllDayHeight).
    property bool allDayExpanded: false
    readonly property int allDayCollapsedHeight: 24
    readonly property int maxAllDayHeight: 200
    property int allDayMax: {
        var m = 0
        for (var i = 0; i < days.length; i++)
            m = Math.max(m, CU.allDayOnDay(events, days[i]).length)
        return m
    }
    readonly property int allDayExpandedHeight: Math.min(Math.max(allDayMax, 1) * 20 + 6, maxAllDayHeight)

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // ── day header cells (date number top-left) ───────────────
        Row {
            Layout.fillWidth: true
            Layout.preferredHeight: root.headerHeight
            Item { width: root.gutterWidth; height: root.headerHeight }
            Repeater {
                model: root.days
                delegate: Rectangle {
                    id: hcell
                    property date day: modelData
                    property bool isToday: CU.sameDay(day, root.today)
                    property bool isSel:   CU.sameDay(day, root.selectedDate)
                    width: root.colWidth
                    height: root.headerHeight
                    color: "transparent"

                    Rectangle { anchors.left: parent.left; width: 1; height: parent.height; color: root.lineColor }
                    Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: root.lineColor }

                    // date number — top-left
                    Text {
                        id: num
                        anchors.left: parent.left
                        anchors.top: parent.top
                        anchors.leftMargin: 8
                        anchors.topMargin: 5
                        text: hcell.day.getDate()
                        font.family: root.monoFont
                        font.pixelSize: Style.font.title
                        font.bold: hcell.isToday
                        color: hcell.isToday ? root.accentColor
                             : CU.isWeekend(hcell.day) ? root.dimColor : root.textColor
                    }
                    Text {
                        anchors.left: num.right
                        anchors.leftMargin: 8
                        anchors.baseline: num.baseline
                        text: CU.DAYS_SHORT[CU.isoIndex(hcell.day)].toUpperCase()
                        font.family: root.monoFont
                        font.pixelSize: Style.font.caption
                        font.letterSpacing: 1
                        color: root.dimColor
                    }
                    Rectangle {
                        visible: hcell.isSel
                        anchors.bottom: parent.bottom
                        width: parent.width; height: 2
                        color: root.accentColor
                    }
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: root.dayClicked(hcell.day)
                        onDoubleClicked: root.dayActivated(hcell.day)
                    }
                }
            }
        }

        // ── all-day row (expandable) ──────────────────────────────
        Item {
            id: allDayRow
            Layout.fillWidth: true
            Layout.preferredHeight: root.allDayExpanded ? root.allDayExpandedHeight : root.allDayCollapsedHeight
            clip: true
            Behavior on Layout.preferredHeight { NumberAnimation { duration: 120; easing.type: Easing.OutQuad } }

            Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: root.lineColor }

            // label + chevron: click to expand / collapse
            Item {
                id: allDayLabel
                width: root.gutterWidth; height: root.allDayCollapsedHeight
                Row {
                    anchors.right: parent.right; anchors.rightMargin: 6
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 4
                    Text {
                        text: root.allDayExpanded ? "\u25be" : "\u25b8"
                        font.family: root.monoFont; font.pixelSize: Style.font.caption
                        color: allDayToggle.containsMouse ? root.accentColor : root.dimColor
                    }
                    Text {
                        text: "all day"
                        font.family: root.monoFont; font.pixelSize: Style.font.caption
                        color: allDayToggle.containsMouse ? root.accentColor : root.dimColor
                    }
                }
                MouseArea {
                    id: allDayToggle
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: root.allDayExpanded = !root.allDayExpanded
                }
            }

            Row {
                x: root.gutterWidth
                width: parent.width - root.gutterWidth
                height: parent.height
                Repeater {
                    model: root.days
                    delegate: Item {
                        id: adCell
                        property var list: CU.allDayOnDay(root.events, modelData)
                        width: root.colWidth
                        height: parent.height
                        clip: true

                        Rectangle { anchors.left: parent.left; width: 1; height: parent.height; color: root.lineColor }

                        // collapsed: first event + "+N more"
                        Row {
                            visible: !root.allDayExpanded && adCell.list.length > 0
                            anchors.left: parent.left; anchors.right: parent.right
                            anchors.leftMargin: 3; anchors.rightMargin: 3
                            y: 3
                            spacing: 3
                            Rectangle {
                                width: parent.width - (more.visible ? more.width + parent.spacing : 0)
                                height: 18
                                color: Qt.rgba(root.accentColor.r, root.accentColor.g, root.accentColor.b, adCell.list.length > 0 && root.isSel(adCell.list[0]) ? 0.4 : 0.22)
                                border.width: adCell.list.length > 0 && root.isSel(adCell.list[0]) ? 1 : 0
                                border.color: root.accentColor
                                Text {
                                    anchors.fill: parent; anchors.leftMargin: 5; anchors.rightMargin: 3
                                    verticalAlignment: Text.AlignVCenter
                                    text: adCell.list.length > 0 ? adCell.list[0].title : ""
                                    elide: Text.ElideRight
                                    font.family: root.monoFont; font.pixelSize: Style.font.caption
                                    color: root.textColor
                                }
                            }
                            Text {
                                id: more
                                visible: adCell.list.length > 1
                                height: 18
                                verticalAlignment: Text.AlignVCenter
                                text: "+" + (adCell.list.length - 1)
                                font.family: root.monoFont; font.pixelSize: Style.font.caption; font.bold: true
                                color: root.accentColor
                            }
                        }

                        // expanded: every event, scrolls if there are very many
                        Flickable {
                            visible: root.allDayExpanded
                            anchors.fill: parent
                            anchors.margins: 3
                            contentWidth: width
                            contentHeight: expandedColumn.implicitHeight
                            boundsBehavior: Flickable.StopAtBounds
                            interactive: contentHeight > height
                            Column {
                                id: expandedColumn
                                width: parent.width
                                spacing: 2
                                Repeater {
                                    model: adCell.list
                                    delegate: Rectangle {
                                        width: parent.width; height: 18
                                        color: Qt.rgba(root.accentColor.r, root.accentColor.g, root.accentColor.b, root.isSel(modelData) ? 0.4 : 0.22)
                                        border.width: root.isSel(modelData) ? 1 : 0
                                        border.color: root.accentColor
                                        Text {
                                            anchors.fill: parent; anchors.leftMargin: 5; anchors.rightMargin: 3
                                            verticalAlignment: Text.AlignVCenter
                                            text: modelData.title
                                            elide: Text.ElideRight
                                            font.family: root.monoFont; font.pixelSize: Style.font.caption
                                            color: root.textColor
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // ── scrollable hour grid ──────────────────────────────────
        Flickable {
            id: flick
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            contentWidth: width
            contentHeight: root.hourHeight * 24
            boundsBehavior: Flickable.StopAtBounds
            // Pin the first visible hour until the user scrolls, so the initial
            // position survives layout passes that resize the viewport.
            property bool _userScrolled: false
            onMovingChanged: if (moving) _userScrolled = true
            onHeightChanged: {
                if (!_userScrolled && height > 0)
                    contentY = Math.min(root.firstVisibleHour * root.hourHeight,
                                        Math.max(0, contentHeight - height))
            }

            // hour lines + labels
            Repeater {
                model: 24
                delegate: Item {
                    y: index * root.hourHeight
                    width: flick.width
                    height: root.hourHeight
                    Text {
                        x: 0
                        width: root.gutterWidth - 12
                        horizontalAlignment: Text.AlignRight
                        y: index === 0 ? 0 : -height / 2
                        text: index === 0 ? "" : CU.hourLabel(index)
                        font.family: root.monoFont; font.pixelSize: Style.font.caption
                        color: root.dimColor
                    }
                    Rectangle {
                        x: root.gutterWidth; width: parent.width - root.gutterWidth
                        height: 1; color: root.lineColor
                    }
                }
            }

            // day columns
            Row {
                x: root.gutterWidth
                height: flick.contentHeight
                Repeater {
                    model: root.days
                    delegate: Item {
                        id: col
                        property date day: modelData
                        property bool isToday: CU.sameDay(day, root.today)
                        property var  timed: CU.layoutOverlaps(CU.timedOnDay(root.events, day))
                        width: root.colWidth
                        height: flick.contentHeight

                        Rectangle { width: 1; height: parent.height; color: root.lineColor }
                        Rectangle {
                            anchors.fill: parent
                            visible: col.isToday
                            color: Qt.rgba(root.accentColor.r, root.accentColor.g, root.accentColor.b, 0.04)
                        }
                        MouseArea {
                            anchors.fill: parent
                            onClicked: root.dayClicked(col.day)
                            onDoubleClicked: root.dayActivated(col.day)
                        }

                        // keyboard cursor: the highlighted 30-minute slot
                        Rectangle {
                            visible: root.cursorMin >= 0 && CU.sameDay(col.day, root.selectedDate)
                            y: root.cursorMin / 60 * root.hourHeight
                            width: parent.width; height: root.hourHeight * CU.SLOT / 60
                            color: Qt.rgba(root.accentColor.r, root.accentColor.g, root.accentColor.b, 0.12)
                            Rectangle { width: 2; height: parent.height; color: root.accentColor }
                        }

                        Repeater {
                            model: col.timed
                            delegate: Rectangle {
                                property var ev: modelData
                                x: 2 + (col.width - 4) * ev.col / ev.cols
                                width: (col.width - 4) / ev.cols - 2
                                y: ev.startMin / 60 * root.hourHeight + 1
                                height: Math.max(16, (ev.endMin - ev.startMin) / 60 * root.hourHeight - 2)
                                opacity: ev.raw.pending ? 0.55 : 1      // queued, not yet on the server
                                color: Qt.rgba(root.accentColor.r, root.accentColor.g, root.accentColor.b, root.isSel(ev) ? 0.4 : 0.22)
                                border.width: root.isSel(ev) ? 1 : 0
                                border.color: root.accentColor
                                clip: true
                                Rectangle { width: 2; height: parent.height; color: root.barColor(ev) }
                                Column {
                                    anchors.fill: parent
                                    anchors.leftMargin: 6; anchors.topMargin: 2; anchors.rightMargin: 3
                                    Text {
                                        width: parent.width
                                        text: ev.title
                                        elide: Text.ElideRight
                                        font.family: root.monoFont; font.pixelSize: Style.font.caption; font.bold: true
                                        color: root.textColor
                                    }
                                    Text {
                                        width: parent.width
                                        visible: parent.parent.height > 30
                                        text: CU.timeLabel(ev.startMin) + "–" + CU.timeLabel(ev.endMin)
                                              + (ev.location ? "  " + ev.location : "")
                                        elide: Text.ElideRight
                                        font.family: root.monoFont; font.pixelSize: Style.font.caption
                                        color: root.dimColor
                                    }
                                }
                            }
                        }

                        // now-line
                        Rectangle {
                            visible: col.isToday
                            y: (root.today.getHours() * 60 + root.today.getMinutes()) / 60 * root.hourHeight
                            width: parent.width; height: 1
                            color: Commons.Color.urgent
                        }
                    }
                }
            }
        }
    }
}
