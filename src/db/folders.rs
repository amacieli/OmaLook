//! Mail folders cache.

use super::*;

impl Database {

    /// Upsert a folder from Graph API
    pub fn upsert_folder(&self, folder: &MailFolder) -> Result<()> {
        // Assign sort_order: well-known folders get fixed low numbers, others 999
        let sort_order: i32 = match folder.well_known_name.as_deref() {
            Some("inbox")        => 1,
            Some("drafts")       => 2,
            Some("sentitems")    => 3,
            Some("deleteditems") => 4,
            Some("junkemail")    => 5,
            Some("archive")      => 6,
            _                    => 999,
        };

        self.conn.execute(
            "INSERT INTO folders (id, display_name, parent_folder_id, unread_item_count, total_item_count, well_known_name, sort_order, cached_at, account_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, CURRENT_TIMESTAMP, ?8)
             ON CONFLICT(id) DO UPDATE SET
               display_name      = excluded.display_name,
               unread_item_count = excluded.unread_item_count,
               total_item_count  = excluded.total_item_count,
               sort_order        = excluded.sort_order,
               cached_at         = CURRENT_TIMESTAMP",
            params![
                folder.id,
                folder.display_name,
                folder.parent_folder_id,
                folder.unread_item_count,
                folder.total_item_count,
                folder.well_known_name,
                sort_order,
                self.account_id,
            ],
        )?;
        Ok(())
    }


    /// This handle's account's folders, sorted: well-known first (by sort_order), then alphabetical.
    /// Scoped to the account: each account's daemon must only ever sync its own folders.
    pub fn get_folders(&self) -> Result<Vec<MailFolder>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, display_name, parent_folder_id, unread_item_count, total_item_count, well_known_name
             FROM folders
             WHERE account_id = ?1
             ORDER BY sort_order ASC, display_name ASC",
        )?;

        let folders = stmt
            .query_map(params![self.account_id], |row| {
                Ok(MailFolder {
                    id:                row.get(0)?,
                    display_name:      row.get(1)?,
                    parent_folder_id:  row.get(2)?,
                    unread_item_count: row.get(3)?,
                    total_item_count:  row.get(4)?,
                    well_known_name:   row.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(folders)
    }
}

/// One folder as the MCP mail tools show it (R3.5).
pub struct McpFolderRow {
    pub id: String,
    pub display_name: String,
    pub unread: i64,
    pub total: i64,
    pub well_known: String,
    pub account_id: String,
    pub account_email: String,
}

impl Database {
    /// Folders of the given accounts, sorted as the MCP tool lists them.
    pub fn mcp_folders(&self, accounts: &[String]) -> rusqlite::Result<Vec<McpFolderRow>> {
        let mut a: Vec<rusqlite::types::Value> = vec![];
        let cond = account_in("f.account_id", accounts, &mut a);
        let sql = format!(
            "SELECT f.id, f.display_name, COALESCE(f.unread_item_count,0), COALESCE(f.total_item_count,0), COALESCE(f.well_known_name,''), \
                    COALESCE(f.account_id,''), COALESCE(x.email,'') \
             FROM folders f LEFT JOIN accounts x ON x.id = f.account_id WHERE {} \
             ORDER BY f.account_id, f.sort_order, f.display_name", cond);
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(a.iter()), |r| Ok(McpFolderRow {
            id: r.get(0)?, display_name: r.get(1)?, unread: r.get(2)?, total: r.get(3)?,
            well_known: r.get(4)?, account_id: r.get(5)?, account_email: r.get(6)?,
        }))?;
        Ok(rows.flatten().collect())
    }
}
