//! Email synchronization daemon with pluggable provider support
//!
//! This module runs an async task that:
//! - Polls for new emails at a configured interval (default: 2 minutes)
//! - Syncs folder list every 10 minutes (folders change rarely)
//! - Fetches messages per-folder, storing folder_id on each message
//! - Deduplicates against SQLite before insertion
//! - Respects token validity and retries on auth failure

use crate::db::Database;
use crate::models::Settings;
use crate::providers::EmailProvider;
use crate::errors::Result;
use crate::perf;
use crate::sync_state;
use log::{debug, error, info, warn};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;

/// Marker that this account's change feeds were reset for the extended field list
/// (bump the suffix when `providers::graph::MSG_SELECT` changes again).
const FIELDS_FLAG: &str = "fields_v1";
const SPARSE_META_FLAG: &str = "sparse_meta_v1";

/// Configuration for the email daemon
pub struct DaemonConfig {
    /// Fallback poll interval; when `settings_path` is set the value in that file wins and is
    /// re-read every cycle.
    pub poll_interval_secs: u64,
    pub settings_path: Option<std::path::PathBuf>,
    pub folder_sync_interval_secs: u64,
    pub max_retries: usize,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: crate::settings::DEFAULT_POLL_SECS,
            settings_path: None,
            folder_sync_interval_secs: 600,  // 10 minutes for folders
            max_retries: 10,
        }
    }
}

impl From<&Settings> for DaemonConfig {
    fn from(settings: &Settings) -> Self {
        Self {
            poll_interval_secs: settings.sync.poll_interval_secs.max(0) as u64,
            settings_path: None,
            folder_sync_interval_secs: 600,
            max_retries: 10,
        }
    }
}

/// Email daemon that periodically syncs folders and messages
pub struct EmailDaemon {
    config: DaemonConfig,
    db: Arc<Database>,
    provider: Arc<dyn EmailProvider>,
    /// Last `read_push_serial` whose pushes (reads, actions, categories) were carried out.
    seen_push: std::sync::atomic::AtomicU64,
    /// When the UI was last told about a delta page while repopulating details (throttle).
    last_meta_bump: std::sync::Mutex<Option<Instant>>,
}

impl EmailDaemon {
    pub fn new(
        config: DaemonConfig,
        db: Arc<Database>,
        provider: Arc<dyn EmailProvider>,
    ) -> Self {
        Self {
            config, db, provider,
            seen_push: std::sync::atomic::AtomicU64::new(sync_state::read_push_serial()),
            last_meta_bump: std::sync::Mutex::new(None),
        }
    }

    /// Current mail poll interval: `[sync] poll_interval_secs` from settings.toml when a path is
    /// configured (clamped 10..=3600 s), else the configured fallback.
    fn poll_interval(&self) -> Duration {
        match &self.config.settings_path {
            Some(p) => Duration::from_secs(crate::settings::read_poll_interval(p)),
            None => Duration::from_secs(self.config.poll_interval_secs),
        }
    }

