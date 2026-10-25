# PLAN — New calendar entries: meetings, attendees, free time, online meetings, recurrence

Status: PLAN ONLY. No code written. First written 2026-10-25; revised twice on 2026-10-25 after the user's answers (section 0).
Tracker item: OL-021 (also covers the "join action" half of OL-010).
Scope: create (then edit / cancel) calendar entries from the keyboard, on any signed-in account. Microsoft Graph work/school
accounts first; the design lets Google Calendar (read-only in the app today) and later CalDAV slot in without rework.

## 0. Decisions so far (user, 2026-10-25)

| # | Decision | Where it lands |
|---|---|---|
| 1 | Work/school Microsoft accounts first. An **account switcher** chooses which account gets the entry, answers free time, etc. | 5.1, 5.6 |
| 2 | The user can add the extra Graph rights if needed. The exact list and what each is for are in section 4.1. | 4.1 |
| 3 | Meeting-provider accounts are added in **Settings -> Accounts**, in a separate section on the same screen, using the email-account mechanism: dropdown, **Add**, then that provider's sign-in. | 7.5 |
| 4 | Cursor keys: open to suggestions. Recommendation and precedent in 5.2. | 5.2 |
| 5 | Undo window reuses the existing setting (`[mail] send_delay_secs`, Settings -> Mail, default 3 s, max 60). **Every** calendar write goes through it for consistency: create (personal ones too), edit, delete, cancel. | 6 |
| 6 | Time entry: typed **and** picker, as long as it is not confusing. Design: one field, the picker only fills it. | 5.4 |
| 7 | Defaults: 30 min, working hours, 15 min reminder, default online provider. All adjustable in Settings -> Calendar. | 9 |
| 8 | Free-time finder and online meetings are independent tracks, built in whichever order the user wants. | 11 |
| 9 | **Recurrence is in the first version.** How far ahead recurring entries are expanded is a setting; the default is what the read-only calendar does today (5 years back, 10 ahead). | 5.7, 9 |
| 10 | Cursor keys accepted as proposed (hjkl cursor, `[ ]` periods, `c`/`n` new). | 5.2 |
| 11 | Zoom OAuth is a later phase (the user will register the app then). Until then Zoom ships as a **stub**: present in the dropdown and picker, reporting "not set up yet". The stored personal Zoom link works meanwhile. Webex the same. | 7.5, 11 |
| 12 | New-entry account when nothing decides it: **last used**. Mail has the same gap today (new mail takes the first signed-in account); tracked as OL-022 and shares the mechanism. | 5.1, 9 |
| 13 | Finder checking your other accounts for double-booking: **opt-in**. | 5.6 |
| 14 | A new series ends after a configurable number of months, **default 12**, set in Settings -> Calendar (0 = never). | 5.7, 9 |
| 15 | The user decides the next phases; the documents come first and nothing is built until then. | 11 |

## 1. Findings (what exists today)

Read from the code on 2026-10-25, not assumed.

- **The calendar is read-only.** `CalendarProvider` (`src/providers/calendar.rs`) has `fetch_events`, `fetch_all_events`,
  `fetch_window`, `is_token_valid`; no create / update / delete. `GoogleCalendarProvider` implements the same trait (primary
  calendar, read-only), so the trait is already the multi-provider seam.
- **The cached event is thin.** `CalendarEvent` = id, subject, body, start, end, is_all_day, time_zone, event_type, series_master_id.
  Graph `$select` is `id,subject,body,start,end,isAllDay,type,seriesMasterId`. No location, attendees, organizer, response, show-as,
  join URL or recurrence pattern, so the UI cannot show or edit them yet.
- **Multiple accounts already flow through the calendar backend.** `calendar_events` rows carry `account_id`; `GET /calendar/events`
  accepts `?account=` and returns `account_id` / `account_email`. One calendar sync thread runs per signed-in account.
  The UI has no account switcher for the calendar yet.
- **Times are local wall-clock strings** in this machine's zone, requested from Graph with `Prefer: outlook.timezone="<IANA zone>"`.
  Series are expanded per window (`calendarView`). Google ids are scoped `<account>:<id>`.
- **Recurrence window.** `[calendar] recurrence_years_back` (default 5, 0..20) and `recurrence_years_ahead` (default 10, 1..30), whole
  years, edited in Settings -> Calendar and served at `GET|POST /settings/calendar` (`CalendarSettings`, `src/models.rs`).
- **The calendar UI has no selection.** `CalendarView.qml` / `CalendarTimeGrid.qml` render events and step by period
  (`j/k/[/]` next/prev period, `t d W w m r`). There is no cursor, no detail pane, and `c` ("New event", `AppCommands.qml:416`) is a stub.
- **Settings -> Calendar is mostly placeholders.** Week start, week numbers, day start and a "15" reminder are shown but not persisted
  (the page is flagged `placeholder: true`); only the recurrence windows are wired.
- **Settings -> Accounts** (`AccountPage.qml`): an "ADD ACCOUNT" section (provider `Dropdown` + **Add** button -> provider sign-in),
  then one block per account with signed in / out status, Log in / Log out and a two-step Remove. Meeting accounts can copy this.
- **Scopes.** Graph full tier already has `Calendars.ReadWrite`, `Contacts.ReadWrite`, `Tasks.ReadWrite`, `User.Read`, `Mail.*`, plus
  `MailboxSettings.ReadWrite` in the top tier; accounts that consented earlier sit on a read-only base tier (`Calendars.Read`) via the
  tier fallback in `src/auth.rs`. Google already has full `.../auth/calendar`.
- **Reusable pieces:** `qml/compose/RecipientField.qml` (chips, suggestions, paste lists); the contacts cache; the outbox worker pattern
  (`src/outbox.rs`: claim before network call, transient-only retry, idempotent per id) with the `send_delay_secs` undo window;
  `ComposeView` overlay + Esc save/discard prompt; the command registry + which-key + `docs/KEYS.md`; `invites.rs` (accept / decline
  already work).
