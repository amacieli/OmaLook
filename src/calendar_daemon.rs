//! Calendar synchronization daemon (mirrors email_daemon.rs)
//!
//! - First authenticated run: full paginated sync of the entire calendar
//!   history via a channel, writing each ~50-event page as it arrives
//! - Subsequent polls (default 2 minutes): 50 most recent events
//! - Recurring series: `/me/events` returns a series once (as its master), so occurrences are
//!   pulled separately through Graph `calendarView`, which expands them. A rolling window is
//!   kept: the whole window (Settings → Calendar: default 5 years back, 10 ahead) on the first
//!   run, every 6 hours, and whenever the setting changes (occurrences outside a narrowed
//!   window are dropped),
//!   the near window (last 7 days, next 90 days) every 15 minutes. Each 90-day chunk replaces
//!   the occurrences stored for it, so cancelled or moved occurrences disappear.
//! - Events are upserted into SQLite so edits and reschedules propagate
//! - Fully independent of the mail daemon: own provider, own DB handle, own loop

use crate::perf;
use chrono::{Duration as Days, Months, NaiveDate};
use std::path::PathBuf;
use crate::db::Database;
use crate::errors::Result;
use crate::providers::CalendarProvider;
use log::{debug, error, info, warn};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct CalendarDaemonConfig {
    pub poll_interval_secs: u64,
    /// How often the near occurrence window is refreshed
    pub near_window_interval_secs: u64,
    /// How often the whole occurrence window is refreshed
    pub full_window_interval_secs: u64,
    /// settings.toml holding [calendar] recurrence_years_back / _ahead. It is re-read on every
    /// cycle, so changing the window in Settings takes effect without a restart.
    /// None = built-in defaults (5 years back, 10 ahead).
    pub settings_path: Option<PathBuf>,
}

impl Default for CalendarDaemonConfig {
    fn default() -> Self {
        Self { poll_interval_secs: 120, near_window_interval_secs: 900, full_window_interval_secs: 6 * 3600, settings_path: None }
    }
}

/// Near-term occurrence window, in days relative to today (the full window comes from Settings).
const NEAR_BACK_DAYS: i64 = 7;
const NEAR_AHEAD_DAYS: i64 = 90;
/// Size of one calendarView request/replace unit.
const CHUNK_DAYS: i64 = 90;

/// `[today - years_back years, today + years_ahead years)`.
pub fn window_bounds(today: NaiveDate, years_back: i32, years_ahead: i32) -> (NaiveDate, NaiveDate) {
    let from = today.checked_sub_months(Months::new(12 * years_back.max(0) as u32)).unwrap_or(today);
    let to = today.checked_add_months(Months::new(12 * years_ahead.max(0) as u32)).unwrap_or(today);
    (from, to)
}

pub struct CalendarDaemon {
    config: CalendarDaemonConfig,
    db: Arc<Database>,
    provider: Arc<dyn CalendarProvider>,
}

impl CalendarDaemon {
    pub fn new(config: CalendarDaemonConfig, db: Arc<Database>, provider: Arc<dyn CalendarProvider>) -> Self {
        Self { config, db, provider }
    }

