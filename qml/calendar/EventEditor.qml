import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"
import "when.js" as When
import "recurrence.js" as Rep
import "draft.js" as Draft

// New-event editor (docs/PLAN-calendar-events.md 5.3, phase 2: personal entries, with repeat).
// It sits over the calendar grid. Pure view: it owns the fields and reports intent through signals;
// CalendarView posts the request, shows the pending mark and the undo bar.
//
// Keys (text fields own theirs; the app's global bindings are off while this is open):
//   Tab / Shift-Tab  next / previous field        Ctrl-Enter  save (the undo window starts)
//   Esc              close (asks once if edited)  Up / Down on When: start +-15 min,
//   Shift-Up/Down    +-1 day                      Alt-Up/Down: length +-15 min
//   Left / Right     (Account and Show-as rows) change the choice
FocusScope {
    id: root

    // ---- inputs from the owner
    property date   now: new Date()
    property date   initialStart: new Date()
    property int    defaultMinutes: 30
    property int    defaultReminder: 15
    property int    defaultSeriesMonths: 12
    property var    accounts: []            // [{id, provider, email, signed_in}]
    property string accountId: ""
    property var    events: []              // normalised events, for the conflict line
    property string status: ""              // result line from the owner ("" | "saving…" | an error)
    property var    restore: null           // a draft handed back after Undo (see snapshot())

    signal saveRequested(var body)
    signal cancelRequested()
    signal accountPicked(string id)

    // ---- state
    property string whenText: ""
    property var    baseWhen: ({ start: initialStart, end: new Date(initialStart.getTime() + defaultMinutes * 60000), allDay: false })
    property string repeatText: ""
    property string showAs: "busy"
    property string localStatus: ""
    property bool   confirmDiscard: false
    readonly property string txId: "ol-" + Date.now().toString(16) + "-" + Math.floor(Math.random() * 4096).toString(16)
    readonly property string shownStatus: localStatus !== "" ? localStatus : status

    readonly property var whenResult: whenText.trim() === ""
        ? { ok: true, start: baseWhen.start, end: baseWhen.end, allDay: baseWhen.allDay === true,
            summary: When.formatSummary(baseWhen.start, baseWhen.end, baseWhen.allDay === true, now) }
        : When.parseWhen(whenText, { now: now, defaultMinutes: defaultMinutes, base: baseWhen })
    readonly property var repeatResult: Rep.parseRepeat(repeatText,
        { start: whenResult.ok ? whenResult.start : initialStart, defaultSeriesMonths: defaultSeriesMonths })
    readonly property var clashes: whenResult.ok ? Draft.conflicts(events, whenResult.start, whenResult.end, whenResult.allDay) : []
    readonly property var account: {
        for (var i = 0; i < accounts.length; i++) if (accounts[i].id === accountId) return accounts[i]
        return null
    }
    readonly property bool dirty: titleField.text !== "" || whenText !== "" || repeatText !== "" || whereField.text !== ""
        || notesField.text !== "" || reminderField.text !== String(defaultReminder) || showAs !== "busy"
        || baseWhen.start.getTime() !== initialStart.getTime()
    readonly property color fg: Commons.Color.foreground
    readonly property var showOptions: [
        { value: "busy", label: "Busy" }, { value: "free", label: "Free" },
        { value: "tentative", label: "Tentative" }, { value: "oof", label: "Out of office" } ]

    onWhenTextChanged: confirmDiscard = false
    onStatusChanged: if (status !== "") localStatus = ""

    // Everything typed, so an Undo can put it all back (nothing typed is lost).
    function snapshot() {
        return { subject: titleField.text, notes: notesField.text, location: whereField.text, reminder: reminderField.text,
                 showAs: showAs, base: whenResult.ok ? { start: whenResult.start, end: whenResult.end, allDay: whenResult.allDay } : baseWhen,
                 repeatText: repeatText }
    }

    function save() {
        var r = Draft.buildRequest({
            accountId: accountId, subject: titleField.text, notes: notesField.text,
            when: whenResult, location: whereField.text,
            reminder: reminderField.text.trim() === "" ? null : parseInt(reminderField.text),
            showAs: showAs, repeat: repeatResult.ok && repeatResult.pattern ? repeatResult : (repeatResult.ok ? null : repeatResult),
            transactionId: txId })
        if (!r.ok) { localStatus = r.error; return }
        localStatus = ""
        saveRequested(r.body)
    }

    function nudgeWhen(kind, dir) {
        var cur = whenResult.ok ? whenResult : baseWhen
        baseWhen = When.nudge({ start: cur.start, end: cur.end, allDay: cur.allDay === true }, kind, dir)
        whenField.text = ""
        whenText = ""
    }

    function tryCancel() {
        if (!dirty || confirmDiscard) { cancelRequested(); return }
        confirmDiscard = true
        localStatus = "Discard this event?  Esc again to discard, or keep typing."
    }

    Component.onCompleted: {
        if (restore) {
            titleField.text = restore.subject; notesField.text = restore.notes; whereField.text = restore.location
            reminderField.text = restore.reminder; showAs = restore.showAs; baseWhen = restore.base; repeatText = restore.repeatText
            repeatField.text = restore.repeatText
        }
        titleField.forceActiveFocus()
    }

    Keys.onPressed: function(e) {
        var ctrl = (e.modifiers & Qt.ControlModifier) !== 0
        if (ctrl && (e.key === Qt.Key_Return || e.key === Qt.Key_Enter)) { save(); e.accepted = true }
        else if (e.key === Qt.Key_Escape) { tryCancel(); e.accepted = true }
        else if (confirmDiscard && !(e.modifiers & ~Qt.ShiftModifier) && e.key !== Qt.Key_Shift) { confirmDiscard = false; localStatus = "" }
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
            UiText { text: "New event"; font.bold: true; font.pixelSize: Style.font.subtitle; foreground: Commons.Color.accent }
            UiText { text: "·  Calendar"; dim: true; font.pixelSize: Style.font.subtitle }
            Item { Layout.fillWidth: true }
            UiText { text: "Esc discard    Ctrl-Enter save"; dim: true; font.pixelSize: Style.font.caption }
        }
        PanelSeparator { Layout.fillWidth: true }

        GridLayout {
            Layout.fillWidth: true
            columns: 2
            columnSpacing: Style.spacing.xl
            rowSpacing: Style.spacing.md

            // ---- Account
            UiText { text: "Account"; dim: true; Layout.alignment: Qt.AlignTop; Layout.preferredWidth: Style.space(84) }
            FocusScope {
                id: accountRow
                Layout.fillWidth: true
                implicitHeight: accountText.implicitHeight + Style.spacing.md * 2
                activeFocusOnTab: true
                Rectangle {
                    anchors.fill: parent; radius: Style.cornerRadius
                    color: "transparent"
                    border.width: accountRow.activeFocus ? 1 : 0; border.color: Commons.Color.accent
                }
                RowLayout {
                    anchors.fill: parent; anchors.leftMargin: Style.spacing.lg; spacing: Style.spacing.md
                    Rectangle { width: Style.font.caption; height: width; radius: width / 2
                        color: root.account ? Hues.accountColor(root.accounts, root.account.email) : Commons.Color.urgent }
                    UiText {
                        id: accountText
                        Layout.fillWidth: true; elide: Text.ElideRight
                        text: root.account ? root.account.email : "no account can create events"
                        foreground: root.account ? root.fg : Commons.Color.urgent
                    }
                    UiText { text: root.accounts.length > 1 ? "\u2190 \u2192 change" : ""; dim: true; font.pixelSize: Style.font.caption; Layout.rightMargin: Style.spacing.lg }
                }
                Keys.onPressed: function(e) {
                    var dir = e.key === Qt.Key_Right || e.key === Qt.Key_Space ? 1 : e.key === Qt.Key_Left ? -1 : 0
                    if (dir === 0) return
                    root.accountPicked(Draft.cycleAccount(root.accounts, root.accountId, dir)); e.accepted = true
                }
                MouseArea { anchors.fill: parent; onClicked: { accountRow.forceActiveFocus(); root.accountPicked(Draft.cycleAccount(root.accounts, root.accountId, 1)) } }
            }

            // ---- Title
            UiText { text: "Title"; dim: true; Layout.preferredWidth: Style.space(84) }
            TextField { id: titleField; Layout.fillWidth: true; placeholderText: "What is it?"; activeFocusOnTab: true }

            // ---- When
            UiText { text: "When"; dim: true; Layout.alignment: Qt.AlignTop; Layout.preferredWidth: Style.space(84); Layout.topMargin: Style.spacing.md }
            ColumnLayout {
                Layout.fillWidth: true; spacing: Style.spacing.sm
                TextField {
                    id: whenField
                    Layout.fillWidth: true
                    placeholderText: "tomorrow 3pm  \u00b7  fri 10-10:30  \u00b7  +30m  \u00b7  all day mon"
                    onTextChanged: root.whenText = text
                    activeFocusOnTab: true
                    Keys.onPressed: function(e) {
                        var dir = e.key === Qt.Key_Up ? 1 : e.key === Qt.Key_Down ? -1 : 0
                        if (dir === 0) return
                        var kind = (e.modifiers & Qt.ShiftModifier) ? "day" : (e.modifiers & Qt.AltModifier) ? "length" : "start"
                        root.nudgeWhen(kind, dir); e.accepted = true
                    }
                }
                UiText {
                    Layout.fillWidth: true; wrapMode: Text.WordWrap
                    text: root.whenResult.ok ? root.whenResult.summary
                        : (root.whenResult.understood ? "Understood: " + root.whenResult.understood + "\n" : "") + root.whenResult.error
                    foreground: root.whenResult.ok ? root.fg : Commons.Color.urgent
                    font.pixelSize: Style.font.bodySmall
                }
                UiText {
                    visible: root.clashes.length > 0
                    Layout.fillWidth: true; wrapMode: Text.WordWrap
                    text: "\u26a0 overlaps \"" + root.clashes.slice(0, 2).join("\", \"") + "\"" + (root.clashes.length > 2 ? " and " + (root.clashes.length - 2) + " more" : "")
                    foreground: Hues.yellow; font.pixelSize: Style.font.bodySmall
                }
            }

            // ---- Repeat
            UiText { text: "Repeat"; dim: true; Layout.alignment: Qt.AlignTop; Layout.preferredWidth: Style.space(84); Layout.topMargin: Style.spacing.md }
            ColumnLayout {
                Layout.fillWidth: true; spacing: Style.spacing.sm
                TextField {
                    id: repeatField
                    Layout.fillWidth: true
                    placeholderText: "none  \u00b7  daily  \u00b7  weekly  \u00b7  every 2 weeks on tue,thu  \u00b7  monthly 2nd tue"
                    onTextChanged: root.repeatText = text
                    activeFocusOnTab: true
                }
                UiText {
                    visible: root.repeatText.trim() !== ""
                    Layout.fillWidth: true; wrapMode: Text.WordWrap
                    text: root.repeatResult.ok
                        ? root.repeatResult.sentence + (root.repeatResult.next && root.repeatResult.next.length
                            ? "  \u00b7  next: " + root.repeatResult.next.map(function(d) { return d.getDate() + " " + ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"][d.getMonth()] }).join(", ") : "")
                        : root.repeatResult.error
                    foreground: root.repeatResult.ok ? root.fg : Commons.Color.urgent
                    font.pixelSize: Style.font.bodySmall
                }
            }

            // ---- Where
            UiText { text: "Where"; dim: true; Layout.preferredWidth: Style.space(84) }
            TextField { id: whereField; Layout.fillWidth: true; placeholderText: "Room, address or link"; activeFocusOnTab: true }

            // ---- Reminder
            UiText { text: "Reminder"; dim: true; Layout.preferredWidth: Style.space(84) }
            RowLayout {
                Layout.fillWidth: true; spacing: Style.spacing.md
                TextField {
                    id: reminderField
                    Layout.preferredWidth: Style.space(80)
                    text: String(root.defaultReminder)
                    validator: IntValidator { bottom: 0; top: 10080 }
                    inputMethodHints: Qt.ImhDigitsOnly
                    activeFocusOnTab: true
                }
                UiText { text: reminderField.text.trim() === "" ? "no reminder" : (reminderField.text === "0" ? "at the time of the event" : "minutes before"); dim: true; font.pixelSize: Style.font.bodySmall }
            }

            // ---- Show as
            UiText { text: "Show as"; dim: true; Layout.preferredWidth: Style.space(84) }
            FocusScope {
                id: showRow
                Layout.fillWidth: true
                implicitHeight: showText.implicitHeight + Style.spacing.md * 2
                activeFocusOnTab: true
                Rectangle {
                    anchors.fill: parent; radius: Style.cornerRadius; color: "transparent"
                    border.width: showRow.activeFocus ? 1 : 0; border.color: Commons.Color.accent
                }
                UiText {
                    id: showText
                    anchors.verticalCenter: parent.verticalCenter; anchors.left: parent.left; anchors.leftMargin: Style.spacing.lg
                    text: { for (var i = 0; i < root.showOptions.length; i++) if (root.showOptions[i].value === root.showAs) return root.showOptions[i].label + "    \u2190 \u2192 change"; return "" }
                }
                function step(dir) {
                    var i = 0
                    for (var k = 0; k < root.showOptions.length; k++) if (root.showOptions[k].value === root.showAs) i = k
                    root.showAs = root.showOptions[(i + dir + root.showOptions.length) % root.showOptions.length].value
                }
                Keys.onPressed: function(e) {
                    var dir = e.key === Qt.Key_Right || e.key === Qt.Key_Space ? 1 : e.key === Qt.Key_Left ? -1 : 0
                    if (dir === 0) return
                    step(dir); e.accepted = true
                }
                MouseArea { anchors.fill: parent; onClicked: { showRow.forceActiveFocus(); showRow.step(1) } }
            }

            // ---- Notes
            UiText { text: "Notes"; dim: true; Layout.preferredWidth: Style.space(84) }
            TextField { id: notesField; Layout.fillWidth: true; placeholderText: "Optional"; activeFocusOnTab: true }
        }

        Item { Layout.fillHeight: true }

        UiText {
            Layout.fillWidth: true; wrapMode: Text.WordWrap
            visible: root.shownStatus !== ""
            text: root.shownStatus
            foreground: root.shownStatus === "saving\u2026" || root.confirmDiscard || root.shownStatus.indexOf("Undone") === 0 ? root.fg : Commons.Color.urgent
        }
        PanelSeparator { Layout.fillWidth: true }
        UiText {
            Layout.fillWidth: true
            text: "Personal entry. No invitations.        Attendees and online meetings come in the next phase."
            dim: true; font.pixelSize: Style.font.caption
        }
    }
}
