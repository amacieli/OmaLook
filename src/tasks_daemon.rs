//! Background sync of To Do lists and tasks into the cache, so the Tasks view never waits on the
//! network. Same rhythm as the contacts daemon: wait for mail's priority stages, then read
//! everything every few minutes; a list is replaced only after it was read in full.

use crate::db::Database;
use crate::errors::Result;
use crate::providers::TasksProvider;
use log::{debug, error, info, warn};
use std::sync::Arc;
use std::time::Duration;

pub struct TasksDaemonConfig {
    pub poll_interval_secs: u64,
}

impl Default for TasksDaemonConfig {
    fn default() -> Self {
        Self { poll_interval_secs: 300 }
    }
}

pub struct TasksDaemon {
    config: TasksDaemonConfig,
    db: Arc<Database>,
    provider: Arc<dyn TasksProvider>,
}

impl TasksDaemon {
    pub fn new(config: TasksDaemonConfig, db: Arc<Database>, provider: Arc<dyn TasksProvider>) -> Self {
        Self { config, db, provider }
    }

    pub async fn start(&self) {
        info!("Tasks daemon started (every {}s)", self.config.poll_interval_secs);
        let acct = self.db.account_id().to_string();
        crate::sync_state::wait_gate(&acct, Duration::from_secs(60)).await;
        tokio::time::sleep(Duration::from_secs(8)).await;
        let interval = Duration::from_secs(self.config.poll_interval_secs);
        loop {
            match self.provider.is_token_valid().await {
                Ok(true) => {
                    match self.sync().await {
                        Ok(n) => {
                            if n > 0 { info!("tasks[{}]: {} changes", acct, n); crate::sync_state::bump_mail("tasks changed"); }
                            else { debug!("tasks[{}]: no changes", acct); }
                        }
                        Err(e) => error!("Failed to sync tasks for {}: {}", acct, e),
                    }
                    tokio::time::sleep(interval).await;
                }
                Ok(false) => { warn!("Tasks daemon: auth token not valid, waiting..."); tokio::time::sleep(Duration::from_secs(5)).await; }
                Err(e) => { error!("Tasks daemon: token check failed: {}", e); tokio::time::sleep(Duration::from_secs(5)).await; }
            }
        }
    }

    /// One pass: lists, then every list in full. Returns the number of changed rows. A list that
    /// cannot be read in full is left as cached and the pass reports the error.
    pub(crate) async fn sync(&self) -> Result<usize> {
        let lists = self.provider.fetch_lists().await?;
        let mut changed = self.db.replace_task_lists(&lists)?;
        let mut first_err = None;
        for l in &lists {
            match self.provider.fetch_list_tasks(&l.id).await {
                Ok(tasks) => changed += self.db.replace_list_tasks(&l.id, &tasks)?,
                Err(e) => { warn!("Tasks list '{}' not read: {}", l.name, e); first_err.get_or_insert(e); }
            }
            tokio::task::yield_now().await;
        }
        match first_err { Some(e) => Err(e), None => Ok(changed) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tasks::{Task, TaskList};
    use async_trait::async_trait;
    use std::sync::Mutex;

    struct Fake { fail_list: Mutex<Option<String>>, tasks: Mutex<Vec<Task>> }
    #[async_trait]
    impl TasksProvider for Fake {
        async fn fetch_lists(&self) -> Result<Vec<TaskList>> {
            Ok(vec![TaskList { id: "A".into(), name: "A".into(), is_default: true }, TaskList { id: "B".into(), name: "B".into(), is_default: false }])
        }
        async fn fetch_list_tasks(&self, list_id: &str) -> Result<Vec<Task>> {
            if self.fail_list.lock().unwrap().as_deref() == Some(list_id) {
                return Err(crate::errors::OmarchyError::HttpError("error sending request".into()));
            }
            Ok(self.tasks.lock().unwrap().iter().filter(|t| t.list_id == list_id).cloned().collect())
        }
        async fn is_token_valid(&self) -> Result<bool> { Ok(true) }
    }
    fn task(id: &str, list: &str) -> Task { Task { id: id.into(), list_id: list.into(), title: id.into(), importance: "normal".into(), ..Default::default() } }
    fn run<F: std::future::Future>(f: F) -> F::Output { tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f) }

    #[test]
    fn a_failed_list_keeps_its_cached_tasks_and_the_others_update() {
        let db = Arc::new(Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap());
        let fake = Arc::new(Fake { fail_list: Mutex::new(None), tasks: Mutex::new(vec![task("a1", "A"), task("b1", "B")]) });
        let d = TasksDaemon::new(TasksDaemonConfig::default(), db.clone(), fake.clone());
        assert_eq!(run(d.sync()).unwrap(), 2, "two new tasks");
        assert_eq!(db.query_tasks(None).unwrap().len(), 2);
        // offline for list B, a new task in A
        *fake.fail_list.lock().unwrap() = Some("B".into());
        fake.tasks.lock().unwrap().push(task("a2", "A"));
        assert!(run(d.sync()).is_err());
        let ids: Vec<String> = db.query_tasks(None).unwrap().into_iter().map(|r| r.id).collect();
        assert!(ids.contains(&"a2".to_string()) && ids.contains(&"b1".to_string()), "{ids:?}");
    }
}
