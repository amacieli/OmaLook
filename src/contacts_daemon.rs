//! Contacts synchronization daemon (mirrors email_daemon.rs: contact folders play the role
//! of mail folders).
//!
//! - Polls on the same schedule as mail: contacts every 2 minutes, folders every 10
//! - First authenticated run: full paginated sync of every folder, each ~50-contact page
//!   written as it arrives, until the whole address book is loaded
//! - Later polls: a delta read per folder (only what changed, deletions included) where the provider
//!   has one; otherwise the 50 most recently modified contacts per folder
//! - Every 6 hours the delta links are dropped and every folder is read in full again (safety net)
//! - Contacts are upserted (edits propagate); the local favorite flag is never overwritten
//! - Independent of the mail and calendar daemons: own provider, own DB handle, own loop
//!   (they only share the account's token broker)

use crate::db::Database;
use crate::errors::Result;
use crate::models::{Contact, ContactDeltaPage, ContactFolder};
use crate::providers::ContactsProvider;
use crate::perf;
use log::{debug, error, info, warn};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Set (per account) once a full walk of the contacts has finished.
pub const FULL_FLAG: &str = "contacts_full_walk";
/// How often the whole contact list is walked again.
const FULL_EVERY: Duration = Duration::from_secs(6 * 3600);

pub struct ContactsDaemonConfig {
    pub poll_interval_secs: u64,
    pub folder_sync_interval_secs: u64,
    /// List link: create a contact for a list member that has none (settings.toml `[lists] create_contacts = "always"`).
    pub create_contacts: bool,
}

impl Default for ContactsDaemonConfig {
    fn default() -> Self {
        Self { poll_interval_secs: 120, folder_sync_interval_secs: 600, create_contacts: false }
    }
}

pub struct ContactsDaemon {
    config: ContactsDaemonConfig,
    db: Arc<Database>,
    provider: Arc<dyn ContactsProvider>,
}

impl ContactsDaemon {
    pub fn new(config: ContactsDaemonConfig, db: Arc<Database>, provider: Arc<dyn ContactsProvider>) -> Self {
        Self { config, db, provider }
    }

