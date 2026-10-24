import QtQuick
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../common"

// Base for every settings sub-page: title, description, optional placeholder
// badge, then a content column (children declared in a page land in `body`).
//
// Pages report `editing` while one of their inputs owns the keyboard so the
// shell can suspend its j/k/h/l handling.
Item {
  id: root

  property string title: ""
  property string description: ""
  property bool placeholder: false
  property bool editing: false

  // PanelSlider wants a `bar`-like object for its colours.
  readonly property QtObject sliderBar: QtObject {
    readonly property color foreground: Commons.Color.foreground
    readonly property color background: Commons.Color.background
    readonly property color urgent: Commons.Color.urgent
  }

  default property alias content: body.data

  implicitHeight: column.implicitHeight

  Column {
    id: column
    width: parent.width
    spacing: Style.space(12)

    Row {
      spacing: Style.spacing.lg

      UiText {
        text: root.title
        font.pixelSize: Style.font.heading
        font.bold: true
      }

      BorderSurface {
        visible: root.placeholder
        anchors.verticalCenter: parent.verticalCenter
        implicitWidth: badge.implicitWidth + Style.spacing.lg * 2
        implicitHeight: badge.implicitHeight + Style.spacing.xs * 2
        color: "transparent"
        radius: Style.cornerRadius
        borderSpec: Border.controlSpec("normal", Commons.Color.foreground, Commons.Color.accent)

        UiText {
          id: badge
          anchors.centerIn: parent
          text: "PLACEHOLDER"
          dim: true
          font.pixelSize: Style.font.caption
          font.bold: true
        }
      }
    }

    UiText {
      visible: root.description !== ""
      width: parent.width
      text: root.description
      dim: true
      wrapMode: Text.WordWrap
      font.pixelSize: Style.font.bodySmall
    }

    PanelSeparator { width: parent.width }

    Column {
      id: body
      width: parent.width
      spacing: Style.space(12)
    }
  }
}
