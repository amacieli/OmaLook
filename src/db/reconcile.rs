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
    /// on another client). Rows with an unpushed local read change are kept.
    pub fn reconcile_deleted(&self, folder_id: &str, live_ids: &[String]) -> Result<usize> {
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
                   AND id NOT IN (SELECT id FROM live_ids)",
                params![folder_id, self.account_id],
            )?)
        })();
        match result {
            Ok(n) => { self.conn.execute_batch("COMMIT")?; Ok(n) }
            Err(e) => { let _ = self.conn.execute_batch("ROLLBACK"); Err(e) }
        }
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
                for e in &page.upserts {
                    changed += up.execute(params![e.id, e.subject, e.from, e.body, e.received, e.is_read, now, e.folder_id, self.account_id,
                        e.from_name, e.to_text, e.cc_text, e.bcc_text, categories_json(&e.categories),
                        e.sent_at, e.importance, e.has_attachments, e.conversation_id, e.meta])?;
                }
            }
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
        let total: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE folder_id = ?1 AND account_id = ?2",
            params![folder_id, self.account_id], |r| r.get(0))?;
        let server_total: Option<i64> = self.conn.query_row(
            "SELECT total_item_count FROM folders WHERE id = ?1 AND account_id = ?2",
            params![folder_id, self.account_id], |r| r.get::<_, Option<i64>>(0),
        ).optional()?.flatten();
        let believable = match server_total {
            Some(t) => {
                let slack = (t / 50).max(3);
                (seen.len() as i64 - t).abs() <= slack
            }
            None => candidates.len() as i64 * 5 <= total,
        };
        if !believable {
            warn!(
                "Not purging {} of {} cached rows in folder {}: the enumeration ({} messages) does not match the provider's count ({:?})",
                candidates.len(), total, folder_id, seen.len(), server_total
            );
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
