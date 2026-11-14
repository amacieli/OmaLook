//! Message actions: queued read/flag/move operations and their reverts.

use super::*;

impl Database {

    /// Queue `action` for each message in `ids` (those not already queued). The rows disappear
    /// from lists straight away and their folders' unread counts follow; the provider is
    /// told once `due` (unix seconds) has passed. Returns how many messages were queued.
    pub fn queue_message_actions(&self, ids: &[String], action: &crate::models::MessageAction, due: i64) -> Result<usize> {
        let db_action = action.to_db();
        let mut queued = 0;
        for id in ids {
            let changed = self.conn.execute(
                "UPDATE messages SET action_pending = ?1, action_due = ?2 WHERE id = ?3 AND action_pending IS NULL",
                params![db_action, due, id],
            )?;
            if changed > 0 {
                queued += 1;
                self.adjust_unread_for(id, -1)?;
            }
        }
        Ok(queued)
    }


    /// Take queued actions back (undo) for messages the provider has not been told about.
    pub fn cancel_message_actions(&self, ids: &[String]) -> Result<usize> {
        let mut undone = 0;
        for id in ids {
            let changed = self.conn.execute(
                "UPDATE messages SET action_pending = NULL, action_due = 0 WHERE id = ?1 AND action_pending IS NOT NULL",
                params![id],
            )?;
            if changed > 0 {
                undone += 1;
                self.adjust_unread_for(id, 1)?;
            }
        }
        Ok(undone)
    }


    /// Nudge the unread count of a message's folder by `sign` (±1) if the message is unread.
    fn adjust_unread_for(&self, id: &str, sign: i32) -> Result<()> {
        self.conn.execute(
            "UPDATE folders SET unread_item_count = MAX(0, COALESCE(unread_item_count, 0) + ?1)
             WHERE id = (SELECT folder_id FROM messages WHERE id = ?2 AND is_read = 0)",
            params![sign, id],
        )?;
        Ok(())
    }


    /// Queued actions of this account whose undo window is over: (id, folder it is in, action).
    pub fn due_actions(&self, now: i64) -> Result<Vec<(String, String, crate::models::MessageAction)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, COALESCE(folder_id, ''), action_pending FROM messages
             WHERE action_pending IS NOT NULL AND action_due <= ?1 AND account_id = ?2",
        )?;
        let rows = stmt
            .query_map(params![now, self.account_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, folder, a)| crate::models::MessageAction::from_db(&a).map(|act| (id, folder, act)))
            .collect())
    }


    /// The provider carried the action out: the local row is no longer this message's home
    /// (a moved message arrives in its new folder with the next sync).
    pub fn finish_message_action(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM messages WHERE id = ?1 AND action_pending IS NOT NULL", params![id])?;
        Ok(())
    }


    /// The provider refused the action for good: put the message back in its list.
    pub fn revert_message_action(&self, id: &str) -> Result<()> {
        self.cancel_message_actions(&[id.to_string()]).map(|_| ())
    }


    /// Read-state changes made locally that the provider has not heard about yet.
    pub fn pending_reads(&self) -> Result<Vec<(String, bool)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, is_read FROM messages WHERE read_pending = 1 AND account_id = ?1",
        )?;
        let rows = stmt
            .query_map(params![self.account_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }


    /// The provider accepted `is_read` for this message. Stays pending if the user has
    /// flipped it again in the meantime.
    pub fn clear_read_pending(&self, id: &str, is_read: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET read_pending = 0 WHERE id = ?1 AND is_read = ?2",
            params![id, is_read],
        )?;
        Ok(())
    }
}
