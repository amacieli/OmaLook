//! Per-account sync scheduler.
//!
//! Each running account gets one mail, one calendar, one contacts and one outbox thread, each with
//! its own current-thread tokio runtime (the daemons use blocking HTTP/SQLite
//! calls, so a stall in one account can't delay another). Both threads of an
//! account share that account's `TokenBroker`, hence one token refresh at a
//! time. Daemon logic is unchanged — the scheduler only decides which accounts
//! run and gives each its own provider and database handle.

use crate::auth::{broker_for, AuthManager};
use crate::accounts::provider_of;
use crate::calendar_daemon::{CalendarDaemon, CalendarDaemonConfig};
use crate::contacts_daemon::{ContactsDaemon, ContactsDaemonConfig};
use crate::db::Database;
use crate::email_daemon::{DaemonConfig, EmailDaemon};
use crate::outbox::OutboxWorker;
use crate::calendar_outbox::CalendarOutboxWorker;
use crate::providers::{
    CalendarProvider, ContactsProvider, EmailProvider, GmailProvider, GoogleCalendarProvider, GoogleContactsProvider,
    GraphCalendarProvider, GraphContactsProvider, GraphEmailProvider,
};
use log::{error, info, warn};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// Providers that currently have sync implementations (Microsoft Graph, Gmail).
pub fn is_supported(provider: &str) -> bool {
    matches!(provider, "exchange" | "outlook" | "gmail")
}

/// The three providers one account syncs through. Providers are built on their own daemon
/// thread (`Rc`-free but not all `Send`-constructible), hence one small constructor per kind.
fn email_provider(provider: &str, id: &str, db_path: &str) -> Arc<dyn EmailProvider> {
    match provider {
        "gmail" => Arc::new(GmailProvider::new(id, Some(PathBuf::from(db_path)))),
        _ => Arc::new(GraphEmailProvider::new(AuthManager::for_account(id))),
    }
}

fn calendar_provider(provider: &str, id: &str) -> Arc<dyn CalendarProvider> {
    match provider {
        "gmail" => Arc::new(GoogleCalendarProvider::new(id)),
        _ => Arc::new(GraphCalendarProvider::new(AuthManager::for_account(id))),
    }
}

fn contacts_provider(provider: &str, id: &str) -> Arc<dyn ContactsProvider> {
    match provider {
        "gmail" => Arc::new(GoogleContactsProvider::new(id)),
        _ => Arc::new(GraphContactsProvider::new(AuthManager::for_account(id))),
    }
}

pub struct SyncScheduler {
    db_path: PathBuf,
    settings_path: PathBuf,
    running: Mutex<HashMap<String, Vec<Arc<Notify>>>>,
}

impl SyncScheduler {
    pub fn new(config_dir: &std::path::Path) -> Self {
        Self {
            db_path: config_dir.join("messages.db"),
            settings_path: config_dir.join("settings.toml"),
            running: Mutex::new(HashMap::new()),
        }
    }

    fn db_str(&self) -> String {
        self.db_path.to_string_lossy().to_string()
    }

    /// Start sync for every account in the database that is signed in.
    pub fn start_all(&self) {
        let db = match Database::open(&self.db_str()) {
            Ok(db) => db,
            Err(e) => {
                error!("Scheduler: cannot open database: {}", e);
                return;
            }
        };
        // The pre-multi-account login keeps its id; register it once its token exists.
        if broker_for(crate::token_store::DEFAULT_ACCOUNT).is_authenticated() {
            if let Err(e) = db.ensure_legacy_account() {
                warn!("Scheduler: could not register legacy account: {}", e);
            }
        }
        match db.list_accounts() {
            Ok(accounts) => {
                for a in accounts {
                    if a.enabled && broker_for(&a.id).is_authenticated() {
                        self.start_account(&a.id);
                    } else {
                        info!("Scheduler: account {} is signed out; not syncing", a.id);
                    }
                }
            }
            Err(e) => error!("Scheduler: cannot list accounts: {}", e),
        }
    }

