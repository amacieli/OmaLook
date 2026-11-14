//! Calendar cache: events, occurrences, etags and month queries.

use super::*;

impl Database {

    // ------------------------------------------------------------ calendar

    /// Insert or update a calendar event (updates so reschedules/edits propagate)
    pub fn upsert_event(&self, ev: &CalendarEvent) -> Result<()> {
        self.conn.execute(
            "INSERT INTO calendar_events (id, subject, body, start_at, end_at, is_all_day, time_zone, cached_at, account_id,
                                          event_type, series_master_id,
                                          location, organizer_name, organizer_email, is_organizer, attendees_json, my_response,
                                          show_as, online_provider, online_url, reminder_minutes, web_link, sensitivity,
                                          recurrence_json, calendar_id, tags_json, etag)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, CURRENT_TIMESTAMP, ?8, ?9, ?10,
                     ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)
             ON CONFLICT(id) DO UPDATE SET
               subject    = excluded.subject,
               body       = excluded.body,
               start_at   = excluded.start_at,
               end_at     = excluded.end_at,
               is_all_day = excluded.is_all_day,
               time_zone  = excluded.time_zone,
               event_type = excluded.event_type,
               series_master_id = excluded.series_master_id,
               location = excluded.location, organizer_name = excluded.organizer_name,
               organizer_email = excluded.organizer_email, is_organizer = excluded.is_organizer,
               attendees_json = excluded.attendees_json, my_response = excluded.my_response,
               show_as = excluded.show_as, online_provider = excluded.online_provider,
               online_url = excluded.online_url, reminder_minutes = excluded.reminder_minutes,
               web_link = excluded.web_link, sensitivity = excluded.sensitivity,
               recurrence_json = excluded.recurrence_json, calendar_id = excluded.calendar_id,
               tags_json = excluded.tags_json,
               -- an answer without a version keeps the known one: a stale etag can only cause a
               -- refused write (412), never an overwrite
               etag = CASE WHEN excluded.etag <> '' THEN excluded.etag ELSE calendar_events.etag END,
               cached_at  = CURRENT_TIMESTAMP",
            params![
                ev.id, ev.subject, ev.body, ev.start, ev.end, ev.is_all_day, ev.time_zone, self.account_id,
                if ev.event_type.is_empty() { "singleInstance" } else { ev.event_type.as_str() },
                ev.series_master_id,
                ev.location, ev.organizer_name, ev.organizer_email, ev.is_organizer, ev.attendees_json, ev.my_response,
                ev.show_as, ev.online_provider, ev.online_url, ev.reminder_minutes, ev.web_link, ev.sensitivity,
                ev.recurrence_json, ev.calendar_id,
                if ev.tags.is_empty() { String::new() } else { serde_json::to_string(&ev.tags).unwrap_or_default() },
                ev.etag,
            ],
        )?;
        Ok(())
    }


    /// Version (etag) of a cached event; "" when unknown or not cached.
    pub fn event_etag(&self, event_id: &str) -> String {
        self.conn
            .query_row("SELECT etag FROM calendar_events WHERE id = ?1 AND account_id = ?2", params![event_id, self.account_id], |r| r.get::<_, String>(0))
            .unwrap_or_default()
    }


    /// Remember the version a successful write of ours returned, so our next change to the same event
    /// is not refused as a conflict before the next sync.
    pub fn set_event_etag(&self, event_id: &str, etag: &str) -> Result<()> {
        if etag.is_empty() { return Ok(()) }
        self.conn.execute("UPDATE calendar_events SET etag = ?3 WHERE id = ?1 AND account_id = ?2", params![event_id, self.account_id, etag])?;
        Ok(())
    }


    /// Store the result of expanding a date window ([from, to), "YYYY-MM-DDT00:00:00" bounds).
    /// Recurring occurrences/exceptions previously stored inside the window are replaced, so an
    /// occurrence that was cancelled or moved at the server disappears locally. Single events
    /// returned alongside are upserted as usual. Call only with a COMPLETE fetch of the window.
    pub fn replace_occurrences(&self, from: &str, to: &str, events: &[CalendarEvent]) -> Result<usize> {
        self.in_transaction(|| {
            self.conn.execute(
                "DELETE FROM calendar_events
                 WHERE account_id = ?1 AND event_type IN ('occurrence', 'exception')
                   AND start_at >= ?2 AND start_at < ?3",
                params![self.account_id, from, to],
            )?;
            for ev in events {
                self.upsert_event(ev)?;
            }
            Ok(events.len())
        })
    }


    /// Drop recurring occurrences outside `[from, to)` (used when the occurrence window is narrowed).
    pub fn purge_occurrences_outside(&self, from: &str, to: &str) -> Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM calendar_events
             WHERE account_id = ?1 AND event_type IN ('occurrence', 'exception')
               AND (start_at < ?2 OR start_at >= ?3)",
            params![self.account_id, from, to],
        )?;
        Ok(n)
    }


    /// Events overlapping the given month ("YYYY-MM"), ordered by start
    pub fn get_events_for_month(&self, month: &str) -> Result<Vec<CalendarEvent>> {
        Ok(self.get_events_for_month_by_account(month, None)?.into_iter().map(|(e, _, _)| e).collect())
    }


    /// Month events of every account (or just `account`: its id or email address), each with
    /// the id and email address of the account it belongs to.
    pub fn get_events_for_month_by_account(&self, month: &str, account: Option<&str>) -> Result<Vec<(CalendarEvent, String, String)>> {
        let (y, m) = month.split_once('-').unwrap_or(("1970", "01"));
        let (y, m): (i32, u32) = (y.parse().unwrap_or(1970), m.parse().unwrap_or(1));
        let (ny, nm) = if m >= 12 { (y + 1, 1) } else { (y, m + 1) };
        let from = format!("{:04}-{:02}-01T00:00:00", y, m);
        let to = format!("{:04}-{:02}-01T00:00:00", ny, nm);

        let mut stmt = self.conn.prepare(
            "SELECT e.id, e.subject, e.body, e.start_at, e.end_at, e.is_all_day, COALESCE(e.time_zone, ''), e.event_type,
                    e.series_master_id, COALESCE(e.account_id, ''), COALESCE(a.email, ''),
                    e.location, e.organizer_name, e.organizer_email, e.is_organizer, e.attendees_json, e.my_response,
                    e.show_as, e.online_provider, e.online_url, e.reminder_minutes, e.web_link, e.sensitivity,
                    e.recurrence_json, e.calendar_id, e.tags_json
             FROM calendar_events e LEFT JOIN accounts a ON a.id = e.account_id
             WHERE e.start_at < ?2 AND e.end_at >= ?1 AND e.event_type != 'seriesMaster'
               AND (?3 IS NULL OR e.account_id = ?3 OR lower(a.email) = lower(?3))
             ORDER BY e.start_at ASC",
        )?;
        let events = stmt
            .query_map(params![from, to, account], |row| {
                Ok((CalendarEvent {
                    id:         row.get(0)?,
                    subject:    row.get(1)?,
                    body:       row.get(2)?,
                    start:      row.get(3)?,
                    end:        row.get(4)?,
                    is_all_day: row.get(5)?,
                    time_zone:  row.get(6)?,
                    event_type: row.get(7)?,
                    series_master_id: row.get(8)?,
                    location:         row.get(11)?,
                    organizer_name:   row.get(12)?,
                    organizer_email:  row.get(13)?,
                    is_organizer:     row.get(14)?,
                    attendees_json:   row.get(15)?,
                    my_response:      row.get(16)?,
                    show_as:          row.get(17)?,
                    online_provider:  row.get(18)?,
                    online_url:       row.get(19)?,
                    reminder_minutes: row.get(20)?,
                    web_link:         row.get(21)?,
                    sensitivity:      row.get(22)?,
                    recurrence_json:  row.get(23)?,
                    calendar_id:      row.get(24)?,
                    tags:             serde_json::from_str::<Vec<String>>(&row.get::<_, String>(25)?).unwrap_or_default(),
                    etag:             String::new(), // the version stays in the backend (used for If-Match only)
                }, row.get(9)?, row.get(10)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(events)
    }
}

/// One cached calendar event as the MCP calendar tools read it (R3.5).
pub struct McpEventRow {
    pub id: String,
    pub subject: String,
    pub body: Option<String>,
    pub start: String,
    pub end: String,
    pub is_all_day: bool,
    pub time_zone: String,
    pub account_id: String,
    pub account_email: String,
}

const MCP_EV_COLS: &str = "e.id, e.subject, e.body, e.start_at, e.end_at, e.is_all_day, COALESCE(e.time_zone,''), COALESCE(e.account_id,''), COALESCE(a.email,'')";

fn mcp_event_row(r: &rusqlite::Row) -> rusqlite::Result<McpEventRow> {
    Ok(McpEventRow {
        id: r.get(0)?, subject: r.get(1)?, body: r.get(2)?, start: r.get(3)?, end: r.get(4)?, is_all_day: r.get(5)?,
        time_zone: r.get(6)?, account_id: r.get(7)?, account_email: r.get(8)?,
    })
}

impl Database {
    /// Events overlapping `[from_ts, to_ts)` (ISO local date-times) of the given accounts, one page.
    pub fn mcp_events(&self, accounts: &[String], from_ts: &str, to_ts: &str, limit: i64, offset: i64) -> rusqlite::Result<Vec<McpEventRow>> {
        let mut a: Vec<rusqlite::types::Value> = vec![from_ts.to_string().into(), to_ts.to_string().into()];
        let acc = account_in("e.account_id", accounts, &mut a);
        a.push((limit + 1).into());
        a.push(offset.into());
        let sql = format!(
            "SELECT {} FROM calendar_events e LEFT JOIN accounts a ON a.id = e.account_id \
             WHERE e.start_at < ?2 AND e.end_at >= ?1 AND e.event_type != 'seriesMaster' AND {} ORDER BY e.start_at LIMIT ?{} OFFSET ?{}",
            MCP_EV_COLS, acc, a.len() - 1, a.len());
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(a.iter()), mcp_event_row)?;
        Ok(rows.flatten().collect())
    }

    /// One event by id, whatever its account (the caller checks the account).
    pub fn mcp_event(&self, id: &str) -> rusqlite::Result<Option<McpEventRow>> {
        let sql = format!("SELECT {} FROM calendar_events e LEFT JOIN accounts a ON a.id = e.account_id WHERE e.id = ?1", MCP_EV_COLS);
        self.conn.query_row(&sql, [id], mcp_event_row).optional()
    }

    /// `(account_id, event_type)` of a cached event, for the single-event write checks.
    pub fn mcp_event_kind(&self, id: &str) -> rusqlite::Result<Option<(String, String)>> {
        self.conn.query_row("SELECT account_id, COALESCE(event_type, '') FROM calendar_events WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).optional()
    }
}

impl Database {
    /// The account and the event type of one cached event (`""` when the type is unknown).
    pub fn cal_event_kind(&self, id: &str) -> rusqlite::Result<Option<(String, String)>> {
        match self.conn.query_row("SELECT account_id, COALESCE(event_type, '') FROM calendar_events WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))) {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
