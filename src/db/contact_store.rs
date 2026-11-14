//! Contacts cache: folders, contact rows, favourites, and queries.

use super::*;

impl Database {


    // ------------------------------------------------------------ contacts

    pub fn upsert_contact_folder(&self, f: &ContactFolder) -> Result<()> {
        self.conn.execute(
            "INSERT INTO contact_folders (id, display_name, parent_folder_id, cached_at, account_id)
             VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, ?4)
             ON CONFLICT(id) DO UPDATE SET
               display_name = excluded.display_name,
               parent_folder_id = excluded.parent_folder_id,
               cached_at = CURRENT_TIMESTAMP",
            params![f.id, f.display_name, f.parent_folder_id, self.account_id],
        )?;
        Ok(())
    }


    pub fn get_contact_folders(&self) -> Result<Vec<ContactFolder>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, display_name, parent_folder_id FROM contact_folders WHERE account_id = ?1 ORDER BY display_name",
        )?;
        let rows = stmt
            .query_map(params![self.account_id], |r| {
                Ok(ContactFolder { id: r.get(0)?, display_name: r.get(1)?, parent_folder_id: r.get(2)? })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }


    /// Insert or update a contact from a pull. Local-only state (is_favorite) is never overwritten, and a
    /// row with a write of ours in flight (`sync_state` not `ok`) is left alone.
    pub fn upsert_contact(&self, c: &Contact) -> Result<()> {
        self.write_contact_row(c, "ok", true)?;
        Ok(())
    }


    /// After a COMPLETE walk of a folder: drop this account's cached contacts of that folder
    /// the walk did not see (deleted elsewhere). Returns how many.
    pub fn purge_contacts_unseen(&self, folder_id: &str, seen: &std::collections::HashSet<String>) -> Result<usize> {
        let ids: Vec<String> = {
            let mut stmt = self.conn.prepare("SELECT id FROM contacts WHERE account_id = ?1 AND folder_id = ?2")?;
            let v = stmt.query_map(params![self.account_id, folder_id], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            v
        };
        self.in_transaction(|| {
            let mut n = 0;
            for id in ids.iter().filter(|i| !seen.contains(*i)) {
                n += self.conn.execute("DELETE FROM contacts WHERE id = ?1 AND account_id = ?2 AND sync_state = 'ok'", params![id, self.account_id])?;
            }
            Ok(n)
        })
    }


    pub fn set_contact_favorite(&self, id: &str, favorite: bool) -> Result<()> {
        self.conn.execute("UPDATE contacts SET is_favorite = ?1 WHERE id = ?2", params![favorite, id])?;
        Ok(())
    }


    /// Contacts for the People view. `view`: all | favorites | lists (grouped by folder,
    /// folders alphabetical). `sort`: first | last | company | recent. Entries missing the
    /// sort key always go last.
    pub fn query_contacts(&self, view: &str, sort: &str) -> Result<Vec<ContactRow>> {
        self.query_contacts_for(view, sort, None)
    }


    /// As `query_contacts`, optionally limited to one account (its id or its email address).
    pub fn query_contacts_for(&self, view: &str, sort: &str, account: Option<&str>) -> Result<Vec<ContactRow>> {
        self.query_contacts_full(view, sort, account, None, None, None)
    }


    /// The People view's query. `search`: every word must appear in the contact's search text (names,
    /// addresses, phones, company, notes, categories...); `folder`: one contact folder id; `category`:
    /// contacts carrying that category (exact, ignoring case). Contacts with a delete queued are hidden.
    pub fn query_contacts_full(
        &self,
        view: &str,
        sort: &str,
        account: Option<&str>,
        search: Option<&str>,
        folder: Option<&str>,
        category: Option<&str>,
    ) -> Result<Vec<ContactRow>> {
        // `x = ''` sorts empty values after real ones.
        let key = match sort {
            "last"    => "(c.surname = ''), lower(c.surname), lower(c.given_name), lower(c.display_name)",
            "company" => "(c.company = ''), lower(c.company), lower(c.display_name)",
            "recent"  => "c.created_at DESC, lower(c.display_name)",
            _         => "(c.given_name = ''), lower(c.given_name), lower(c.surname), lower(c.display_name)",
        };
        let mut conds = vec![
            "(?1 IS NULL OR c.account_id = ?1 OR lower(a.email) = lower(?1))".to_string(),
            "c.sync_state <> 'deleted'".to_string(),
        ];
        let mut args: Vec<rusqlite::types::Value> = vec![match account {
            Some(a) => rusqlite::types::Value::Text(a.to_string()),
            None => rusqlite::types::Value::Null,
        }];
        if view == "favorites" {
            conds.push("c.is_favorite = 1".to_string());
        }
        if let Some(f) = folder.filter(|f| !f.is_empty()) {
            args.push(f.to_string().into());
            conds.push(format!("c.folder_id = ?{}", args.len()));
        }
        if let Some(cat) = category.filter(|c| !c.is_empty()) {
            args.push(format!("%\"{}\"%", crate::db::like_escape(&cat.replace('"', "").to_lowercase())).into());
            conds.push(format!("lower(c.categories) LIKE ?{} ESCAPE '\\'", args.len()));
        }
        for word in search.unwrap_or("").split_whitespace() {
            args.push(crate::db::like_contains(&word.to_lowercase()).into());
            conds.push(format!("c.search_text LIKE ?{} ESCAPE '\\'", args.len()));
        }
        let order = if view == "lists" {
            format!("lower(COALESCE(f.display_name, 'Contacts')), {}", key)
        } else {
            key.to_string()
        };
        let sql = format!(
            "SELECT c.id, c.display_name, c.given_name, c.surname, c.company, c.job_title, c.emails, c.phones,
                    c.addresses, c.folder_id, c.created_at, c.modified_at, COALESCE(f.display_name, 'Contacts'), c.is_favorite,
                    COALESCE(c.account_id, ''), COALESCE(a.email, ''), c.categories, c.sync_state
             FROM contacts c LEFT JOIN contact_folders f ON f.id = c.folder_id
                             LEFT JOIN accounts a ON a.id = c.account_id
             WHERE {} ORDER BY {}",
            conds.join(" AND "), order
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                let emails: String = r.get(6)?;
                let phones: String = r.get(7)?;
                let addresses: String = r.get(8)?;
                let categories: String = r.get(16)?;
                Ok(ContactRow {
                    contact: Contact {
                        id: r.get(0)?, display_name: r.get(1)?, given_name: r.get(2)?, surname: r.get(3)?,
                        company: r.get(4)?, job_title: r.get(5)?,
                        emails: serde_json::from_str(&emails).unwrap_or_default(),
                        phones: serde_json::from_str::<Vec<ContactPhone>>(&phones).unwrap_or_default(),
                        addresses: serde_json::from_str::<Vec<ContactAddress>>(&addresses).unwrap_or_default(),
                        folder_id: r.get(9)?, created_at: r.get(10)?, modified_at: r.get(11)?,
                        categories: serde_json::from_str(&categories).unwrap_or_default(),
                        ..Default::default()
                    },
                    folder_name: r.get(12)?,
                    is_favorite: r.get(13)?,
                    account_id: r.get(14)?,
                    account_email: r.get(15)?,
                    sync_state: r.get(17)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}
