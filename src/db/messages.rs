//! Messages: cache rows, bodies, senders, previews, attachments, invite info, and inserts.

use super::*;


/// How one sender's mail should be shown. Keyed by lower-cased address.
#[derive(Debug, Clone, PartialEq)]
pub struct SenderPref {
    pub email: String,
    /// Always render this sender's mail as HTML.
    pub always_html: bool,
    /// Always load remote images in this sender's mail.
    pub always_images: bool,
}


/// Result of `add_sender`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SenderAdd {
    Added,
    AlreadyListed,
    /// Not shaped like an address (no single `@`, spaces, empty parts).
    Invalid,
}


/// Subset of `ids` already stored in `messages` (read-only side connection, so a provider can
/// skip fetching messages it already has without holding the daemon's `Database` across awaits).
pub fn known_message_ids(path: &std::path::Path, ids: &[String]) -> std::collections::HashSet<String> {
    let mut known = std::collections::HashSet::new();
    let Ok(conn) = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) else { return known };
    for chunk in ids.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        let Ok(mut stmt) = conn.prepare(&format!("SELECT id FROM messages WHERE id IN ({})", marks)) else { continue };
        let found: Vec<String> = match stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |r| r.get::<_, String>(0)) {
            Ok(rows) => rows.flatten().collect(),
            Err(_) => Vec::new(),
        };
        known.extend(found);
    }
    known
}



/// The preview snippet of a plain-text body: whitespace collapsed, at most 255 characters
/// (what the providers' own preview fields hold).
pub fn body_snippet(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.chars().take(255).collect()
}

impl Database {

    /// Cache a message from Graph API
    pub fn cache_message(&self, msg: &Message) -> Result<()> {
        let from_email = msg.from.as_ref()
            .and_then(|r| r.email_address.as_ref())
            .map(|ea| ea.address.clone())
            .unwrap_or_default();
        
        let from_name = msg.from.as_ref()
            .and_then(|r| r.email_address.as_ref())
            .and_then(|ea| ea.name.clone());
        
        let body = msg.body.as_ref()
            .map(|b| b.content.clone())
            .unwrap_or_default();
        
        let received_at = msg.received_date_time.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.to_utc())
            .unwrap_or_else(Utc::now);
        
        // debug!("Caching message: {}", msg.id);
        
