import QtQuick
import qs.Commons
import qs.Commons as Commons
import "../common"
import "CalendarUtil.js" as CU

// Read-only detail pane for the selected calendar event (docs/PLAN-calendar-events.md 5.2, phase 1).
// `event` is a normalised event (CU.normalise): the backend row is `event.raw`.
Rectangle {
    id: root

    property var    event: null
    property var    accounts: []
    property string monoFont:     Style.font.family
    property color  accentColor:  Commons.Color.accent
    property color  textColor:    Commons.Color.foreground
    property color  dimColor:     Qt.darker(Commons.Color.foreground, 1.4)

    readonly property var raw: event ? event.raw : null
    readonly property string joinHref: CU.joinUrl(raw)
    readonly property var attendees: raw && raw.attendees ? raw.attendees : []

    color: "transparent"
    clip: true
    Rectangle { anchors.left: parent.left; width: 1; height: parent.height; color: Qt.rgba(Commons.Color.foreground.r, Commons.Color.foreground.g, Commons.Color.foreground.b, 0.12) }

    function whenText() {
        if (!event) return ""
        var d = event.startDate
        var day = CU.DAYS_SHORT[CU.isoIndex(d)] + " " + d.getDate() + " " + CU.MONTHS[d.getMonth()].substring(0, 3) + " " + d.getFullYear()
        if (event.allDay) return day + "  all day"
        return day + "  " + CU.timeLabel(event.startMin) + " – " + CU.timeLabel(event.endMin)
    }
    function glyph(resp) {
        return resp === "accepted" ? "✓" : resp === "declined" ? "✗" : resp === "tentativelyAccepted" ? "~" : resp === "organizer" ? "★" : "·"
    }

    Text {
        visible: !root.event
        anchors.centerIn: parent
        text: "No event selected"
        font.family: root.monoFont; font.pixelSize: Style.font.bodySmall; color: root.dimColor
    }

    Flickable {
        id: flick
        visible: !!root.event
        anchors.fill: parent
        anchors.margins: 14
        contentWidth: width
        contentHeight: col.implicitHeight
        boundsBehavior: Flickable.StopAtBounds
        interactive: contentHeight > height

        Column {
            id: col
            width: flick.width
            spacing: 8

            Text {
                width: parent.width
                text: root.event ? root.event.title : ""
                wrapMode: Text.Wrap
                font.family: root.monoFont; font.pixelSize: Style.font.subtitle; font.bold: true
                color: root.textColor
            }
            Text {
                width: parent.width
                text: root.whenText()
                font.family: root.monoFont; font.pixelSize: Style.font.bodySmall; color: root.accentColor
            }
            Text {
                visible: !!root.raw && root.raw.location !== undefined && root.raw.location !== ""
                width: parent.width; wrapMode: Text.Wrap
                text: root.raw ? "at  " + root.raw.location : ""
                font.family: root.monoFont; font.pixelSize: Style.font.bodySmall; color: root.textColor
            }
            Text {
                visible: root.joinHref !== ""
                width: parent.width; wrapMode: Text.WrapAnywhere
                text: "join  " + CU.joinLabel(root.raw) + "   J open · y copy\n" + root.joinHref
                font.family: root.monoFont; font.pixelSize: Style.font.caption; color: root.accentColor
            }
            Text {
                visible: !!root.raw && !!root.raw.organizer_email
                width: parent.width; wrapMode: Text.Wrap
                text: root.raw ? "from  " + (root.raw.organizer_name ? root.raw.organizer_name + " <" + root.raw.organizer_email + ">" : root.raw.organizer_email) : ""
                font.family: root.monoFont; font.pixelSize: Style.font.caption; color: root.dimColor
            }
            Text {
                width: parent.width; wrapMode: Text.Wrap
                visible: text !== ""
                text: {
                    if (!root.raw) return ""
                    var bits = []
                    var r = CU.responseLabel(root.raw.my_response)
                    if (r) bits.push(r)
                    if (root.raw.show_as) bits.push("shown as " + root.raw.show_as)
                    if (root.raw.reminder_minutes !== undefined && root.raw.reminder_minutes !== null && root.raw.reminder_minutes >= 0)
                        bits.push("reminder " + root.raw.reminder_minutes + " min")
                    if (root.raw.sensitivity && root.raw.sensitivity !== "normal") bits.push(root.raw.sensitivity)
                    return bits.join("  ·  ")
                }
                font.family: root.monoFont; font.pixelSize: Style.font.caption; color: root.dimColor
            }
            Text {
                visible: root.accounts.length > 1 && !!root.raw && !!root.raw.account_email
                width: parent.width
                text: root.raw ? "calendar  " + root.raw.account_email : ""
                font.family: root.monoFont; font.pixelSize: Style.font.caption
                color: root.raw ? Hues.accountColor(root.accounts, root.raw.account_email) : root.dimColor
            }

            Text {
                visible: root.attendees.length > 0
                text: "ATTENDEES (" + root.attendees.length + ")"
                font.family: root.monoFont; font.pixelSize: Style.font.caption; font.letterSpacing: 1; color: root.dimColor
            }
            Repeater {
                model: root.attendees
                delegate: Text {
                    width: col.width
                    elide: Text.ElideRight
                    text: root.glyph(modelData.response) + " " + (modelData.name || modelData.email)
                          + (modelData.role === "optional" ? "  (optional)" : modelData.role === "resource" ? "  (resource)" : "")
                    font.family: root.monoFont; font.pixelSize: Style.font.caption; color: root.textColor
                }
            }

            Text {
                visible: !!root.raw && !!root.raw.body && root.raw.body.trim() !== ""
                width: parent.width; wrapMode: Text.Wrap
                text: root.raw && root.raw.body ? root.raw.body.trim().substring(0, 1500) : ""
                font.family: root.monoFont; font.pixelSize: Style.font.caption; color: root.dimColor
                textFormat: Text.PlainText
            }
        }
    }
}
