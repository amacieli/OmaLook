//! Cached To Do lists and tasks (read side). The tasks daemon fills these in the background; the
//! UI only ever reads them, so the Tasks view opens at once and works offline.

use super::Database;
use crate::errors::Result;
use rusqlite::params;
use serde::Serialize;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TaskList {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Task {
    pub id: String,
    pub list_id: String,
    pub title: String,
    pub notes: String,
    pub completed: bool,
    /// Due date as `YYYY-MM-DD`, or empty.
    pub due: String,
    /// "low" | "normal" | "high".
    pub importance: String,
    pub modified_at: String,
    pub completed_at: String,
}

/// What `GET /tasks` returns per task.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TaskRow {
    pub id: String,
    pub title: String,
    pub notes: String,
    pub completed: bool,
    pub due: String,
    pub importance: String,
    pub list: String,
    pub list_id: String,
    pub account_id: String,
}

impl Database {
    pub(crate) fn init_tasks_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS task_lists (
                id TEXT PRIMARY KEY,
                account_id TEXT NOT NULL,
                name TEXT NOT NULL DEFAULT '',
                is_default INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                account_id TEXT NOT NULL,
                list_id TEXT NOT NULL,
                title TEXT NOT NULL DEFAULT '',
                notes TEXT NOT NULL DEFAULT '',
                completed INTEGER NOT NULL DEFAULT 0,
                due TEXT NOT NULL DEFAULT '',
                importance TEXT NOT NULL DEFAULT 'normal',
                modified_at TEXT NOT NULL DEFAULT '',
                completed_at TEXT NOT NULL DEFAULT '',
                cached_at DATETIME DEFAULT CURRENT_TIMESTAMP
             );
             CREATE INDEX IF NOT EXISTS idx_tasks_account_list ON tasks(account_id, list_id);",
        )?;
        Ok(())
    }

    /// Replace the account's list set with `lists` (the provider's complete answer): new and renamed
    /// lists are stored, lists no longer there are dropped with their tasks.
    pub fn replace_task_lists(&self, lists: &[TaskList]) -> Result<usize> {
        let keep: HashSet<&str> = lists.iter().map(|l| l.id.as_str()).collect();
        for l in lists {
            self.conn.execute(
                "INSERT INTO task_lists (id, account_id, name, is_default) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET name = excluded.name, is_default = excluded.is_default",
                params![l.id, self.account_id, l.name, l.is_default],
            )?;
        }
        let old: Vec<String> = {
            let mut stmt = self.conn.prepare("SELECT id FROM task_lists WHERE account_id = ?1")?;
            let v = stmt.query_map(params![self.account_id], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            v
        };
        let mut gone = 0;
        for id in old.into_iter().filter(|i| !keep.contains(i.as_str())) {
            self.conn.execute("DELETE FROM tasks WHERE account_id = ?1 AND list_id = ?2", params![self.account_id, id])?;
            self.conn.execute("DELETE FROM task_lists WHERE id = ?1 AND account_id = ?2", params![id, self.account_id])?;
            gone += 1;
        }
        Ok(gone)
    }

    pub fn task_lists(&self) -> Result<Vec<TaskList>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, is_default FROM task_lists WHERE account_id = ?1 ORDER BY is_default DESC, name")?;
        let rows = stmt.query_map(params![self.account_id], |r| Ok(TaskList { id: r.get(0)?, name: r.get(1)?, is_default: r.get(2)? }))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Make the cached tasks of `list_id` exactly `tasks` (a COMPLETE read of the list).
    /// Returns how many rows changed (added, edited or removed).
    pub fn replace_list_tasks(&self, list_id: &str, tasks: &[Task]) -> Result<usize> {
        let mut changed = 0;
        let keep: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        for t in tasks {
            changed += self.conn.execute(
                "INSERT INTO tasks (id, account_id, list_id, title, notes, completed, due, importance, modified_at, completed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(id) DO UPDATE SET list_id = excluded.list_id, title = excluded.title, notes = excluded.notes,
                   completed = excluded.completed, due = excluded.due, importance = excluded.importance,
                   modified_at = excluded.modified_at, completed_at = excluded.completed_at
                 WHERE tasks.modified_at != excluded.modified_at OR tasks.list_id != excluded.list_id
                    OR tasks.completed != excluded.completed",
                params![t.id, self.account_id, list_id, t.title, t.notes, t.completed, t.due, t.importance, t.modified_at, t.completed_at],
            )?;
        }
        let old: Vec<String> = {
            let mut stmt = self.conn.prepare("SELECT id FROM tasks WHERE account_id = ?1 AND list_id = ?2")?;
            let v = stmt.query_map(params![self.account_id, list_id], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            v
        };
        for id in old.into_iter().filter(|i| !keep.contains(i.as_str())) {
            changed += self.conn.execute("DELETE FROM tasks WHERE id = ?1 AND account_id = ?2", params![id, self.account_id])?;
        }
        Ok(changed)
    }

    /// Cached tasks of every account (or of `account`), open ones first, by due date.
    pub fn query_tasks(&self, account: Option<&str>) -> Result<Vec<TaskRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.title, t.notes, t.completed, t.due, t.importance, COALESCE(l.name, ''), t.list_id, t.account_id
               FROM tasks t LEFT JOIN task_lists l ON l.id = t.list_id AND l.account_id = t.account_id
              WHERE (?1 IS NULL OR t.account_id = ?1)
              ORDER BY t.completed, (t.due = ''), t.due, t.title COLLATE NOCASE")?;
        let rows = stmt.query_map(params![account], |r| Ok(TaskRow {
            id: r.get(0)?, title: r.get(1)?, notes: r.get(2)?, completed: r.get(3)?, due: r.get(4)?,
            importance: r.get(5)?, list: r.get(6)?, list_id: r.get(7)?, account_id: r.get(8)?,
        }))?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str, title: &str, done: bool, modified: &str) -> Task {
        Task { id: id.into(), title: title.into(), completed: done, modified_at: modified.into(), importance: "normal".into(), ..Default::default() }
    }

    #[test]
    fn a_list_walk_adds_edits_and_removes() {
        let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
        db.replace_task_lists(&[TaskList { id: "L".into(), name: "Tasks".into(), is_default: true }]).unwrap();
        assert_eq!(db.replace_list_tasks("L", &[t("1", "one", false, "a"), t("2", "two", false, "a")]).unwrap(), 2);
        assert_eq!(db.replace_list_tasks("L", &[t("1", "one", false, "a"), t("2", "two", false, "a")]).unwrap(), 0, "unchanged: no writes");
        // 1 completed elsewhere, 2 deleted, 3 new
        assert_eq!(db.replace_list_tasks("L", &[t("1", "one", true, "b"), t("3", "three", false, "b")]).unwrap(), 3);
        let rows = db.query_tasks(None).unwrap();
        let ids: Vec<_> = rows.iter().map(|r| (r.id.as_str(), r.completed)).collect();
        assert_eq!(ids, vec![("3", false), ("1", true)], "open first");
        assert_eq!(rows[0].list, "Tasks");
    }

    #[test]
    fn a_list_removed_elsewhere_takes_its_tasks_and_other_accounts_are_untouched() {
        let a = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
        a.replace_task_lists(&[TaskList { id: "L1".into(), name: "A".into(), is_default: true }, TaskList { id: "L2".into(), name: "B".into(), is_default: false }]).unwrap();
        a.replace_list_tasks("L1", &[t("1", "x", false, "a")]).unwrap();
        a.replace_list_tasks("L2", &[t("2", "y", false, "a")]).unwrap();
        assert_eq!(a.replace_task_lists(&[TaskList { id: "L1".into(), name: "A".into(), is_default: true }]).unwrap(), 1);
        let rows = a.query_tasks(Some("exchange-aaaaaa")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "1");
    }
}
