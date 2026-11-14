//! SMS read service (R3.5): threads, messages and contact names from the phone's cache.

use super::{clip, Page, LIST_PREVIEW, MAX_TEXT};
use crate::db::Database;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Thread {
    pub device: String,
    pub thread: i64,
    pub title: String,
    pub addresses: Vec<String>,
    pub last_date_ms: i64,
    pub last_message: String,
    pub last_direction: &'static str,
    pub message_count: i64,
    pub unread_count: i64,
}

#[derive(Debug, Serialize)]
pub struct Message {
    pub uid: i64,
    pub date_ms: i64,
    pub direction: &'static str,
    pub body: String,
    pub is_read: bool,
    pub addresses: Vec<String>,
    pub attachment_count: usize,
}

#[derive(Debug, Serialize)]
pub struct Messages {
    pub count: usize,
    pub results: Vec<Message>,
    /// Pass as `before` for the next (older) page.
    pub next_before: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ContactName {
    pub name: String,
    pub number: String,
}

#[derive(Debug, Serialize)]
pub struct Contacts {
    pub device: String,
    pub contacts: Vec<ContactName>,
}

#[derive(Debug, Serialize)]
pub struct UnreadCounts {
    pub messages: i64,
    pub threads: i64,
}

/// Android SMS types as OmaLook stores them (docs/SMS.md).
fn direction_name(d: i32) -> &'static str {
    match d { 1 => "received", 2 => "sent", 5 => "send_failed", 6 => "queued", _ => "other" }
}

fn names_of(device: &str, addresses: &[String]) -> Vec<String> {
    addresses.iter().map(|a| crate::sms_names::lookup(device, a).unwrap_or_default()).collect()
}

fn title_of(addresses: &[String], names: &[String]) -> String {
    let mut seen: Vec<String> = vec![];
    for (a, n) in addresses.iter().zip(names) {
        let shown = if n.is_empty() { a.clone() } else { n.clone() };
        if !seen.contains(&shown) { seen.push(shown); }
    }
    seen.join(", ")
}

/// Threads of one device (or all), newest first. `query` matches names, numbers or message text.
pub fn threads(db: &Database, device: Option<&str>, query: Option<&str>, limit: i64, offset: i64) -> Result<Page<Thread>, String> {
    let q = query.unwrap_or("").to_lowercase();
    let q_digits: String = q.chars().filter(|c| c.is_ascii_digit()).collect();
    let text_hits: std::collections::HashSet<(String, i64)> = if q.is_empty() { Default::default() } else {
        db.sms_search_bodies(device, &q, 20000).map_err(|e| e.to_string())?.into_iter().map(|(d, t, _)| (d, t)).collect()
    };
    let all = db.sms_threads(device, if q.is_empty() { 5000 } else { 20000 }).map_err(|e| e.to_string())?;
    let rows: Vec<Thread> = all
        .into_iter()
        .filter_map(|t| {
            let names = names_of(&t.device_id, &t.addresses);
            let title = title_of(&t.addresses, &names);
            if !q.is_empty() {
                let by_name = names.iter().chain(std::iter::once(&title)).any(|n| n.to_lowercase().contains(&q));
                let by_number = q_digits.len() >= 3 && t.addresses.iter().any(|a| a.chars().filter(|c| c.is_ascii_digit()).collect::<String>().contains(&q_digits));
                if !(by_name || by_number || text_hits.contains(&(t.device_id.clone(), t.thread_id))) { return None; }
            }
            Some(Thread {
                device: t.device_id, thread: t.thread_id, title, addresses: t.addresses,
                last_date_ms: t.last_date_ms, last_message: clip(&t.last_body, LIST_PREVIEW),
                last_direction: direction_name(t.last_direction),
                message_count: t.message_count, unread_count: t.unread_count,
            })
        })
        .skip(offset as usize)
        .take(limit as usize + 1)
        .collect();
    Ok(Page::of(rows, limit, offset))
}

/// Messages of one thread, newest first, one page. `before` is the date of the last page's oldest.
pub fn messages(db: &Database, device: Option<&str>, thread: i64, before: Option<i64>, limit: i64) -> Result<Messages, String> {
    let rows = db.sms_messages(device, thread, before, limit + 1).map_err(|e| e.to_string())?;
    let more = rows.len() as i64 > limit;
    let results: Vec<Message> = rows
        .into_iter()
        .take(limit as usize)
        .map(|m| Message {
            uid: m.uid, date_ms: m.date_ms, direction: direction_name(m.direction),
            body: clip(&m.body, MAX_TEXT), is_read: m.is_read, addresses: m.addresses,
            attachment_count: m.attachments.as_array().map(|a| a.len()).unwrap_or(0),
        })
        .collect();
    let next_before = if more { results.last().map(|r| r.date_ms.to_string()) } else { None };
    Ok(Messages { count: results.len(), results, next_before })
}

/// Phone contacts with a name, filtered by `query` (name, or digits of the number).
pub fn contacts(db: &Database, device: Option<&str>, query: Option<&str>) -> Result<Contacts, String> {
    let q = query.unwrap_or("").to_lowercase();
    let qd: String = q.chars().filter(|c| c.is_ascii_digit()).collect();
    let dev = device.map(String::from).or_else(|| db.sms_default_device().ok().flatten()).unwrap_or_default();
    let list: Vec<ContactName> = crate::sms_names::contacts(&dev)
        .into_iter()
        .filter(|(n, num)| q.is_empty() || n.to_lowercase().contains(&q) || (qd.len() >= 2 && num.chars().filter(|c| c.is_ascii_digit()).collect::<String>().contains(&qd)))
        .take(100)
        .map(|(n, num)| ContactName { name: n, number: num })
        .collect();
    Ok(Contacts { device: dev, contacts: list })
}

pub fn unread_counts(db: &Database) -> Result<UnreadCounts, String> {
    let (messages, threads) = db.sms_unread_totals().map_err(|e| e.to_string())?;
    Ok(UnreadCounts { messages, threads })
}
