import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"
import "../compose"
import "when.js" as When
import "recurrence.js" as Rep
import "draft.js" as Draft

// New-event editor (docs/PLAN-calendar-events.md 5.3, phase 2: personal entries, with repeat).
// Phase 3: the same editor edits an event (mode "edit": PATCH with only the changed fields, the whole attendee list
// on every save) and takes Required / Optional attendees (RecipientField rows).
// It sits over the calendar grid. Pure view: it owns the fields and reports intent through signals;
// CalendarView posts the request, shows the pending mark and the undo bar.
//
// Keys (text fields own theirs; the app's global bindings are off while this is open):
//   Tab / Shift-Tab  next / previous field        Ctrl-Enter  save (the undo window starts)
//   Esc              close (asks once if edited)  Up / Down on When: start +-15 min,
//   Shift-Up/Down    +-1 day                      Alt-Up/Down: length +-15 min
//   Left / Right     (Account and Show-as rows) change the choice
//   Alt-Down / Alt-Up in an attendee field: move the last chip from Required to Optional, and back
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
    property string mode: "create"          // "create" | "edit" (an existing event: PATCH on save)
    property var    original: null          // edit: { raw (cached row), start, end, allDay }
    property string ownEmail: ""            // the account's own address: never added as an attendee
    property var    suggestions: []         // [{name, email}] best first (owner merges contacts, people, directory)
    property var    meetingOptions: []      // GET /calendar/meeting-options rows for the account (the Online meeting picker)

    signal saveRequested(var body)
    signal cancelRequested()
    signal accountPicked(string id)
    signal directoryQuery(string text)      // what was typed in an attendee field (the owner searches, debounced)
    signal findRequested()                  // Ctrl-F: the owner opens the free-time finder with this editor's people and length

    // ---- state
    property string whenText: ""
    property var    baseWhen: ({ start: initialStart, end: new Date(initialStart.getTime() + defaultMinutes * 60000), allDay: false })
    property string repeatText: ""
    property string showAs: "busy"
    property string onlineChoice: ""       // Online meeting: "" = not touched (a new event takes the account's default)
    property string localStatus: ""
    property bool   confirmDiscard: false
    readonly property bool editing: mode === "edit" && original !== null
    readonly property string txId: "ol-" + Date.now().toString(16) + "-" + Math.floor(Math.random() * 4096).toString(16)
    readonly property string shownStatus: localStatus !== "" ? localStatus : status

    readonly property var whenResult: whenText.trim() === ""
        ? { ok: true, start: baseWhen.start, end: baseWhen.end, allDay: baseWhen.allDay === true,
            summary: When.formatSummary(baseWhen.start, baseWhen.end, baseWhen.allDay === true, now) }
        : When.parseWhen(whenText, { now: now, defaultMinutes: defaultMinutes, base: baseWhen })
    readonly property var repeatResult: Rep.parseRepeat(repeatText,
        { start: whenResult.ok ? whenResult.start : initialStart, defaultSeriesMonths: defaultSeriesMonths })
    readonly property var clashes: whenResult.ok ? Draft.conflicts(events.filter(function(e) { return !root.editing || !root.original || !root.original.raw || (e.raw ? e.raw.id : e.id) !== root.original.raw.id }), whenResult.start, whenResult.end, whenResult.allDay) : []
    readonly property var account: {
        for (var i = 0; i < accounts.length; i++) if (accounts[i].id === accountId) return accounts[i]
        return null
    }

    // Online meeting: the rows, the shown choice and what is sent. An edit sends nothing unless the choice moved off the
    // event's own meeting (absent = the backend keeps it).
    readonly property string originalOnline: editing ? (original.raw.online_provider || "") : ""
    readonly property var onlineRows: {
        var rows = Draft.onlineRows(meetingOptions, editing)
        if (editing && originalOnline !== "") {
            var have = false
            for (var i = 0; i < rows.length; i++) if (rows[i].id === originalOnline) have = true
            if (!have) rows.splice(1, 0, { id: originalOnline, label: "Current meeting", enabled: true, note: "", isDefault: false })
        }
        return rows
    }
    readonly property string shownOnline: onlineChoice !== "" ? onlineChoice : (editing ? (originalOnline || "none") : Draft.defaultOnline(onlineRows))
    readonly property string sentOnline: editing ? (shownOnline === (originalOnline || "none") ? "" : shownOnline) : shownOnline
    readonly property string onlineText: Draft.onlineLine(editing && sentOnline === "" ? "none" : shownOnline, onlineRows, editing)

    // Attendees: the chips in the two rows, plus rooms kept from an edit. people.list is what gets sent.
    readonly property var keptRooms: editing ? (original.raw.attendees || []).filter(function(a) { return a.role === "resource" }) : []
    readonly property var people: Draft.attendeeList(reqField.recipients, optField.recipients, keptRooms, ownEmail)
    readonly property int inviteeCount: Draft.inviteeCount(people.list)
    property string editScope: "occurrence"  // edit: occurrence | series | following (set by CalendarView)
    property string occurrenceDay: ""         // edit: the day of the occurrence picked (YYYY-MM-DD)
    readonly property string reachText: Draft.reachLine(inviteeCount, editing ? "edit" : "create", editScope, occurrenceDay)

    // Edit: the original as the editor would have it, compared with the form. Only changed keys are sent.
    readonly property var originalRows: editing ? (original.raw.attendees || []) : []
    readonly property var originalPeople: editing ? Draft.attendeeList(
        originalRows.filter(function(a) { return a.role !== "optional" && a.role !== "resource" }),
        originalRows.filter(function(a) { return a.role === "optional" }), keptRooms, ownEmail).list : []
    readonly property var originalState: editing ? Draft.stateOf({
        subject: original.raw.subject || "", notes: original.raw.body || "",
        when: { start: original.start, end: original.end, allDay: original.allDay === true },
        location: original.raw.location || "", reminder: original.raw.reminder_minutes, showAs: original.raw.show_as,
        attendees: originalPeople }) : null
    readonly property var currentState: whenResult.ok ? Draft.stateOf({
        subject: titleField.text, notes: notesField.text, when: whenResult, location: whereField.text,
        reminder: reminderField.text.trim() === "" ? null : parseInt(reminderField.text),
        showAs: showAs, attendees: people.list, online: sentOnline }) : null
    readonly property var changed: editing && originalState && currentState ? Draft.changedKeys(originalState, currentState) : []

    readonly property bool dirty: editing ? changed.length > 0
        : titleField.text !== "" || whenText !== "" || repeatText !== "" || whereField.text !== ""
          || notesField.text !== "" || reminderField.text !== String(defaultReminder) || showAs !== "busy"
          || shownOnline !== Draft.defaultOnline(onlineRows)
          || baseWhen.start.getTime() !== initialStart.getTime() || people.list.length > 0
    readonly property color fg: Commons.Color.foreground
    readonly property var showOptions: {
        var out = [
            { value: "busy", label: "Busy" }, { value: "free", label: "Free" },
            { value: "tentative", label: "Tentative" }, { value: "oof", label: "Out of office" } ]
        for (var i = 0; i < out.length; i++) if (out[i].value === showAs) return out
        out.push({ value: showAs, label: showAs === "workingElsewhere" ? "Working elsewhere" : showAs })   // a value this editor does not set itself
        return out
    }


    onWhenTextChanged: confirmDiscard = false
    onStatusChanged: if (status !== "") localStatus = ""

    // Everything typed, so an Undo can put it all back (nothing typed is lost).
    function snapshot() {
        return { subject: titleField.text, notes: notesField.text, location: whereField.text, reminder: reminderField.text,
                 showAs: showAs, base: whenResult.ok ? { start: whenResult.start, end: whenResult.end, allDay: whenResult.allDay } : baseWhen,
                 repeatText: repeatText, required: reqField.recipients, optional: optField.recipients, online: onlineChoice }
    }

    function save() {
        if (people.bad.length > 0) { localStatus = "Not an address: " + people.bad.join(", ") + ". Fix it or remove the chip."; return }
        if (editing) {
            var u = Draft.buildPatch({
                accountId: original.raw.account_id || accountId, when: whenResult, subject: titleField.text,
                notes: notesField.text, location: whereField.text,
                reminder: reminderField.text.trim() === "" ? null : parseInt(reminderField.text),
                showAs: showAs, attendees: people.list, changed: changed, originalLocation: original.raw.location || "", online: sentOnline })
            if (!u.ok) { localStatus = u.nothing ? "Nothing changed yet. Esc closes." : u.error; return }
            localStatus = ""
            saveRequested(u.body)
            return
        }
        var r = Draft.buildRequest({
            accountId: accountId, subject: titleField.text, notes: notesField.text,
            when: whenResult, location: whereField.text,
            reminder: reminderField.text.trim() === "" ? null : parseInt(reminderField.text),
            showAs: showAs, repeat: repeatResult.ok && repeatResult.pattern ? repeatResult : (repeatResult.ok ? null : repeatResult),
            attendees: people.list, transactionId: txId, online: sentOnline })
        if (!r.ok) { localStatus = r.error; return }
        localStatus = ""
        saveRequested(r.body)
    }

    // The free-time finder's pick: the slot becomes the When (the typed text is cleared, the summary shows the slot)
    function applySlot(s, e) {
        baseWhen = { start: s, end: e, allDay: false }
        whenField.text = ""
        whenText = ""
        localStatus = "Time filled from the free-time finder"
    }
    function focusFirst() { titleField.forceActiveFocus() }

    function nudgeWhen(kind, dir) {
        var cur = whenResult.ok ? whenResult : baseWhen
        baseWhen = When.nudge({ start: cur.start, end: cur.end, allDay: cur.allDay === true }, kind, dir)
        whenField.text = ""
        whenText = ""
    }

    // Alt-Down moves the last Required chip to Optional, Alt-Up the reverse. Only the chips move; nobody is sent anything.
    function moveChip(from, to) {
        var n = from.recipients.length
        if (n === 0) return false
        var last = from.recipients[n - 1]
        from.removeAt(n - 1)
        to.addRecipients([last])
        return true
    }

    function tryCancel() {
        if (!dirty || confirmDiscard) { cancelRequested(); return }
        confirmDiscard = true
        localStatus = "Discard this event?  Esc again to discard, or keep typing."
    }

    Component.onCompleted: {
        if (editing) {
            titleField.text = original.raw.subject || ""
            notesField.text = original.raw.body || ""
            whereField.text = original.raw.location || ""
            var rm = original.raw.reminder_minutes
            reminderField.text = rm === null || rm === undefined ? "" : String(rm)
            showAs = original.raw.show_as || "busy"
            baseWhen = { start: original.start, end: original.end, allDay: original.allDay === true }
            reqField.setRecipients(originalPeople.filter(function(a) { return a.role !== "optional" && a.role !== "resource" })
                .map(function(a) { return { email: a.email, name: a.name } }))
            optField.setRecipients(originalPeople.filter(function(a) { return a.role === "optional" })
                .map(function(a) { return { email: a.email, name: a.name } }))
        } else if (restore) {
            titleField.text = restore.subject; notesField.text = restore.notes; whereField.text = restore.location
            reminderField.text = restore.reminder; showAs = restore.showAs; baseWhen = restore.base; repeatText = restore.repeatText
            repeatField.text = restore.repeatText
            onlineChoice = restore.online || ""
            if (restore.required) reqField.setRecipients(restore.required)
            if (restore.optional) optField.setRecipients(restore.optional)
        }
        titleField.forceActiveFocus()
    }

    onSuggestionsChanged: { reqField._refreshMatches(); optField._refreshMatches() }

    Keys.onPressed: function(e) {
        var ctrl = (e.modifiers & Qt.ControlModifier) !== 0
        if (ctrl && (e.key === Qt.Key_Return || e.key === Qt.Key_Enter)) { save(); e.accepted = true }
        else if (e.key === Qt.Key_Escape) { tryCancel(); e.accepted = true }
        else if ((e.modifiers & Qt.ControlModifier) && e.key === Qt.Key_F) { findRequested(); e.accepted = true }
        else if ((e.modifiers & Qt.AltModifier) && (e.key === Qt.Key_Down || e.key === Qt.Key_Up)) {
            if (e.key === Qt.Key_Down && reqField.input.activeFocus) { moveChip(reqField, optField); e.accepted = true }
            else if (e.key === Qt.Key_Up && optField.input.activeFocus) { moveChip(optField, reqField); e.accepted = true }
        }
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
            UiText { text: root.editing ? "Edit event" : "New event"; font.bold: true; font.pixelSize: Style.font.subtitle; foreground: Commons.Color.accent }
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
                    UiText { text: root.accounts.length > 1 && !root.editing ? "\u2190 \u2192 change" : ""; dim: true; font.pixelSize: Style.font.caption; Layout.rightMargin: Style.spacing.lg }
                }
                Keys.onPressed: function(e) {
                    var dir = e.key === Qt.Key_Right || e.key === Qt.Key_Space ? 1 : e.key === Qt.Key_Left ? -1 : 0
                    if (dir === 0 || root.editing) return
                    root.accountPicked(Draft.cycleAccount(root.accounts, root.accountId, dir)); e.accepted = true
                }
                MouseArea { anchors.fill: parent; onClicked: { accountRow.forceActiveFocus(); if (!root.editing) root.accountPicked(Draft.cycleAccount(root.accounts, root.accountId, 1)) } }
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
                    visible: !root.editing
                    Layout.fillWidth: true
                    placeholderText: "none  \u00b7  daily  \u00b7  weekly  \u00b7  every 2 weeks on tue,thu  \u00b7  monthly 2nd tue"
                    onTextChanged: root.repeatText = text
                    activeFocusOnTab: true
                }
                UiText {
                    visible: root.editing
                    Layout.fillWidth: true; wrapMode: Text.WordWrap
                    text: "Stays as it is. Change a repeat pattern in Outlook."
                    dim: true; font.pixelSize: Style.font.bodySmall
                }
                UiText {
                    visible: !root.editing && root.repeatText.trim() !== ""
                    Layout.fillWidth: true; wrapMode: Text.WordWrap
                    text: root.repeatResult.ok
                        ? root.repeatResult.sentence + (root.repeatResult.next && root.repeatResult.next.length
                            ? "  \u00b7  next: " + root.repeatResult.next.map(function(d) { return d.getDate() + " " + ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"][d.getMonth()] }).join(", ") : "")
                        : root.repeatResult.error
                    foreground: root.repeatResult.ok ? root.fg : Commons.Color.urgent
                    font.pixelSize: Style.font.bodySmall
                }
            }

            // ---- Attendees (phase 3): Required and Optional rows, each a RecipientField; suggestions come from the owner
            RecipientField {
                id: reqField
                Layout.fillWidth: true; Layout.columnSpan: 2
                label: "Required"
                contacts: root.suggestions
            }
            RecipientField {
                id: optField
                Layout.fillWidth: true; Layout.columnSpan: 2
                label: "Optional"
                contacts: root.suggestions
            }
            Connections { target: reqField.input; function onTextChanged() { root.directoryQuery(reqField.input.text) } }
            Connections { target: optField.input; function onTextChanged() { root.directoryQuery(optField.input.text) } }
            UiText {
                visible: root.people.own
                Layout.columnSpan: 2; Layout.fillWidth: true; wrapMode: Text.WordWrap
                text: "Your own address is not added: you are the organiser."
                dim: true; font.pixelSize: Style.font.bodySmall
            }

            // ---- Where
            UiText { text: "Where"; dim: true; Layout.preferredWidth: Style.space(84) }
            TextField { id: whereField; Layout.fillWidth: true; placeholderText: "Room, address or link"; activeFocusOnTab: true }

            // ---- Online meeting (OL-021 track B): None / the providers for this account; Zoom and Webex are shown but off
            UiText { text: "Meeting"; dim: true; Layout.alignment: Qt.AlignTop; Layout.preferredWidth: Style.space(84); Layout.topMargin: Style.spacing.md }
            FocusScope {
                id: onlineRow
                Layout.fillWidth: true
                implicitHeight: onlineFlow.implicitHeight + Style.spacing.md * 2
                activeFocusOnTab: true
                Rectangle {
                    anchors.fill: parent; radius: Style.cornerRadius; color: "transparent"
                    border.width: onlineRow.activeFocus ? 1 : 0; border.color: Commons.Color.accent
                }
                Flow {
                    id: onlineFlow
                    anchors.left: parent.left; anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter
                    anchors.leftMargin: Style.spacing.lg; anchors.rightMargin: Style.spacing.lg
                    spacing: Style.spacing.xl
                    Repeater {
                        model: root.onlineRows
                        delegate: UiText {
                            required property var modelData
                            readonly property bool picked: modelData.id === root.shownOnline
                            text: (picked ? "\u25cf " : "\u25cb ") + modelData.label + (modelData.note !== "" ? "  (" + modelData.note + ")" : "")
                            font.bold: picked
                            foreground: picked ? Commons.Color.accent : root.fg
                            dim: !modelData.enabled
                            MouseArea {
                                anchors.fill: parent
                                onClicked: { onlineRow.forceActiveFocus(); if (modelData.enabled) root.onlineChoice = modelData.id }
                            }
                        }
                    }
                    UiText { visible: root.onlineRows.length === 0; text: "loading\u2026"; dim: true }
                }
                Keys.onPressed: function(e) {
                    var dir = e.key === Qt.Key_Right || e.key === Qt.Key_Space ? 1 : e.key === Qt.Key_Left ? -1 : 0
                    if (dir === 0) return
                    root.onlineChoice = Draft.stepOnline(root.onlineRows, root.shownOnline, dir); e.accepted = true
                }
            }

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
            foreground: root.shownStatus === "saving\u2026" || root.confirmDiscard || root.shownStatus.indexOf("Undone") === 0 || root.shownStatus.indexOf("Time filled") === 0 ? root.fg : Commons.Color.urgent
        }
        PanelSeparator { Layout.fillWidth: true }
        UiText {
            Layout.fillWidth: true
            text: root.reachText + (root.onlineText !== "" ? "  " + root.onlineText : "") + "    Ctrl-F find a time    Ctrl-Enter save"
            dim: true; font.pixelSize: Style.font.caption
        }
    }
}
