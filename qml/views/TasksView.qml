import QtQuick
import QtQuick.Layouts

// TasksView — To-do list with filters, omarchy aesthetic
// Data source: http://127.0.0.1:27182/tasks
Rectangle {
    id: root
    color: "#000000"

    property string monoFont:     "JetBrainsMono Nerd Font"
    property color  accentColor:  "#7c6af7"
    property color  successColor: "#51cf66"
    property color  dangerColor:  "#ff6b6b"
    property color  textColor:    "#e8e8e8"
    property color  dimColor:     "#555555"
    property color  veryDimColor: "#2a2a2a"

    property var tasks: []
    property bool loading: true
    property string filter: "all"   // "all" | "active" | "done"
    property int selectedIndex: -1

    // ── keyboard (called by the key registry) ─────────────────────
    function keyMove(dx, dy) {
        if (dy === 0) return false
        var n = filteredTasks().length
        if (n === 0) return true
        root.selectedIndex = Math.max(0, Math.min(root.selectedIndex + dy, n - 1))
        taskList.positionViewAtIndex(root.selectedIndex, ListView.Contain)
        return true
    }
    function keyPage(frac) {
        var n = filteredTasks().length
        if (n === 0) return
        var step = Math.max(1, Math.round(taskList.height / 46 * Math.abs(frac))) * (frac < 0 ? -1 : 1)
        root.selectedIndex = Math.max(0, Math.min(root.selectedIndex + step, n - 1))
        taskList.positionViewAtIndex(root.selectedIndex, ListView.Contain)
    }
    function keyEdge(bottom) {
        var n = filteredTasks().length
        if (n === 0) return
        root.selectedIndex = bottom ? n - 1 : 0
        taskList.positionViewAtIndex(root.selectedIndex, ListView.Contain)
    }
    function keyActivate() { toggleTask(root.selectedIndex) }

    function filteredTasks() {
        if (root.filter === "active") return root.tasks.filter(function(t){ return !t.completed })
        if (root.filter === "done")   return root.tasks.filter(function(t){ return  t.completed })
        return root.tasks
    }

    function dueLabel(dateStr) {
        if (!dateStr) return ""
        try {
            var d = new Date(dateStr)
            var now = new Date()
            var diff = Math.floor((d - now) / 86400000)
            if (diff < 0)  return "overdue"
            if (diff === 0) return "today"
            if (diff === 1) return "tomorrow"
            if (diff < 7)  return "in " + diff + "d"
            return d.toLocaleDateString(undefined, { month: "short", day: "numeric" })
        } catch(e) { return "" }
    }

    function dueColor(dateStr, completed) {
        if (completed) return root.dimColor
        if (!dateStr) return root.dimColor
        try {
            var d = new Date(dateStr)
            var diff = Math.floor((d - new Date()) / 86400000)
            if (diff < 0)  return root.dangerColor
            if (diff === 0) return "#ffd43b"
            return root.dimColor
        } catch(e) { return root.dimColor }
    }

    property string _lastJson: ""
    function loadTasks() {
        if (root.tasks.length === 0) loading = true
        var xhr = new XMLHttpRequest()
        xhr.onreadystatechange = function() {
            if (xhr.readyState !== XMLHttpRequest.DONE) return
            loading = false
            if (xhr.status === 200) {
                // Unchanged answers must not rebuild the list (it would collapse the open row).
                if (xhr.responseText !== root._lastJson) {
                    try { root.tasks = JSON.parse(xhr.responseText); root._lastJson = xhr.responseText }
                    catch(e) { root.tasks = [] }
                }
            }
        }
        xhr.open("GET", "http://127.0.0.1:27182/tasks")
        xhr.send()
    }

    function toggleTask(taskIndex) {
        var ft = root.filteredTasks()
        if (taskIndex < 0 || taskIndex >= ft.length) return
        var task = ft[taskIndex]
        var xhr = new XMLHttpRequest()
        var newDone = !task.completed
        xhr.open("POST", "http://127.0.0.1:27182/tasks/" + encodeURIComponent(task.id) + "/complete")
        xhr.setRequestHeader("Content-Type", "application/json")
        xhr.send(JSON.stringify({ completed: newDone }))
        // optimistic update
        task.completed = newDone
        root.tasks = root.tasks.slice()
    }

    // ── layout ────────────────────────────────────────────────────
    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // ── toolbar ───────────────────────────────────────────────
        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 38
            color: "#000000"

            Rectangle {
                anchors.bottom: parent.bottom
                width: parent.width; height: 1; color: "#111111"
            }

            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: 14
                anchors.rightMargin: 14
                spacing: 0

                Text {
                    text: "\uf0ae  "  // tasks icon
                    font.family: root.monoFont
                    font.pixelSize: 13
                    color: root.accentColor
                }

                Text {
                    text: "Tasks"
                    font.family: root.monoFont
                    font.pixelSize: 13
                    font.bold: true
                    color: root.textColor
                }

                Item { Layout.fillWidth: true }

                // filter chips
                Repeater {
                    model: [
                        { label: "all",    key: "all"    },
                        { label: "active", key: "active" },
                        { label: "done",   key: "done"   },
                    ]

                    delegate: Rectangle {
                        Layout.leftMargin: 6
                        width: filterText.implicitWidth + 16
                        height: 22
                        radius: 2
                        color: root.filter === modelData.key ? Qt.rgba(0.49, 0.42, 0.97, 0.18) : "transparent"
                        border.color: root.filter === modelData.key ? root.accentColor : "#222222"
                        border.width: 1

                        Text {
                            id: filterText
                            anchors.centerIn: parent
                            text: modelData.label
                            font.family: root.monoFont
                            font.pixelSize: 10
                            color: root.filter === modelData.key ? root.accentColor : root.dimColor
                        }

                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: root.filter = modelData.key
                        }
                    }
                }

                // refresh
                Text {
                    text: "  ↻"
                    font.family: root.monoFont
                    font.pixelSize: 14
                    color: refreshMouse.containsMouse ? root.accentColor : root.dimColor
                    MouseArea {
                        id: refreshMouse
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: root.loadTasks()
                    }
                }
            }
        }

        // ── summary bar ───────────────────────────────────────────
        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 26
            color: "#000000"

            Rectangle {
                anchors.bottom: parent.bottom
                width: parent.width; height: 1; color: "#0d0d0d"
            }

            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: 14
                anchors.rightMargin: 14
                spacing: 20

                Text {
                    property int total:  root.tasks.length
                    property int done_n: root.tasks.filter(function(t){ return t.completed }).length
                    property int active: total - done_n
                    text: active + " remaining  ·  " + done_n + " done"
                    font.family: root.monoFont
                    font.pixelSize: 10
                    color: root.dimColor
                }

                Item { Layout.fillWidth: true }

                Text {
                    visible: root.loading
                    text: "loading…"
                    font.family: root.monoFont
                    font.pixelSize: 10
                    color: root.veryDimColor
                }
            }
        }

        // ── task list ─────────────────────────────────────────────
        ListView {
            id: taskList
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            spacing: 0
            model: root.filteredTasks()
            currentIndex: root.selectedIndex

            // empty state
            Text {
                anchors.centerIn: parent
                visible: taskList.count === 0 && !root.loading
                text: root.filter === "done" ? "nothing completed yet"
                    : root.filter === "active" ? "all done ✓"
                    : "no tasks"
                font.family: root.monoFont
                font.pixelSize: 12
                color: root.veryDimColor
            }

            delegate: Rectangle {
                id: taskRow
                width:  taskList.width
                height: expanded ? 80 : 46
                color:  "#000000"
                clip:   true

                property bool expanded: root.selectedIndex === index
                property var task: modelData

                Behavior on height { NumberAnimation { duration: 120; easing.type: Easing.OutCubic } }

                // bottom separator
                Rectangle {
                    anchors.bottom: parent.bottom
                    width: parent.width; height: 1; color: "#0d0d0d"
                }

                // completed left bar
                Rectangle {
                    visible: task.completed
                    anchors.left: parent.left
                    anchors.top: parent.top
                    anchors.bottom: parent.bottom
                    width: 2
                    color: root.successColor
                    opacity: 0.5
                }

                // selected indicator
                Rectangle {
                    visible: parent.expanded && !task.completed
                    anchors.left: parent.left
                    anchors.top: parent.top
                    anchors.bottom: parent.bottom
                    width: 2
                    color: root.accentColor
                }

                ColumnLayout {
                    anchors.fill: parent
                    anchors.leftMargin: 12
                    anchors.rightMargin: 12
                    anchors.topMargin: 0
                    anchors.bottomMargin: 0
                    spacing: 0

                    // ── main row ───────────────────────────────────
                    RowLayout {
                        Layout.fillWidth: true
                        Layout.preferredHeight: 46
                        spacing: 10

                        // checkbox (drawn)
                        Rectangle {
                            width: 16; height: 16
                            color: "transparent"
                            border.color: task.completed ? root.successColor : root.dimColor
                            border.width: 1

                            // checkmark
                            Text {
                                anchors.centerIn: parent
                                visible: task.completed
                                text: "✓"
                                font.family: root.monoFont
                                font.pixelSize: 10
                                color: root.successColor
                            }

                            MouseArea {
                                anchors.fill: parent
                                cursorShape: Qt.PointingHandCursor
                                onClicked: root.toggleTask(index)
                            }
                        }

                        // title
                        Text {
                            text: task.title || task.subject || "Untitled"
                            font.family: root.monoFont
                            font.pixelSize: 12
                            color: task.completed ? root.dimColor : root.textColor
                            font.strikeout: task.completed
                            elide: Text.ElideRight
                            Layout.fillWidth: true
                        }

                        // due date
                        Text {
                            visible: (task.due_date_time || task.due || "").length > 0
                            text: root.dueLabel(task.due_date_time || task.due || "")
                            font.family: root.monoFont
                            font.pixelSize: 10
                            color: root.dueColor(task.due_date_time || task.due || "", task.completed)
                        }

                        // expand chevron
                        Text {
                            text: parent.parent.expanded ? "▴" : "▾"
                            font.family: root.monoFont
                            font.pixelSize: 9
                            color: root.dimColor
                        }
                    }

                    // ── expanded detail ────────────────────────────
                    Item {
                        visible: taskRow.expanded
                        Layout.fillWidth: true
                        Layout.fillHeight: true

                        RowLayout {
                            anchors.fill: parent
                            anchors.leftMargin: 26
                            anchors.bottomMargin: 10
                            spacing: 20

                            Text {
                                visible: (task.list_name || task.folder || "").length > 0
                                text: "\uf07b  " + (task.list_name || task.folder || "")
                                font.family: root.monoFont
                                font.pixelSize: 10
                                color: root.dimColor
                            }

                            Text {
                                visible: (task.body || task.notes || "").length > 0
                                text: (task.body || task.notes || "").split("\n")[0].substring(0, 80)
                                font.family: root.monoFont
                                font.pixelSize: 10
                                color: root.dimColor
                                Layout.fillWidth: true
                                elide: Text.ElideRight
                            }
                        }
                    }
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: {
                        if (root.selectedIndex === index) root.selectedIndex = -1
                        else root.selectedIndex = index
                    }
                }
            }
        }
    }

    Component.onCompleted: loadTasks()
    // The daemon fills the cache in the background; pick up what it found.
    Timer { interval: 20000; running: root.visible; repeat: true; onTriggered: root.loadTasks() }
}
