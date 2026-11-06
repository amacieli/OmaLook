//! Cache of each account's mailbox rules (Graph `messageRules`), kept as the provider's raw JSON so the
//! summary wording can improve without a re-fetch. Read-only in phase 2 (OL-040): nothing here is pushed.

use super::Database;
use crate::errors::Result;
use rusqlite::params;
use serde_json::Value;

/// What the last fetch for an account said.
#[derive(Debug, Clone, PartialEq)]
pub struct RulesState {
    /// `ok`, `needs_signin` (permission missing), `unsupported` (provider has no server rules here), `error`.
    pub state: String,
    pub error: String,
    pub fetched_at: i64,
}

impl Database {
    pub(crate) fn init_rules_schema(&self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS mail_rules (
                account_id TEXT NOT NULL,
                rule_id TEXT NOT NULL,
                sequence INTEGER NOT NULL DEFAULT 0,
                raw TEXT NOT NULL,
                PRIMARY KEY (account_id, rule_id)
            )", [])?;
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS rules_state (
                account_id TEXT PRIMARY KEY,
                state TEXT NOT NULL,
                error TEXT NOT NULL DEFAULT '',
                fetched_at INTEGER NOT NULL DEFAULT 0
            )", [])?;
        Ok(())
    }

    /// Replace this account's cached rules with what the provider just returned.
    pub fn replace_rules(&self, rules: &[Value]) -> Result<()> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        self.conn.execute_batch("BEGIN")?;
        let r = (|| -> Result<()> {
            self.conn.execute("DELETE FROM mail_rules WHERE account_id = ?1", params![self.account_id])?;
            for rule in rules {
                let Some(id) = rule["id"].as_str() else { continue };
                self.conn.execute(
                    "INSERT OR REPLACE INTO mail_rules (account_id, rule_id, sequence, raw) VALUES (?1, ?2, ?3, ?4)",
                    params![self.account_id, id, rule["sequence"].as_i64().unwrap_or(0), rule.to_string()])?;
            }
            self.conn.execute(
                "INSERT INTO rules_state (account_id, state, error, fetched_at) VALUES (?1, 'ok', '', ?2)
                 ON CONFLICT(account_id) DO UPDATE SET state = 'ok', error = '', fetched_at = ?2",
                params![self.account_id, now])?;
            Ok(())
        })();
        match r {
            Ok(()) => { self.conn.execute_batch("COMMIT")?; Ok(()) }
            Err(e) => { let _ = self.conn.execute_batch("ROLLBACK"); Err(e) }
        }
    }

    /// The fetch failed or is not possible: keep the cached rules, record why.
    pub fn set_rules_state(&self, state: &str, error: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO rules_state (account_id, state, error, fetched_at) VALUES (?1, ?2, ?3, 0)
             ON CONFLICT(account_id) DO UPDATE SET state = ?2, error = ?3",
            params![self.account_id, state, error])?;
        Ok(())
    }

    /// Cached raw rules of one account, in rule order.
    pub fn rules_raw(&self, account_id: &str) -> Result<Vec<Value>> {
        let mut st = self.conn.prepare("SELECT raw FROM mail_rules WHERE account_id = ?1 ORDER BY sequence, rule_id")?;
        let rows = st.query_map(params![account_id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().filter_map(|s| serde_json::from_str(&s).ok()).collect())
    }

    /// (id, provider, email) of every account, oldest first.
    pub fn accounts_basic(&self) -> Result<Vec<(String, String, String)>> {
        let mut st = self.conn.prepare("SELECT id, provider, COALESCE(email,'') FROM accounts ORDER BY created_at, id")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn rules_state(&self, account_id: &str) -> Result<Option<RulesState>> {
        use rusqlite::OptionalExtension;
        Ok(self.conn.query_row(
            "SELECT state, error, fetched_at FROM rules_state WHERE account_id = ?1", params![account_id],
            |r| Ok(RulesState { state: r.get(0)?, error: r.get(1)?, fetched_at: r.get(2)? })).optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn replace_keeps_one_accounts_rules_apart_from_another_and_records_state() {
        let a = Database::open_for_account(":memory:", "ex").unwrap();
        a.replace_rules(&[json!({ "id": "r2", "sequence": 2, "displayName": "two" }), json!({ "id": "r1", "sequence": 1 }), json!({ "noid": true })]).unwrap();
        let got = a.rules_raw("ex").unwrap();
        assert_eq!(got.iter().map(|r| r["id"].as_str().unwrap()).collect::<Vec<_>>(), vec!["r1", "r2"], "rule order, rows without an id skipped");
        assert!(a.rules_raw("other").unwrap().is_empty());
        assert_eq!(a.rules_state("ex").unwrap().unwrap().state, "ok");
        // a failed fetch keeps what we had and says why
        a.set_rules_state("needs_signin", "403").unwrap();
        assert_eq!(a.rules_raw("ex").unwrap().len(), 2);
        let st = a.rules_state("ex").unwrap().unwrap();
        assert_eq!((st.state.as_str(), st.error.as_str()), ("needs_signin", "403"));
        assert!(st.fetched_at > 0, "the time of the last good fetch is kept");
        // the next good fetch replaces the set (a rule deleted in Outlook disappears)
        a.replace_rules(&[json!({ "id": "r1", "sequence": 1 })]).unwrap();
        assert_eq!(a.rules_raw("ex").unwrap().len(), 1);
        assert_eq!(a.rules_state("ex").unwrap().unwrap().state, "ok");
        assert!(a.rules_state("nobody").unwrap().is_none());
    }
}
