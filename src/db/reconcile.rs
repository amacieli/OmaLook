//! Reconcile and change feed: delta state, delta pages, purge, read-state reconciliation, cleanup.

use super::*;

impl Database {

    /// Forget every folder's change-feed position so the next sync walks each folder again
    /// (the stored links carry the old field list, so only a fresh walk brings the new fields).
    pub fn reset_all_delta_states(&self) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM mail_delta WHERE account_id = ?1", params![self.account_id])?)
    }


    /// One-time markers per account (e.g. "the change feeds were reset for the new fields").
    pub fn flag_is_set(&self, key: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM account_flags WHERE account_id = ?1 AND key = ?2)",
            params![self.account_id, key],
            |r| r.get::<_, bool>(0),
        )?)
    }


    pub fn set_flag(&self, key: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO account_flags (account_id, key) VALUES (?1, ?2)",
            params![self.account_id, key],
        )?;
        Ok(())
    }


    /// Folders whose change-feed walk has not finished (never started, or still paging).
    pub fn delta_walk_incomplete(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM folders f
             LEFT JOIN mail_delta d ON d.account_id = f.account_id AND d.folder_id = f.id
             WHERE f.account_id = ?1 AND (d.folder_id IS NULL OR d.complete = 0)",
            params![self.account_id],
            |r| r.get(0),
        )?)
    }


    /// Make the cached read flags of one folder match the provider's list of unread ids.
    /// Rows with an unpushed local change are left alone. Returns rows changed.
    pub fn reconcile_read_state(&self, folder_id: &str, unread_ids: &[String]) -> Result<usize> {
        self.conn.execute_batch("BEGIN")?;
        let result = (|| -> Result<usize> {
            self.conn.execute_batch(
                "CREATE TEMP TABLE IF NOT EXISTS unread_ids (id TEXT PRIMARY KEY); DELETE FROM unread_ids;",
            )?;
            {
                let mut ins = self.conn.prepare("INSERT OR IGNORE INTO unread_ids (id) VALUES (?1)")?;
                for id in unread_ids {
                    ins.execute(params![id])?;
                }
            }
            let n = self.conn.execute(
                "UPDATE messages
                 SET is_read = (id NOT IN (SELECT id FROM unread_ids))
                 WHERE folder_id = ?1 AND read_pending = 0
                   AND is_read != (id NOT IN (SELECT id FROM unread_ids))",
                params![folder_id],
            )?;
            Ok(n)
        })();
        match result {
            Ok(n) => { self.conn.execute_batch("COMMIT")?; Ok(n) }
            Err(e) => { let _ = self.conn.execute_batch("ROLLBACK"); Err(e) }
        }
    }


    /// Remove cached messages of `folder_id` that the provider no longer lists (deleted or moved
    /// on another client, for a non-delta provider such as Gmail). `live_ids` must be a COMPLETE
    /// listing taken after `started`: only rows cached before `started` are candidates, so mail
    /// that arrived while the listing ran is never removed. Rows with an unpushed local read
    /// change are kept. The same believability rule as `purge_unseen` decides (see
    /// `deletion_is_believable`). Returns rows deleted.
    pub fn reconcile_deleted(
        &self,
        folder_id: &str,
        live_ids: &[String],
        started: chrono::DateTime<Utc>,
    ) -> Result<usize> {
        let live: std::collections::HashSet<&String> = live_ids.iter().collect();
        let candidates: Vec<String> = {
            let mut st = self.conn.prepare(
                "SELECT id FROM messages WHERE folder_id = ?1 AND account_id = ?2 AND cached_at < ?3 AND read_pending = 0")?;
            let rows = st.query_map(params![folder_id, self.account_id, started], |r| r.get::<_, String>(0))?;
            rows.filter_map(|r| r.ok()).filter(|id| !live.contains(id)).collect()
        };
        if candidates.is_empty() {
            return Ok(0);
        }
        if !self.purge_is_believable(folder_id, candidates.len(), live_ids.len())? {
            return Ok(0);
        }
        self.conn.execute_batch("BEGIN")?;
        let result = (|| -> Result<usize> {
            self.conn.execute_batch(
                "CREATE TEMP TABLE IF NOT EXISTS live_ids (id TEXT PRIMARY KEY); DELETE FROM live_ids;",
            )?;
            {
                let mut ins = self.conn.prepare("INSERT OR IGNORE INTO live_ids (id) VALUES (?1)")?;
                for id in live_ids {
                    ins.execute(params![id])?;
                }
            }
            Ok(self.conn.execute(
                "DELETE FROM messages WHERE folder_id = ?1 AND account_id = ?2 AND read_pending = 0
                   AND cached_at < ?3 AND id NOT IN (SELECT id FROM live_ids)",
                params![folder_id, self.account_id, started],
            )?)
        })();
        match result {
            Ok(n) => { self.conn.execute_batch("COMMIT")?; Ok(n) }
            Err(e) => { let _ = self.conn.execute_batch("ROLLBACK"); Err(e) }
        }
    }

    /// The one decision for every destructive "the server no longer lists it" delete (Graph purge
    /// after a complete walk, Gmail reconcile): `candidates` rows of `folder_id` would go, out of
    /// `total` cached rows, given `listed` ids from a complete listing. Logs and refuses when the
    /// listing is not believable. Returns true when the delete may proceed.
    fn purge_is_believable(&self, folder_id: &str, candidates: usize, listed: usize) -> Result<bool> {
        let total: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE folder_id = ?1 AND account_id = ?2",
            params![folder_id, self.account_id], |r| r.get(0))?;
        let server_total: Option<i64> = self.conn.query_row(
            "SELECT total_item_count FROM folders WHERE id = ?1 AND account_id = ?2",
            params![folder_id, self.account_id], |r| r.get::<_, Option<i64>>(0),
        ).optional()?.flatten();
        let ok = deletion_is_believable(total as usize, candidates, listed, server_total);
        if !ok {
            warn!(
                "Not purging {} of {} cached rows in folder {}: the enumeration ({} messages) does not match the provider's count ({:?})",
                candidates, total, folder_id, listed, server_total
            );
        }
        Ok(ok)
    }


    /// Delete old messages (retention policy)
    pub fn cleanup_old_messages(&self, days: i32) -> Result<usize> {
        let count = self.conn.execute(
            "DELETE FROM messages WHERE received_at < datetime('now', ?1)",
            params![format!("-{} days", days)],
        )?;
        
        info!("Cleaned up {} old messages", count);
        Ok(count)
    }

    
    /// Get unread count
    pub fn get_unread_count(&self) -> Result<usize> {
        let mut stmt = self.conn.prepare(
            "SELECT COUNT(*) FROM messages WHERE is_read = 0"
        )?;
        
        let count: i32 = stmt.query_row([], |row| row.get(0))?;
        Ok(count as usize)
    }


    // ───────────────────────────── change feed (Graph delta) ─────────────────────────────

    /// Stored change-feed position of a folder: (link, complete).
    pub fn delta_state(&self, folder_id: &str) -> Result<Option<(String, bool)>> {
        match self.conn.query_row(
            "SELECT link, complete FROM mail_delta WHERE account_id = ?1 AND folder_id = ?2",
            params![self.account_id, folder_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0)),
        ) {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }


    pub fn set_delta_state(&self, folder_id: &str, link: &str, complete: bool) -> Result<()> {
        self.conn.execute(
            "INSERT INTO mail_delta (account_id, folder_id, link, complete, updated_at)
             VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)
             ON CONFLICT(account_id, folder_id) DO UPDATE SET link = ?3, complete = ?4, updated_at = CURRENT_TIMESTAMP",
            params![self.account_id, folder_id, link, complete as i64],
        )?;
        Ok(())
    }


    pub fn clear_delta_state(&self, folder_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM mail_delta WHERE account_id = ?1 AND folder_id = ?2",
            params![self.account_id, folder_id],
        )?;
        Ok(())
    }


    // ───────────────────────────── Gmail history position ─────────────────────────────

    /// The mailbox historyId the account's changes were applied up to; None = never seeded.
    pub fn history_position(&self) -> Result<Option<String>> {
        match self.conn.query_row(
            "SELECT history_id FROM mail_history WHERE account_id = ?1",
            params![self.account_id],
            |r| r.get::<_, String>(0),
        ) {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Apply the changes of one history walk in ONE transaction, and move the position to
    /// `position` in the same transaction (None keeps it). `upserts` are full message details
    /// (folder already chosen by the provider); `removed` are scoped ids to delete from every
    /// folder. Explicit server deletions, so no purge guard applies. Returns (rows changed, rows
    /// removed).
    pub fn apply_history(
        &self,
        upserts: &[crate::models::EmailMessage],
        removed: &[String],
        position: Option<&str>,
    ) -> Result<(usize, usize)> {
        self.conn.execute_batch("BEGIN")?;
        let result = (|| -> Result<(usize, usize)> {
            let changed = self.upsert_messages(upserts)?;
            let mut deleted = 0usize;
            {
                let mut del = self.conn.prepare_cached("DELETE FROM messages WHERE id = ?1 AND account_id = ?2")?;
                for id in removed {
                    deleted += del.execute(params![id, self.account_id])?;
                }
            }
            if let Some(p) = position {
                self.conn.execute(
                    "INSERT INTO mail_history (account_id, history_id, updated_at) VALUES (?1, ?2, CURRENT_TIMESTAMP)
                     ON CONFLICT(account_id) DO UPDATE SET history_id = ?2, updated_at = CURRENT_TIMESTAMP",
                    params![self.account_id, p],
                )?;
            }
            Ok((changed, deleted))
        })();
        match result {
            Ok(v) => { self.conn.execute_batch("COMMIT")?; Ok(v) }
            Err(e) => { let _ = self.conn.execute_batch("ROLLBACK"); Err(e) }
        }
    }

    /// Record the position a seeding walk started from (no rows touched).
    pub fn set_history_position(&self, history_id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO mail_history (account_id, history_id, updated_at) VALUES (?1, ?2, CURRENT_TIMESTAMP)
             ON CONFLICT(account_id) DO UPDATE SET history_id = ?2, updated_at = CURRENT_TIMESTAMP",
            params![self.account_id, history_id],
        )?;
        Ok(())
    }

    /// Insert or update message rows (shared by delta pages and history applies). A message keeps
    /// an unpushed local read change; folder, details and categories follow the provider.
    /// Returns rows inserted or changed.
    fn upsert_messages(&self, upserts: &[crate::models::EmailMessage]) -> Result<usize> {
        let now = Utc::now();
        let mut changed = 0usize;
        {
                let mut up = self.conn.prepare_cached(
                    "INSERT INTO messages (id, subject, from_email, body, received_at, is_read, cached_at, folder_id, account_id,
                            from_name, to_text, cc_text, bcc_text, categories, categories_synced, sent_at, importance, has_attachments, conversation_id, meta_ok)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14, ?15, ?16, ?17, ?18, ?19)
                     ON CONFLICT(id) DO UPDATE SET
                        is_read   = CASE WHEN messages.read_pending = 1 THEN messages.is_read ELSE excluded.is_read END,
                        folder_id = excluded.folder_id,
                        -- details: when the feed carries them (meta_ok), take them; a local category change
                        -- that is not pushed yet keeps its own value
                        from_name = CASE WHEN excluded.meta_ok = 1 THEN excluded.from_name ELSE messages.from_name END,
                        to_text   = CASE WHEN excluded.meta_ok = 1 THEN excluded.to_text ELSE messages.to_text END,
                        cc_text   = CASE WHEN excluded.meta_ok = 1 THEN excluded.cc_text ELSE messages.cc_text END,
                        bcc_text  = CASE WHEN excluded.meta_ok = 1 THEN excluded.bcc_text ELSE messages.bcc_text END,
                        sent_at   = CASE WHEN excluded.meta_ok = 1 THEN excluded.sent_at ELSE messages.sent_at END,
                        importance = CASE WHEN excluded.meta_ok = 1 THEN excluded.importance ELSE messages.importance END,
                        has_attachments = CASE WHEN excluded.meta_ok = 1 THEN excluded.has_attachments ELSE messages.has_attachments END,
                        conversation_id = CASE WHEN excluded.meta_ok = 1 THEN excluded.conversation_id ELSE messages.conversation_id END,
                        categories = CASE WHEN excluded.meta_ok = 1 AND messages.cat_pending = 0 THEN excluded.categories ELSE messages.categories END,
                        categories_synced = CASE WHEN excluded.meta_ok = 1 THEN excluded.categories ELSE messages.categories_synced END,
                        meta_ok   = CASE WHEN excluded.meta_ok = 1 THEN 1 ELSE messages.meta_ok END
                     WHERE (messages.read_pending = 0 AND messages.is_read != excluded.is_read)
                        OR messages.folder_id IS NOT excluded.folder_id
                        OR (excluded.meta_ok = 1 AND messages.meta_ok = 0)
                        OR (excluded.meta_ok = 1 AND messages.categories_synced IS NOT excluded.categories)")?;
                for e in upserts {
                    changed += up.execute(params![e.id, e.subject, e.from, e.body, e.received, e.is_read, now, e.folder_id, self.account_id,
                        e.from_name, e.to_text, e.cc_text, e.bcc_text, categories_json(&e.categories),
                        e.sent_at, e.importance, e.has_attachments, e.conversation_id, e.meta])?;
                }
        }
        Ok(changed)
    }

    /// Apply one change-feed page in ONE transaction: insert new messages, update the read flag
    /// and folder of existing ones (a message with an unpushed local read change keeps its local
    /// flag), delete removed ones from `folder_id`. `progress` (the page's next link) is stored
    /// in the same transaction so an interrupted first enumeration resumes where it stopped.
    /// Returns (rows inserted or changed, rows deleted).
    pub fn apply_delta_page(
        &self,
        folder_id: &str,
        page: &crate::models::DeltaPage,
        progress: Option<&str>,
    ) -> Result<(usize, usize)> {
        self.conn.execute_batch("BEGIN")?;
        let result = (|| -> Result<(usize, usize)> {
            let changed = self.upsert_messages(&page.upserts)?;
            let mut deleted = 0usize;
            {
                // Only from THIS folder: a message that was moved keeps its row via the other folder's feed.
                let mut del = self.conn.prepare_cached("DELETE FROM messages WHERE id = ?1 AND folder_id = ?2")?;
                for id in &page.removed {
                    deleted += del.execute(params![id, folder_id])?;
                }
            }
            if let Some(link) = progress {
                self.conn.execute(
                    "INSERT INTO mail_delta (account_id, folder_id, link, complete, updated_at)
                     VALUES (?1, ?2, ?3, 0, CURRENT_TIMESTAMP)
                     ON CONFLICT(account_id, folder_id) DO UPDATE SET link = ?3, complete = 0, updated_at = CURRENT_TIMESTAMP",
                    params![self.account_id, folder_id, link],
                )?;
            }
            Ok((changed, deleted))
        })();
        match result {
            Ok(v) => { self.conn.execute_batch("COMMIT")?; Ok(v) }
            Err(e) => { let _ = self.conn.execute_batch("ROLLBACK"); Err(e) }
        }
    }


    /// After a COMPLETE full enumeration of a folder: drop cached rows the provider no longer
    /// has (deleted long ago, before change tracking existed). `seen` = every id the
    /// enumeration returned; `started` = when it began, so rows stored since (e.g. by the
    /// recent-50 fetch) are never touched.
    ///
    /// Safety: the enumeration must be believable. When the provider's own item count for the
    /// folder is known, `seen` has to match it (within 2%, minimum 3: mail arrives and goes
    /// while the walk runs); that is what lets a mostly-emptied folder such as Deleted Items be
    /// cleaned. With no count to compare against, it refuses to delete more than 20% of the
    /// folder. Returns rows deleted.
    pub fn purge_unseen(
        &self,
        folder_id: &str,
        seen: &std::collections::HashSet<String>,
        started: chrono::DateTime<Utc>,
    ) -> Result<usize> {
        if seen.is_empty() {
            return Ok(0);
        }
        let candidates: Vec<String> = {
            let mut st = self.conn.prepare(
                "SELECT id FROM messages WHERE folder_id = ?1 AND account_id = ?2 AND cached_at < ?3")?;
            let rows = st.query_map(params![folder_id, self.account_id, started], |r| r.get::<_, String>(0))?;
            rows.filter_map(|r| r.ok()).filter(|id| !seen.contains(id)).collect()
        };
        if !self.purge_is_believable(folder_id, candidates.len(), seen.len())? {
            return Ok(0);
        }
        // Chunked: each delete also removes the row from the FTS index, which is not cheap.
        let mut deleted = 0;
        for chunk in candidates.chunks(200) {
            self.conn.execute_batch("BEGIN")?;
            let r = (|| -> Result<usize> {
                let mut n = 0;
                let mut del = self.conn.prepare_cached("DELETE FROM messages WHERE id = ?1")?;
                for id in chunk {
                    n += del.execute(params![id])?;
                }
                Ok(n)
            })();
            match r {
                Ok(n) => { self.conn.execute_batch("COMMIT")?; deleted += n; }
                Err(e) => { let _ = self.conn.execute_batch("ROLLBACK"); return Err(e); }
            }
        }
        Ok(deleted)
    }
}

