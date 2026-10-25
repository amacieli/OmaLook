import QtQuick
import QtQuick.Layouts
import Quickshell
import qs.Ui
import "CalendarUtil.js" as CU

// CalendarView — header (view dropdown, navigation, title) above one of four
// separate views: DayView, WorkWeekView, FullWeekView, MonthView.
// Data source: http://127.0.0.1:27182/calendar/events?month=YYYY-MM
Rectangle {
    id: root
    color: "#000000"

    // ── theme props forwarded from AppShell ───────────────────────
    property string monoFont:     "JetBrainsMono Nerd Font"
    property color  accentColor:  "#7c6af7"
    property color  successColor: "#51cf66"
    property color  dangerColor:  "#ff6b6b"
    property color  textColor:    "#e8e8e8"
    property color  dimColor:     "#555555"
    property color  veryDimColor: "#2a2a2a"

    // "day" | "workweek" | "week" | "month"  (AppShell keeps this across tab switches)
    property string mode: "month"
    property date   anchorDate: new Date()
    property date   today: new Date()

    // OMALOOK_PORT lets a scratch instance's UI talk to its own backend (same rule as AppState.backendUrl)
    readonly property string backendUrl: "http://127.0.0.1:" + (Quickshell.env("OMALOOK_PORT") || "27182")
    property var  rawEvents: []
    // keyboard cursor: anchorDate is the cursor day; cursorMin the 30-minute slot in the grids
    property int    cursorMin: CU.defaultMin(new Date(), new Date())
    property string accountFilter: ""          // "" = all accounts, else an account email
    property var    accounts: CU.accountsOf(rawEvents)
    property var  events: CU.normalise(CU.filterAccount(rawEvents, accountFilter))
    property int  selIdx: 0                    // which of the events under the cursor is selected
    property bool detailOpen: false
    readonly property var atCursor: CU.eventsAtCursor(events, anchorDate, cursorMin, mode)
    readonly property var selectedEvent: atCursor.length > 0 ? atCursor[Math.min(selIdx, atCursor.length - 1)] : null
    readonly property string selectedId: selectedEvent ? selectedEvent.raw.id : ""
    property bool loading: false
    property int  _reqSerial: 0

    readonly property var viewOptions: [
        { value: "day",      label: "Day" },
        { value: "workweek", label: "Work week" },
        { value: "week",     label: "Week" },
        { value: "month",    label: "Month" }
    ]

    // ── navigation ────────────────────────────────────────────────
    function step(dir) {
        switch (mode) {
        case "day":      anchorDate = CU.addDays(anchorDate, dir);   break
        case "workweek":
        case "week":     anchorDate = CU.addDays(anchorDate, 7 * dir); break
        default:         anchorDate = CU.addMonths(anchorDate, dir)
        }
    }
    function goToday() { today = new Date(); anchorDate = today; cursorMin = CU.defaultMin(today, today) }

    // ── keyboard (called by the key registry; see AppCommands.move/page/edge/activate) ──
    function keyMove(dx, dy) {
        var c = CU.moveCursor({ date: anchorDate, min: cursorMin }, mode, dx, dy)
        cursorMin = c.min
        anchorDate = c.date
        return true
    }
    function keyPage(frac) {
        if (mode === "month") { step(frac < 0 ? -1 : 1); return }
        var c = CU.moveSlots({ date: anchorDate, min: cursorMin }, Math.round(frac * 16))
        cursorMin = c.min
    }
    function keyEdge(bottom) { if (mode !== "month") cursorMin = bottom ? 24 * 60 - CU.SLOT : 0 }
    function keyActivate() {
        if (selectedEvent) detailOpen = !detailOpen
        else if (mode === "month") openDay(anchorDate)
    }
    function cycleSelection(dir) {
        var n = atCursor.length
        if (n > 0) selIdx = (Math.min(selIdx, n - 1) + dir + n) % n
    }
    function cycleAccount() {
        accountFilter = CU.nextAccount(accounts, accountFilter)
        return accountFilter === "" ? "All accounts" : accountFilter
    }
    // Both return a one-line message for the status bar.
    function joinSelected() {
        var url = selectedEvent ? CU.joinUrl(selectedEvent.raw) : ""
        if (url === "") return selectedEvent ? "No join link on this event" : "No event selected"
        Qt.openUrlExternally(url)
        return "Opening " + CU.joinLabel(selectedEvent.raw) + " link"
    }
    function copyJoin() {
        var url = selectedEvent ? CU.joinUrl(selectedEvent.raw) : ""
        if (url === "") return selectedEvent ? "No join link on this event" : "No event selected"
        Quickshell.execDetached(["wl-copy", url])
        return "Copied join link"
    }

    function openDay(d) { anchorDate = d; mode = "day" }

    // inclusive range of dates the current view can show
    function visibleRange() {
        switch (mode) {
        case "day":      return [CU.startOfDay(anchorDate), CU.startOfDay(anchorDate)]
        case "workweek":
        case "week":     var s = CU.weekStart(anchorDate); return [s, CU.addDays(s, 6)]
        default:         var g = CU.monthGridStart(anchorDate); return [g, CU.addDays(g, 41)]
        }
    }

    readonly property string title: {
        var a = anchorDate
        if (mode === "day")
            return CU.DAYS_SHORT[CU.isoIndex(a)] + "  " + a.getDate() + " " + CU.MONTHS[a.getMonth()] + " " + a.getFullYear()
        if (mode === "workweek" || mode === "week") {
            var s = CU.weekStart(a), e = CU.addDays(s, mode === "workweek" ? 4 : 6)
            var l = s.getDate() + (s.getMonth() !== e.getMonth() ? " " + CU.MONTHS[s.getMonth()].substring(0, 3) : "")
            return l + " – " + e.getDate() + " " + CU.MONTHS[e.getMonth()].substring(0, 3) + " " + e.getFullYear()
        }
        return CU.MONTHS[a.getMonth()] + "  " + a.getFullYear()
    }

    // ── data loading: fetch every month the visible range touches ─
    property var _cache: ({})       // "YYYY-MM" -> array

    function _merge() {
        var r = visibleRange(), months = CU.monthsBetween(r[0], r[1]), out = [], seen = {}
        for (var i = 0; i < months.length; i++) {
            var list = _cache[months[i]] || []
            for (var j = 0; j < list.length; j++) {
                var k = JSON.stringify(list[j])
                if (!seen[k]) { seen[k] = true; out.push(list[j]) }
            }
        }
        rawEvents = out
    }

    function loadEvents(force) {
        var r = visibleRange(), months = CU.monthsBetween(r[0], r[1])
        var pending = 0, serial = ++_reqSerial
        if (force) _cache = ({})
        months.forEach(function(m) {
            if (_cache[m] !== undefined) return
            pending++
            var xhr = new XMLHttpRequest()
            xhr.onreadystatechange = function() {
                if (xhr.readyState !== XMLHttpRequest.DONE) return
                var res = []
                if (xhr.status === 200) { try { res = JSON.parse(xhr.responseText) } catch (e) { res = [] } }
                var c = root._cache; c[m] = res; root._cache = c
                if (--pending === 0 && serial === root._reqSerial) { root.loading = false; root._merge() }
            }
            xhr.open("GET", root.backendUrl + "/calendar/events?month=" + m)
            xhr.send()
        })
        if (pending === 0) { loading = false; _merge() } else loading = true
    }

    // Bumped by the app after an accepted / tentative invitation (the backend has just re-expanded
    // the calendar), so the cache is dropped and the visible months are fetched again.
    property int dataRev: 0
    onDataRevChanged:    loadEvents(true)

    onModeChanged:       { selIdx = 0; loadEvents(false) }
    onAnchorDateChanged: { selIdx = 0; loadEvents(false) }
    onCursorMinChanged:  selIdx = 0
    onSelectedEventChanged: if (!selectedEvent) detailOpen = false
    Component.onCompleted: loadEvents(true)

    // ── layout ────────────────────────────────────────────────────
    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // header bar
        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 44
            color: "#000000"
            Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: "#111111" }

            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: 12
                anchors.rightMargin: 12
                spacing: 12

                // view chooser
                Dropdown {
                    id: viewDropdown
                    Layout.preferredWidth: 150
                    Layout.alignment: Qt.AlignVCenter
                    showLabel: false
                    options: root.viewOptions
                    value: root.mode
                    onChanged: function(v) { root.mode = v }
                }

                Text {
                    text: "‹"
                    font.family: root.monoFont; font.pixelSize: 20
                    color: prevMouse.containsMouse ? root.accentColor : root.dimColor
                    MouseArea { id: prevMouse; anchors.fill: parent; anchors.margins: -6
                        hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: root.step(-1) }
                }
                Text {
                    text: "›"
                    font.family: root.monoFont; font.pixelSize: 20
                    color: nextMouse.containsMouse ? root.accentColor : root.dimColor
                    MouseArea { id: nextMouse; anchors.fill: parent; anchors.margins: -6
                        hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: root.step(1) }
                }
                Text {
                    text: "today"
                    font.family: root.monoFont; font.pixelSize: 11
                    color: todayMouse.containsMouse ? root.accentColor : root.dimColor
                    MouseArea { id: todayMouse; anchors.fill: parent; anchors.margins: -6
                        hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: root.goToday() }
                }

                Text {
                    text: root.title
                    font.family: root.monoFont; font.pixelSize: 13; font.bold: true
                    font.letterSpacing: 1
                    color: root.accentColor
                    Layout.fillWidth: true
                    elide: Text.ElideRight
                }

                Text {
                    visible: root.accounts.length > 1
                    text: (root.accountFilter === "" ? "all accounts" : root.accountFilter) + "  (A)"
                    font.family: root.monoFont; font.pixelSize: 10
                    color: root.accountFilter === "" ? root.dimColor : root.accentColor
                }

                Text {
                    visible: root.loading
                    text: "⟳"
                    font.family: root.monoFont; font.pixelSize: 12
                    color: root.dimColor
                    RotationAnimation on rotation {
                        from: 0; to: 360; duration: 1200
                        loops: Animation.Infinite; running: root.loading
                    }
                }
            }
        }

        RowLayout {
          Layout.fillWidth: true
          Layout.fillHeight: true
          spacing: 0

        // active view
        Loader {
            id: viewLoader
            Layout.fillWidth: true
            Layout.fillHeight: true
            sourceComponent: {
                switch (root.mode) {
                case "day":      return dayComponent
                case "workweek": return workWeekComponent
                case "week":     return fullWeekComponent
                default:         return monthComponent
                }
            }
        }

        CalendarDetail {
            visible: root.detailOpen
            Layout.preferredWidth: Math.min(360, root.width * 0.4)
            Layout.fillHeight: true
            event: root.selectedEvent
            accounts: root.accounts
            monoFont: root.monoFont; accentColor: root.accentColor
            textColor: root.textColor; dimColor: root.dimColor
        }
        }
    }


    Component {
        id: dayComponent
        DayView {
            cursorMin: root.cursorMin; selectedId: root.selectedId; accounts: root.accounts;
            anchorDate: root.anchorDate; events: root.events; today: root.today
            monoFont: root.monoFont; accentColor: root.accentColor; textColor: root.textColor
            dimColor: root.dimColor; veryDimColor: root.veryDimColor
            onDayClicked: function(d) { root.anchorDate = d }
        }
    }
    Component {
        id: workWeekComponent
        WorkWeekView {
            cursorMin: root.cursorMin; selectedId: root.selectedId; accounts: root.accounts;
            anchorDate: root.anchorDate; events: root.events; today: root.today
            monoFont: root.monoFont; accentColor: root.accentColor; textColor: root.textColor
            dimColor: root.dimColor; veryDimColor: root.veryDimColor
            onDayClicked: function(d) { root.anchorDate = d }
            onDayActivated: function(d) { root.openDay(d) }
        }
    }
    Component {
        id: fullWeekComponent
        FullWeekView {
            cursorMin: root.cursorMin; selectedId: root.selectedId; accounts: root.accounts;
            anchorDate: root.anchorDate; events: root.events; today: root.today
            monoFont: root.monoFont; accentColor: root.accentColor; textColor: root.textColor
            dimColor: root.dimColor; veryDimColor: root.veryDimColor
            onDayClicked: function(d) { root.anchorDate = d }
            onDayActivated: function(d) { root.openDay(d) }
        }
    }
    Component {
        id: monthComponent
        MonthView {
            selectedId: root.selectedId; accounts: root.accounts;
            anchorDate: root.anchorDate; events: root.events; today: root.today
            monoFont: root.monoFont; accentColor: root.accentColor; textColor: root.textColor
            dimColor: root.dimColor; veryDimColor: root.veryDimColor
            onDayClicked: function(d) { root.anchorDate = d }
            onDayActivated: function(d) { root.openDay(d) }
        }
    }
}