    /// Start the daemon (runs indefinitely until cancelled)
    pub async fn start(&self) {
        info!(
            "Email daemon started (messages every {}s, folders every {}s)",
            self.config.poll_interval_secs, self.config.folder_sync_interval_secs
        );

        let folder_interval = Duration::from_secs(self.config.folder_sync_interval_secs);
        let mut last_folder_sync: Option<Instant> = None;
        let mut last_message_sync: Option<Instant> = None;
        let mut message_interval = self.poll_interval();
        // Last `mail_sync_serial` this loop has acted on; a change means the user asked for mail now.
        let mut seen_mail_sync = sync_state::mail_sync_serial();

        // Poll every 5s while waiting for auth; switch to normal interval once synced
        let mut authenticated_once = false;

        let acct = self.db.account_id().to_string();
        perf::mark(&format!("mail[{}] daemon started", acct));
        loop {
            // Re-read each cycle so a change in settings.toml applies without a restart.
            message_interval = self.poll_interval();
            let manual_sync = sync_state::mail_sync_serial() != seen_mail_sync;
            seen_mail_sync = sync_state::mail_sync_serial();
            let tok_t = Instant::now();
            let token_check = self.provider.is_token_valid().await;
            if !authenticated_once {
                perf::mark(&format!("mail[{}] token check {:?} in {:.1}ms", acct, token_check.as_ref().ok(), tok_t.elapsed().as_secs_f64() * 1000.0));
            }
            match token_check {
                Ok(true) => {
                    debug!("Token valid, proceeding with email sync");

                    let is_first_run = !authenticated_once;
                    authenticated_once = true;

                    // Stage 1 (first cycle after launch): the 50 newest Inbox messages, before
                    // anything else, so the UI can show fresh mail at once.
                    if is_first_run {
                        self.sync_recent(50).await;
                    }

                    // Sync folders: on first authenticated run, or when interval elapsed
                    let should_sync_folders = last_folder_sync
                        .map(|t| t.elapsed() >= folder_interval)
                        .unwrap_or(true);

                    if should_sync_folders {
                        let _t = perf::span(format!("mail[{}] sync_folders", acct));
                        match self.sync_folders().await {
                            Ok(count) => {
                                info!("Synced {} folders", count);
                                self.sync_categories().await;
                                last_folder_sync = Some(Instant::now());
                                sync_state::bump_mail("folder counts updated");
                            }
                            Err(e) => {
                                error!("Failed to sync folders: {}", e);
                            }
                        }
                    }

                    // Sync messages: on first authenticated run, or when interval elapsed
                    let should_sync_messages = manual_sync || is_first_run || last_message_sync
                        .map(|t| t.elapsed() >= message_interval)
                        .unwrap_or(true);

                    if should_sync_messages {
                        let _t = perf::span(format!("mail[{}] sync_messages(first_run={}) TOTAL", acct, is_first_run));
                        match self.sync_messages(is_first_run).await {
                            Ok(count) => {
                                if count > 0 {
                                    info!("Successfully synced {} new emails", count);
                                } else {
                                    debug!("No new emails");
                                }
                                last_message_sync = Some(Instant::now());
                            }
                            Err(e) => {
                                error!("Failed to sync messages: {}", e);
                            }
                        }
                    }

                    // Message details for providers without a change feed, a slice per cycle.
                    self.backfill_meta_per_message(Duration::from_secs(60)).await;
                    if self.provider.supports_delta() {
                        let still = self.db.delta_walk_incomplete().unwrap_or(0) > 0;
                        sync_state::set_meta_active(&acct, still && self.db.meta_remaining().unwrap_or(0) > 0);
                    }

                    // Safety net: whatever happened above, never hold calendar/contacts back.
                    sync_state::open_gate(&acct);

                    // After a successful sync, wait out the poll interval. A read/unread click
                    // wakes this early and is pushed to the provider straight away.
                    let deadline = tokio::time::Instant::now() + message_interval;
                    loop {
                        // A Refresh (F5) request that arrived during this cycle: run the next one now.
                        if sync_state::mail_sync_serial() != seen_mail_sync { break; }
                        let wake = sync_state::read_push_requested();
                        tokio::pin!(wake);
                        // Registered before the serial check, so a click cannot slip between them.
                        let _ = wake.as_mut().enable();
                        let mail_now = sync_state::mail_sync_requested();
                        tokio::pin!(mail_now);
                        let _ = mail_now.as_mut().enable();
                        if sync_state::mail_sync_serial() != seen_mail_sync { break; }
                        let now_push = sync_state::read_push_serial();
                        if now_push != self.seen_push.load(std::sync::atomic::Ordering::SeqCst) {
                            self.service_pushes().await;
                        }
                        tokio::select! {
                            _ = tokio::time::sleep_until(deadline) => break,
                            _ = &mut wake => {}
                            _ = &mut mail_now => {}
                        }
                    }
                }
                Ok(false) => {
                    warn!("Auth token not valid, waiting for authentication...");
                    // Poll every 5s while unauthenticated so we pick up login quickly
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                Err(e) => {
                    error!("Failed to check token validity: {}", e);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }

    /// Fetch and upsert all top-level folders
    async fn sync_folders(&self) -> Result<usize> {
        debug!("Syncing folders...");
        let folders = self.provider.fetch_folders().await?;
        let count = folders.len();
        for folder in &folders {
            if let Err(e) = self.db.upsert_folder(folder) {
                error!("Failed to upsert folder {}: {}", folder.display_name, e);
            }
        }
        debug!("Folder sync complete: {} folders", count);
        Ok(count)
    }

    /// Sync messages for all known folders
    /// - First run (is_initial): full paginated sync via channel, non-blocking
    /// - Subsequent runs: fetch 50 most recent per folder for incremental updates
    async fn sync_messages(&self, is_initial: bool) -> Result<usize> {
        // Local read/unread changes go out first, so the reconcile below cannot undo them.
        self.push_pending_reads().await;
        self.push_due_actions().await;
        self.push_pending_categories().await;
        self.prepare_meta_backfill().await;

        // Providers with a change feed (Graph delta) sync by changes: one cheap call per folder
        // instead of reading flags and re-crawling every folder.
        if self.provider.supports_delta() {
            return self.sync_messages_delta().await;
        }

        let folders = self.db.get_folders()?;

        if folders.is_empty() {
            // Fall back to inbox if no folders cached yet
            return self.sync_folder_messages("inbox", is_initial).await;
        }

        // Read flags first: one cheap request per folder, so they are right within seconds
        // of launch instead of after the full paginated sync of every folder.
        {
            let _t = perf::span(format!("mail[{}] reconcile_read_state x{} folders", self.db.account_id(), folders.len()));
            for folder in &folders {
                self.reconcile_read_state(&folder.id, &folder.display_name).await;
            }
        }

        // Priority stages done (recent 50, folders, read flags): calendar and contacts may
        // start now, concurrently with the older-mail crawl below.
        sync_state::open_gate(self.db.account_id());

        let mut total = 0;
        for folder in &folders {
            let _t = perf::span(format!("mail[{}] folder '{}' sync", self.db.account_id(), folder.display_name));
            match self.sync_folder_messages(&folder.id, is_initial).await {
                Ok(count) => total += count,
                Err(e) => error!("Failed to sync folder {}: {}", folder.display_name, e),
            }
        }
        Ok(total)
    }

    /// Change-feed sync of every folder (Inbox first: folders come sorted). The first walk of a
    /// folder is a full enumeration (500 per page, newest first, resumable); after that each
    /// cycle is one request per folder returning only what changed.
    async fn sync_messages_delta(&self) -> Result<usize> {
        let acct = self.db.account_id().to_string();
        let folders = self.db.get_folders()?;
        // Recent 50 and the folder list are already done: calendar/contacts may start.
        sync_state::open_gate(&acct);

        let mut total = 0usize;
        let mut any_change = false;
        for folder in &folders {
            let _t = perf::span(format!("mail[{}] delta '{}'", acct, folder.display_name));
            match self.sync_folder_delta(&folder.id, &folder.display_name).await {
                Ok(n) => {
                    if n > 0 {
                        any_change = true;
                        total += n;
                    }
                }
                Err(e) => error!("Delta sync failed for folder {}: {}", folder.display_name, e),
            }
        }
        if any_change {
            // Unread/total counts come from the folder list: refresh them now, not in 10 minutes.
            if let Err(e) = self.sync_folders().await {
                warn!("Could not refresh folder counts after delta: {}", e);
            }
            sync_state::bump_mail("folder counts after delta changes");
        }
        Ok(total)
    }

    /// One folder's change-feed pass. Returns rows inserted/changed/deleted.
    async fn sync_folder_delta(&self, folder_id: &str, folder_name: &str) -> Result<usize> {
        let acct = self.db.account_id().to_string();
        let mut state = self.db.delta_state(folder_id)?;
        // A link that was resumed (not a fresh walk) may be rejected once; then start over.
        for attempt in 0..2 {
            let (link, complete) = match &state {
                Some((l, c)) => (Some(l.clone()), *c),
                None => (None, false),
            };
            // Only a walk that begins from scratch sees the whole folder, so only it may purge ghosts.
            let fresh_full = link.is_none();
            let started = chrono::Utc::now();
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

            let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::models::DeltaPage>(4);
            let provider = Arc::clone(&self.provider);
            let fid = folder_id.to_string();
            let walk_link = link.clone();
            let handle = tokio::spawn(async move { provider.fetch_delta(&fid, walk_link, tx).await });

            let mut changed = 0usize;
            let mut removed = 0usize;
            let mut pages = 0usize;
            while let Some(page) = rx.recv().await {
                pages += 1;
                if pages == 1 && (link.is_none() || !complete) {
                    perf::mark(&format!("mail[{}] '{}' initial enumeration: first page of {}", acct, folder_name, page.upserts.len()));
                }
                if fresh_full {
                    seen.extend(page.upserts.iter().map(|m| m.id.clone()));
                }
                match self.db.apply_delta_page(folder_id, &page, page.next_link.as_deref()) {
                    Ok((c, d)) => {
                        changed += c;
                        removed += d;
                        // Tell the UI as the newest pages land, not only at the end.
                        if (c > 0 || d > 0) && (!complete || pages == 1) {
                            // While details are being repopulated nearly every page "changes"; tell the
                            // UI at most every 30 s then, so the list is not reloaded under the user.
                            let repopulating = sync_state::meta_active();
                            let mut last = self.last_meta_bump.lock().unwrap_or_else(|p| p.into_inner());
                            if !repopulating || last.map(|t| t.elapsed() >= Duration::from_secs(30)).unwrap_or(true) {
                                *last = Some(Instant::now());
                                sync_state::bump_mail(&format!("delta page: {} changed, {} removed ({})", c, d, folder_name));
                            }
                        }
                    }
                    Err(e) => {
                        error!("Could not apply delta page for {}: {}", folder_name, e);
                        handle.abort();
                        return Err(e);
                    }
                }
                self.service_pushes().await;
                tokio::task::yield_now().await;
            }

            match handle.await {
                Ok(Ok(crate::models::DeltaEnd::Done(new_link))) => {
                    self.db.set_delta_state(folder_id, &new_link, true)?;
                    let mut purged = 0;
                    if fresh_full {
                        purged = self.db.purge_unseen(folder_id, &seen, started)?;
                        if purged > 0 {
                            sync_state::bump_mail(&format!("purged {} messages deleted on the server ({})", purged, folder_name));
                        }
                    }
                    if changed + removed + purged > 0 {
                        info!("Delta '{}': {} changed, {} removed, {} purged ({} pages)", folder_name, changed, removed, purged, pages);
                    }
                    return Ok(changed + removed + purged);
                }
                Ok(Ok(crate::models::DeltaEnd::Reset)) if attempt == 0 => {
                    warn!("Delta link for '{}' was rejected; starting a full enumeration", folder_name);
                    self.db.clear_delta_state(folder_id)?;
                    state = None;
                    continue;
                }
                Ok(Ok(crate::models::DeltaEnd::Reset)) => {
                    return Err(crate::errors::OmarchyError::HttpError("delta link rejected twice".into()))
                }
                Ok(Err(e)) => return Err(e),
                Err(e) => return Err(crate::errors::OmarchyError::HttpError(format!("delta task failed: {}", e))),
            }
        }
        Ok(0)
    }

    /// Push read/unread changes made in the UI to the provider.
    async fn push_pending_reads(&self) {
        let pending = match self.db.pending_reads() {
            Ok(p) => p,
            Err(e) => { error!("Could not list pending read changes: {}", e); return; }
        };
        for (id, is_read) in pending {
            match self.provider.set_message_read(&id, is_read).await {
                Ok(()) => {
                    if let Err(e) = self.db.clear_read_pending(&id, is_read) {
                        error!("Could not clear pending flag for {}: {}", id, e);
                    }
                }
                // Stays pending: retried on the next cycle.
                Err(e) => warn!("Could not push read={} for {}: {}", is_read, id, e),
            }
        }
    }

    /// Carry out whatever the UI queued (read flags, archive/delete/move, categories) if it
    /// asked since the last time. Also called between the pages of a long walk, so a click is
    /// not stuck behind the one-time repopulate.
    async fn service_pushes(&self) {
        let now = sync_state::read_push_serial();
        if now == self.seen_push.swap(now, std::sync::atomic::Ordering::SeqCst) { return; }
        self.push_pending_reads().await;
        self.push_due_actions().await;
        self.push_pending_categories().await;
    }

    /// One-time repopulate of the message details (sender name, To/Cc/Bcc, categories, …) for
    /// rows stored before those fields existed.
    ///  * Providers with a change feed: the stored feed links carry the old field list, so the
    ///    feeds are reset once and the next walk of each folder brings everything.
    ///  * Others: details are fetched message by message in the background, throttled.
    async fn prepare_meta_backfill(&self) {
        let acct = self.db.account_id().to_string();
        if !self.db.flag_is_set(SPARSE_META_FLAG).unwrap_or(true) {
            match self.db.requeue_sparse_meta() {
                Ok(n) => { info!("{} messages were stored without their details; fetching them again", n); let _ = self.db.set_flag(SPARSE_META_FLAG); }
                Err(e) => warn!("Could not requeue messages without details: {}", e),
            }
        }
        let remaining = self.db.meta_remaining().unwrap_or(0);
        if remaining == 0 {
            sync_state::set_meta_active(&acct, false);
            return;
        }
        if self.provider.supports_delta() {
            if !self.db.flag_is_set(FIELDS_FLAG).unwrap_or(true) {
                match self.db.reset_all_delta_states() {
                    Ok(n) => {
                        info!("Message fields: {} messages need details; walking {} folder feeds again", remaining, n);
                        let _ = self.db.set_flag(FIELDS_FLAG);
                    }
                    Err(e) => warn!("Could not reset the change feeds for the new fields: {}", e),
                }
            }
            sync_state::set_meta_active(&acct, self.db.delta_walk_incomplete().unwrap_or(0) > 0 || !self.db.flag_is_set(FIELDS_FLAG).unwrap_or(true));
        } else {
            sync_state::set_meta_active(&acct, true);
        }
    }

    /// Per-message repopulate for providers without a change feed. Works for at most
    /// `budget`, then yields so mail sync and pushes keep their rhythm; the next cycle carries on.
    async fn backfill_meta_per_message(&self, budget: Duration) {
        // Change-feed providers fill details through the feed; only stragglers (items that came
        // without them) are fetched one by one, and only once the folder walks are done.
        let straggler_mode = self.provider.supports_delta() && self.db.delta_walk_incomplete().unwrap_or(1) == 0;
        if !self.provider.repopulates_meta_per_message() && !straggler_mode { return; }
        let acct = self.db.account_id().to_string();
        let started = Instant::now();
        let mut skip: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut filled = 0usize;
        while started.elapsed() < budget {
            let rows = match self.db.messages_needing_meta(200) {
                Ok(r) => r,
                Err(e) => { warn!("Could not list messages needing details: {}", e); break; }
            };
            let ids: Vec<String> = rows.into_iter().map(|(id, _)| id).filter(|id| !skip.contains(id)).take(40).collect();
            if ids.is_empty() { break; }
            let metas = self.provider.fetch_message_meta(&ids).await;
            for m in &metas {
                if let Err(e) = self.db.update_message_meta(m) { warn!("Could not store details of {}: {}", m.id, e); }
            }
            filled += metas.len();
            let got: std::collections::HashSet<&str> = metas.iter().map(|m| m.id.as_str()).collect();
            for id in &ids { if !got.contains(id.as_str()) { skip.insert(id.clone()); } }
            sleep(Duration::from_millis(400)).await;
        }
        if filled > 0 {
            debug!("mail[{}] message details: filled {} ({} skipped this round)", acct, filled, skip.len());
            sync_state::bump_mail("message details filled in");
        }
        if self.db.meta_remaining().unwrap_or(0) == 0 || skip.len() >= self.db.meta_remaining().unwrap_or(0) as usize {
            sync_state::set_meta_active(&acct, false);
        }
    }

    /// Read the account's categories / labels with their colours.
    async fn sync_categories(&self) {
        match self.provider.fetch_categories().await {
            Ok(defs) => {
                if let Err(e) = self.db.replace_category_defs(&defs) { warn!("Could not store categories: {}", e); }
                let _ = self.db.set_master_list_available(true);
            }
            Err(e) => {
                let m = e.to_string();
                // Exchange refuses the list until the account has signed in again with the
                // MailboxSettings permission; categories still work by name, just uncoloured here.
                if m.contains("403") || m.contains("401") || m.to_lowercase().contains("access") || m.contains("not supported") {
                    debug!("Category list not available for this account: {}", m);
                    let _ = self.db.set_master_list_available(false);
                } else {
                    warn!("Could not read categories: {}", m);
                }
            }
        }
    }

    /// Create categories made in the UI, then push category changes on messages.
    async fn push_pending_categories(&self) {
        if let Ok(pending) = self.db.pending_categories() {
            for (name, hex) in pending {
                let color = crate::models::CATEGORY_PALETTE.iter().find(|c| c.hex == hex || c.gmail_bg == hex)
                    .unwrap_or(&crate::models::CATEGORY_PALETTE[6]);
                match self.provider.create_category(&name, color).await {
                    Ok(def) => { let _ = self.db.finish_pending_category(&name, def.provider_color.as_deref()); }
                    Err(e) if crate::compose::is_transient(&e) => warn!("Category '{}' will be created later: {}", name, e),
                    Err(e) => {
                        // Exchange without the MailboxSettings permission: the name still works on messages.
                        warn!("Could not create category '{}' on the provider: {}", name, e);
                        let _ = self.db.finish_pending_category(&name, None);
                    }
                }
            }
        }
        let pushes = match self.db.pending_category_pushes() {
            Ok(p) => p,
            Err(e) => { error!("Could not list pending category changes: {}", e); return; }
        };
        for (id, wanted, had) in pushes {
            match self.provider.set_message_categories(&id, &wanted, &had).await {
                Ok(()) => { let _ = self.db.finish_category_push(&id, &wanted); }
                Err(e) if crate::compose::is_transient(&e) => warn!("Categories of {} will be pushed later: {}", id, e),
                Err(e) => {
                    warn!("Provider refused the category change for {}: {}", id, e);
                    let _ = self.db.revert_category_push(&id);
                    sync_state::bump_mail("category change refused");
                }
            }
        }
    }

    /// Push archive / delete / move actions whose undo window is over. A transient failure
    /// leaves the action queued for the next cycle; a refusal puts the message back in its list.
    async fn push_due_actions(&self) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let due = match self.db.due_actions(now) {
            Ok(d) => d,
            Err(e) => { error!("Could not list due message actions: {}", e); return; }
        };
        if due.is_empty() { return; }
        let mut changed = false;
        for (id, folder, action) in due {
            match self.provider.apply_message_action(&id, &folder, &action).await {
                Ok(()) => {
                    if let Err(e) = self.db.finish_message_action(&id) { error!("Could not finish action for {}: {}", id, e); }
                    changed = true;
                }
                Err(e) if crate::compose::is_transient(&e) => warn!("Action {:?} for {} will be retried: {}", action, id, e),
                Err(e) => {
                    warn!("Action {:?} for {} was refused, putting the message back: {}", action, id, e);
                    if let Err(e2) = self.db.revert_message_action(&id) { error!("Could not revert action for {}: {}", id, e2); }
                    changed = true;
                }
            }
        }
        if changed { sync_state::bump_mail("message actions pushed"); }
    }

    /// Bring the cached read flags of a folder in line with the provider's.
    async fn reconcile_read_state(&self, folder_id: &str, folder_name: &str) {
        match self.provider.fetch_unread_ids(folder_id).await {
            Ok(unread) => match self.db.reconcile_read_state(folder_id, &unread) {
                Ok(0) => {}
                Ok(n) => {
                    debug!("Read state: {} message(s) updated in {}", n, folder_name);
                    sync_state::bump_mail("read flags changed");
                }
                Err(e) => error!("Read-state reconcile failed for {}: {}", folder_name, e),
            },
            Err(e) => warn!("Could not read unread list for {}: {}", folder_name, e),
        }
    }

    /// Fetch and insert messages for a single folder.
    /// is_initial=true: paginate all messages; false: fetch 50 most recent only.
    async fn sync_folder_messages(&self, folder_id: &str, is_initial: bool) -> Result<usize> {
        if is_initial {
            debug!("Full paginated sync for folder: {}", folder_id);
            let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<crate::models::EmailMessage>>(4);

            let provider = Arc::clone(&self.provider);
            let folder_id_owned = folder_id.to_string();

            // Spawn the paginator so it runs concurrently with DB writes
            let fetch_handle = tokio::spawn(async move {
                provider.fetch_all_folder_messages(&folder_id_owned, tx).await
            });

            let db = Arc::clone(&self.db);
            let mut inserted = 0usize;
            let mut first_batch = true;

            // Consume each page and persist it in ONE transaction (one fsync per page, and the
            // write lock is held for microseconds so readers/UI never wait on it).
            while let Some(batch) = rx.recv().await {
                if first_batch {
                    first_batch = false;
                    perf::mark(&format!("mail[{}] folder {} first page of {} received", db.account_id(), &folder_id[..folder_id.len().min(8)], batch.len()));
                }
                match db.insert_emails_batch(&batch) {
                    Ok(n) => inserted += n,
                    Err(e) => error!("Failed to store page for {}: {}", folder_id, e),
                }
                tokio::task::yield_now().await;
            }

            if let Ok(Err(e)) = fetch_handle.await {
                error!("Paginated fetch error for {}: {}", folder_id, e);
            }
            if inserted > 0 {
                sync_state::bump_mail(&format!("crawl stored {} older/new messages", inserted));
            }

            Ok(inserted)
        } else {
            // Incremental: 50 most recent
            debug!("Incremental sync for folder: {}", folder_id);
            let emails = self.provider.fetch_folder_messages(folder_id, 50).await?;
            let inserted = self.db.insert_emails_batch(&emails)?;
            if inserted > 0 {
                sync_state::bump_mail(&format!("poll stored {} new messages", inserted));
            }
            Ok(inserted)
        }
    }

    /// Stage 1 of a launch: the newest `limit` Inbox messages, stored in one transaction and
    /// announced to the UI immediately. Errors are logged and ignored: the regular sync that
    /// follows covers the same ground.
    async fn sync_recent(&self, limit: usize) {
        let acct = self.db.account_id().to_string();
        let t = Instant::now();
        let emails = match self.provider.fetch_inbox(limit).await {
            Ok(e) => e,
            Err(e) => {
                warn!("Recent-mail fetch failed for {}: {}", acct, e);
                return;
            }
        };
        perf::mark(&format!("mail[{}] recent {}: fetched {} in {:.1}ms", acct, limit, emails.len(), t.elapsed().as_secs_f64() * 1000.0));
        match self.db.insert_emails_batch(&emails) {
            Ok(n) => {
                perf::mark(&format!("mail[{}] recent {}: {} new stored", acct, limit, n));
                if n > 0 {
                    sync_state::bump_mail(&format!("recent {}: {} new", limit, n));
                }
            }
            Err(e) => error!("Could not store recent mail for {}: {}", acct, e),
        }
    }
}

/// Trigger an immediate email sync (used by HTTP trigger)
pub async fn trigger_sync(daemon: &EmailDaemon) {
    match daemon.sync_messages(false).await {
        Ok(count) => {
            info!("Triggered sync: stored {} new emails", count);
        }
        Err(e) => {
            error!("Triggered sync failed: {}", e);
        }
    }
}

#[cfg(test)]
mod delta_tests {
    use super::*;
    use crate::models::{DeltaEnd, DeltaPage, EmailMessage, MailFolder};
    use crate::providers::EmailProvider;
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// Scripted change feed: each `fetch_delta` call pops the next (pages, end) and records the link it got.
    struct Fake {
        script: Mutex<Vec<(Vec<DeltaPage>, DeltaEnd)>>,
        links_seen: Mutex<Vec<Option<String>>>,
    }

    #[async_trait]
    impl EmailProvider for Fake {
        async fn fetch_inbox(&self, _l: usize) -> Result<Vec<EmailMessage>> { Ok(vec![]) }
        async fn fetch_folder_messages(&self, _f: &str, _l: usize) -> Result<Vec<EmailMessage>> { Ok(vec![]) }
        async fn fetch_folders(&self) -> Result<Vec<MailFolder>> { Ok(vec![]) }
        async fn is_token_valid(&self) -> Result<bool> { Ok(true) }
        fn supports_delta(&self) -> bool { true }
        async fn fetch_delta(&self, _f: &str, link: Option<String>, tx: tokio::sync::mpsc::Sender<DeltaPage>) -> Result<DeltaEnd> {
            self.links_seen.lock().unwrap().push(link);
            let (pages, end) = self.script.lock().unwrap().remove(0);
            for p in pages {
                tx.send(p).await.unwrap();
            }
            Ok(end)
        }
    }

    fn msg(id: &str, read: bool) -> EmailMessage {
        EmailMessage { id: id.into(), from: "a@b.c".into(), subject: format!("s{id}"), received: "2026-10-06T12:00:00Z".into(),
                       body: "p".into(), folder_id: Some("F".into()), is_read: read, ..Default::default() }
    }
    fn page(up: Vec<EmailMessage>, removed: Vec<&str>, next: Option<&str>) -> DeltaPage {
        DeltaPage { upserts: up, removed: removed.into_iter().map(String::from).collect(), next_link: next.map(String::from) }
    }
    fn setup(script: Vec<(Vec<DeltaPage>, DeltaEnd)>) -> (EmailDaemon, Arc<Fake>) {
        let db = Arc::new(Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap());
        db.upsert_folder(&MailFolder { id: "F".into(), display_name: "Inbox".into(), parent_folder_id: None,
                                       unread_item_count: None, total_item_count: None, well_known_name: Some("inbox".into()) }).unwrap();
        let fake = Arc::new(Fake { script: Mutex::new(script), links_seen: Mutex::new(vec![]) });
        (EmailDaemon::new(DaemonConfig::default(), db, fake.clone()), fake)
    }
    fn run<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
    }
    fn read_flag(d: &EmailDaemon, id: &str) -> Option<bool> {
        d.db.message_is_read(id).ok().flatten()
    }

    #[test]
    fn first_walk_stores_everything_then_incremental_applies_only_changes() {
        let (d, fake) = setup(vec![
            (vec![page(vec![msg("1", false), msg("2", false)], vec![], Some("n1")), page(vec![msg("3", true)], vec![], None)], DeltaEnd::Done("L1".into())),
            (vec![page(vec![msg("2", true), msg("4", false)], vec!["3"], None)], DeltaEnd::Done("L2".into())),
        ]);
        run(async {
            d.sync_messages_delta().await.unwrap();
            assert_eq!(d.db.delta_state("F").unwrap(), Some(("L1".into(), true)));
            assert!(d.db.email_exists("1").unwrap() && d.db.email_exists("3").unwrap());

            let n = d.sync_messages_delta().await.unwrap();
            assert_eq!(n, 3, "2 flipped read + 4 new + 3 removed");
            assert_eq!(d.db.delta_state("F").unwrap(), Some(("L2".into(), true)));
            assert!(!d.db.email_exists("3").unwrap(), "removed on the server");
            assert!(d.db.email_exists("4").unwrap());
            assert_eq!(read_flag(&d, "2"), Some(true));
        });
        let seen = fake.links_seen.lock().unwrap().clone();
        assert_eq!(seen, vec![None, Some("L1".to_string())], "second cycle resumes from the stored link");
    }

    #[test]
    fn interrupted_first_walk_resumes_from_its_progress_link() {
        // The walk dies after page 1 (error): progress link n1 is stored incomplete; next cycle resumes there.
        let (d, fake) = setup(vec![
            (vec![page(vec![msg("2", false)], vec![], None)], DeltaEnd::Done("L1".into())),
        ]);
        // Simulate the crash directly: page 1 applied with its progress link, never completed.
        d.db.apply_delta_page("F", &page(vec![msg("1", false)], vec![], Some("n1")), Some("n1")).unwrap();
        assert_eq!(d.db.delta_state("F").unwrap(), Some(("n1".into(), false)));
        run(async { d.sync_messages_delta().await.unwrap(); });
        assert_eq!(fake.links_seen.lock().unwrap().clone(), vec![Some("n1".to_string())]);
        assert_eq!(d.db.delta_state("F").unwrap(), Some(("L1".into(), true)));
        assert!(d.db.email_exists("1").unwrap() && d.db.email_exists("2").unwrap());
    }

    #[test]
    fn rejected_link_starts_a_full_walk_once() {
        let (d, fake) = setup(vec![
            (vec![], DeltaEnd::Reset),
            (vec![page(vec![msg("1", false)], vec![], None)], DeltaEnd::Done("L9".into())),
        ]);
        d.db.set_delta_state("F", "stale", true).unwrap();
        run(async { d.sync_messages_delta().await.unwrap(); });
        assert_eq!(fake.links_seen.lock().unwrap().clone(), vec![Some("stale".to_string()), None]);
        assert_eq!(d.db.delta_state("F").unwrap(), Some(("L9".into(), true)));
        assert!(d.db.email_exists("1").unwrap());
    }

    #[test]
    fn full_walk_purges_ghosts_deleted_before_tracking_existed() {
        let (d, _f) = setup(vec![(vec![page((1..=9).map(|i| msg(&i.to_string(), false)).collect(), vec![], None)], DeltaEnd::Done("L".into()))]);
        // 9 live rows + 1 ghost already cached (10 rows; the ghost is 10% < the 20% guard)
        for i in 1..=9 { d.db.insert_email(&msg(&i.to_string(), false)).unwrap(); }
        d.db.insert_email(&msg("ghost", false)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        run(async { d.sync_messages_delta().await.unwrap(); });
        assert!(!d.db.email_exists("ghost").unwrap());
        assert!(d.db.email_exists("5").unwrap());
    }

    #[test]
    fn purge_refuses_to_wipe_most_of_a_folder() {
        let (d, _f) = setup(vec![(vec![page(vec![msg("1", false)], vec![], None)], DeltaEnd::Done("L".into()))]);
        for i in 1..=10 { d.db.insert_email(&msg(&i.to_string(), false)).unwrap(); }
        std::thread::sleep(std::time::Duration::from_millis(5));
        run(async { d.sync_messages_delta().await.unwrap(); });
        assert!(d.db.email_exists("7").unwrap(), "9 of 10 unseen is a bad enumeration, not ghosts");
    }

    fn set_server_total(d: &EmailDaemon, n: i32) {
        d.db.upsert_folder(&MailFolder { id: "F".into(), display_name: "Inbox".into(), parent_folder_id: None,
                                         unread_item_count: None, total_item_count: Some(n), well_known_name: Some("inbox".into()) }).unwrap();
    }

    #[test]
    fn mostly_emptied_folder_is_purged_when_the_enumeration_matches_the_server_count() {
        // Deleted Items style: 10 cached, the server has 3 (matches what the walk returned).
        let (d, _f) = setup(vec![(vec![page(vec![msg("1", false), msg("2", false), msg("3", false)], vec![], None)], DeltaEnd::Done("L".into()))]);
        for i in 1..=10 { d.db.insert_email(&msg(&i.to_string(), false)).unwrap(); }
        set_server_total(&d, 3);
        std::thread::sleep(std::time::Duration::from_millis(5));
        run(async { d.sync_messages_delta().await.unwrap(); });
        assert!(d.db.email_exists("3").unwrap());
        assert!(!d.db.email_exists("7").unwrap(), "7 of 10 were ghosts and the walk matched the server count");
    }

    #[test]
    fn short_enumeration_is_not_trusted_even_when_few_rows_would_go() {
        // The server says 100 items but the walk returned only 1: something is wrong, delete nothing.
        let (d, _f) = setup(vec![(vec![page(vec![msg("1", false)], vec![], None)], DeltaEnd::Done("L".into()))]);
        for i in 1..=5 { d.db.insert_email(&msg(&i.to_string(), false)).unwrap(); }
        set_server_total(&d, 100);
        std::thread::sleep(std::time::Duration::from_millis(5));
        run(async { d.sync_messages_delta().await.unwrap(); });
        assert!(d.db.email_exists("5").unwrap());
    }

    #[test]
    fn local_unpushed_read_change_survives_a_delta_page() {
        let (d, _f) = setup(vec![(vec![page(vec![msg("1", false)], vec![], None)], DeltaEnd::Done("L".into()))]);
        d.db.insert_email(&msg("1", false)).unwrap();
        d.db.set_message_read("1", true).unwrap();
        run(async { d.sync_messages_delta().await.unwrap(); });
        assert_eq!(read_flag(&d, "1"), Some(true), "read_pending row keeps the local flag");
    }

    #[test]
    fn poll_interval_follows_settings_toml_and_rereads() {
        let (mut d, _f) = setup(vec![]);
        let path = std::env::temp_dir().join(format!("omalook-daemon-poll-{}.toml", std::process::id()));
        std::fs::write(&path, "[sync]\npoll_interval_secs = 30\n").unwrap();
        d.config.settings_path = Some(path.clone());
        assert_eq!(d.poll_interval(), Duration::from_secs(30));
        std::fs::write(&path, "[sync]\npoll_interval_secs = 90\n").unwrap();
        assert_eq!(d.poll_interval(), Duration::from_secs(90), "an edit applies on the next cycle");
        d.config.settings_path = None;
        assert_eq!(d.poll_interval(), Duration::from_secs(d.config.poll_interval_secs));
    }
}