/// Provider-neutral sanity check before deleting cached rows the server no longer lists.
/// An empty listing never deletes. With the provider's own item count for the folder, the
/// listing has to match it (within 2%, minimum 3: mail arrives and goes while a walk runs), which
/// is what lets a mostly-emptied folder such as Deleted Items be cleaned. With no count, at most
/// 20% of the cached rows may go. Shared by the Graph purge (`purge_unseen`) and the Gmail
/// reconcile (`reconcile_deleted`); there is no other copy of this rule.
pub(crate) fn deletion_is_believable(cached: usize, candidates: usize, listed: usize, server_total: Option<i64>) -> bool {
    if listed == 0 {
        return false;
    }
    match server_total {
        Some(t) => (listed as i64 - t).abs() <= (t / 50).max(3),
        None => candidates * 5 <= cached,
    }
}

#[cfg(test)]
mod purge_rule_tests {
    use super::*;
    use crate::models::EmailMessage;

    #[test]
    fn empty_listing_is_never_believable() {
        assert!(!deletion_is_believable(200, 200, 0, None));
        assert!(!deletion_is_believable(200, 1, 0, Some(0)));
    }

    #[test]
    fn with_a_server_count_the_listing_must_match_it() {
        assert!(deletion_is_believable(10, 7, 3, Some(3)), "mostly emptied folder, walk matches the count");
        assert!(deletion_is_believable(100, 1, 100, Some(101)), "within max(3, 2%)");
        assert!(!deletion_is_believable(10, 1, 1, Some(100)), "short enumeration is not trusted");
    }