    pub async fn start(&self) {
        info!("Calendar daemon started (events every {}s)", self.config.poll_interval_secs);
        let interval = Duration::from_secs(self.config.poll_interval_secs);
        let mut last_sync: Option<Instant> = None;
        let mut last_near: Option<Instant> = None;
        let mut last_full: Option<Instant> = None;
        let mut applied_years: Option<(i32, i32)> = None;
        let mut authenticated_once = false;

        // Mail's priority stages first; calendar starts a few seconds after contacts.
        let acct = self.db.account_id().to_string();
        crate::sync_state::wait_gate(&acct, Duration::from_secs(60)).await;
        tokio::time::sleep(Duration::from_secs(5)).await;

        loop {
            match self.provider.is_token_valid().await {
                Ok(true) => {
                    let is_first_run = !authenticated_once;
                    authenticated_once = true;

                    let should_sync = is_first_run
                        || last_sync.map(|t| t.elapsed() >= interval).unwrap_or(true);

                    // Recurring occurrences (independent of the event poll above)
                    let today = chrono::Local::now().date_naive();
                    let years = self.window_years();
                    let full_due = applied_years != Some(years)
                        || last_full
                            .map(|t| t.elapsed() >= Duration::from_secs(self.config.full_window_interval_secs))
                            .unwrap_or(true);
                    let near_due = last_near
                        .map(|t| t.elapsed() >= Duration::from_secs(self.config.near_window_interval_secs))
                        .unwrap_or(true);
                    if full_due || near_due {
                        let _t = perf::span(format!("calendar[{}] occurrences {}", self.db.account_id(), if full_due { "FULL window" } else { "near window" }));
                        let result = if full_due {
                            self.sync_full_window(today, years.0, years.1).await
                        } else {
                            let (from, to) = (today - Days::days(NEAR_BACK_DAYS), today + Days::days(NEAR_AHEAD_DAYS));
                            self.sync_occurrences(from, to).await
                        };
                        match result {
                            Ok(n) => {
                                info!("Synced {} calendar occurrences ({})", n, if full_due { "full window" } else { "near window" });
                                last_near = Some(Instant::now());
                                if full_due {
                                    last_full = Some(Instant::now());
                                    applied_years = Some(years);
                                }
                            }
                            Err(e) => error!("Failed to sync recurring occurrences: {}", e),
                        }
                    }

                    if should_sync {
                        let _t = perf::span(format!("calendar[{}] sync_events(first_run={})", self.db.account_id(), is_first_run));
                        match self.sync_events(is_first_run).await {
                            Ok(count) => {
                                if count > 0 {
                                    info!("Successfully synced {} calendar events", count);
                                } else {
                                    debug!("No new calendar events");
                                }
                                last_sync = Some(Instant::now());
                            }
                            Err(e) => error!("Failed to sync calendar: {}", e),
                        }
                    }
                    tokio::time::sleep(interval).await;
                }
                Ok(false) => {
                    warn!("Calendar daemon: auth token not valid, waiting for authentication...");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                Err(e) => {
                    error!("Calendar daemon: failed to check token validity: {}", e);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }

    /// (years back, years ahead) currently configured; re-read from settings.toml each call.
    fn window_years(&self) -> (i32, i32) {
        let cs = match &self.config.settings_path {
            Some(p) => crate::settings::read_calendar_settings(p),
            None => crate::models::CalendarSettings::default(),
        };
        (cs.recurrence_years_back, cs.recurrence_years_ahead)
    }

    /// Re-expand the near window now (used right after accepting or tentatively accepting an
    /// invitation, so the event shows in the calendar without waiting for the next poll).
    pub async fn refresh_near_window(&self) -> Result<usize> {
        let today = chrono::Local::now().date_naive();
        self.sync_occurrences(today - Days::days(NEAR_BACK_DAYS), today + Days::days(NEAR_AHEAD_DAYS)).await
    }

    /// Re-expand the whole configured window now (right after a new recurring series was created).
    pub async fn refresh_full_window(&self) -> Result<usize> {
        let today = chrono::Local::now().date_naive();
        let years = self.window_years();
        self.sync_full_window(today, years.0, years.1).await
    }

    /// Re-expand the few days around `date` (an event answered far outside the near window
    /// would otherwise stay stale until the next full-window sync).
    pub async fn refresh_around(&self, date: NaiveDate) -> Result<usize> {
        self.sync_occurrences(date - Days::days(1), date + Days::days(2)).await
    }

    /// Expand the whole configured window, then drop stored occurrences outside it (they exist
    /// when the window was narrowed). Nothing is purged unless the whole window was fetched.
    pub(crate) async fn sync_full_window(&self, today: NaiveDate, years_back: i32, years_ahead: i32) -> Result<usize> {
        let (from, to) = window_bounds(today, years_back, years_ahead);
        let n = self.sync_occurrences(from, to).await?;
        self.db.purge_occurrences_outside(&format!("{}T00:00:00", from), &format!("{}T00:00:00", to))?;
        Ok(n)
    }

    /// Expand recurring series over `[from, to)`, one 90-day chunk at a time. A chunk is only
    /// replaced after it was fetched completely, so a failed request never deletes anything.
    pub(crate) async fn sync_occurrences(&self, from: NaiveDate, to: NaiveDate) -> Result<usize> {
        let mut total = 0;
        let mut start = from;
        while start < to {
            let end = std::cmp::min(start + Days::days(CHUNK_DAYS), to);
            let events = self.provider.fetch_window(start, end).await?;
            total += self.db.replace_occurrences(
                &format!("{}T00:00:00", start),
                &format!("{}T00:00:00", end),
                &events,
            )?;
            start = end;
            tokio::task::yield_now().await;
        }
        Ok(total)
    }

    /// is_initial=true: paginate the full history; false: 50 most recent only.
    async fn sync_events(&self, is_initial: bool) -> Result<usize> {
        if is_initial {
            debug!("Full paginated calendar sync");
            let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<crate::models::CalendarEvent>>(4);
            let provider = Arc::clone(&self.provider);
            let fetch_handle = tokio::spawn(async move { provider.fetch_all_events(tx).await });

            let mut written = 0usize;
            while let Some(batch) = rx.recv().await {
                for ev in &batch {
                    match self.db.upsert_event(ev) {
                        Ok(()) => written += 1,
                        Err(e) => error!("Failed to store event {}: {}", ev.id, e),
                    }
                }
                tokio::task::yield_now().await;
            }

            if let Ok(Err(e)) = fetch_handle.await {
                error!("Paginated calendar fetch error: {}", e);
            }
            Ok(written)
        } else {
            debug!("Incremental calendar sync");
            let events = self.provider.fetch_events(50).await?;
            let mut written = 0;
            for ev in &events {
                self.db.upsert_event(ev)?;
                written += 1;
            }
            Ok(written)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::CalendarEvent;
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// Serves a weekly series (09:00 Mondays) for any window, optionally failing.
    struct FakeProvider {
        windows: Mutex<Vec<(NaiveDate, NaiveDate)>>,
        cancel_week_of: Mutex<Option<NaiveDate>>,
        fail: Mutex<bool>,
    }
    #[async_trait]
    impl CalendarProvider for FakeProvider {
        async fn fetch_events(&self, _limit: usize) -> Result<Vec<CalendarEvent>> { Ok(vec![]) }
        async fn fetch_all_events(&self, _tx: tokio::sync::mpsc::Sender<Vec<CalendarEvent>>) -> Result<usize> { Ok(0) }
        async fn fetch_window(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<CalendarEvent>> {
            if *self.fail.lock().unwrap() {
                return Err(crate::errors::OmarchyError::HttpError("boom".into()));
            }
            self.windows.lock().unwrap().push((from, to));
            let cancelled = *self.cancel_week_of.lock().unwrap();
            let mut out = Vec::new();
            let mut d = from;
            while d < to {
                if d.format("%a").to_string() == "Mon" && Some(d) != cancelled {
                    out.push(CalendarEvent {
                        id: format!("occ-{}", d), subject: "Weekly".into(),
                        start: format!("{}T09:00:00", d), end: format!("{}T09:30:00", d),
                        event_type: "occurrence".into(), series_master_id: Some("M".into()), ..Default::default()
                    });
                }
                d += Days::days(1);
            }
            Ok(out)
        }
        async fn is_token_valid(&self) -> Result<bool> { Ok(true) }
    }

    fn daemon() -> (CalendarDaemon, Arc<FakeProvider>) {
        let p = Arc::new(FakeProvider { windows: Mutex::new(vec![]), cancel_week_of: Mutex::new(None), fail: Mutex::new(false) });
        let db = Arc::new(Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap());
        (CalendarDaemon::new(CalendarDaemonConfig::default(), db, p.clone()), p)
    }
    fn d(y: i32, m: u32, day: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, day).unwrap() }

    #[tokio::test(flavor = "current_thread")]
    async fn every_occurrence_of_a_recurring_meeting_is_stored_across_chunks() {
        let (dm, p) = daemon();
        // 2026-09-07 is a Monday; 200 days spans three 90-day chunks
        let n = dm.sync_occurrences(d(2026, 9, 7), d(2027, 3, 25)).await.unwrap();
        let windows = p.windows.lock().unwrap().clone();
        assert_eq!(windows.len(), 3, "{:?}", windows);
        assert_eq!(windows[0], (d(2026, 9, 7), d(2026, 12, 6)));
        assert_eq!(windows[2].1, d(2027, 3, 25), "last chunk is clipped to the window end");
        assert_eq!(n, 29, "one per Monday in the window");
        let oct = dm.db.get_events_for_month("2026-10").unwrap();
        assert_eq!(oct.len(), 4, "four Mondays in October 2026");
        assert!(oct.iter().all(|e| e.event_type == "occurrence"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_cancelled_occurrence_disappears_on_the_next_refresh() {
        let (dm, p) = daemon();
        dm.sync_occurrences(d(2026, 10, 1), d(2026, 11, 1)).await.unwrap();
        assert_eq!(dm.db.get_events_for_month("2026-10").unwrap().len(), 4);
        *p.cancel_week_of.lock().unwrap() = Some(d(2026, 10, 12));
        dm.sync_occurrences(d(2026, 10, 1), d(2026, 11, 1)).await.unwrap();
        let left: Vec<_> = dm.db.get_events_for_month("2026-10").unwrap().into_iter().map(|e| e.start[..10].to_string()).collect();
        assert_eq!(left, vec!["2026-10-05", "2026-10-19", "2026-10-26"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_failed_fetch_deletes_nothing() {
        let (dm, p) = daemon();
        dm.sync_occurrences(d(2026, 10, 1), d(2026, 11, 1)).await.unwrap();
        *p.fail.lock().unwrap() = true;
        assert!(dm.sync_occurrences(d(2026, 10, 1), d(2026, 11, 1)).await.is_err());
        assert_eq!(dm.db.get_events_for_month("2026-10").unwrap().len(), 4, "existing occurrences must survive a failed refresh");
    }

    #[test]
    fn window_bounds_are_whole_years_either_side_of_today() {
        let (from, to) = window_bounds(d(2026, 10, 3), 5, 10);
        assert_eq!((from, to), (d(2021, 10, 3), d(2036, 10, 3)));
        assert_eq!(window_bounds(d(2024, 2, 29), 1, 1), (d(2023, 2, 28), d(2025, 2, 28)), "leap day clamps");
        assert_eq!(window_bounds(d(2026, 10, 3), 0, 1).0, d(2026, 10, 3));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn narrowing_the_window_drops_occurrences_outside_it_and_widening_adds_them() {
        let (dm, _p) = daemon();
        let today = d(2026, 10, 3);
        dm.sync_full_window(today, 3, 3).await.unwrap();
        let wide = dm.db.get_events_for_month("2028-10").unwrap().len();
        assert!(wide > 0);

        dm.sync_full_window(today, 1, 1).await.unwrap();           // narrowed: 2028 is now outside
        assert_eq!(dm.db.get_events_for_month("2028-10").unwrap().len(), 0, "outside the new window");
        assert!(dm.db.get_events_for_month("2027-03").unwrap().len() > 0, "inside the new window");

        dm.sync_full_window(today, 3, 3).await.unwrap();           // widened again
        assert_eq!(dm.db.get_events_for_month("2028-10").unwrap().len(), wide);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_failed_full_window_fetch_does_not_purge_anything() {
        let (dm, p) = daemon();
        let today = d(2026, 10, 3);
        dm.sync_full_window(today, 3, 3).await.unwrap();
        *p.fail.lock().unwrap() = true;
        assert!(dm.sync_full_window(today, 1, 1).await.is_err());
        assert!(dm.db.get_events_for_month("2028-10").unwrap().len() > 0, "narrowing must wait for a successful fetch");
    }

    #[test]
    fn the_daemon_reads_the_window_from_settings_each_time() {
        let path = std::env::temp_dir().join(format!("omalook-daemon-settings-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let (mut dm, _p) = (daemon().0, ());
        dm.config.settings_path = Some(path.clone());
        assert_eq!(dm.window_years(), (5, 10), "defaults when no file");
        crate::settings::write_calendar_settings(&path, &crate::models::CalendarSettings { recurrence_years_back: 2, recurrence_years_ahead: 4, ..Default::default() }).unwrap();
        assert_eq!(dm.window_years(), (2, 4), "changes apply without restarting the daemon");
        let _ = std::fs::remove_file(&path);
    }
}
