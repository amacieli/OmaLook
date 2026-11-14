//! Message categories and category definitions (provider push queue).

use super::*;


/// `(id, wanted names, names the provider last had)`.
pub type CategoryPush = (String, Vec<String>, Vec<String>);

/// `["A","B"]` as stored in `messages.categories`; `None` stays NULL (not fetched).
pub fn categories_json(c: &Option<Vec<String>>) -> Option<String> {
    c.as_ref().map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".into()))
}


/// Inverse of `categories_json` (NULL or garbage -> empty).
pub fn categories_from_json(s: Option<&str>) -> Vec<String> {
    s.and_then(|t| serde_json::from_str::<Vec<String>>(t).ok()).unwrap_or_default()
}

impl Database {

    /// Add or remove `name` on each message (local edit, pushed to the provider later).
    /// Returns how many messages changed.
    pub fn change_message_categories(&self, ids: &[String], add: bool, name: &str) -> Result<usize> {
        let mut changed = 0;
        for id in ids {
            // Compare-and-swap: two runs (other rules, other connections) may tag the same message at the same
            // time, so the write only lands if the categories are still what we read; otherwise read again.
            for _ in 0..8 {
                let cur: Option<Option<String>> = self.conn
                    .query_row("SELECT categories FROM messages WHERE id = ?1", params![id], |r| r.get::<_, Option<String>>(0))
                    .optional()?;
                let Some(cur) = cur else { break };
                let mut list = categories_from_json(cur.as_deref());
                let had = list.iter().any(|c| c.eq_ignore_ascii_case(name));
                if add && !had { list.push(name.to_string()); }
                else if !add && had { list.retain(|c| !c.eq_ignore_ascii_case(name)); }
                else { break }
                let n = self.conn.execute(
                    "UPDATE messages SET categories = ?1, cat_pending = 1 WHERE id = ?2 AND categories IS ?3",
                    params![serde_json::to_string(&list).unwrap_or_else(|_| "[]".into()), id, cur],
                )?;
                if n > 0 { changed += 1; break }
            }
        }
        Ok(changed)
    }


    /// Category changes not yet pushed: (id, wanted names, names the provider last had).
    pub fn pending_category_pushes(&self) -> Result<Vec<CategoryPush>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, categories, categories_synced FROM messages WHERE cat_pending = 1 AND account_id = ?1",
        )?;
        let rows = stmt
            .query_map(params![self.account_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .map(|(id, want, had)| (id, categories_from_json(want.as_deref()), categories_from_json(had.as_deref())))
            .collect())
    }


    /// The provider took `pushed` for this message. The change stays pending if the user has
    /// edited the categories again in the meantime.
    pub fn finish_category_push(&self, id: &str, pushed: &[String]) -> Result<()> {
        let json = serde_json::to_string(pushed).unwrap_or_else(|_| "[]".into());
        self.conn.execute(
            "UPDATE messages SET categories_synced = ?1,
                    cat_pending = CASE WHEN categories = ?1 THEN 0 ELSE 1 END
             WHERE id = ?2",
            params![json, id],
        )?;
        Ok(())
    }


    // ───────────────────────── category definitions ─────────────────────────

    /// Replace this account's provider-known categories with `defs` (those created here and
    /// not yet on the provider are kept).
    pub fn replace_category_defs(&self, defs: &[crate::models::CategoryDef]) -> Result<()> {
        self.conn.execute_batch("BEGIN")?;
        let r = (|| -> Result<()> {
            // Rows being deleted here stay until the provider delete is done; a colour chosen here and
            // not pushed yet is not overwritten by what the provider still says.
            self.conn.execute("DELETE FROM category_defs WHERE account_id = ?1 AND pending = 0 AND delete_pending = 0 AND color_push = 0", params![self.account_id])?;
            for d in defs {
                self.conn.execute(
                    "INSERT INTO category_defs (account_id, name, color, provider_color, pending) VALUES (?1, ?2, ?3, ?4, 0)
                     ON CONFLICT(account_id, name) DO UPDATE SET
                        color = CASE WHEN color_push = 1 THEN color ELSE ?3 END, provider_color = ?4, pending = 0",
                    params![self.account_id, d.name, d.color, d.provider_color],
                )?;
            }
            Ok(())
        })();
        match r {
            Ok(()) => { self.conn.execute_batch("COMMIT")?; Ok(()) }
            Err(e) => { let _ = self.conn.execute_batch("ROLLBACK"); Err(e) }
        }
    }


    /// A category made in the UI: usable at once, created on the provider by the daemon.
    pub fn add_pending_category(&self, name: &str, color_hex: &str) -> Result<bool> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO category_defs (account_id, name, color, provider_color, pending) VALUES (?1, ?2, ?3, NULL, 1)",
            params![self.account_id, name, color_hex],
        )?;
        Ok(n > 0)
    }


    pub fn pending_categories(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare("SELECT name, color FROM category_defs WHERE account_id = ?1 AND pending = 1")?;
        let rows = stmt
            .query_map(params![self.account_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }


    pub fn finish_pending_category(&self, name: &str, provider_color: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE category_defs SET pending = 0, provider_color = ?1 WHERE account_id = ?2 AND name = ?3",
            params![provider_color, self.account_id, name],
        )?;
        Ok(())
    }


    /// The provider refused a category change for good: show what it has again.
    pub fn revert_category_push(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET categories = categories_synced, cat_pending = 0 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }


    /// Remember whether this account's category list could be read (Exchange needs the
    /// MailboxSettings permission, which an account only has after signing in again).
    pub fn set_master_list_available(&self, available: bool) -> Result<()> {
        self.conn.execute(
            "INSERT INTO category_state (account_id, master_list) VALUES (?1, ?2)
             ON CONFLICT(account_id) DO UPDATE SET master_list = ?2",
            params![self.account_id, available as i64],
        )?;
        Ok(())
    }
}

/// One category definition of an account (R3.5 MCP read).
pub struct McpCategoryRow {
    pub account_id: String,
    pub name: String,
    pub color: String,
}

impl Database {
    /// Category definitions of the given accounts.
    pub fn mcp_category_defs(&self, accounts: &[String]) -> rusqlite::Result<Vec<McpCategoryRow>> {
        let mut a: Vec<rusqlite::types::Value> = vec![];
        let cond = account_in("account_id", accounts, &mut a);
        let mut st = self.conn.prepare(&format!("SELECT account_id, name, color FROM category_defs WHERE {} ORDER BY account_id, lower(name)", cond))?;
        let rows = st.query_map(rusqlite::params_from_iter(a.iter()), |r| Ok(McpCategoryRow { account_id: r.get(0)?, name: r.get(1)?, color: r.get(2)? }))?;
        Ok(rows.flatten().collect())
    }
}