- **Security note.** The UI backend on 27182 is unauthenticated with CORS `*` (OL-006). Event create/cancel send invitations to real
  people, so the new routes should ship behind whatever OL-006 decides, or at least check `Origin`/`Host`.

## 2. Design principles

1. **Separate "where the event lives" from "who makes the meeting link."** Calendar provider (Graph, Google, CalDAV) and meeting
   provider (Teams, Zoom, Meet, Webex, Jitsi, pasted link) are two traits. Teams on Graph and Meet on Google are *native*; Zoom and Webex
   are *external* (API call first, then the link goes into the event). Any meeting provider can attach to any calendar provider.
2. **Account-scoped everything.** Capabilities, calendars, working hours, meeting providers, directory, defaults and free/busy
   all belong to the account chosen in the editor. Switching account re-evaluates them (5.1).
3. **One neutral data model, provider-specific serialisers.** UI and cache see `EventDraft` / `CalendarEvent`; only
   `graph_calendar_write.rs` knows Graph JSON. Request-body builders are pure functions with unit tests (no network).
4. **Capabilities, not assumptions.** Each account reports what it can do; the editor greys out what it cannot, with the reason.
5. **Personal entry is the zero-friction path.** No attendees = no invitations and no scheduling lookup; one keystroke to save, and it is covered by the same undo window as everything else.
6. **Nothing is final without a window.** Every create, edit, delete and cancel goes through the existing undo window (`[mail] send_delay_secs`), so the rule is the same everywhere and 0 still means "at once".
7. **Keyboard-first, mouse-possible.** Every field and action has a command id, is rebindable, and is documented in `docs/KEYS.md`.
8. **One source of truth per field.** Typed text, arrow nudges, pickers and the free-time finder all write the same field and the field
   always shows the resolved result, so the several ways in never disagree.

## 3. Data model

Add to `CalendarEvent` (and cache columns, via the existing `ALTER TABLE ... ADD COLUMN` migration pattern):
`location`, `organizer_name`, `organizer_email`, `is_organizer`, `attendees_json` (`[{name,email,role,response}]`, role =
`required|optional|resource`), `my_response`, `show_as`, `online_provider`, `online_url`, `reminder_minutes`, `web_link`, `sensitivity`,
`recurrence_json` (the series pattern, series masters only), `calendar_id`. Graph `$select` grows accordingly (`location,attendees,organizer,
isOrganizer,responseStatus,showAs,isOnlineMeeting,onlineMeeting,onlineMeetingProvider,isReminderOn,reminderMinutesBeforeStart,webLink,
sensitivity,recurrence`).

New write-side types (Rust, `src/calendar_write.rs`):

```
EventDraft {
  account_id, calendar_id: Option<String>,        // None = the account's default calendar
  subject, body: Body{Text|Html},                 // Text first; reuse compose HTML later
  start, end (local wall-clock), time_zone, is_all_day,
  location: Option<String>,
  attendees: Vec<Attendee{name,email,role}>,      // empty = personal entry
  meeting: Option<MeetingRequest{meeting_account_id | native, ..}>,   // section 7
  reminder_minutes: Option<u32>, show_as, sensitivity,
  recurrence: Option<Recurrence{pattern, range}>, // section 5.7
  transaction_id: String,                         // generated once per draft; makes retries idempotent
}
FreeBusyQuery { account_id, people: Vec<email>, from, to, interval_min }
FreeBusy { person, busy: Vec<{start,end,status}>, working_hours: Option<..>, known: bool }
```

Trait additions (all with a default `Err(Unsupported)` so Google/CalDAV can lag):

```
CalendarProvider:
  capabilities() -> CalendarCaps{create, attendees, free_busy, directory, recurrence, native_meetings: [..], writable: bool}
  create_event(&EventDraft) -> CreatedEvent{id, web_link, join_url?}
  update_event(id, scope, &EventPatch)  // scope = occurrence | series, phase 3
  cancel_event(id, scope, notify, message)
  free_busy(&FreeBusyQuery) -> Vec<FreeBusy>
  list_calendars() -> Vec<CalendarInfo{id,name,can_edit,allowed_meeting_providers,default_meeting_provider}>
MeetingProvider (new trait, src/meetings/):
  id(), label(), kind() -> Native|External|Static|Generated, available(account) -> Availability{ok|needs_setup|unsupported, why}
  create(&MeetingSpec{subject,start,end,attendees,recurrence}) -> MeetingLink{url, dial_in?, passcode?, external_id}   // External/Generated
  delete(external_id)                                                                                                // best-effort cleanup
```

Local only: a `calendar_outbox` table (id, account_id, payload_json, state, attendees_count, attempts, last_error, due_at, transaction_id),
mirroring the mail outbox; a `meeting_accounts` table (section 7.5). Free/busy answers are cached in memory for 2 minutes only, never
in SQLite: it is other people's schedule.

## 4. Microsoft Graph mapping