        self.conn.execute(
            "INSERT OR REPLACE INTO messages (id, subject, from_email, from_name, body, received_at, is_read, account_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                msg.id,
                msg.subject,
                from_email,
                from_name,
                body,
                received_at,
                msg.is_read.unwrap_or(false),
                self.account_id,
            ],
        )?;
        
        Ok(())
    }

    
    /// Check if email already exists in database (for deduplication)
    pub fn email_exists(&self, id: &str) -> Result<bool> {
        let mut stmt = self.conn.prepare(
            "SELECT 1 FROM messages WHERE id = ?1 LIMIT 1"
        )?;
        
        let exists = stmt.exists(params![id])?;
        
        if exists {
            // debug!("Email {} exists in database", id);
        }
        
        Ok(exists)
    }

    
    /// Get cached message by ID
    pub fn get_message(&self, id: &str) -> Result<Option<CachedMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, subject, from_email, from_name, body, received_at, is_read, cached_at
             FROM messages WHERE id = ?1"
        )?;
        
        let msg = stmt.query_row(params![id], |row| {
            Ok(CachedMessage {
                id: row.get(0)?,
                subject: row.get(1)?,
                from_email: row.get(2)?,
                from_name: row.get(3)?,
                body: row.get(4)?,
                received_at: row.get(5)?,
                is_read: row.get(6)?,
                cached_at: row.get(7)?,
            })
        }).optional()?;
        
        Ok(msg)
    }

    
    /// Get recent cached messages
    pub fn get_recent_messages(&self, limit: usize) -> Result<Vec<CachedMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, subject, from_email, from_name, body, received_at, is_read, cached_at
             FROM messages ORDER BY received_at DESC LIMIT ?1"
        )?;
        
        let messages = stmt.query_map(params![limit as i32], |row| {
            Ok(CachedMessage {
                id: row.get(0)?,
                subject: row.get(1)?,
                from_email: row.get(2)?,
                from_name: row.get(3)?,
                body: row.get(4)?,
                received_at: row.get(5)?,
                is_read: row.get(6)?,
                cached_at: row.get(7)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
        
        Ok(messages)
    }

    
    /// Mark message as read
    pub fn mark_read(&self, id: &str, is_read: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET is_read = ?1 WHERE id = ?2",
            params![is_read, id],
        )?;
        Ok(())
    }

    
    /// Local read/unread change from the UI. Flags the row so the daemon pushes it to the
    /// provider, and keeps the folder's unread count in step until the next folder sync.
    /// Returns whether the state actually changed.
    pub fn set_message_read(&self, id: &str, is_read: bool) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE messages SET is_read = ?1, read_pending = 1 WHERE id = ?2 AND is_read != ?1",
            params![is_read, id],
        )?;
        if changed > 0 {
            let delta: i32 = if is_read { -1 } else { 1 };
            self.conn.execute(
                "UPDATE folders SET unread_item_count = MAX(0, COALESCE(unread_item_count, 0) + ?1)
                 WHERE id = (SELECT folder_id FROM messages WHERE id = ?2)",
                params![delta, id],
            )?;
        }
        Ok(changed > 0)
    }


    /// The cached full body of a message as (content type "html"|"text", content).
    pub fn cached_body(&self, id: &str) -> Result<Option<(String, String)>> {
        let row = self.conn.query_row(
            "SELECT body_type, body_full FROM messages WHERE id = ?1",
            params![id],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
        );
        match row {
            Ok((Some(t), Some(c))) => Ok(Some((t, c))),
            Ok(_) => Ok(None),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }


    pub fn store_body(&self, id: &str, content_type: &str, content: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET body_type = ?1, body_full = ?2 WHERE id = ?3",
            params![content_type, content, id],
        )?;
        // A row stored without its preview snippet gets one from the body it now has.
        let missing: bool = self.conn.query_row(
            "SELECT body = '' FROM messages WHERE id = ?1", params![id], |r| r.get(0),
        ).unwrap_or(false);
        if missing {
            let text = if content_type == "html" { crate::compose::html_to_text(content) } else { content.to_string() };
            let snippet = body_snippet(&text);
            if !snippet.is_empty() {
                self.conn.execute("UPDATE messages SET body = ?1 WHERE id = ?2", params![snippet, id])?;
            }
        }
        Ok(())
    }


    /// Ids (newest first) of this account's messages received in the last `days` days whose
    /// full body is not cached yet: the daemon's prefetch queue.
    pub fn messages_needing_body(&self, days: i64, limit: usize) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM messages WHERE account_id = ?1 AND body_full IS NULL
               AND received_at >= strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?2)
             ORDER BY received_at DESC LIMIT ?3",
        )?;
        let rows = stmt
            .query_map(params![self.account_id, format!("-{} days", days), limit as i64], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }


    /// Give rows that hold a full body but no preview snippet one. Returns how many.
    pub fn fill_missing_previews(&self) -> Result<usize> {
        let rows: Vec<(String, String, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, body_type, body_full FROM messages WHERE account_id = ?1 AND body = '' AND body_full IS NOT NULL AND body_full != ''",
            )?;
            let v = stmt.query_map(params![self.account_id], |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default(), r.get(2)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            v
        };
        let mut n = 0;
        for (id, t, c) in rows {
            let text = if t == "html" { crate::compose::html_to_text(&c) } else { c };
            let snippet = body_snippet(&text);
            if !snippet.is_empty() {
                self.conn.execute("UPDATE messages SET body = ?1 WHERE id = ?2", params![snippet, id])?;
                n += 1;
            }
        }
        Ok(n)
    }


    /// Normalise an address for the sender table: trimmed, lower-cased. None if it is not
    /// shaped like `local@domain` (one `@`, no whitespace, both sides non-empty, a dot in
    /// the domain).
    pub fn normalize_sender(email: &str) -> Option<String> {
        let e = email.trim().to_lowercase();
        let (local, domain) = e.split_once('@')?;
        let ok = !local.is_empty()
            && !domain.is_empty()
            && !domain.contains('@')
            && domain.contains('.')
            && !domain.starts_with('.')
            && !domain.ends_with('.')
            && !e.chars().any(|c| c.is_whitespace());
        ok.then_some(e)
    }


    /// Add a sender with both preferences off.
    pub fn add_sender(&self, email: &str) -> Result<SenderAdd> {
        let Some(email) = Self::normalize_sender(email) else { return Ok(SenderAdd::Invalid) };
        let n = self.conn.execute("INSERT OR IGNORE INTO sender_prefs (email) VALUES (?1)", params![email])?;
        Ok(if n > 0 { SenderAdd::Added } else { SenderAdd::AlreadyListed })
    }


    /// All listed senders, alphabetical.
    pub fn list_senders(&self) -> Result<Vec<SenderPref>> {
        let mut stmt = self.conn.prepare(
            "SELECT email, always_html, always_images FROM sender_prefs ORDER BY email ASC",
        )?;
        let rows = stmt
            .query_map([], |r| Ok(SenderPref { email: r.get(0)?, always_html: r.get(1)?, always_images: r.get(2)? }))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }


    /// Set one preference (`"html"` or `"images"`) for a listed sender. Returns whether a
    /// listed sender was updated (false: unknown address or unknown field).
    pub fn set_sender_pref(&self, email: &str, field: &str, value: bool) -> Result<bool> {
        let column = match field {
            "html" => "always_html",
            "images" => "always_images",
            _ => return Ok(false),
        };
        let Some(email) = Self::normalize_sender(email) else { return Ok(false) };
        let n = self.conn.execute(
            &format!("UPDATE sender_prefs SET {} = ?1 WHERE email = ?2", column),
            params![value, email],
        )?;
        Ok(n > 0)
    }


    pub fn remove_sender(&self, email: &str) -> Result<bool> {
        let Some(email) = Self::normalize_sender(email) else { return Ok(false) };
        Ok(self.conn.execute("DELETE FROM sender_prefs WHERE email = ?1", params![email])? > 0)
    }


    /// The short preview the daemon synced (what the list row is built from).
    pub fn message_preview(&self, id: &str) -> Option<String> {
        self.conn
            .query_row("SELECT body FROM messages WHERE id = ?1", params![id], |r| r.get::<_, String>(0))
            .ok()
    }


    /// Cached invitation lookup of a message: (info JSON, unix seconds it was fetched), if any.
    pub fn invite_info(&self, message_id: &str) -> Result<Option<(String, i64)>> {
        Ok(self.conn.query_row(
            "SELECT info, checked_at FROM invite_info WHERE message_id = :id",
            rusqlite::named_params! { ":id": message_id },
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        ).optional()?)
    }


    /// Store the invitation lookup of a message (upsert, keyed by message id).
    pub fn set_invite_info(&self, message_id: &str, info_json: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO invite_info (message_id, info, checked_at) VALUES (:id, :i, :now)
             ON CONFLICT(message_id) DO UPDATE SET info = excluded.info, checked_at = excluded.checked_at",
            rusqlite::named_params! {
                ":id": message_id,
                ":i": info_json,
                ":now": Utc::now().timestamp(),
            },
        )?;
        Ok(())
    }


    /// The cached attachment list of a message, if it has been fetched.
    pub fn cached_attachments(&self, id: &str) -> Result<Option<Vec<crate::attachments::Attachment>>> {
        match self.conn.query_row("SELECT json FROM attachment_cache WHERE message_id = ?1", params![id], |r| r.get::<_, String>(0)) {
            Ok(j) => Ok(serde_json::from_str(&j).ok()),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }


    /// Remember a message's attachment list. `has_real` (any non-inline attachment) overrides the
    /// list view's coarse server hint, which counts signature logos; it lives here, not in
    /// `messages`, so a later sync cannot overwrite the correction.
    pub fn store_attachments(&self, id: &str, list: &[crate::attachments::Attachment]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO attachment_cache (message_id, json, has_real, fetched_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(message_id) DO UPDATE SET json = excluded.json, has_real = excluded.has_real, fetched_at = excluded.fetched_at",
            params![id, serde_json::to_string(list).unwrap_or_else(|_| "[]".into()), crate::attachments::any_real(list) as i32, Utc::now().timestamp()],
        )?;
        Ok(())
    }


    /// The account a cached message belongs to.
    pub fn message_account(&self, id: &str) -> Result<Option<String>> {
        match self.conn.query_row(
            "SELECT account_id FROM messages WHERE id = ?1",
            params![id],
            |r| r.get::<_, Option<String>>(0),
        ) {
            Ok(a) => Ok(a),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }


    // ───────────────────────── message details & categories ─────────────────────────

    /// Fill in the detail fields of an already stored message (the one-time repopulate for
    /// providers without a change feed). Never touches a category change that is still
    /// waiting to be pushed.
    pub fn update_message_meta(&self, e: &EmailMessage) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE messages SET from_name = ?1, to_text = ?2, cc_text = ?3, bcc_text = ?4,
                    categories = CASE WHEN cat_pending = 1 THEN categories ELSE ?5 END,
                    categories_synced = ?5, sent_at = ?6, importance = ?7, has_attachments = ?8,
                    conversation_id = ?9, meta_ok = 1
             WHERE id = ?10",
            params![e.from_name, e.to_text, e.cc_text, e.bcc_text, categories_json(&e.categories),
                    e.sent_at, e.importance, e.has_attachments, e.conversation_id, e.id],
        )?;
        Ok(n > 0)
    }


    /// Mark rows stored as complete although their details never arrived (no conversation id)
    /// as waiting for details again. Returns how many.
    pub fn requeue_sparse_meta(&self) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE messages SET meta_ok = 0 WHERE account_id = ?1 AND meta_ok = 1\n               AND (conversation_id IS NULL OR conversation_id = '')",
            params![self.account_id],
        )?)
    }


    /// Ids (newest first) of this account's messages still waiting for the detail repopulate.
    pub fn messages_needing_meta(&self, limit: usize) -> Result<Vec<(String, Option<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, folder_id FROM messages WHERE account_id = ?1 AND meta_ok = 0
             ORDER BY received_at DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![self.account_id, limit as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }


    /// How many of this account's messages still lack their details.
    pub fn meta_remaining(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE account_id = ?1 AND meta_ok = 0",
            params![self.account_id],
            |r| r.get(0),
        )?)
    }


    /// Insert a new email message (used by daemon)
    pub fn insert_email(&self, email: &EmailMessage) -> Result<()> {
        let now = Utc::now();
        // debug!("Inserting email: {}", email.id);

        self.conn.execute(
            "INSERT OR IGNORE INTO messages (id, subject, from_email, body, received_at, is_read, cached_at, folder_id, account_id,
                    from_name, to_text, cc_text, bcc_text, categories, categories_synced, sent_at, importance, has_attachments, conversation_id, meta_ok)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                email.id,
                email.subject,
                email.from,
                email.body,
                email.received,
                email.is_read,
                now,
                email.folder_id,
                self.account_id,
                email.from_name, email.to_text, email.cc_text, email.bcc_text,
                categories_json(&email.categories),
                email.sent_at, email.importance, email.has_attachments, email.conversation_id, email.meta,
            ],
        )?;

        // Backfill folder_id on existing rows that were inserted before the column existed
        if email.folder_id.is_some() {
            self.conn.execute(
                "UPDATE messages SET folder_id = ?1 WHERE id = ?2 AND folder_id IS NULL",
                params![email.folder_id, email.id],
            )?;
        }

        Ok(())
    }


    /// Insert many messages in ONE transaction (one fsync instead of one per row).
    /// Returns how many were new. Rows that already exist are left alone, except that a
    /// missing `folder_id` is backfilled (same rule as `insert_email`).
    pub fn insert_emails_batch(&self, emails: &[EmailMessage]) -> Result<usize> {
        if emails.is_empty() {
            return Ok(0);
        }
        let now = Utc::now();
        self.conn.execute_batch("BEGIN")?;
        let result = (|| -> Result<usize> {
            let mut ins = self.conn.prepare_cached(
                "INSERT OR IGNORE INTO messages (id, subject, from_email, body, received_at, is_read, cached_at, folder_id, account_id,
                        from_name, to_text, cc_text, bcc_text, categories, categories_synced, sent_at, importance, has_attachments, conversation_id, meta_ok)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14, ?15, ?16, ?17, ?18, ?19)")?;
            let mut fix = self.conn.prepare_cached(
                "UPDATE messages SET folder_id = ?1 WHERE id = ?2 AND folder_id IS NULL")?;
            let mut new = 0usize;
            for e in emails {
                let n = ins.execute(params![e.id, e.subject, e.from, e.body, e.received, e.is_read, now, e.folder_id, self.account_id,
                    e.from_name, e.to_text, e.cc_text, e.bcc_text, categories_json(&e.categories),
                    e.sent_at, e.importance, e.has_attachments, e.conversation_id, e.meta])?;
                if n > 0 {
                    new += 1;
                } else if e.folder_id.is_some() {
                    fix.execute(params![e.folder_id, e.id])?;
                }
            }
            Ok(new)
        })();
        match result {
            Ok(n) => { self.conn.execute_batch("COMMIT")?; Ok(n) }
            Err(e) => { let _ = self.conn.execute_batch("ROLLBACK"); Err(e) }
        }
    }


    /// Read flag of one cached message (None if it is not cached).
    pub fn message_is_read(&self, id: &str) -> Result<Option<bool>> {
        Ok(self.conn
            .query_row("SELECT is_read FROM messages WHERE id = ?1", params![id], |r| r.get::<_, bool>(0))
            .optional()?)
    }


    /// Get cached messages for a specific folder (by folder_id)
    pub fn get_messages_for_folder(&self, folder_id: &str, limit: usize) -> Result<Vec<CachedMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, subject, from_email, from_name, body, received_at, is_read, cached_at
             FROM messages
             WHERE folder_id = ?1
             ORDER BY received_at DESC
             LIMIT ?2",
        )?;

        let messages = stmt
            .query_map(params![folder_id, limit as i32], |row| {
                Ok(CachedMessage {
                    id:          row.get(0)?,
                    subject:     row.get(1)?,
                    from_email:  row.get(2)?,
                    from_name:   row.get(3)?,
                    body:        row.get(4)?,
                    received_at: row.get(5)?,
                    is_read:     row.get(6)?,
                    cached_at:   row.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(messages)
    }
}

/// One cached message as the MCP mail tools read it (R3.5). `body` is `None` only if the stored
/// value is NULL; the service decides whether that matters for the answer.
pub struct McpMessageRow {
    pub id: String,
    pub subject: String,
    pub from_email: String,
    pub from_name: Option<String>,
    pub received_at: String,
    pub is_read: bool,
    pub account_id: String,
    pub account_email: String,
    pub folder_id: String,
    pub importance: Option<String>,
    pub has_attachments: Option<bool>,
    pub categories: Option<String>,
    pub body: Option<String>,
    pub to_text: Option<String>,
    pub cc_text: Option<String>,
    pub bcc_text: Option<String>,
    pub sent_at: Option<String>,
    pub conversation_id: Option<String>,
}

const MCP_MSG_COLS: &str = "m.id, m.subject, m.from_email, m.from_name, m.received_at, m.is_read, COALESCE(m.account_id,''), COALESCE(a.email,''), \
    COALESCE(m.folder_id,''), m.importance, COALESCE((SELECT c.has_real FROM attachment_cache c WHERE c.message_id = m.id), m.has_attachments), \
    m.categories, m.body, m.to_text, m.cc_text, m.bcc_text, m.sent_at, m.conversation_id";

fn mcp_message_row(r: &rusqlite::Row) -> rusqlite::Result<McpMessageRow> {
    Ok(McpMessageRow {
        id: r.get(0)?, subject: r.get(1)?, from_email: r.get(2)?, from_name: r.get(3)?, received_at: r.get(4)?,
        is_read: r.get(5)?, account_id: r.get(6)?, account_email: r.get(7)?, folder_id: r.get(8)?,
        importance: r.get(9)?, has_attachments: r.get(10)?, categories: r.get(11)?, body: r.get(12)?,
        to_text: r.get(13)?, cc_text: r.get(14)?, bcc_text: r.get(15)?, sent_at: r.get(16)?, conversation_id: r.get(17)?,
    })
}

impl Database {
    /// Cached messages of the given accounts, newest first, one page (`limit + 1` rows are fetched
    /// so the caller can tell whether there is a next page). `folder` is a folder id or a kind
    /// such as "inbox", across the given accounts.
    pub fn mcp_messages(&self, accounts: &[String], folder: Option<&str>, unread_only: bool,
                        search: Option<&crate::search::Query>, limit: i64, offset: i64) -> rusqlite::Result<Vec<McpMessageRow>> {
        let mut a: Vec<rusqlite::types::Value> = vec![];
        let mut conds = vec![account_in("m.account_id", accounts, &mut a), "m.action_pending IS NULL".to_string()];
        if let Some(f) = folder {
            a.push(f.to_string().into());
            let n = a.len();
            let acc = account_in("account_id", accounts, &mut a);
            conds.push(format!("m.folder_id IN (SELECT id FROM folders WHERE (id = ?{n} OR lower(well_known_name) = lower(?{n})) AND {acc})"));
        }
        if unread_only { conds.push("m.is_read = 0".into()); }
        if let Some(q) = search { conds.extend(q.to_sql(&mut a)); }
        a.push((limit + 1).into());
        a.push(offset.into());
        let sql = format!(
            "SELECT {} FROM messages m LEFT JOIN accounts a ON a.id = m.account_id WHERE {} ORDER BY m.received_at DESC LIMIT ?{} OFFSET ?{}",
            MCP_MSG_COLS, conds.join(" AND "), a.len() - 1, a.len());
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(a.iter()), mcp_message_row)?;
        Ok(rows.flatten().collect())
    }

    /// One message by id, whatever its account (the caller checks the account).
    pub fn mcp_message(&self, id: &str) -> rusqlite::Result<Option<McpMessageRow>> {
        let sql = format!("SELECT {} FROM messages m LEFT JOIN accounts a ON a.id = m.account_id WHERE m.id = ?1", MCP_MSG_COLS);
        self.conn.query_row(&sql, [id], mcp_message_row).optional()
    }

    /// Account of a message that is not waiting on a queued provider action.
    pub fn mcp_message_account(&self, id: &str) -> rusqlite::Result<Option<String>> {
        self.conn.query_row("SELECT COALESCE(account_id, '') FROM messages WHERE id = ?1 AND action_pending IS NULL", [id], |r| r.get(0)).optional()
    }

    /// Stored preview text of a message.
    pub fn mcp_message_body(&self, id: &str) -> rusqlite::Result<Option<String>> {
        self.conn.query_row("SELECT body FROM messages WHERE id = ?1", [id], |r| r.get(0)).optional()
    }
}
