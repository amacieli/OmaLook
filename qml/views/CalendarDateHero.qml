import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import "../common"
import "CalendarUtil.js" as CU

// Today's date as a quiet masthead: a very large day number in the accent colour, a hairline, then the
// weekday in spaced capitals over day / month / year. Colours and sizes come from the theme only.
Item {
    id: root

    property date today: new Date()
    signal clicked()

    readonly property color fg: Commons.Color.foreground
    readonly property var dayNames: ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"]

    implicitHeight: row.implicitHeight
    implicitWidth: row.implicitWidth
    clip: true

    RowLayout {
        id: row
        width: root.width
        spacing: Style.spacing.lg

        UiText {
            id: num
            text: root.today.getDate()
            foreground: Commons.Color.accent
            font.pixelSize: Style.font.title * 4
            font.weight: Font.Light
            Layout.alignment: Qt.AlignVCenter
        }

        Rectangle {   // hairline between the number and the words
            Layout.alignment: Qt.AlignVCenter
            Layout.preferredWidth: 1
            Layout.preferredHeight: num.implicitHeight * 0.62
            color: Qt.rgba(Commons.Color.accent.r, Commons.Color.accent.g, Commons.Color.accent.b, 0.55)
        }

        ColumnLayout {
            Layout.alignment: Qt.AlignVCenter
            Layout.fillWidth: true
            Layout.minimumWidth: 0
            spacing: Style.spacing.sm

            UiText {
                text: root.dayNames[CU.isoIndex(root.today)].toUpperCase()
                font.pixelSize: Style.font.subtitle
                font.bold: true
                font.letterSpacing: 3
                Layout.fillWidth: true; elide: Text.ElideRight
            }
            UiText {
                text: CU.MONTHS[root.today.getMonth()] + " " + root.today.getFullYear()
                dim: true
                font.pixelSize: Style.font.subtitle
                font.letterSpacing: 2
            }
            UiText {
                text: CU.pad(root.today.getDate()) + " / " + CU.pad(root.today.getMonth() + 1) + " / " + root.today.getFullYear()
                dim: true
                font.pixelSize: Style.font.caption
                font.letterSpacing: 3
                opacity: 0.8
            }
        }
    }

    MouseArea {
        anchors.fill: parent
        cursorShape: Qt.PointingHandCursor
        onClicked: root.clicked()
    }
}
