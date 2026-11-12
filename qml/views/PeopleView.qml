import QtQuick
import QtQuick.Layouts
import QtQuick.Controls
import Quickshell
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"
import "../people"
import "../people/fields.js" as PF

// PeopleView: the address book (docs/PLAN-contacts.md). A keyboard cursor walks the contact list, Enter
// opens a detail pane with every field, `/` filters by any word of any field, Space marks, and the keys
// that act on a contact (`w` mail, `S` text, `O` open, `y` copy) live in the registry (AppCommands) and
// call the functions below. Colours and fonts come from the Omarchy theme; nothing here is a literal.
// Data: http://127.0.0.1:27182/contacts?view=&sort=&account=&q=  and  /contacts/get?id=
Item {
    id: root

    // ── theme ──────────────────────────────────────────────────────
    readonly property string monoFont:     Style.font.family
    readonly property color  accentColor:  Commons.Color.accent
    readonly property color  textColor:    Commons.Color.foreground
    readonly property color  dimColor:     Qt.darker(Commons.Color.foreground, 1.4)
    readonly property color  veryDimColor: Qt.rgba(Commons.Color.foreground.r, Commons.Color.foreground.g, Commons.Color.foreground.b, 0.3)

    property string backendUrl: "http://127.0.0.1:27182"

    // "all" | "favorites" | "lists"   and   "first" | "last" | "company" | "recent"
    // (AppShell keeps both across tab switches)
    property string viewMode: "all"
    property string sortKey:  "first"

    // [{id, provider, email, signed_in}] from the app; "" = every account, else one account's email
    property var    appAccounts: []
    property string accountFilter: ""

    // ── list state ─────────────────────────────────────────────────
    property var  contacts: []
    property bool loading: false
    property bool loaded: false
    property int  _serial: 0
    property int  cur: 0                       // keyboard cursor (index into `contacts`)
    property string searchText: ""             // applied filter ("" = none)
    property bool searching: false             // the search field has the keyboard
    property var  marks: ({})                  // id -> true
    readonly property int markCount: Object.keys(marks).length
    property bool detailOpen: false
    property var  detail: null
    property bool detailLoading: false
    property int  _detailSerial: 0
    property string _wantId: ""                // keep the cursor on this contact across a reload

    readonly property var selected: contacts.length > 0 ? contacts[Math.max(0, Math.min(cur, contacts.length - 1))] : null
    readonly property string selectedId: selected ? selected.id : ""
    property bool editorOpen: false            // the editor is over the list
    property var  editorRaw: null              // the contact it edits (null = a new one)
    property var  editorTargets: []            // where a new contact may be saved
    property string editorStatus: ""
    readonly property bool editing: searching || editorOpen   // AppShell hands every key to the field / editor meanwhile

    signal notice(string text)

    readonly property int rowHeight: 64
    readonly property int lineHeight: 16
    readonly property int sectionHeight: 28
    readonly property real nameColumnWidth: Math.max(180, Math.min(320, width * (detailOpen ? 0.2 : 0.28)))

    readonly property var viewOptions: [
        { value: "all",       label: "All contacts" },
        { value: "favorites", label: "Favorites" },
        { value: "lists",     label: "Contact folders" }
    ]
    readonly property var sortOptions: [
        { value: "first",   label: "First name" },
        { value: "last",    label: "Last name" },
        { value: "company", label: "Company" },
        { value: "recent",  label: "Recently added" }
    ]

    // ── line builders ──────────────────────────────────────────────
    function emailLine(c)   { return (c.emails || []).join("; ") }
    function phoneLine(c)   { return (c.phones || []).map(function(p) { return p.kind + ": " + p.number }).join("  \u00b7  ") }
    function addressLine(c) { return (c.addresses || []).map(function(a) { return a.kind + ": " + a.text }).join("  |  ") }
    function subtitle(c) {
        var parts = []
        if (c.job_title) parts.push(c.job_title)
        if (c.company)   parts.push(c.company)
        return parts.join(" \u00b7 ")
    }

    // ── data ───────────────────────────────────────────────────────
    function _url() {
        var u = backendUrl + "/contacts?view=" + viewMode + "&sort=" + sortKey
        if (accountFilter !== "") u += "&account=" + encodeURIComponent(accountFilter)
        if (searchText.trim() !== "") u += "&q=" + encodeURIComponent(searchText.trim())
        return u
    }
    function load() {
        var serial = ++_serial
        loading = true
        var xhr = new XMLHttpRequest()
        xhr.onreadystatechange = function() {
            if (xhr.readyState !== XMLHttpRequest.DONE || serial !== root._serial) return
            root.loading = false
            if (xhr.status === 200) {
                try {
                    var list = JSON.parse(xhr.responseText)
                    var keep = root._wantId !== "" ? root._wantId : root.selectedId
                    root.contacts = list
                    root.loaded = true
                    var at = -1
                    for (var i = 0; i < list.length; i++) if (list[i].id === keep) { at = i; break }
                    root.cur = at >= 0 ? at : Math.max(0, Math.min(root.cur, list.length - 1))
                    root._wantId = ""
                    // marks of contacts that are gone
                    var m = {}, ids = {}
                    for (var j = 0; j < list.length; j++) ids[list[j].id] = true
                    Object.keys(root.marks).forEach(function(k) { if (ids[k]) m[k] = true })
                    root.marks = m
                    if (root.detailOpen) root.loadDetail()
                } catch (e) { console.log("[People] parse error:", e) }
            }
        }
        xhr.open("GET", _url())
        xhr.send()
    }

    function loadDetail() {
        var id = selectedId
        if (id === "") { detail = null; return }
        var serial = ++_detailSerial
        detailLoading = detail === null || (detail.id !== id)
        var xhr = new XMLHttpRequest()
        xhr.onreadystatechange = function() {
            if (xhr.readyState !== XMLHttpRequest.DONE || serial !== root._detailSerial) return
            root.detailLoading = false
            try { root.detail = JSON.parse(xhr.responseText) } catch (e) { root.detail = { ok: false, error: "unreadable answer" } }
        }
        xhr.open("GET", backendUrl + "/contacts/get?id=" + encodeURIComponent(id))
        xhr.send()
    }

    function _post(path, cb) {
        var xhr = new XMLHttpRequest()
        xhr.onreadystatechange = function() {
            if (xhr.readyState !== XMLHttpRequest.DONE) return
            if (cb) cb(xhr)
        }
        xhr.open("POST", backendUrl + path)
        xhr.send()
    }

    function _postJson(path, body, cb) {
        var xhr = new XMLHttpRequest()
        xhr.onreadystatechange = function() {
            if (xhr.readyState !== XMLHttpRequest.DONE) return
            var r = null
            try { r = JSON.parse(xhr.responseText) } catch (e) {}
            cb(r || { ok: false, error: "the backend did not answer" })
        }
        xhr.open("POST", backendUrl + path)
        xhr.send(JSON.stringify(body))
    }

    // ── create / edit / delete / undo (instant saves; docs/PLAN-contacts.md decision 4) ──
    function _writableAccount(id) {
        for (var i = 0; i < appAccounts.length; i++) {
            var a = appAccounts[i]
            if (a.id === id) return a.signed_in && a.provider !== "gmail"
        }
        return false
    }
    function newContact() {
        var xhr = new XMLHttpRequest()
        xhr.onreadystatechange = function() {
            if (xhr.readyState !== XMLHttpRequest.DONE) return
            var list = []
            try { list = JSON.parse(xhr.responseText).folders || [] } catch (e) {}
            var t = list.filter(function(f) { return root._writableAccount(f.account_id) })
                        .map(function(f) { return { accountId: f.account_id, accountEmail: f.account_email, folderId: f.id, folderName: f.name } })
            // the account being looked at goes first
            var want = root.accountFilter !== "" ? root.accountFilter : (root.selected ? root.selected.account_email : "")
            t.sort(function(a, b) { return (b.accountEmail === want ? 1 : 0) - (a.accountEmail === want ? 1 : 0) })
            root.editorTargets = t
            root.editorRaw = null
            root.editorStatus = ""
            root.editorOpen = true
        }
        xhr.open("GET", backendUrl + "/contacts/folders")
        xhr.send()
        return ""
    }
    function editContact() {
        if (!selected) return "No contact selected"
        ensureDetail(function(ok) {
            if (!ok) { root.notice("Could not read that contact"); return }
            if (detail.sync_state === "deleted") { root.notice("That contact is being deleted"); return }
            root.editorRaw = detail.contact
            root.editorStatus = ""
            root.editorOpen = true
        })
        return ""
    }
    function saveFromEditor(body) {
        editorStatus = "saving\u2026"
        _postJson("/contacts/save", body, function(r) {
            if (r.ok !== true) { root.editorStatus = r.error || "not saved"; return }
            root.editorOpen = false
            root.editorStatus = ""
            if (r.unchanged === true) { root.notice("No changes"); return }
            root._wantId = r.id
            root.load()
            root.notice(r.kind === "create" ? "Contact saved \u2014 Ctrl-Z takes it back" : "Saved \u2014 Ctrl-Z takes it back")
        })
    }
    function keyDelete() {
        var ids = targetIds()
        if (ids.length === 0) return
        var n = ids.length
        _postJson("/contacts/delete", { ids: ids }, function(r) {
            if (r.ok !== true) { root.notice(r.error || "not deleted"); return }
            root.clearMarks()
            root.load()
            root.notice((n === 1 ? "Contact deleted" : n + " contacts deleted") + " \u2014 Ctrl-Z brings it back")
        })
    }
    function undoLast() {
        _postJson("/contacts/undo", {}, function(r) {
            root.notice(r.ok === true ? "Undone" : (r.error || "Nothing to undo"))
            root.load()
        })
    }
    // a write that failed goes round again (R); Ctrl-Z drops it instead
    function retryFailed() {
        if (!selected || selected.sync_state === "ok") return "Nothing to retry on this contact"
        _post("/contacts/retry?id=" + encodeURIComponent(selected.id), function() { root.load() })
        return "Trying again"
    }

    function setFavorite(id, value) {
        _post("/contacts/favorite?id=" + encodeURIComponent(id) + "&value=" + value, function() { root.load() })
    }
    function toggleFavorite() {
        if (!selected) return "No contact selected"
        var was = selected.is_favorite
        setFavorite(selected.id, !was)
        return was ? "Removed from favorites" : "Added to favorites"
    }

    onViewModeChanged: load()
    onSortKeyChanged:  load()
    onAccountFilterChanged: { cur = 0; load() }
    onSearchTextChanged: { cur = 0; load() }
    onSelectedIdChanged: if (detailOpen) loadDetail()
    onDetailOpenChanged: if (detailOpen) loadDetail()
    Component.onCompleted: load()

    // the initial sync fills the address book over time, and the daemon changes rows: keep the list fresh
    Timer { interval: 20000; running: true; repeat: true; onTriggered: root.load() }

    // ── keyboard (called by the key registry; see AppCommands.move/page/edge/activate) ──
    function _clamp(i) { return Math.max(0, Math.min(contacts.length - 1, i)) }
    function _show() { list.positionViewAtIndex(cur, ListView.Contain) }
    function keyMove(dx, dy) {
        if (dy !== 0) { cur = _clamp(cur + dy); _show(); return true }
        if (dx > 0) { if (selected) detailOpen = true; return true }
        if (dx < 0) { if (detailOpen) { detailOpen = false; return true } return false }
        return true
    }
    function keyPage(frac) { cur = _clamp(cur + Math.round(frac * Math.max(1, Math.floor(list.height / rowHeight)))); _show() }
    function keyEdge(bottom) { cur = bottom ? contacts.length - 1 : 0; _show() }
    function keyActivate() { if (selected) detailOpen = !detailOpen; return true }
    function pageScroll(dir) { keyPage(dir * 0.9) }

    // `/`: type a filter; Enter keeps it, Esc drops it
    function startSearch() { searching = true; searchField.text = searchText; searchField.forceActiveFocus(); searchField.selectAll() }
    function endSearch(keep) {
        searching = false
        if (!keep) { searchField.text = ""; searchText = "" }
        else searchText = searchField.text
    }
    function clearSearch() { searchField.text = ""; searchText = "" }

    function cycleSort() {
        var i = 0
        for (var k = 0; k < sortOptions.length; k++) if (sortOptions[k].value === sortKey) i = k
        var n = sortOptions[(i + 1) % sortOptions.length]
        sortKey = n.value
        return "Sorted by " + n.label.toLowerCase()
    }
    function cycleView() {
        var i = 0
        for (var k = 0; k < viewOptions.length; k++) if (viewOptions[k].value === viewMode) i = k
        var n = viewOptions[(i + 1) % viewOptions.length]
        viewMode = n.value
        return "Showing " + n.label.toLowerCase()
    }
    // A: every account, then each account in turn (only with two or more)
    readonly property var accountEmails: appAccounts.filter(function(a) { return a.email }).map(function(a) { return a.email })
    function cycleAccount() {
        var list = [""].concat(accountEmails)
        var i = Math.max(0, list.indexOf(accountFilter))
        accountFilter = list[(i + 1) % list.length]
        return accountFilter === "" ? "Showing every account" : "Showing " + accountFilter
    }

    function toggleMark() {
        if (!selected) return
        var m = Object.assign({}, marks)
        if (m[selected.id]) delete m[selected.id]; else m[selected.id] = true
        marks = m
        keyMove(0, 1)
    }
    function markAll() {
        var m = {}
        contacts.forEach(function(c) { m[c.id] = true })
        marks = m
    }
    function clearMarks() { marks = ({}) }
    // The contacts an action works on: the marked ones, else the one under the cursor.
    function targetIds() {
        var ids = Object.keys(marks)
        if (ids.length > 0) return ids
        return selected ? [selected.id] : []
    }

    // Make `detail` the contact under the cursor (it holds every field), then call cb(ok).
    function ensureDetail(cb) {
        if (!selected) { cb(false); return }
        if (detail && detail.ok === true && detail.id === selected.id) { cb(true); return }
        var serial = ++_detailSerial
        var xhr = new XMLHttpRequest()
        xhr.onreadystatechange = function() {
            if (xhr.readyState !== XMLHttpRequest.DONE) return
            var d = null
            try { d = JSON.parse(xhr.responseText) } catch (e) {}
            if (serial === root._detailSerial && d) root.detail = d
            cb(!!d && d.ok === true)
        }
        xhr.open("GET", backendUrl + "/contacts/get?id=" + encodeURIComponent(selected.id))
        xhr.send()
    }
    // What the keys can act on in the contact under the cursor: [{kind, label, value, shown}] (after ensureDetail).
    function currentTargets(kinds) {
        return detail && detail.ok === true && selected && detail.id === selected.id ? PF.targets(detail.contact, kinds) : []
    }
    // The first address of every marked contact (or of the one under the cursor), for "write to these".
    function addressesOfTargets() {
        var ids = {}, out = []
        targetIds().forEach(function(i) { ids[i] = true })
        contacts.forEach(function(c) { if (ids[c.id] && (c.emails || []).length > 0) out.push(c.emails[0]) })
        return out
    }
    function cardText() { return detail && detail.ok === true ? PF.asText(detail.contact) : "" }
    function copyText(text, what) {
        Quickshell.execDetached(["wl-copy", text])
        return "Copied " + what
    }
    function openTarget(t) {
        if (t.kind === "url") Qt.openUrlExternally(PF.openableUrl(t.value))
        else if (t.kind === "address") Qt.openUrlExternally(PF.mapUrl(t.value))
        else if (t.kind === "email") Qt.openUrlExternally("mailto:" + t.value)
        else if (t.kind === "phone") Qt.openUrlExternally("tel:" + t.value)
    }
    function scrollDetail(px) { detailPane.scrollBy(px) }

    // ── layout ─────────────────────────────────────────────────────
    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // header bar
        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 44
            color: "transparent"
            Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: Qt.rgba(root.textColor.r, root.textColor.g, root.textColor.b, 0.1) }

            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: 12
                anchors.rightMargin: 12
                spacing: 12

                Dropdown {
                    Layout.preferredWidth: 170
                    Layout.alignment: Qt.AlignVCenter
                    showLabel: false
                    options: root.viewOptions
                    value: root.viewMode
                    onChanged: function(v) { root.viewMode = v }
                    visible: !root.searching
                }

                UiText {
                    visible: !root.searching
                    text: "Sort by"
                    dim: true; font.pixelSize: Style.font.caption
                    Layout.alignment: Qt.AlignVCenter
                    Layout.leftMargin: 8
                }

                Dropdown {
                    Layout.preferredWidth: 170
                    Layout.alignment: Qt.AlignVCenter
                    showLabel: false
                    options: root.sortOptions
                    value: root.sortKey
                    onChanged: function(v) { root.sortKey = v }
                    visible: !root.searching
                }

                UiText {
                    visible: !root.searching && root.accountFilter !== ""
                    text: "\u25cf " + root.accountFilter
                    foreground: Hues.accountColor(root.appAccounts, root.accountFilter)
                    font.pixelSize: Style.font.caption
                }

                // the filter: typed after `/`
                UiText { visible: root.searching; text: "/"; foreground: root.accentColor; font.bold: true }
                TextField {
                    id: searchField
                    visible: root.searching
                    Layout.fillWidth: true
                    placeholderText: "every word must match a name, address, phone, company, note or tag"
                    onTextChanged: if (root.searching) searchDebounce.restart()
                    Keys.onReturnPressed: root.endSearch(true)
                    Keys.onEnterPressed: root.endSearch(true)
                    Keys.onEscapePressed: root.endSearch(false)
                }
                Timer { id: searchDebounce; interval: 160; onTriggered: root.searchText = searchField.text }

                UiText {
                    visible: !root.searching && root.searchText !== ""
                    text: "/ " + root.searchText + "   (Esc clears)"
                    foreground: Hues.yellow
                    font.pixelSize: Style.font.caption
                }

                Item { Layout.fillWidth: true; visible: !root.searching }

                UiText {
                    visible: root.markCount > 0
                    text: root.markCount + " marked"
                    foreground: root.accentColor
                    font.pixelSize: Style.font.caption
                }

                UiText {
                    text: root.contacts.length + (root.contacts.length === 1 ? " contact" : " contacts")
                    dim: true; font.pixelSize: Style.font.caption
                }

                UiText {
                    visible: root.loading
                    text: "\u27f3"
                    dim: true
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

            ColumnLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                spacing: 0

                // column headings
                Rectangle {
                    Layout.fillWidth: true
                    Layout.preferredHeight: 24
                    color: "transparent"
                    Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: Qt.rgba(root.textColor.r, root.textColor.g, root.textColor.b, 0.1) }
                    UiText {
                        x: 44; anchors.verticalCenter: parent.verticalCenter
                        text: "NAME"; dim: true; font.pixelSize: Style.font.caption; font.letterSpacing: 1
                    }
                    UiText {
                        x: 44 + root.nameColumnWidth + 12; anchors.verticalCenter: parent.verticalCenter
                        text: "CONTACT INFO"; dim: true; font.pixelSize: Style.font.caption; font.letterSpacing: 1
                    }
                }

                ListView {
                    id: list
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    clip: true
                    model: root.contacts
                    currentIndex: root.cur
                    boundsBehavior: Flickable.StopAtBounds
                    ScrollBar.vertical: ThemedScrollBar {}

                    // "Contact folders": group by folder (the backend already orders by folder)
                    section.property: root.viewMode === "lists" ? "folder_name" : ""
                    section.criteria: ViewSection.FullString
                    section.delegate: Rectangle {
                        required property string section
                        width: list.width
                        height: root.sectionHeight
                        color: Qt.rgba(root.textColor.r, root.textColor.g, root.textColor.b, 0.04)
                        UiText {
                            anchors.left: parent.left; anchors.leftMargin: 14
                            anchors.verticalCenter: parent.verticalCenter
                            text: section
                            foreground: root.accentColor
                            font.bold: true; font.pixelSize: Style.font.caption; font.letterSpacing: 1
                        }
                    }

                    UiText {
                        anchors.centerIn: parent
                        visible: list.count === 0 && root.loaded
                        text: root.searchText !== "" ? "nothing matches \u201c" + root.searchText + "\u201d" : (root.viewMode === "favorites" ? "no favorites yet \u2014 press f on a contact" : "no contacts")
                        dim: true
                    }

                    delegate: Rectangle {
                        id: row
                        required property var modelData
                        required property int index
                        readonly property bool isCur: index === root.cur
                        readonly property bool isMarked: root.marks[modelData.id] === true
                        width: list.width
                        height: root.rowHeight     // identical for every contact
                        color: isCur ? Qt.rgba(root.accentColor.r, root.accentColor.g, root.accentColor.b, 0.14)
                                     : (rowHover.hovered ? Qt.rgba(root.textColor.r, root.textColor.g, root.textColor.b, 0.05) : "transparent")
                        opacity: modelData.sync_state === "pending" ? 0.7 : 1
                        HoverHandler { id: rowHover }
                        TapHandler { onTapped: { root.cur = row.index; root.detailOpen = true } }

                        Rectangle { visible: row.isCur; width: 3; height: parent.height; color: root.accentColor }
                        Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: Qt.rgba(root.textColor.r, root.textColor.g, root.textColor.b, 0.08) }

                        // favorite star, or the mark
                        UiText {
                            x: 12; y: 10
                            text: row.isMarked ? "\u25cf" : (row.modelData.is_favorite ? "\uf005" : "\uf006")
                            foreground: row.isMarked ? root.accentColor : (row.modelData.is_favorite ? Hues.yellow : root.veryDimColor)
                            font.pixelSize: Style.font.body
                            MouseArea {
                                anchors.fill: parent; anchors.margins: -6
                                cursorShape: Qt.PointingHandCursor
                                onClicked: root.setFavorite(row.modelData.id, !row.modelData.is_favorite)
                            }
                        }
                        // state of a write
                        UiText {
                            x: 14; y: 36
                            visible: row.modelData.sync_state !== "ok"
                            text: row.modelData.sync_state === "pending" ? "\u27f3" : (row.modelData.sync_state === "conflict" ? "!" : "\u2715")
                            foreground: row.modelData.sync_state === "pending" ? Hues.yellow : Commons.Color.urgent
                            font.pixelSize: Style.font.bodySmall
                        }

                        // name column
                        Column {
                            x: 44; y: 10
                            width: root.nameColumnWidth
                            spacing: 3
                            UiText {
                                width: parent.width
                                text: row.modelData.display_name
                                elide: Text.ElideRight; wrapMode: Text.NoWrap; maximumLineCount: 1
                                font.bold: true
                            }
                            UiText {
                                width: parent.width
                                text: root.subtitle(row.modelData)
                                elide: Text.ElideRight; wrapMode: Text.NoWrap; maximumLineCount: 1
                                dim: true; font.pixelSize: Style.font.caption
                            }
                            UiText {
                                width: parent.width
                                visible: (row.modelData.categories || []).length > 0
                                text: (row.modelData.categories || []).join("  ")
                                elide: Text.ElideRight; wrapMode: Text.NoWrap; maximumLineCount: 1
                                foreground: root.accentColor; font.pixelSize: Style.font.caption
                            }
                        }

                        // contact info column: emails / phones / addresses, one line each, never wrapped
                        Column {
                            x: 44 + root.nameColumnWidth + 12; y: 8
                            width: row.width - x - 12
                            spacing: 0
                            InfoLine { icon: "\uf0e0"; value: root.emailLine(row.modelData) }
                            InfoLine { icon: "\uf095"; value: root.phoneLine(row.modelData) }
                            InfoLine { icon: "\uf3c5"; value: root.addressLine(row.modelData) }
                        }
                    }
                }
            }

            VSeparator { visible: root.detailOpen; Layout.fillHeight: true }

            ContactDetail {
                id: detailPane
                visible: root.detailOpen
                Layout.preferredWidth: root.detailOpen ? Math.max(320, root.width * 0.42) : 0
                Layout.fillHeight: true
                detail: root.detail
                loading: root.detailLoading
            }
        }
    }

    Loader {
        anchors.fill: parent
        active: root.editorOpen
        z: 100
        sourceComponent: ContactEditor {
            raw: root.editorRaw
            targets: root.editorTargets
            accounts: root.appAccounts
            status: root.editorStatus
            onSaveRequested: function(body) { root.saveFromEditor(body) }
            onCancelRequested: { root.editorOpen = false; root.editorStatus = "" }
        }
    }

    // One fixed-height, single-line field. Overflow is cut with an ellipsis, never wrapped.
    component InfoLine: Item {
        property string icon: ""
        property string value: ""
        width: parent.width
        height: root.lineHeight
        UiText {
            id: ic
            width: 18
            text: parent.icon
            foreground: root.veryDimColor
            font.pixelSize: Style.font.caption
            verticalAlignment: Text.AlignVCenter
            height: parent.height
        }
        UiText {
            anchors.left: ic.right
            anchors.right: parent.right
            height: parent.height
            verticalAlignment: Text.AlignVCenter
            text: parent.value
            wrapMode: Text.NoWrap
            maximumLineCount: 1
            elide: Text.ElideRight
            font.pixelSize: Style.font.bodySmall
        }
    }
}