| Need | Graph call | Notes |
|---|---|---|
| Create | `POST /me/events` (or `/me/calendars/{id}/events`) | `subject, body{contentType,content}, start/end{dateTime,timeZone}, isAllDay, location{displayName}, attendees[{emailAddress{address,name}, type:"required"/"optional"/"resource"}], reminderMinutesBeforeStart, isReminderOn, showAs, sensitivity, recurrence, transactionId`. With attendees Graph sends the invitations itself; without, none. |
| Recurrence | `recurrence: {pattern{type, interval, daysOfWeek, dayOfMonth, month, index, firstDayOfWeek}, range{type: endDate/noEnd/numbered, startDate, endDate, numberOfOccurrences, recurrenceTimeZone}}` | Pattern types: `daily, weekly, absoluteMonthly, relativeMonthly, absoluteYearly, relativeYearly`. |
| Teams link | same request: `isOnlineMeeting:true, onlineMeetingProvider:"teamsForBusiness"` | Response carries `onlineMeeting.joinUrl`. What a calendar allows: `GET /me/calendars` -> `allowedOnlineMeetingProviders` (`unknown, skypeForBusiness, skypeForConsumer, teamsForBusiness`), `defaultOnlineMeetingProvider`. |
| Free/busy | `POST /me/calendar/getSchedule` `{schedules:[emails], startTime, endTime, availabilityViewInterval}` | Per person: `availabilityView` digits (0 free, 1 tentative, 2 busy, 3 out of office, 4 working elsewhere), `scheduleItems`, `workingHours`. Interval 5..1440 min. A slot holding more than 1000 entries returns error 5006. |
| Suggestions (optional) | `POST /me/findMeetingTimes` | Server-ranked suggestions with confidence and a reason; an extra to `getSchedule`, not a replacement. |
| Update | `PATCH /me/events/{id}` | On an occurrence id it makes an exception; on the series master id it changes the whole series. Time or attendee changes re-send updates. |
| Cancel (organizer) | `POST /me/events/{id}/cancel` `{comment}`; plain `DELETE` for personal entries | Attendee side already done (`respond_to_event`). |
| Attendee lookup | contacts cache first; `GET /me/people?$search=` (People.Read); `GET /users?$filter=startswith(...)` (User.ReadBasic.All) | Directory is work/school only. |

Time zone: send `start.timeZone` as the IANA zone already used, with `Prefer: outlook.timezone`. A DST-gap time (02:30 on the spring day) is
rejected in the editor, never sent. Retry safety: `transactionId` is the idempotency key, so a retried create after a timeout returns the
original event instead of making a duplicate.

### 4.1 Permissions (answer to your question 2)

Checked against Microsoft's Graph reference on 2026-10-25. This **corrects my first draft**, which said `getSchedule` needed
`Calendars.Read.Shared`. It does not.