    #[test]
    fn without_a_count_at_most_a_fifth_may_go() {
        assert!(deletion_is_believable(10, 2, 8, None));
        assert!(!deletion_is_believable(10, 3, 7, None));
    }

    fn msg(id: &str) -> EmailMessage {
        EmailMessage { id: id.into(), from: "a@b.c".into(), subject: "s".into(), received: "2026-10-06T12:00:00Z".into(),
                       body: "p".into(), folder_id: Some("F".into()), is_read: false, ..Default::default() }
    }

    fn cached(db: &Database) -> usize { db.get_messages_for_folder("F", 10_000).unwrap().len() }

    #[test]
    fn reconcile_never_removes_rows_cached_after_the_listing_started() {
        let db = Database::open_for_account(":memory:", "gmail-t1").unwrap();
        for i in 0..10 { db.insert_email(&msg(&format!("m{i}"))).unwrap(); }
        std::thread::sleep(std::time::Duration::from_millis(5));
        let started = Utc::now();
        db.insert_email(&msg("late")).unwrap(); // arrived while the listing was running
        let live: Vec<String> = (0..9).map(|i| format!("m{i}")).collect();
        assert_eq!(db.reconcile_deleted("F", &live, started).unwrap(), 1, "only m9 is an old, unlisted row");
        assert_eq!(cached(&db), 10);
        assert!(db.email_exists("late").unwrap());
    }

    #[test]
    fn reconcile_and_purge_refuse_the_same_listings() {
        let db = Database::open_for_account(":memory:", "gmail-t1").unwrap();
        for i in 0..10 { db.insert_email(&msg(&format!("m{i}"))).unwrap(); }
        std::thread::sleep(std::time::Duration::from_millis(5));
        let started = Utc::now();
        assert_eq!(db.reconcile_deleted("F", &["m0".to_string()], started).unwrap(), 0, "9 of 10 unlisted: over the cap");
        let seen: std::collections::HashSet<String> = ["m0".to_string()].into_iter().collect();
        assert_eq!(db.purge_unseen("F", &seen, started).unwrap(), 0);
        assert_eq!(cached(&db), 10);
    }
}
