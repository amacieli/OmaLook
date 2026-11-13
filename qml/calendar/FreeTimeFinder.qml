import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"
import "../views/CalendarUtil.js" as CU
import "../state"

// Free-time finder (docs/PLAN-calendar-events.md 5.6, track A). A panel over the calendar, opened from the event
// editor (its attendees and length) or from the calendar (the selected event's attendees). Asks GET /calendar/find
// on the chosen work account and lists the ranked slots. Enter hands the slot back; the owner fills the editor
// or opens a new one. Pure view: no write happens here.
//
// Keys (the panel owns the keyboard while open):
//   j / k or Down / Up  move the selection      Enter  use the selected slot
//   + / -               length +-15 min         [ / ]  shift the window one week
//   a                   check your other work accounts too (opt-in; off by default)      Esc  back, no change
FocusScope {
    id: root

    // ---- inputs from the owner
    property string accountId: ""            // the calendar account the answer is for (Exchange)
    property string accountLabel: ""         // its address, shown in the header
    property var    requiredEmails: []       // [string]
    property var    optionalEmails: []       // [string]
    property var    others: []               // [{id, email}] other signed-in work accounts (the opt-in row)
    property date   initialStart: new Date()
    property int    initialMinutes: 30
    property string title: "Find a time"

    signal pickRequested(date start, date end)
    signal closeRequested()

    // ---- state
    property int    minutes: initialMinutes
    property int    windowDays: 5
    property date   fromDay: new Date()
    property bool   useOthers: false
    property var    slots: []
    property int    sel: 0
    property string message: ""              // an error, or "" 
    property string reason: ""               // why the answer is partial (own calendar only, unknown people)
    property var    unknownRequired: []
    property var    otherUnread: []
    property int    otherChecked: 0
    property bool   loading: false
    property int    _serial: 0

    readonly property color fg: Commons.Color.foreground
    readonly property color dim: Qt.darker(Commons.Color.foreground, 1.4)
    readonly property string windowText: {
        var a = fromDay, b = CU.addDays(fromDay, windowDays - 1)
        return CU.DAYS_SHORT[CU.isoIndex(a)] + " " + a.getDate() + " " + CU.MONTHS[a.getMonth()].substring(0, 3)
            + " \u2013 " + CU.DAYS_SHORT[CU.isoIndex(b)] + " " + b.getDate() + " " + CU.MONTHS[b.getMonth()].substring(0, 3)
    }

    function _pad(n) { return (n < 10 ? "0" : "") + n }
    function _ymd(d) { return d.getFullYear() + "-" + _pad(d.getMonth() + 1) + "-" + _pad(d.getDate()) }
    // "2026-11-16T10:00:00" (the backend's local wall clock) -> Date, or null
    function _parse(s) {
        var m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})/.exec(String(s || ""))
        return m ? new Date(+m[1], +m[2] - 1, +m[3], +m[4], +m[5]) : null
    }
    function _slotLabel(s) {
        var a = _parse(s.start), b = _parse(s.end)
        if (!a || !b) return ""
        return CU.DAYS_SHORT[CU.isoIndex(a)] + " " + a.getDate() + " " + CU.MONTHS[a.getMonth()].substring(0, 3)
            + "   " + CU.timeLabel(a.getHours() * 60 + a.getMinutes()) + " \u2013 " + CU.timeLabel(b.getHours() * 60 + b.getMinutes())
    }
    function _people(list) { return list && list.length ? list.join(", ") : "" }

    function query() {
        if (accountId === "") { message = "No work account to check free time on."; slots = []; return }
        var to = CU.addDays(fromDay, windowDays)
        var q = "account=" + encodeURIComponent(accountId)
              + "&attendees=" + encodeURIComponent(requiredEmails.join(","))
              + "&optional=" + encodeURIComponent(optionalEmails.join(","))
              + (useOthers && others.length > 0 ? "&accounts=" + encodeURIComponent(others.map(function(o) { return o.id }).join(",")) : "")
              + "&from=" + _ymd(fromDay) + "&to=" + _ymd(to) + "&duration=" + minutes + "&limit=5"
        var serial = ++_serial
        loading = true
        Backend.request("GET", "/calendar/find?" + q, function(xhr) {
            if (serial !== root._serial) return
            loading = false
            var r = {}
            try { r = JSON.parse(xhr.responseText) } catch (e) { r = { ok: false, error: xhr.status === 0 ? "the backend is not answering" : "unreadable answer" } }
            _apply(r)
        })
    }

    function _apply(r) {
        if (!r.ok) { message = r.error || "free time could not be checked"; slots = []; reason = ""; sel = 0; return }
        message = ""
        slots = r.slots || []
        reason = r.reason || ""
        unknownRequired = r.unknown_required || []
        otherUnread = (r.other_accounts && r.other_accounts.unread) || []
        otherChecked = (r.other_accounts && r.other_accounts.checked) || 0
        sel = Math.min(sel, Math.max(0, slots.length - 1))
    }

    function pick(i) {
        var s = slots[i]
        if (!s) return
        var a = _parse(s.start), b = _parse(s.end)
        if (a && b) pickRequested(a, b)
    }

    function move(d) { if (slots.length > 0) sel = Math.max(0, Math.min(slots.length - 1, sel + d)) }

    Timer { id: queryTimer; interval: 150; onTriggered: root.query() }
    onMinutesChanged: queryTimer.restart()
    onFromDayChanged: queryTimer.restart()
    onUseOthersChanged: queryTimer.restart()
    onAccountIdChanged: queryTimer.restart()

    Component.onCompleted: {
        var s = initialStart
        fromDay = new Date(s.getFullYear(), s.getMonth(), s.getDate())
        forceActiveFocus()
        query()
    }

    Keys.onPressed: function(e) {
        var t = e.text
        if (e.key === Qt.Key_Escape) { closeRequested(); e.accepted = true }
        else if (e.key === Qt.Key_Return || e.key === Qt.Key_Enter) { pick(sel); e.accepted = true }
        else if (e.key === Qt.Key_J || e.key === Qt.Key_Down) { move(1); e.accepted = true }
        else if (e.key === Qt.Key_K || e.key === Qt.Key_Up) { move(-1); e.accepted = true }
        else if (t === "+" || t === "=") { minutes = Math.min(480, minutes + 15); e.accepted = true }
        else if (t === "-" || t === "_") { minutes = Math.max(15, minutes - 15); e.accepted = true }
        else if (t === "]") { fromDay = CU.addDays(fromDay, 7); e.accepted = true }
        else if (t === "[") { fromDay = CU.addDays(fromDay, -7); e.accepted = true }
        else if (t === "a") { if (others.length > 0) useOthers = !useOthers; e.accepted = true }
    }

    Rectangle {
        anchors.fill: parent
        color: Qt.rgba(Commons.Color.background.r, Commons.Color.background.g, Commons.Color.background.b, 1.0)
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: Style.spacing.xl
        spacing: Style.spacing.md

        RowLayout {
            Layout.fillWidth: true
            UiText { text: root.title; font.bold: true; font.pixelSize: Style.font.subtitle; foreground: Commons.Color.accent }
            UiText { text: "\u00b7  " + root.accountLabel; dim: true; font.pixelSize: Style.font.subtitle; elide: Text.ElideRight; Layout.fillWidth: true }
            UiText { text: "Esc back    Enter use slot"; dim: true; font.pixelSize: Style.font.caption }
        }
        PanelSeparator { Layout.fillWidth: true }

        GridLayout {
            Layout.fillWidth: true
            columns: 2
            columnSpacing: Style.spacing.xl
            rowSpacing: Style.spacing.sm

            UiText { text: "Length"; dim: true; Layout.preferredWidth: Style.space(84) }
            UiText { text: root.minutes + " min    +/- change"; foreground: root.fg }

            UiText { text: "Window"; dim: true; Layout.preferredWidth: Style.space(84) }
            UiText { text: root.windowText + "    [ ] week"; foreground: root.fg }

            UiText { text: "Hours"; dim: true; Layout.preferredWidth: Style.space(84) }
            UiText { text: "Mon-Fri 09:00-17:30 (default; a person's own hours are used when reported)"; dim: true; wrapMode: Text.WordWrap; Layout.fillWidth: true }

            UiText { text: "Required"; dim: true; Layout.preferredWidth: Style.space(84); visible: root.requiredEmails.length > 0 }
            UiText { visible: root.requiredEmails.length > 0; text: root.requiredEmails.join(", "); foreground: root.fg; wrapMode: Text.WordWrap; Layout.fillWidth: true }

            UiText { text: "Optional"; dim: true; Layout.preferredWidth: Style.space(84); visible: root.optionalEmails.length > 0 }
            UiText { visible: root.optionalEmails.length > 0; text: root.optionalEmails.join(", ") + "  (ties only)"; foreground: root.fg; wrapMode: Text.WordWrap; Layout.fillWidth: true }

            UiText { text: "Others"; dim: true; Layout.preferredWidth: Style.space(84); visible: root.others.length > 0 }
            UiText {
                visible: root.others.length > 0
                text: (root.useOthers ? "on" : "off") + ": " + root.others.map(function(o) { return o.email }).join(", ") + "    a toggle"
                foreground: root.useOthers ? Commons.Color.accent : root.fg; wrapMode: Text.WordWrap; Layout.fillWidth: true
            }
        }

        PanelSeparator { Layout.fillWidth: true }

        UiText {
            visible: root.message !== ""
            text: root.message; foreground: Commons.Color.urgent
            wrapMode: Text.WordWrap; Layout.fillWidth: true
        }
        UiText {
            visible: root.loading && root.slots.length === 0 && root.message === ""
            text: "checking\u2026"; dim: true
        }

        // ranked slots: best first. A required person without free/busy data is flagged with "?", never assumed free.
        ColumnLayout {
            Layout.fillWidth: true
            spacing: Style.spacing.xs
            Repeater {
                model: root.slots
                delegate: Rectangle {
                    required property var modelData
                    required property int index
                    Layout.fillWidth: true
                    implicitHeight: slotRow.implicitHeight + Style.spacing.md * 2
                    radius: Style.cornerRadius
                    color: index === root.sel ? Qt.rgba(Commons.Color.accent.r, Commons.Color.accent.g, Commons.Color.accent.b, 0.18) : "transparent"
                    RowLayout {
                        id: slotRow
                        anchors.left: parent.left; anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.leftMargin: Style.spacing.lg; anchors.rightMargin: Style.spacing.lg
                        spacing: Style.spacing.md
                        UiText {
                            text: (index + 1) + "."; dim: true; Layout.preferredWidth: Style.space(20)
                        }
                        UiText {
                            text: root._slotLabel(modelData)
                            foreground: modelData.confirmed ? root.fg : Commons.Color.urgent
                            font.bold: index === root.sel
                        }
                        UiText {
                            Layout.fillWidth: true; elide: Text.ElideRight
                            dim: true; font.pixelSize: Style.font.caption
                            text: {
                                var bits = []
                                if (modelData.confirmed) bits.push("free for everyone")
                                else bits.push("? unknown: " + modelData.unknown.join(", "))
                                if (modelData.optional_busy && modelData.optional_busy.length) bits.push("optional busy: " + modelData.optional_busy.length)
                                if (modelData.lunch) bits.push("lunch")
                                if (modelData.next_to_busy) bits.push("next to a busy block")
                                return bits.join("  \u00b7  ")
                            }
                        }
                    }
                    MouseArea {
                        anchors.fill: parent
                        onClicked: { root.sel = index; root.pick(index) }
                    }
                }
            }
            UiText {
                visible: !root.loading && root.slots.length === 0 && root.message === ""
                text: "No time in this window fits " + root.minutes + " minutes. Try [ ] for another week or - for a shorter length."
                dim: true; wrapMode: Text.WordWrap; Layout.fillWidth: true
            }
        }

        Item { Layout.fillHeight: true }

        PanelSeparator { Layout.fillWidth: true }
        UiText {
            Layout.fillWidth: true; wrapMode: Text.WordWrap
            text: {
                var bits = []
                if (root.loading) bits.push("checking\u2026")
                if (root.reason !== "") bits.push(root.reason)
                if (root.unknownRequired.length > 0) bits.push(root.unknownRequired.length + " required person(s) unknown: " + root.unknownRequired.join(", "))
                if (root.useOthers && root.otherChecked > 0) bits.push("your other accounts: " + root.otherChecked + " checked")
                for (var i = 0; i < root.otherUnread.length; i++) bits.push("not checked: " + (root.otherUnread[i].account_id || "account") + " (" + (root.otherUnread[i].reason || "") + ")")
                return bits.join("   ")
            }
            dim: true; font.pixelSize: Style.font.caption
        }
        UiText {
            Layout.fillWidth: true
            text: "j k move    Enter use slot    + - length    [ ] week    a other accounts    Esc back"
            dim: true; font.pixelSize: Style.font.caption
        }
    }
}