| Permission (delegated) | State | Used for | Admin consent |
|---|---|---|---|
| `Calendars.ReadWrite` | already requested | create / edit / cancel, Teams link on the event, `getSchedule` free/busy (Calendars.ReadBasic is the documented minimum; ReadWrite and even base-tier `Calendars.Read` cover it) | no |
| `People.Read` | **add** | attendee suggestions: ranked relevant people (recent correspondents plus your organisation's directory) | no |
| `User.ReadBasic.All` | **add** | directory search by name beyond the ranked list | not confirmed (see below) |
| `Calendars.Read.Shared` | **add (optional)** | `findMeetingTimes` server suggestions; reading shared/delegate calendars | no |
| `MailboxSettings.ReadWrite` | already in the top tier | the user's working hours (mailbox setting) | no |

Notes:
- **Nothing new is needed for the free-time grid itself** (track A core), or for Teams links. The three additions buy the people
  search (`People.Read`, `User.ReadBasic.All`) and the optional server suggestions (`Calendars.Read.Shared`).
- Microsoft's own reference lists `People.Read` and `Calendars.Read.Shared` as not requiring admin consent. For `User.ReadBasic.All` I could not
  confirm it from the reference (one third-party guide says admin consent is required), and any tenant can still restrict user consent.
  If a tenant blocks it, the app falls back to the ranked people list and the contacts cache, exactly like the existing tier fallback.
- **Please list all three in the Azure app registration** (client id `9c277d6f-...`) as delegated Microsoft Graph permissions. Users then get a
  re-consent prompt on their next sign-in; accounts that decline, or tenants that block it, keep working on the lower tiers
  (same mechanism as `MailboxSettings.ReadWrite`).
- **Personal Microsoft accounts:** Microsoft documents `getSchedule`, `findMeetingTimes` and the directory user list as *not supported* for
  them. That is why work/school first is right; personal accounts later get create / edit only, no finder, no directory.

## 5. User flow and keyboard model

### 5.1 Accounts: the switcher

- **In the calendar module:** `A` (`cal.account`) cycles the filter All -> account 1 -> account 2 ... (the backend already takes `?account=`).
  Events are tagged with a coloured dot per account when "All" is shown. Only signed-in accounts with a calendar appear;
  base-tier (read-only) accounts appear dimmed with the reason.
- **Which account a new event goes to:** the filtered account if one is chosen; else the account of the event or day under the cursor; else
  the **last account used for a calendar entry** (the default; Settings -> Calendar can pin a specific account instead). "Last used" is remembered in `view_state.json` (OL-014's store) as `last_account.calendar`, written only when an entry is actually saved. The editor always shows the choice. The same rule, with its own key `last_account.mail`, is proposed for new mail (OL-022).
- **In the editor:** the first row is **Account** (and **Calendar** when the account has more than one writable calendar). `Alt-a` jumps there;
  `Left`/`Right` or `Space` cycle, `Enter` opens a list where `1`..`9` pick. The row shows a one-line capability summary, for
  example `work@contoso.com · Teams · free/busy · directory`.
- **Switching account in the middle of an edit** keeps title, time, notes and attendees (addresses are portable) and re-evaluates the rest:
  the Online provider resets to that account's default if the old one is not available (with a visible note, never silently), the
  calendar list reloads, the free-time finder discards its cached answers, attendee suggestions switch directory, and defaults
  (length, reminder) are not re-applied over values the user already changed.
- **The finder answers as the chosen account**: Graph can only show attendee availability through the organiser's tenant (5.6).

### 5.2 Getting to "new event": the calendar cursor

Calendar needs a cursor first (phase 1): a highlighted day (month view) or day + 30-minute slot (day and week views), plus a "selected
event" state when the cursor sits on one.

**What mutt does:** nothing: mutt is a mail client with no calendar (invitations are handled by add-ons or other tools). The closest
keyboard calendar precedent is **khal's `ikhal`** (and calcurse): `h/j/k/l` or the arrows move the cursor, `t` goes to today, `n` creates an event on the selected
date, `Enter` edits, `d` deletes (from khal's documented defaults). The vim convention of j = down, k = up is the common thread.

**Recommendation** (follows ikhal and vim, and fits the which-key model already here):

| Key | Id | Does |
|---|---|---|
| `h` `l` | `cal.cursor.left/right` | Previous / next day |
| `j` `k` | `cal.cursor.down/up` | Month view: next / previous week. Day and week views: next / previous 30-minute slot |
| `[` `]` (also `PgUp` `PgDn`) | `cal.prev` / `cal.next` | Previous / next period (these two keys are kept from today) |
| `Ctrl-d` `Ctrl-u` | `cal.cursor.half` | Half a screen of slots (day / week views) |
| `t` | `cal.today` | Today (kept) |
| `c` or `n` | `cal.new` | New event at the cursor: date and slot start, default length from settings; on month view 09:00 |
| `C` | `cal.new.meeting` | Same, but opens in the Attendees field with the account's default online meeting selected |
| `Enter` | `cal.open` | Detail pane (read) |
| `e` | `cal.edit` | Edit the selected event |
| `x` / `Delete` | `cal.delete` | Delete (personal), cancel (organiser, with optional message), decline (invitee) |
| `J` | `cal.join` | Open the join link of the selected event (any provider); `y` copies it |
| `A` | `cal.account` | Cycle the account filter |
| `d W w m` | existing | Day, work week, week, month view (unchanged) |

The one behaviour change: in the day and week grids `j/k` stop meaning "next period" and become cursor moves, the periods keep `[ ]`. In the
month view `j/k` already feel like "week" and become exactly that. `c` is kept because it is the compose key in mail, `n` is added for ikhal habit.
The alternative (keep `j/k` as periods, use `Ctrl-j/k` or arrows for the cursor) is possible but gives up the vim feel; my advice is the table above.
From mail, `Ctrl-E` on a message opens the editor prefilled (subject, sender and recipients as attendees); later phase.

### 5.3 The editor (an overlay like ComposeView, scope `event/edit`)

```
 New event  ·  Calendar                                                   Esc discard   Ctrl-Enter save
 ┌──────────────────────────────────────────────────────────────────────────────────────────┐
 │ Account    ● work@contoso.com ▸        Teams · free/busy · directory                       │
 │ Title      Quarterly planning                                                              │
 │ When       Tue 27 Oct  10:00 → 10:30   (30m)   [ ] all day       ⚠ overlaps "Standup"      │
 │ Repeat     Weekly on Tue · until 29 Dec 2026 · next: 3 Nov, 10 Nov, 17 Nov                  │
 │ Required   [Jennifer Jackson ×] [gary@x.org ×] ▏                                           │
 │ Optional   [Erika Lang ×] ▏                                                                │
 │ Where      Room 4B                                                                         │
 │ Online     Teams ▾                      join link is added when you save                   │
 │ Reminder   15 min     Show as  Busy                                                        │
 │ Notes      ▏                                                                               │
 └──────────────────────────────────────────────────────────────────────────────────────────┘
 Invitations will be sent to 3 people.     Ctrl-F find a time · Ctrl-M online meeting · Alt-a account
```

With no attendees the footer reads **"Personal entry. No invitations."**, Required/Optional are dim and empty, Online defaults to None (or
the account default if "add automatically" is set), and `Ctrl-Enter` saves (undo window applies, section 6).

| Key | Id | Does |
|---|---|---|
| `Tab` / `Shift-Tab` | `event.field.next/prev` | Next / previous field (text fields own their keys, as everywhere else) |
| `Alt-a t w p r o l m n` | `event.goto.*` | Jump to Account, Title, When, rePeat, Required, Optional, Where, Meeting, Notes |
| `Ctrl-Enter` | `event.save` | Save; with attendees this also sends invitations. Either way the undo window starts |
| `Esc` | `event.cancel` | Empty form closes silently; otherwise Save / Discard / Cancel prompt (same as compose) |
| `Ctrl-F` | `event.findtime` | Free-time finder (5.6) |
| `Ctrl-M` | `event.meeting.pick` | Online meeting picker (7.2) |
| `Alt-Up` / `Alt-Down` (on a chip) | `event.attendee.role` | Move the chip between Required and Optional |
| `Ctrl-Z` (in the bar after any save, edit, delete or cancel) | `event.undo` | Undo during the window, same as undo-send |

### 5.4 When: typed and picked, one field

The **When** row is a single text field. Whatever way you fill it, the row shows the resolved result ("Tue 27 Oct 10:00 → 10:30 (30m)"), so there is
never a second hidden state to be confused by.

- **Typed** (default; parser is JS in `qml/calendar/when.js` with node tests like `tests/search.test.js`, so it is checked without launching the app):
  `tomorrow 3pm`, `fri 10-10:30`, `next tue 2pm for 45m`, `27 oct 14:00 1h`, `all day mon`, `+30m` (shift), `10:30` (keep the date).
  Length defaults from settings (30 min). An ambiguous entry never guesses: the field turns red and shows what it understood.
- **Nudged:** `Up/Down` move the start by 15 min, `Shift-Up/Down` by a day, `Alt-Up/Down` change the length by 15 min (on the When row).
- **Picked:** `F2` or `Ctrl-D` opens a small popup: a month grid (`h j k l` to move, `[ ]` months) then `Tab` to a time list in 15-minute steps
  (`j/k`, `Tab` again for the end time). `Enter` writes the text back into the field and closes it; `Esc` leaves the field unchanged.
- **Found:** the free-time finder (5.6) also just writes the field.

Conflict line: as soon as the time is valid, the cache is checked for overlapping events on the chosen account and on your other
accounts (local, instant, no network). A warning, never a block.

### 5.5 Attendees

- Same chip field as mail (`RecipientField`), fed by: contacts cache, people you mailed recently, then (after two typed letters, debounced
  150 ms, async) the chosen account's directory / ranked people. Suggestions never delay typing.
- A plain address is accepted as is (external invitees), same validity check as compose.
- **Required** and **Optional** are two rows, matching how Graph models it (`type`); moving a chip between them is `Alt-Up/Down`. **Resource**
  (rooms) is a third row later, hidden until the account reports rooms.
- Deleting every attendee turns the entry personal again and the footer updates at once.
- Your own address is never added (the organiser is implicit); pasting it is ignored with a note.

### 5.6 Free-time finder (`Ctrl-F`, scope `event/findtime`)

A panel over the editor: rows = you + each required attendee + each optional attendee; columns = slots for the next working days (default 5,
`[` `]` shifts a week). Cells use the calendar's own colour vocabulary (free blank, tentative hatched, busy solid, out of office or outside working
hours dim, unknown `?`). A highlighted band shows the proposed duration at the cursor. It queries the **account chosen in the editor**.

| Key | Id | Does |
|---|---|---|
| `h` `l` / `j` `k` | `ft.slot` / `ft.day` | Move cursor one slot / one day |
| `n` / `N` | `ft.next` / `ft.prev` | Jump to the next / previous time when **every required attendee is free** (optional ones only break ties) |
| `+` / `-` | `ft.longer/shorter` | Change duration by 15 min |
| `o` | `ft.optional` | Whether optional attendees count toward "free for everyone" |
| `a` | `ft.accounts` | Add / remove a row for your **other signed-in accounts**, so you do not double-book across them (**off by default**, opt-in; the choice is remembered in `view_state.json` as `finder_other_accounts`) |
| `s` | `ft.suggest` | Show the top five ranked slots as a list; `1`..`5` picks one |
| `Enter` | `ft.pick` | Write the chosen start/end back to When and return to the editor |
| `Esc` | `ft.close` | Back to the editor, no change |

Algorithm (Rust, shared by every provider, unit-tested): normalise each provider's answer to busy intervals (Graph digits -> intervals; Google `freeBusy`
already is intervals), clip to working hours (the person's own when the provider reports them, else the user's setting), merge, then for duration D find
start times on the 15-minute grid where all required people are free; rank by (optional attendees free, outside the lunch window, earlier beats later, not
within 15 min of a busy block). People we cannot read (external domain, no consent, no access) show `?`, count as free, and the footer says how many were
unknown: we never claim availability we do not know. Without a free/busy-capable account (personal Microsoft, tier issue) the panel shows your own column only and says why.
Optional extra, later: `findMeetingTimes` as a second source for `s` (needs `Calendars.Read.Shared`).

### 5.7 Recurrence (in version 1)

- **Repeat row**, default None. `Enter` (or typing) works like When: pick from a list (None, Daily, Weekdays, Weekly on <day>,
  Every 2 weeks on <day>, Monthly on the <date>, Monthly on the <nth weekday>, Yearly, Custom...) or type, such as `weekly`, `every 2 weeks on tue,thu`,
  `monthly 2nd tue`, `weekdays`, `daily for 10`. Parser: `qml/calendar/recurrence.js`, node tests.
- **End:** a new series defaults to **ending after `default_series_months` (12 by default; Settings -> Calendar)**, shown as an explicit date ("until 25 Oct 2027") that can be typed over. Other choices: Never, On a date, After N occurrences (`until 29 dec`, `for 10`, `never`). A default is a starting value only; a user's typed end is never overridden.
- **Always shown resolved:** the row prints a sentence ("Weekly on Tue until 29 Dec 2026") and the **next three dates**, so a wrong pattern is caught before saving.
- **Mapping:** Graph `patternedRecurrence` (section 4). The builder fixes the details Graph is strict about (series start date must be the first occurrence and, for weekly
  patterns, a listed weekday; range start date equals the event's start date) and is unit-tested with a table of patterns. "Weekdays" is weekly Mon-Fri.
- **How far ahead it shows is a setting** (section 9): the existing recurrence window, default 5 years back / 10 years ahead, now adjustable in months or years. A series
  with end "Never" (or an end beyond the window) exists on the server forever but is only expanded into the calendar through the window; the editor says so ("shown on your calendar through <date>").
  After a save the daemon re-expands that window at once so the new series is visible immediately. The 12-month default end keeps ordinary series small; the window only matters for series the user makes open-ended.
- **Editing and cancelling a series** asks "This event / All events in the series". Graph does this natively: an occurrence id makes an exception, the series master id changes
  all. **"This and following" is not native in Graph** (it means ending the old series and creating a new one, with attendees receiving two updates), so it is deferred to
  the extras track; v1 offers only the first two choices.
- Teams links on recurring events, and the effect of changing time on a series with attendees, are checked in the spike (phase 0).

## 6. Saving and sending

1. Validate: title (empty becomes "(no title)", as the reader already does), end after start, all attendee addresses valid, recurrence resolves.
2. **Every write is queued, then sent after the undo window** (`due_at = now + [mail] send_delay_secs`, the existing setting, default 3 s, 0 = at once). The write goes in `calendar_outbox`
   as a typed operation: `create`, `update`, `delete`, `cancel`. The calendar shows the result immediately and marks it **pending** (dimmed, with a clock glyph): a new row for a create, the new
   values for an update, a struck-through row for a delete. A bar reads "Event queued (personal). Ctrl-Z undo" or "... for 3 people. Ctrl-Z undo"; `Ctrl-Z` (`event.undo`) cancels the queued
   operation and restores the previous state exactly. Personal entries take the same path as meetings; they just have nobody to notify.
3. **Sending:** the worker claims the operation (`pending` to `sending`) before the network call, so an undo that arrives after the claim is refused, creates the meeting link first if the provider
   is External (section 7), then calls the provider. Transient errors (network, 5xx, 429 with `Retry-After`) retry with backoff using the same `transactionId`; 4xx surface an error, drop the pending
   mark, and reopen the draft so nothing typed is lost.
5. After a crash, an event left in `sending` is reported as "check your calendar", never re-sent blindly.

Read-only accounts (base scope tier): `c` still opens the editor but Save is replaced by "Sign in again to allow editing" with a one-key action that starts the re-consent flow.

## 7. Online meetings: Teams, Zoom and the rest

### 7.1 Provider kinds

| Kind | How the link is made | Providers |
|---|---|---|
| Native | By the calendar service in the create request; rides on the calendar account, no separate sign-in | Teams on Graph work/school; Google Meet on Google (`conferenceData.createRequest`) |
| External API | OmaLook calls the vendor first (needs a meeting account), then writes the URL into location and body | Zoom (`POST /users/me/meetings`), Webex (`POST /meetings`) |
| Static | A fixed room URL stored once | Zoom Personal Meeting ID link, a Teams meeting-room link, any `https://` bridge |
| Generated | A URL needing no account | Jitsi (`https://meet.jit.si/<random-slug>`, or your own server) |
| Pasted | A URL typed for this one event | everything else |

**Zoom has no Graph integration**, and Zoom's Outlook add-ins are no use here, so the routes are the Zoom REST API (an OAuth app) or a stored link.

### 7.2 Choosing one (`Ctrl-M` or the Online row)

A small picker listing what is usable for the chosen account. External providers show one row per meeting account, so two Zoom logins are two rows:

```
 Online meeting
  n  None
  t  Microsoft Teams            (account default)
  z  Zoom — work@acme.com
  Z  Zoom — personal
  j  Jitsi
  l  Paste a link...
  g  Google Meet                not available for this account
```

`Enter` or the letter selects; `Esc` leaves it unchanged. Unavailable rows stay visible with the reason (so the user knows what to set up) and are skipped by the cursor.

### 7.3 What lands in the event

- Native: the service writes the join URL, dial-in block and body text.
- External / Static / Generated / Pasted: the URL goes in the structured `location` field (so Outlook and phones show a Join button) **and** into the body as a short block
  (`Join Zoom Meeting`, URL, meeting id, passcode, dial-in lines when the API gave them) above the user's notes. The event is stored with `online_provider` / `online_url` so
  `J` works later.
- Atomicity: for External kinds the link is made first. If the event create then fails we try to delete the vendor meeting (best effort, logged). If the vendor call fails the editor asks
  "Save without a link / Retry / Cancel"; it never silently saves a meeting that was meant to have a link and does not.
- Editing a time or deleting an event updates or deletes the vendor meeting as well (phase 3), else Zoom ends up with orphans and stale times.
- Recurring events: a Zoom/Webex vendor meeting is created as a recurring meeting with the same pattern (the API supports it), or, for Static / Generated, one URL serves the series.

### 7.4 Joining (reading side, shippable alone)

`J` on any selected event opens its join link. Detection order: structured `onlineMeeting.joinUrl` (Graph), `conferenceData` (Google), then a regex over location and body for
known hosts (`zoom.us/j/`, `*.zoom.us/my/`, `teams.microsoft.com/l/meetup-join`, `teams.live.com/meet`, `meet.google.com/`, `*.webex.com/`, `meet.jit.si/`), then the first `https://` URL in
location. Only `https`, shown before opening. No new permissions needed.

### 7.5 Meeting accounts in Settings -> Accounts (decision 3)

A second section on the **same** Accounts screen, below the mail accounts, styled and behaving the same:

```
 ADD MEETING ACCOUNT
 [ Zoom                  ▾ ]  [ + Add ]
 --------------------------------------------------------------
 MEETING ACCOUNTS
   Microsoft Teams   included with work@contoso.com                (read-only row)
   Google Meet       included with you@gmail.com                   (read-only row)
 ZOOM ACCOUNT
   work@acme.com     signed in     [Test] [Log out] [Remove]
 PERSONAL MEETING LINK
   "My Zoom room"    https://zoom.us/j/...     [Edit] [Remove]
```

- **Dropdown -> Add -> that provider's sign-in**, exactly the mail flow. Provider list: Zoom, Webex, Jitsi, Personal meeting link (more later). Teams and Meet are not addable here
  because they come with the Microsoft / Google account; they appear as read-only rows so the user sees the whole picture.
- **Authentication per provider:** Zoom: browser sign-in with a loopback redirect (same machinery as Google sign-in in `google_auth.rs`); Webex: browser sign-in or device-code
  (to be checked in the spike); Jitsi: no sign-in, asks only for an optional server URL (default `meet.jit.si`); Personal link: asks for a label and a URL, no sign-in.
  Whether Zoom's OAuth can run as a public PKCE client or needs a client secret, and who registers the OAuth app, is unverified (spike).
- **Zoom and Webex start as stubs** (decision 11). Both are registered as providers and appear in the Add dropdown and in the Online picker, but `available()` returns
  `needs_setup("Zoom sign-in is not built yet")`; choosing Add shows that sentence, exactly as the mail page does today for providers whose sign-in is not supported yet
  (`AccountPage.addAccount()`). No OAuth code, no network calls, no keyring entries. The Zoom **personal meeting link** (Static) and Jitsi work from the first release, so the
  user can already put a Zoom link on an event. When the user registers the Zoom OAuth app, only the stub's `login` / `create` / `delete` bodies are filled in (phase B3).
- **Same buttons as mail accounts:** Log in / Log out keep the account and its data; Remove is two-step and deletes the token; plus **Test** (creates and deletes a throw-away meeting, or just
  validates the token for Static / Generated).
- **Storage:** `meeting_accounts` table (`id` = `<provider>-<suffix>`, provider, label, email, config_json); OAuth tokens in the keyring under service name `omalook` (never rename it), one
  entry per meeting account. Routes: `GET /meeting-accounts`, `POST /meeting-accounts/login?provider=`, `/logout`, `/remove`, `/test`.
- **Default per calendar account:** Settings -> Calendar picks the default online provider for each calendar account from the list above (and "None").
  No meeting account is tied to a calendar account otherwise: a work Zoom can be used on a personal calendar entry if the user chooses it.
- **Code shape:** the section is its own component (`MeetingAccountsSection.qml`) embedded in `AccountPage.qml`, so the existing page does not grow another 200 lines.

## 8. Second provider: Google Calendar (design check, built after Graph)

Everything above maps without changing the editor: `events.insert` with `attendees[{email, optional}]`, `sendUpdates=all|none` (none for personal entries), `conferenceData` for Meet,
`recurrence: ["RRULE:..."]`, `POST /freeBusy` for the finder (scope `.../auth/calendar` already granted). Differences live in the serialiser: no resource row, ids carry the account prefix, a
`requestId` replaces `transactionId`, reminders are an overrides list. CalDAV would be VEVENT `PUT` with `ATTENDEE;ROLE=REQ-PARTICIPANT|OPT-PARTICIPANT` and a `CALDAV:free-busy-query` REPORT,
no native meetings; the capability flags make "no native meeting provider, no directory" an ordinary state.

## 9. Settings (Settings -> Calendar, plus Accounts)

All adjustable on the Calendar screen. Today most of that page is placeholder; these get wired to the backend (`GET|POST /settings/calendar` grows).

```toml
[calendar]
default_minutes          = 30          # new entry length
working_hours            = "09:00-17:30"   # fallback when the provider does not report them
work_days                = ["mon","tue","wed","thu","fri"]
default_reminder_minutes = 15
default_account          = "last_used" # default; or pin an account id
default_series_months    = 12          # a new recurring series ends this many months after its first date; 0 = never
lunch                    = "12:00-13:00"   # avoided by free-time ranking, never blocked

# How far recurring entries are expanded into the calendar (replaces recurrence_years_back/ahead;
# the old keys are still read and converted, so nothing changes until the user edits it).
recurrence_months_back   = 60          # = 5 years, today's default
recurrence_months_ahead  = 120         # = 10 years, today's default

[calendar.meetings]
auto_add_when_attendees  = false       # add the default online meeting automatically when attendees are added
[calendar.meetings.accounts."<calendar account id>"]
default_provider         = "teams"     # none | teams | meet | <meeting account id>
```

- **Recurrence window control:** one number field plus a **Months | Years** toggle for "back" and for "ahead" (stored as months; the UI shows whole years when it divides evenly). Limits stay
  what they are now (back 0..20 years, ahead 1..30 years). A hint under it shows the resulting dates ("through 25 Oct 2036") and the cost note for daily series.
- **Undo window:** no new setting; every calendar write uses `[mail] send_delay_secs` (Settings -> Mail), and the help text there is extended to say it covers calendar changes too.
- **Series default end:** a number field with a Months label and a "Never" switch under Settings -> Calendar, default 12; the editor shows the resulting date.
- **Defaults in the editor:** length, reminder, working hours and default provider are read when the editor opens; values the user already changed are never overwritten
  by a later account switch (5.1).
- **Working hours source:** the person's own (Graph reports them per mailbox through `getSchedule` / mailbox settings), the setting only when none is reported.

## 10. HTTP routes (port 27182; see security note)

`GET /calendar/accounts` (capabilities, calendars, meeting providers per account) · `GET /calendar/event?id=` · `POST /calendar/event` (create, queues) · `PATCH /calendar/event?id=&scope=` ·
`POST /calendar/event/cancel?id=&scope=` · `POST /calendar/event/undo?id=` · `GET /calendar/outbox` · `POST /calendar/freebusy` · `GET /people/suggest?q=&account=` ·
`GET|POST /meeting-accounts[...]` (7.5) · `POST /calendar/meeting/create` (External kinds, also used by tests). `POST /calendar/event?dry_run=1` returns the exact provider JSON without sending,
for tests and debugging.

## 11. Phases

Each phase ends in something usable and verified. Phases 0-3 are the spine, in order. After phase 3 the tracks **A, B and C are independent**: build whichever the user wants first
(decision 8). Dependencies are stated so nothing is started too early.

| Phase | Deliverable | Needs | Verified by |
|---|---|---|---|
| **0 Spike** (no UI) | Real answers to the unverified items: on a work account, Teams on single and recurring events, `allowedOnlineMeetingProviders`, `getSchedule` / `findMeetingTimes` / `/me/people` / `/users`, `transactionId` behaviour, recurrence strictness, series exception and cancel behaviour, consent for the three new scopes (and the tenant's consent policy); Zoom OAuth style; Webex device flow; Meet `conferenceData`. Findings appended to this file. | nothing | Recorded responses; nothing guessed |
| **1 Read side, cursor, accounts** | Extended `CalendarEvent` + columns + Graph `$select`; calendar cursor and selected event (5.2); account filter `A` and dots; detail pane (location, attendees, response, recurrence, join link); `J` join and `y` copy; join-link detection. Delivers the join half of OL-010 on its own. | 0 | Unit tests for parsers and link regexes; scratch instance (port 27999, own config dir) with real events from two accounts |
| **2 Personal events + recurrence (Graph)** | Editor overlay, account row and switch, `when.js` and `recurrence.js` parsers, conflict line, `cal.new`, create with `transactionId`, `calendar_outbox` with the undo window and pending marks (personal entries included), read-only-tier prompt, Settings -> Calendar defaults (length, reminder, working hours, default account, recurrence months/years). | 1 | Pure builder tests (incl. a table of recurrence patterns), node tests for both parsers, live personal events (single and weekly) on each account |
| **3 Attendees, edit, cancel** | Required/Optional rows, suggestions (contacts, ranked people, directory), invitations, `e` edit, `x` delete / cancel / decline (all through the undo window from phase 2), `PATCH`, occurrence-vs-series prompt. | 2 | Builder tests; live test using a second address of the user's own, **never real colleagues** |
| **A Free-time finder** | `getSchedule`, slot algorithm and ranking in Rust, finder panel and keys, other-accounts row, own-calendar-only fallback; optional `findMeetingTimes`. | 3 (attendees) | Algorithm unit tests (merge, working hours, unknown people, DST day, other accounts); live against own accounts |
| **B Online meetings** | Meeting provider trait; B1 Teams via Graph; B2 meeting-accounts section in Settings -> Accounts with Personal link and Jitsi; picker and default-per-account setting; B2 also registers Zoom and Webex as stubs (7.5); B3 Zoom API (when the user has registered the OAuth app); B4 Webex. B1 and B2 can ship before attendees exist (a Teams meeting without attendees is fine). | 2 (B1, B2); B3, B4 after B2 | Request-body tests; live Teams event; `J` round trip; stub providers report "not set up"; (B3) Zoom create + delete + orphan cleanup |
| **C Google provider** | Same traits for Google: create (with recurrence), freeBusy, Meet. CalDAV decision. | 2 and 3 | Same builder tests; one live Google event |
| **D Extras** | "This and following" series edits, rooms/resources, per-event time zone, create-from-mail (`Ctrl-E`), propose new time, MCP tool exposure with the existing ask/approve model, personal Microsoft accounts (create/edit only). | per item | Per feature |

## 12. Risks

- **Mailing people by mistake.** Mitigation: the footer states how many people will be invited, the undo window, `dry_run`, tests against the user's own addresses only, and no auto-adding of attendees from context.
- **Scope consent.** The three new scopes mean a re-consent prompt and, in some tenants, a block or admin approval. The app must keep working (create events, finder grid) on the lower tiers.
- **Recurrence size.** The 12-month default end keeps a daily series near 365 rows. A series the user makes open-ended is expanded through the window (10 years ahead = about 3,650 rows for a daily one, as the read-only calendar already does for received series); the settings hint says so and the window is adjustable.
- **Undo on everything.** Pending marks and restore-on-undo need the calendar view to hold two states for an event (shown and confirmed) until the worker finishes; a crash while pending must resolve to the confirmed state, never silently apply the change.
- **Time zones.** The cache is local wall-clock. Editing events created in another zone, or events whose zone differs from the machine's, can show the wrong hour. Until per-event zones exist (track D)
  the detail pane shows the event's zone whenever it differs.
- **Account switch mid-edit** can invalidate the meeting provider, calendar and free/busy data. Mitigated by explicit re-evaluation with visible notes (5.1).
- **Zoom drift.** Vendor APIs change and tokens expire; the Static path keeps working with no credentials, so it is the safe default.
- **Behaviour change.** `j/k` in the day and week grids stop meaning "next period" (the `[ ]` keys still do).
- **Unauthenticated local port** (OL-006) now guards operations that email other people.

## 13. Open questions

None. Every question from the earlier drafts is answered (section 0). What remains is the user's choice of next phases (decision 15): phase 0 (spike) and phase 1 (read side, cursor, accounts) are the proposed start, and tracks A, B and C stay independent after phase 3.

## 14. Related work found while planning

- **OL-022: new mail takes the first signed-in account.** `AppState.openCompose()` sets `accountId` to `accounts.filter(signed_in && email)[0]` for a new message, ignoring the folder or
  account being viewed and the account last used (replies use the message's account, which is right). The From dropdown in `ComposeHeader.qml` is the workaround today. Proposed fix, shared
  with the calendar: viewed account if the mail view is filtered to one, else last used (`last_account.mail` in `view_state.json`), else the first. Not part of OL-021; tracked separately.

## 15. Phase 0 findings (spike run 2026-10-25, work/school tenant, adam@macielinski.net)

Tool: `tools/spike/graph_spike.py` (`login`, `probe`, `clean`). Raw responses are in `notes/spike/probe-*.json` (gitignored, personal data). Every test event was
self-only (no attendees, nobody notified), titled "[omalook spike]", and deleted at the end (11 of 11 removed, 0 left over, confirmed by a second query).

| Item | Result |
|---|---|
| Account type | `/organization` returns a tenant: work/school. Default calendar `canEdit=true`. |
| Consent for the 3 new scopes | One device-code sign-in with the extended set (People.Read, User.ReadBasic.All, Calendars.Read.Shared) **succeeded with no admin block**; all three are in the granted scope. Only this tenant is tested; other tenants may differ, so the lower-tier fallbacks stay. |
| `allowedOnlineMeetingProviders` | `["teamsForBusiness"]`, default `teamsForBusiness`. |
| Teams, single event | `isOnlineMeeting:true, onlineMeetingProvider:"teamsForBusiness"` returns `onlineMeeting.joinUrl`. |
| Teams, recurring series | Also 201 with a `joinUrl`. |
| `/me/people`, `/users`, `/users?$filter=startswith(...)` (with `ConsistencyLevel: eventual`) | All 200, rows returned. |
| `getSchedule` (self) | 200, `availabilityView` digits as documented. |
| `findMeetingTimes` | 200 with suggestions. **`timeConstraint.timeslots` items are `{start, end}`**, not `{startTime, endTime}` (the latter gives a misleading "Invalid TimeZone" 400). |
| `transactionId` | Same id + same body: 201 and the **same event id** returned (no duplicate). Same id + different body: **400 `ErrorDuplicateTransactionId`** with `@event.existingEventId`. The retry path must treat that 400 as "already created" rather than a user error. |
| Time zone names | Both Windows names ("Eastern Standard Time") and IANA ("America/New_York") accepted. |
| Recurrence strictness | Graph is **lenient, not strict**: a weekly pattern whose start weekday is not listed is accepted and the first occurrence moves to the next listed day; a `range.startDate` different from the event date is accepted and the series starts on the range date (event start was shifted). So the builder should still set these correctly (plan 5.7) so what the user sees equals what is stored, but a mismatch does not fail. |
| Patterns accepted | weekly, weekly Mon-Fri, relativeMonthly with `index: second` and a numbered range. |
| Series edits | `GET .../instances` lists occurrences. PATCH on an occurrence id makes `type: exception`; PATCH on the master id renames the others and the exception **keeps its own subject**. DELETE on an occurrence removes only that one (5 to 4). |
| Cancel without attendees | `POST /cancel` returned 202 on a series with no attendees. Plan: use plain `DELETE` for personal entries (already stated in section 4); not tested what 202 does to the series. |

Not tested here: the effect of changing a series' time when attendees exist (needs a second address of the user's own), Zoom OAuth style, Webex device flow, Google Meet `conferenceData`.
Those are open; the vendor items do not need a sign-in and the attendee case needs the user's say-so before it sends any mail.
