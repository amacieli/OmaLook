import QtQuick
import QtQuick.Layouts
import QtQuick.Controls
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"
import "draft.js" as Draft

// Contact editor (docs/PLAN-contacts.md section 5). It sits over the People view. Pure view: it owns the
// fields and reports intent through signals; PeopleView posts the save (which is instant) and closes it.
//
// Keys (text fields own theirs; the app's global bindings are off while this is open):
//   Tab / Shift-Tab   next / previous field         Ctrl-Enter   save
//   Ctrl-K            add a field (type to find it)  Ctrl-T       choose the account / folder (new contact)
//   Esc               close (asks once if edited)    Ctrl-D       remove the empty extra fields again
FocusScope {
    id: root

    // ---- inputs from the owner
    property var    raw: null                 // the contact being edited; null = a new contact
    property var    targets: []               // [{accountId, accountEmail, folderId, folderName}] a new contact may go to
    property int    targetIndex: 0
    property var    accounts: []              // for the account colour
    property string status: ""                // "" | "saving…" | an error from the server side

    signal saveRequested(var body)
    signal cancelRequested()

    // ---- state
    readonly property bool isNew: raw === null
    property var    initial: Draft.valuesOf(raw)
    property var    values: Draft.valuesOf(raw)       // id -> text, kept up to date by the fields (not bound)
    property var    shown: Draft.shownIds(initial, [])
    property string localStatus: ""
    property bool   confirmDiscard: false
    property bool   picking: false
    readonly property string shownStatus: localStatus !== "" ? localStatus : status
    readonly property color fg: Commons.Color.foreground
    readonly property var target: targets.length > 0 ? targets[Math.max(0, Math.min(targetIndex, targets.length - 1))] : null
    readonly property var rows: shown.map(function(i) { return Draft.DEFS.filter(function(d) { return d.id === i })[0] })

    function dirty() { return Draft.changeCount(values, initial) > 0 }

    function save() {
        var problem = Draft.validate(values)
        if (problem !== "") { localStatus = problem; return }
        var fields = Draft.buildFields(values, isNew ? ({}) : initial)
        if (Object.keys(fields).length === 0) {
            if (isNew) { localStatus = "Nothing to save yet \u2014 type a name, an address or a number."; return }
            cancelRequested(); return
        }
        localStatus = ""
        var body = { fields: fields }
        if (!isNew) body.id = raw.id
        else {
            if (!target) { localStatus = "No account can save contacts yet."; return }
            body.account_id = target.accountId
            body.folder_id = target.folderId
        }
        saveRequested(body)
    }

    function tryCancel() {
        if (picking) { closePicker(); return }
        if (!dirty() || confirmDiscard) { cancelRequested(); return }
        confirmDiscard = true
        localStatus = "Discard your changes?  Esc again to discard, or keep typing."
    }

    function addFields(ids) {
        shown = Draft.shownIds(values, shown.concat(ids))
        Qt.callLater(function() { focusRow(ids[0]) })
    }
    function focusRow(id) {
        for (var i = 0; i < rep.count; i++) {
            var it = rep.itemAt(i)
            if (it && it.fieldId === id) { it.input.forceActiveFocus(); flick.ensureVisible(it); return }
        }
    }
    function openPicker() { picking = true; pickFilter.text = ""; pickList.currentIndex = 0; pickFilter.forceActiveFocus() }
    function closePicker() { picking = false; Qt.callLater(function() { if (rep.count > 0 && rep.itemAt(0)) rep.itemAt(0).input.forceActiveFocus() }) }
    // drop the extra fields that are still empty
    function tidy() {
        var keep = Draft.shownIds(values, [])
        shown = keep
    }
    function cycleTarget(dir) {
        if (targets.length > 1 && isNew) targetIndex = (targetIndex + dir + targets.length) % targets.length
    }

    Component.onCompleted: Qt.callLater(function() { if (rep.count > 0 && rep.itemAt(0)) rep.itemAt(0).input.forceActiveFocus() })

    Keys.onPressed: function(e) {
        var ctrl = (e.modifiers & Qt.ControlModifier) !== 0
        if (ctrl && (e.key === Qt.Key_Return || e.key === Qt.Key_Enter)) { save(); e.accepted = true }
        else if (e.key === Qt.Key_Escape) { tryCancel(); e.accepted = true }
        else if (ctrl && e.key === Qt.Key_K) { openPicker(); e.accepted = true }
        else if (ctrl && e.key === Qt.Key_T) { cycleTarget(1); e.accepted = true }
        else if (ctrl && e.key === Qt.Key_D) { tidy(); e.accepted = true }
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
            UiText { text: root.isNew ? "New contact" : "Edit contact"; font.bold: true; font.pixelSize: Style.font.subtitle; foreground: Commons.Color.accent }
            UiText {
                visible: !root.isNew
                text: "\u00b7  " + (root.raw && root.raw.displayName ? root.raw.displayName : "")
                dim: true; font.pixelSize: Style.font.subtitle
            }
            Item { Layout.fillWidth: true }
            UiText { text: "Esc discard    Ctrl-Enter save    Ctrl-K add a field"; dim: true; font.pixelSize: Style.font.caption }
        }
        PanelSeparator { Layout.fillWidth: true }

        // where a new contact will be saved
        RowLayout {
            visible: root.isNew
            Layout.fillWidth: true
            spacing: Style.spacing.md
            UiText { text: "Save to"; dim: true; Layout.preferredWidth: Style.space(92) }
            Rectangle { width: Style.font.caption; height: width; radius: width / 2
                color: root.target ? Hues.accountColor(root.accounts, root.target.accountEmail) : Commons.Color.urgent }
            UiText {
                Layout.fillWidth: true; elide: Text.ElideRight
                text: root.target ? root.target.accountEmail + "  \u00b7  " + root.target.folderName : "no account can save contacts yet"
                foreground: root.target ? root.fg : Commons.Color.urgent
            }
            UiText { visible: root.targets.length > 1; text: "Ctrl-T change"; dim: true; font.pixelSize: Style.font.caption }
        }

        Flickable {
            id: flick
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            contentWidth: width
            contentHeight: column.implicitHeight
            boundsBehavior: Flickable.StopAtBounds
            ScrollBar.vertical: ThemedScrollBar {}

            function ensureVisible(item) {
                var y = item.mapToItem(contentItem, 0, 0).y
                if (y < contentY) contentY = y
                else if (y + item.height > contentY + height) contentY = y + item.height - height
            }

            Column {
                id: column
                width: flick.width - Style.spacing.lg
                spacing: Style.spacing.sm

                Repeater {
                    id: rep
                    model: root.rows

                    delegate: FocusScope {
                        id: row
                        required property var modelData
                        required property int index
                        readonly property string fieldId: modelData.id
                        readonly property bool multi: modelData.kind === "notes"
                        readonly property Item input: multi ? field : single
                        width: column.width
                        implicitHeight: Math.max(multi ? Math.max(field.implicitHeight, Style.space(110)) : single.implicitHeight, label.implicitHeight)
                        height: implicitHeight

                        UiText {
                            id: label
                            width: Style.space(120)
                            y: Style.spacing.md
                            text: row.modelData.label
                            dim: true
                            elide: Text.ElideRight
                        }

                        TextArea {
                            id: field
                            visible: row.multi
                            enabled: row.multi
                            x: Style.space(132)
                            width: parent.width - x
                            height: row.multi ? Math.max(implicitHeight, Style.space(110)) : 0
                            wrapMode: TextEdit.Wrap
                            // the same face, size and colors as the single-line fields (a bare TextArea uses the system font)
                            font.family: Style.font.family
                            font.pixelSize: Style.font.body
                            color: Commons.Color.foreground
                            placeholderTextColor: Qt.darker(Commons.Color.foreground, 1.8)
                            selectionColor: Commons.Color.accent
                            selectedTextColor: Commons.Color.background
                            background: Rectangle { color: "transparent"; border.width: 1; border.color: field.activeFocus ? Commons.Color.accent : Qt.darker(Commons.Color.foreground, 2.4); radius: 4 }
                            placeholderText: row.modelData.hint || ""
                            activeFocusOnTab: row.multi
                            Component.onCompleted: text = root.values[row.modelData.id] || ""
                            onTextChanged: root.values[row.modelData.id] = text
                            Keys.onTabPressed: nextItemInFocusChain(true).forceActiveFocus()
                            Keys.onBacktabPressed: nextItemInFocusChain(false).forceActiveFocus()
                            onActiveFocusChanged: if (activeFocus) flick.ensureVisible(row)
                        }
                        // single-line fields use this one; only notes use the area above
                        TextField {
                            id: single
                            visible: !row.multi
                            enabled: !row.multi
                            x: Style.space(132)
                            width: parent.width - x
                            placeholderText: row.modelData.hint || ""
                            activeFocusOnTab: !row.multi
                            Component.onCompleted: text = root.values[row.modelData.id] || ""
                            onTextChanged: root.values[row.modelData.id] = text
                            onActiveFocusChanged: if (activeFocus) flick.ensureVisible(row)
                        }
                    }
                }
            }
        }

        UiText {
            Layout.fillWidth: true; wrapMode: Text.WordWrap
            visible: root.shownStatus !== ""
            text: root.shownStatus
            foreground: root.shownStatus === "saving\u2026" || root.confirmDiscard ? root.fg : Commons.Color.urgent
        }
        PanelSeparator { Layout.fillWidth: true }
        UiText {
            Layout.fillWidth: true
            text: "Saved at once.  Ctrl-Z in the list takes the last change back.  A server keeps up to 3 emails, 2 business and 2 home phones."
            dim: true; font.pixelSize: Style.font.caption
        }
    }

    // ---- Ctrl-K: add a field
    Rectangle {
        visible: root.picking
        anchors.centerIn: parent
        width: Math.min(parent.width - Style.spacing.xl * 2, Style.space(420))
        height: Math.min(parent.height - Style.spacing.xl * 2, Style.space(380))
        radius: Style.cornerRadius
        color: Commons.Color.background
        border.width: 1; border.color: Commons.Color.accent
        z: 10

        readonly property var options: {
            var q = pickFilter.text.trim().toLowerCase()
            return Draft.pickable(root.shown).filter(function(p) { return q === "" || p.label.toLowerCase().indexOf(q) >= 0 })
        }

        ColumnLayout {
            anchors.fill: parent
            anchors.margins: Style.spacing.lg
            spacing: Style.spacing.md
            UiText { text: "Add a field"; font.bold: true; foreground: Commons.Color.accent }
            TextField {
                id: pickFilter
                Layout.fillWidth: true
                placeholderText: "type to filter, Enter adds, Esc closes"
                Keys.onDownPressed: pickList.currentIndex = Math.min(pickList.count - 1, pickList.currentIndex + 1)
                Keys.onUpPressed: pickList.currentIndex = Math.max(0, pickList.currentIndex - 1)
                Keys.onReturnPressed: root.pickCurrent()
                Keys.onEnterPressed: root.pickCurrent()
                Keys.onEscapePressed: root.closePicker()
                onTextChanged: pickList.currentIndex = 0
            }
            ListView {
                id: pickList
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                model: parent.parent.options
                currentIndex: 0
                delegate: Rectangle {
                    required property var modelData
                    required property int index
                    width: pickList.width
                    height: Style.space(28)
                    color: index === pickList.currentIndex ? Qt.rgba(Commons.Color.accent.r, Commons.Color.accent.g, Commons.Color.accent.b, 0.18) : "transparent"
                    UiText { anchors.verticalCenter: parent.verticalCenter; x: Style.spacing.md; text: parent.modelData.label }
                    TapHandler { onTapped: { pickList.currentIndex = parent.index; root.pickCurrent() } }
                }
            }
        }
    }

    function pickCurrent() {
        var opts = pickList.model
        if (!opts || pickList.currentIndex < 0 || pickList.currentIndex >= opts.length) return
        var add = opts[pickList.currentIndex].add
        picking = false
        addFields(add)
    }
}
