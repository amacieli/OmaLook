//! Contacts read service (R3.5): the cached address book, filtered to the accounts the caller
//! passes in.

use super::Page;
use crate::db::Database;
use crate::models::ContactRow;
use serde::Serialize;

pub struct ContactQuery<'a> {
    /// Accounts the caller may see.
    pub accounts: &'a [String],
    pub favorites: bool,
    /// Raw sort name; anything but first/last/company/recent sorts by first name.
    pub sort: Option<&'a str>,
    /// Case-insensitive text matched against name, company, title, emails and phones.
    pub text: Option<&'a str>,
    pub limit: i64,
    pub offset: i64,
}

fn sort_name(raw: Option<&str>) -> &str {
    raw.filter(|v| matches!(*v, "first" | "last" | "company" | "recent")).unwrap_or("first")
}

pub fn search(db: &Database, q: &ContactQuery) -> Result<Page<ContactRow>, String> {
    let view = if q.favorites { "favorites" } else { "all" };
    let rows = db.query_contacts_for(view, sort_name(q.sort), None).map_err(|e| e.to_string())?;
    let needle = q.text.map(|t| t.to_lowercase());
    let rows: Vec<ContactRow> = rows
        .into_iter()
        .filter(|r| q.accounts.contains(&r.account_id))
        .filter(|r| match &needle {
            None => true,
            Some(n) => {
                let c = &r.contact;
                let hay = format!("{} {} {} {} {}", c.display_name, c.company, c.job_title, c.emails.join(" "), serde_json::to_string(&c.phones).unwrap_or_default());
                hay.to_lowercase().contains(n)
            }
        })
        .skip(q.offset as usize)
        .take(q.limit as usize + 1)
        .collect();
    Ok(Page::of(rows, q.limit, q.offset))
}

#[derive(Debug, Serialize)]
pub struct ContactDetail {
    pub id: String,
    pub account_id: String,
    pub folder_id: Option<String>,
    pub is_favorite: bool,
    /// The cached provider object.
    pub contact: serde_json::Value,
}

/// One contact in full, if it belongs to one of `accounts`.
pub fn get(db: &Database, accounts: &[String], id: &str) -> Result<ContactDetail, String> {
    let rec = db.get_contact_record(id).map_err(|e| e.to_string())?.ok_or("no such contact")?;
    if !accounts.contains(&rec.account_id) { return Err("no such contact".into()); }
    Ok(ContactDetail { id: rec.id, account_id: rec.account_id, folder_id: rec.folder_id, is_favorite: rec.is_favorite, contact: rec.raw })
}