    pub async fn start(&self) {
        info!(
            "Contacts daemon started (contacts every {}s, folders every {}s)",
            self.config.poll_interval_secs, self.config.folder_sync_interval_secs
        );
        let folder_interval = Duration::from_secs(self.config.folder_sync_interval_secs);
        let message_interval = Duration::from_secs(self.config.poll_interval_secs);
        let mut last_folder_sync: Option<Instant> = None;
        let mut last_sync: Option<Instant> = None;
        let mut last_full: Option<Instant> = None;

        // Mail's priority stages (recent 50, folders, read flags) go first.
        let acct = self.db.account_id().to_string();
        crate::sync_state::wait_gate(&acct, Duration::from_secs(60)).await;

        loop {
            match self.provider.is_token_valid().await {
                Ok(true) => {
                    // A full walk runs until one has finished (kept in the database, so a start with
                    // no network cannot leave the cache on 50-per-folder updates for good), then
                    // again every FULL_EVERY to pick up edits and deletions made elsewhere.
                    let full_done = self.db.flag_is_set(FULL_FLAG).unwrap_or(false);
                    let full_due = !full_done || last_full.map(|t| t.elapsed() >= FULL_EVERY).unwrap_or(true);

                    if last_folder_sync.map(|t| t.elapsed() >= folder_interval).unwrap_or(true) {
                        let _t = perf::span(format!("contacts[{}] sync_folders", self.db.account_id()));
                        match self.sync_folders().await {
                            Ok(n) => {
                                info!("Synced {} contact folders", n);
                                last_folder_sync = Some(Instant::now());
                            }
                            Err(e) => error!("Failed to sync contact folders: {}", e),
                        }
                    }

                    if full_due || last_sync.map(|t| t.elapsed() >= message_interval).unwrap_or(true) {
                        let _t = perf::span(format!("contacts[{}] sync_contacts(full={})", self.db.account_id(), full_due));
                        match self.sync_contacts(full_due).await {
                            Ok(n) => {
                                if n > 0 { info!("Synced {} contacts", n) } else { debug!("No contact changes") }
                                last_sync = Some(Instant::now());
                                // lists mirrored as categories follow the contacts (Graph only; docs/PLAN-contacts.md section 7)
                                if self.provider.supports_delta() {
                                    let policy = if self.config.create_contacts { crate::list_sync::CreatePolicy::Always } else { crate::list_sync::CreatePolicy::Skip };
                                    let walked = self.db.flag_is_set(FULL_FLAG).unwrap_or(false) || full_due;
                                    let r = crate::list_sync::run_and_record(&self.db, walked, policy);
                                    if let Some(why) = &r.skipped { debug!("List link {}: not run ({})", r.account, why) }
                                }
                                if full_due {
                                    last_full = Some(Instant::now());
                                    if let Err(e) = self.db.set_flag(FULL_FLAG) { warn!("Could not record the full contacts walk: {}", e); }
                                    crate::sync_state::bump_mail("contacts walked");
                                }
                            }
                            Err(e) => error!("Failed to sync contacts: {}", e),
                        }
                    }
                    tokio::time::sleep(message_interval).await;
                }
                Ok(false) => {
                    warn!("Contacts daemon: auth token not valid, waiting for authentication...");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                Err(e) => {
                    error!("Contacts daemon: failed to check token validity: {}", e);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }

    async fn sync_folders(&self) -> Result<usize> {
        let folders = self.provider.fetch_folders().await?;
        for f in &folders {
            if let Err(e) = self.db.upsert_contact_folder(f) {
                error!("Failed to upsert contact folder {}: {}", f.display_name, e);
            }
        }
        Ok(folders.len())
    }

    /// `full`: walk every contact of every folder (and drop the ones deleted elsewhere); an Err
    /// then means the walk did not finish, so the caller tries again. Otherwise only the changes:
    /// a delta read where the provider has one, else the 50 most recently changed per folder.
    async fn sync_contacts(&self, full: bool) -> Result<usize> {
        let folders: Vec<ContactFolder> = self.db.get_contact_folders()?;
        if full && folders.is_empty() {
            return Err(crate::errors::OmarchyError::HttpError("no contact folders known yet".into()));
        }
        let mut total = 0;
        let mut failed = 0;
        for folder in &folders {
            let r = if self.provider.supports_delta() {
                self.sync_folder_delta(&folder.id, full).await
            } else {
                self.sync_folder(&folder.id, full).await
            };
            match r {
                Ok(n) => total += n,
                Err(e) => { failed += 1; error!("Failed to sync contact folder {}: {}", folder.display_name, e); }
            }
        }
        if full && failed > 0 {
            return Err(crate::errors::OmarchyError::HttpError(format!("{} contact folder(s) not fully read", failed)));
        }
        Ok(total)
    }

    /// One folder by delta. With a stored link only the changes come back. With none (first run, a
    /// forced full walk, or a link the server no longer knows) every contact does, and what that
    /// walk did not see is dropped at its end (a walk that failed half way drops nothing).
    async fn sync_folder_delta(&self, folder_id: &str, reset: bool) -> Result<usize> {
        if reset {
            self.db.clear_contact_delta_link(folder_id)?;
        }
        let mut link = self.db.contact_delta_link(folder_id)?;
        loop {
            let walking_all = link.is_none();
            match self.run_delta(folder_id, link.as_deref(), walking_all).await {
                Ok((n, new_link)) => {
                    self.db.set_contact_delta_link(folder_id, &new_link)?;
                    return Ok(n);
                }
                // A stored link that fails is dropped once and the folder is read in full: the link may
                // simply be too old. A failure of the full read itself is the caller's to report.
                Err(e) if !walking_all => {
                    warn!("Contacts delta link of folder {} failed ({}); reading the folder in full", folder_id, e);
                    self.db.clear_contact_delta_link(folder_id)?;
                    link = None;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn run_delta(&self, folder_id: &str, link: Option<&str>, collect_seen: bool) -> Result<(usize, String)> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<ContactDeltaPage>(4);
        let provider = Arc::clone(&self.provider);
        let (folder_owned, link_owned) = (folder_id.to_string(), link.map(|s| s.to_string()));
        let fetch = tokio::spawn(async move { provider.fetch_delta(&folder_owned, link_owned.as_deref(), tx).await });
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut written = 0usize;
        while let Some(page) = rx.recv().await {
            if collect_seen {
                seen.extend(page.upserts.iter().map(|c| c.id.clone()));
            }
            match self.db.apply_contact_delta_page(&page) {
                Ok((stored, removed)) => {
                    written += stored + removed;
                    if removed > 0 { info!("Removed {} contacts deleted elsewhere (folder {})", removed, folder_id); }
                }
                Err(e) => error!("Failed to store a page of contacts: {}", e),
            }
            tokio::task::yield_now().await;
        }
        let new_link = match fetch.await {
            Ok(Ok(l)) => l,
            Ok(Err(e)) => return Err(e),
            Err(e) => return Err(crate::errors::OmarchyError::HttpError(format!("contacts task failed: {}", e))),
        };
        if collect_seen {
            // Only a walk that reached its end may say what is gone.
            let purged = self.db.purge_contacts_unseen(folder_id, &seen)?;
            if purged > 0 { info!("Removed {} contacts deleted elsewhere (folder {})", purged, folder_id); }
        }
        Ok((written, new_link))
    }

    async fn sync_folder(&self, folder_id: &str, full: bool) -> Result<usize> {
        if full {
            debug!("Full paginated contacts sync for folder: {}", folder_id);
            let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<Contact>>(4);
            let provider = Arc::clone(&self.provider);
            let folder_owned = folder_id.to_string();
            let fetch = tokio::spawn(async move { provider.fetch_all_folder_contacts(&folder_owned, tx).await });

            let mut written = 0usize;
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            while let Some(batch) = rx.recv().await {
                for c in &batch {
                    seen.insert(c.id.clone());
                    match self.db.upsert_contact(c) {
                        Ok(()) => written += 1,
                        Err(e) => error!("Failed to store contact {}: {}", c.id, e),
                    }
                }
                tokio::task::yield_now().await;
            }
            match fetch.await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => return Err(e),
                Err(e) => return Err(crate::errors::OmarchyError::HttpError(format!("contacts task failed: {}", e))),
            }
            // Only a walk that reached its end may say what is gone.
            let purged = self.db.purge_contacts_unseen(folder_id, &seen)?;
            if purged > 0 { info!("Removed {} contacts deleted elsewhere (folder {})", purged, folder_id); }
            Ok(written)
        } else {
            let contacts = self.provider.fetch_folder_contacts(folder_id, 50).await?;
            for c in &contacts {
                self.db.upsert_contact(c)?;
            }
            Ok(contacts.len())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// Serves two folders; folder "f1" has 120 contacts so the paged path is exercised.
    struct FakeProvider {
        calls: Mutex<Vec<String>>,
    }
    fn contact(i: usize, folder: &str) -> Contact {
        Contact { id: format!("{}-{}", folder, i), display_name: format!("C{}", i), folder_id: Some(folder.into()), ..Default::default() }
    }
    #[async_trait]
    impl ContactsProvider for FakeProvider {
        async fn fetch_folders(&self) -> Result<Vec<ContactFolder>> {
            Ok(vec![
                ContactFolder { id: "f1".into(), display_name: "Contacts".into(), parent_folder_id: None },
                ContactFolder { id: "f2".into(), display_name: "Clients".into(), parent_folder_id: None },
            ])
        }
        async fn fetch_folder_contacts(&self, folder_id: &str, limit: usize) -> Result<Vec<Contact>> {
            self.calls.lock().unwrap().push(format!("recent:{}:{}", folder_id, limit));
            Ok((0..3).map(|i| contact(i, folder_id)).collect())
        }
        async fn fetch_all_folder_contacts(&self, folder_id: &str, tx: tokio::sync::mpsc::Sender<Vec<Contact>>) -> Result<usize> {
            self.calls.lock().unwrap().push(format!("all:{}", folder_id));
            let n = if folder_id == "f1" { 120 } else { 5 };
            let all: Vec<Contact> = (0..n).map(|i| contact(i, folder_id)).collect();
            for page in all.chunks(50) {
                let _ = tx.send(page.to_vec()).await;
            }
            Ok(n)
        }
        async fn is_token_valid(&self) -> Result<bool> { Ok(true) }
    }

    fn daemon() -> (ContactsDaemon, Arc<FakeProvider>) {
        let p = Arc::new(FakeProvider { calls: Mutex::new(vec![]) });
        let db = Arc::new(Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap());
        (ContactsDaemon::new(ContactsDaemonConfig::default(), db, p.clone()), p)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn initial_sync_loads_every_page_of_every_folder() {
        let (d, p) = daemon();
        assert_eq!(d.sync_folders().await.unwrap(), 2);
        assert_eq!(d.sync_contacts(true).await.unwrap(), 125);
        assert_eq!(d.db.query_contacts("all", "first").unwrap().len(), 125);
        let calls = p.calls.lock().unwrap().clone();
        assert!(calls.contains(&"all:f1".to_string()) && calls.contains(&"all:f2".to_string()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn incremental_sync_fetches_50_most_recent_per_folder_and_is_idempotent() {
        let (d, p) = daemon();
        d.sync_folders().await.unwrap();
        d.sync_contacts(true).await.unwrap();
        d.sync_contacts(false).await.unwrap();
        d.sync_contacts(false).await.unwrap();
        assert_eq!(d.db.query_contacts("all", "first").unwrap().len(), 125, "upserts must not duplicate");
        assert!(p.calls.lock().unwrap().contains(&"recent:f1:50".to_string()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_full_walk_needs_folders_and_removes_contacts_deleted_elsewhere() {
        let (d, _p) = daemon();
        assert!(d.sync_contacts(true).await.is_err(), "no folders known yet: the walk cannot count as done");
        d.sync_folders().await.unwrap();
        d.sync_contacts(true).await.unwrap();
        // a contact cached earlier that the server no longer has
        d.db.upsert_contact(&contact(999, "f2")).unwrap();
        assert_eq!(d.db.query_contacts("all", "first").unwrap().len(), 126);
        d.sync_contacts(false).await.unwrap();
        assert_eq!(d.db.query_contacts("all", "first").unwrap().len(), 126, "an incremental pass never purges");
        d.sync_contacts(true).await.unwrap();
        assert_eq!(d.db.query_contacts("all", "first").unwrap().len(), 125);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn favorites_survive_resync() {
        let (d, _p) = daemon();
        d.sync_folders().await.unwrap();
        d.sync_contacts(true).await.unwrap();
        d.db.set_contact_favorite("f1-1", true).unwrap();
        d.sync_contacts(false).await.unwrap();
        let favs = d.db.query_contacts("favorites", "first").unwrap();
        assert_eq!(favs.len(), 1);
        assert_eq!(favs[0].contact.id, "f1-1");
    }

    // ---- delta ---------------------------------------------------------------------------------

    /// A provider with a delta feed: `script` holds the answers in order; a stored link is `"link-N"`.
    struct DeltaProvider {
        script: Mutex<Vec<std::result::Result<(Vec<ContactDeltaPage>, String), String>>>,
        seen_links: Mutex<Vec<Option<String>>>,
    }
    #[async_trait]
    impl ContactsProvider for DeltaProvider {
        async fn fetch_folders(&self) -> Result<Vec<ContactFolder>> {
            Ok(vec![ContactFolder { id: "f1".into(), display_name: "Contacts".into(), parent_folder_id: None }])
        }
        async fn fetch_folder_contacts(&self, _f: &str, _l: usize) -> Result<Vec<Contact>> { Ok(vec![]) }
        async fn fetch_all_folder_contacts(&self, _f: &str, _tx: tokio::sync::mpsc::Sender<Vec<Contact>>) -> Result<usize> { Ok(0) }
        async fn is_token_valid(&self) -> Result<bool> { Ok(true) }
        fn supports_delta(&self) -> bool { true }
        async fn fetch_delta(&self, _f: &str, link: Option<&str>, tx: tokio::sync::mpsc::Sender<ContactDeltaPage>) -> Result<String> {
            self.seen_links.lock().unwrap().push(link.map(|l| l.to_string()));
            let step = self.script.lock().unwrap().remove(0);
            match step {
                Ok((pages, new_link)) => {
                    for p in pages { let _ = tx.send(p).await; }
                    Ok(new_link)
                }
                Err(m) => Err(crate::errors::OmarchyError::HttpError(m)),
            }
        }
    }
    fn page(ids: &[&str], removed: &[&str]) -> ContactDeltaPage {
        ContactDeltaPage {
            upserts: ids.iter().map(|i| Contact { id: i.to_string(), display_name: format!("N-{}", i), folder_id: Some("f1".into()), ..Default::default() }).collect(),
            removed: removed.iter().map(|s| s.to_string()).collect(),
        }
    }
    fn delta_daemon(script: Vec<std::result::Result<(Vec<ContactDeltaPage>, String), String>>) -> (ContactsDaemon, Arc<DeltaProvider>) {
        let p = Arc::new(DeltaProvider { script: Mutex::new(script), seen_links: Mutex::new(vec![]) });
        let db = Arc::new(Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap());
        (ContactsDaemon::new(ContactsDaemonConfig::default(), db, p.clone()), p)
    }
    fn ids(d: &ContactsDaemon) -> Vec<String> {
        let mut v: Vec<String> = d.db.query_contacts_full("all", "first", None, None, None, None).unwrap().into_iter().map(|r| r.contact.id).collect();
        v.sort();
        v
    }

    #[tokio::test(flavor = "current_thread")]
    async fn first_delta_read_is_a_full_walk_that_stores_the_link_and_drops_what_it_did_not_see() {
        let (d, p) = delta_daemon(vec![Ok((vec![page(&["a", "b"], &[]), page(&["c"], &[])], "link-1".into()))]);
        d.sync_folders().await.unwrap();
        // A contact cached earlier that the server no longer has.
        d.db.upsert_contact(&Contact { id: "gone".into(), display_name: "Gone".into(), folder_id: Some("f1".into()), ..Default::default() }).unwrap();
        d.sync_contacts(false).await.unwrap();
        assert_eq!(ids(&d), vec!["a", "b", "c"], "walk stored everything and purged the unseen row");
        assert_eq!(d.db.contact_delta_link("f1").unwrap().as_deref(), Some("link-1"));
        assert_eq!(p.seen_links.lock().unwrap().clone(), vec![None]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn later_reads_use_the_link_and_apply_changes_and_removals_without_a_purge() {
        let (d, p) = delta_daemon(vec![
            Ok((vec![page(&["a", "b", "c"], &[])], "link-1".into())),
            Ok((vec![page(&["d"], &["b"])], "link-2".into())),
        ]);
        d.sync_folders().await.unwrap();
        d.sync_contacts(false).await.unwrap();
        d.sync_contacts(false).await.unwrap();
        assert_eq!(ids(&d), vec!["a", "c", "d"], "b removed, d added, a and c untouched (no purge on a delta read)");
        assert_eq!(p.seen_links.lock().unwrap().clone(), vec![None, Some("link-1".to_string())]);
        assert_eq!(d.db.contact_delta_link("f1").unwrap().as_deref(), Some("link-2"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_dead_link_falls_back_to_one_full_read_and_a_failed_full_read_is_reported() {
        let (d, p) = delta_daemon(vec![
            Ok((vec![page(&["a"], &[])], "link-1".into())),
            Err("Graph API error: 410 — SyncStateNotFound".into()),
            Ok((vec![page(&["a", "x"], &[])], "link-9".into())),
            Err("Graph API error: 503 — down".into()),
        ]);
        d.sync_folders().await.unwrap();
        d.sync_contacts(false).await.unwrap();
        d.sync_contacts(false).await.unwrap();
        assert_eq!(ids(&d), vec!["a", "x"]);
        assert_eq!(p.seen_links.lock().unwrap().clone(), vec![None, Some("link-1".to_string()), None]);
        // A forced full walk that fails keeps what is cached and says so.
        assert!(d.sync_contacts(true).await.is_err());
        assert_eq!(ids(&d), vec!["a", "x"], "a failed walk deletes nothing");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_pull_never_overwrites_or_deletes_a_row_with_a_write_in_flight() {
        let (d, _p) = delta_daemon(vec![Ok((vec![page(&["a", "b"], &["b"])], "link-1".into()))]);
        d.sync_folders().await.unwrap();
        for id in ["a", "b"] {
            d.db.write_contact_row(&Contact { id: id.into(), display_name: format!("mine-{}", id), folder_id: Some("f1".into()), ..Default::default() }, "pending", false).unwrap();
        }
        d.sync_contacts(false).await.unwrap();
        let rows = d.db.query_contacts_full("all", "first", None, None, None, None).unwrap();
        let names: Vec<_> = rows.iter().map(|r| r.contact.display_name.clone()).collect();
        assert_eq!(rows.len(), 2, "the pending rows survive the pull and the removal");
        assert!(names.iter().all(|n| n.starts_with("mine-")), "and keep the user's edit: {:?}", names);
    }
}