    /// Start mail + calendar sync for an account. Returns false if it was already
    /// running or its provider has no sync implementation yet.
    pub fn start_account(&self, account_id: &str) -> bool {
        let provider = provider_of(account_id).to_string();
        if !is_supported(&provider) {
            warn!("Scheduler: no sync implementation for provider '{}' (account {})", provider, account_id);
            return false;
        }
        let mut running = self.running.lock().unwrap_or_else(|p| p.into_inner());
        if running.contains_key(account_id) {
            return false;
        }

        let mail_stop = Arc::new(Notify::new());
        let cal_stop = Arc::new(Notify::new());
        let contacts_stop = Arc::new(Notify::new());

        let (id, path, kind, mail_settings) = (account_id.to_string(), self.db_str(), provider.clone(), self.settings_path.clone());
        spawn_sync_thread(format!("mail-{}", account_id), mail_stop.clone(), move || async move {
            let provider = email_provider(&kind, &id, &path);
            let db = match Database::open_for_account(&path, &id) {
                Ok(db) => Arc::new(db),
                Err(e) => return error!("Mail sync {}: cannot open database: {}", id, e),
            };
            let cfg = DaemonConfig { settings_path: Some(mail_settings), ..DaemonConfig::default() };
            EmailDaemon::new(cfg, db, provider).start().await;
        });

        let (id, path, settings_path, kind) = (account_id.to_string(), self.db_str(), self.settings_path.clone(), provider.clone());
        spawn_sync_thread(format!("cal-{}", account_id), cal_stop.clone(), move || async move {
            let provider = calendar_provider(&kind, &id);
            let db = match Database::open_for_account(&path, &id) {
                Ok(db) => Arc::new(db),
                Err(e) => return error!("Calendar sync {}: cannot open database: {}", id, e),
            };
            let cfg = CalendarDaemonConfig { settings_path: Some(settings_path), ..CalendarDaemonConfig::default() };
            CalendarDaemon::new(cfg, db, provider).start().await;
        });

        let (id, path, kind) = (account_id.to_string(), self.db_str(), provider.clone());
        spawn_sync_thread(format!("contacts-{}", account_id), contacts_stop.clone(), move || async move {
            let provider = contacts_provider(&kind, &id);
            let db = match Database::open_for_account(&path, &id) {
                Ok(db) => Arc::new(db),
                Err(e) => return error!("Contacts sync {}: cannot open database: {}", id, e),
            };
            ContactsDaemon::new(ContactsDaemonConfig::default(), db, provider).start().await;
        });

        // Sends what the user queued with Send, the moment it is due: its own thread, so a slow
        // mail sync can never hold a message back.
        let outbox_stop = Arc::new(Notify::new());
        let (id, path, kind) = (account_id.to_string(), self.db_str(), provider.clone());
        spawn_sync_thread(format!("outbox-{}", account_id), outbox_stop.clone(), move || async move {
            let provider = email_provider(&kind, &id, &path);
            let db = match Database::open_for_account(&path, &id) {
                Ok(db) => Arc::new(db),
                Err(e) => return error!("Outbox {}: cannot open database: {}", id, e),
            };
            OutboxWorker::new(&id, db, provider).run().await;
        });

        // Calendar writes (new events) wait out the same undo window, then go out from here.
        let cal_outbox_stop = Arc::new(Notify::new());
        let (id, path, kind, settings_path) = (account_id.to_string(), self.db_str(), provider.clone(), self.settings_path.clone());
        spawn_sync_thread(format!("caloutbox-{}", account_id), cal_outbox_stop.clone(), move || async move {
            let provider = calendar_provider(&kind, &id);
            let db = match Database::open_for_account(&path, &id) {
                Ok(db) => Arc::new(db),
                Err(e) => return error!("Calendar outbox {}: cannot open database: {}", id, e),
            };
            CalendarOutboxWorker::new(&id, db, provider, Some(settings_path)).run().await;
        });

        running.insert(account_id.to_string(), vec![mail_stop, cal_stop, contacts_stop, outbox_stop, cal_outbox_stop]);
        info!("Scheduler: started sync for account {}", account_id);
        true
    }

    /// Stop an account's sync threads (they end at their next await point).
    pub fn stop_account(&self, account_id: &str) -> bool {
        let stops = self.running.lock().unwrap_or_else(|p| p.into_inner()).remove(account_id);
        match stops {
            Some(list) => {
                for n in list {
                    n.notify_one(); // stores a permit if the task isn't waiting yet
                }
                info!("Scheduler: stopped sync for account {}", account_id);
                true
            }
            None => false,
        }
    }

    pub fn is_running(&self, account_id: &str) -> bool {
        self.running.lock().unwrap_or_else(|p| p.into_inner()).contains_key(account_id)
    }
}

/// Run a daemon future on its own thread + runtime until it ends or `stop` fires.
/// `make` builds the future on the new thread, so nothing needs to be `Send` but `make` itself.
fn spawn_sync_thread<F, Fut>(name: String, stop: Arc<Notify>, make: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()>,
{
    let thread_name = name.clone();
    let spawned = std::thread::Builder::new().name(name).spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => return error!("{}: cannot create runtime: {}", thread_name, e),
        };
        rt.block_on(async {
            tokio::select! {
                _ = make() => {}
                _ = stop.notified() => {}
            }
        });
    });
    if let Err(e) = spawned {
        error!("Scheduler: cannot spawn thread: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    #[test]
    fn unsupported_providers_are_not_started() {
        let s = SyncScheduler::new(std::path::Path::new("/nonexistent"));
        assert!(!s.start_account("imap-abc123"));
        assert!(!s.is_running("imap-abc123"));
        assert!(is_supported("gmail") && is_supported("exchange") && !is_supported("imap"));
    }

    #[test]
    fn stop_ends_a_running_thread_even_if_signalled_early() {
        // Fire the stop permit BEFORE the thread starts waiting: it must still end.
        let stop = Arc::new(Notify::new());
        stop.notify_one();
        let finished = Arc::new(AtomicBool::new(false));
        let f = finished.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        spawn_sync_thread("t".into(), stop, move || async move {
            tokio::time::sleep(Duration::from_secs(3600)).await; // would run "forever"
            f.store(true, Ordering::SeqCst);
        });
        // The thread exits promptly; detect via a second thread joining indirectly.
        std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(300)); let _ = tx.send(()); });
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(!finished.load(Ordering::SeqCst), "daemon future must have been cancelled, not completed");
    }

    #[test]
    fn independent_threads_run_concurrently() {
        let a = Arc::new(AtomicBool::new(false));
        let b = Arc::new(AtomicBool::new(false));
        for flag in [a.clone(), b.clone()] {
            spawn_sync_thread("t".into(), Arc::new(Notify::new()), move || async move {
                // blocking call stalls only this thread's runtime
                std::thread::sleep(Duration::from_millis(100));
                flag.store(true, Ordering::SeqCst);
            });
        }
        std::thread::sleep(Duration::from_millis(350));
        assert!(a.load(Ordering::SeqCst) && b.load(Ordering::SeqCst));
    }
}
