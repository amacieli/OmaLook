import QtQuick
import qs.Commons
import qs.Ui
import "../../common"
import ".."

// "New events" and "Recurring meeting windows" are wired to the backend; the week layout values
// above them are still placeholders and are not persisted.
SettingsPage {
  id: page

  property var app: null

  property string weekStart: "mon"
  property bool weekNumbers: false
  property int dayStartHour: 8

  editing: accountDropdown.popupOpen || backYears.field.activeFocus || aheadYears.field.activeFocus
        || lengthField.field.activeFocus || reminderField.field.activeFocus || seriesField.field.activeFocus

  title: "Calendar"
  description: "Week layout and default reminders."
  placeholder: true

  Column {
    width: parent.width
    spacing: Style.space(6)

    PanelSectionHeader { text: "WEEK STARTS ON" }
    ButtonGroup {
      options: [{ value: "sun", label: "Sun" }, { value: "mon", label: "Mon" }, { value: "sat", label: "Sat" }]
      value: page.weekStart
      onChanged: function(v) { page.weekStart = v }
    }
  }

  Toggle {
    width: parent.width
    label: "Show week numbers"
    description: page.weekNumbers ? "ISO week numbers visible" : "Week numbers hidden"
    checked: page.weekNumbers
    onClicked: page.weekNumbers = !page.weekNumbers
  }

  Column {
    width: parent.width
    spacing: Style.space(6)

    Row {
      width: parent.width
      PanelSectionHeader { text: "DAY STARTS AT"; width: parent.width - hourReadout.implicitWidth }
      UiText {
        id: hourReadout
        text: (slider.dragging ? Math.round(slider.liveValue) : page.dayStartHour) + ":00"
        dim: true
        font.pixelSize: Style.font.caption
        font.bold: true
      }
    }

    PanelSlider {
      id: slider
      bar: page.sliderBar
      width: parent.width
      minimum: 0
      maximum: 12
      step: 1
      integer: true
      value: page.dayStartHour
      onReleased: function(v) { page.dayStartHour = Math.round(v) }
    }
  }

  PanelSeparator { width: parent.width }

  // ── wired: defaults for new events (docs/PLAN-calendar-events.md section 9) ──
  Column {
    width: parent.width
    spacing: Style.space(6)

    PanelSectionHeader { text: "NEW EVENTS" }

    Row {
      spacing: Style.space(16)
      NumberField {
        id: lengthField
        label: "Length (minutes)"
        value: page.app ? page.app.calendarDefaultMinutes : 30
        from: 5; to: 1440; stepSize: 5
        onModified: function(v) { if (page.app) page.app.setCalendarDefaults(v, page.app.calendarDefaultReminder, page.app.calendarDefaultSeriesMonths, page.app.calendarDefaultAccount) }
      }
      NumberField {
        id: reminderField
        label: "Reminder (minutes before)"
        value: page.app ? page.app.calendarDefaultReminder : 15
        from: 0; to: 10080; stepSize: 5
        onModified: function(v) { if (page.app) page.app.setCalendarDefaults(page.app.calendarDefaultMinutes, v, page.app.calendarDefaultSeriesMonths, page.app.calendarDefaultAccount) }
      }
      NumberField {
        id: seriesField
        label: "Series ends after (months, 0 = never)"
        value: page.app ? page.app.calendarDefaultSeriesMonths : 12
        from: 0; to: 120; stepSize: 1
        onModified: function(v) { if (page.app) page.app.setCalendarDefaults(page.app.calendarDefaultMinutes, page.app.calendarDefaultReminder, v, page.app.calendarDefaultAccount) }
      }
    }

    UiText {
      width: parent.width
      text: "A reminder of 0 fires at the time of the event. A new repeating event ends this many months after its first date unless you type an end."
      dim: true; wrapMode: Text.WordWrap; font.pixelSize: Style.font.caption
    }

    Dropdown {
      id: accountDropdown
      width: parent.width
      label: "New events go to"
      value: page.app ? page.app.calendarDefaultAccount : ""
      options: {
        var o = [{ value: "", label: "The account I used last" }]
        var list = page.app ? page.app.accounts : []
        for (var i = 0; i < list.length; i++)
          if (list[i].provider === "exchange" && list[i].email) o.push({ value: list[i].id, label: list[i].email })
        return o
      }
      onChanged: function(v) { if (page.app) page.app.setCalendarDefaults(page.app.calendarDefaultMinutes, page.app.calendarDefaultReminder, page.app.calendarDefaultSeriesMonths, v) }
    }
  }

  PanelSeparator { width: parent.width }

  // ── wired to the daemon ──────────────────────────────────────
  Column {
    width: parent.width
    spacing: Style.space(6)

    PanelSectionHeader { text: "RECURRING MEETING WINDOWS" }

    UiText {
      width: parent.width
      text: "How far recurring meetings are expanded into individual occurrences, either side of today. "
          + "Applied on the next sync cycle (within a couple of minutes); narrowing the window removes "
          + "the occurrences outside it."
      dim: true
      wrapMode: Text.WordWrap
      font.pixelSize: Style.font.caption
    }

    Row {
      spacing: Style.space(16)

      NumberField {
        id: backYears
        label: "Years back"
        value: page.app ? page.app.recurrenceYearsBack : 5
        from: 0
        to: page.app ? page.app.maxYearsBack : 20
        stepSize: 1
        onModified: function(v) { if (page.app) page.app.setRecurrenceWindow(v, page.app.recurrenceYearsAhead) }
      }

      NumberField {
        id: aheadYears
        label: "Years forward"
        value: page.app ? page.app.recurrenceYearsAhead : 10
        from: 1
        to: page.app ? page.app.maxYearsAhead : 30
        stepSize: 1
        onModified: function(v) { if (page.app) page.app.setRecurrenceWindow(page.app.recurrenceYearsBack, v) }
      }
    }
  }
}
