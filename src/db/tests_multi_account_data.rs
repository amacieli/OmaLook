//! Unit tests (moved from db.rs; module body dedented by 4 spaces, otherwise verbatim).

use super::*;
use crate::models::{CalendarEvent, Contact, ContactFolder, EmailMessage, MailFolder};

/// One shared database file, two accounts (Exchange + Gmail), each with its own handle.
fn two_accounts(name: &str) -> (String, Database, Database) {
    let path = std::env::temp_dir().join(format!("omalook-multi-{}-{}.db", name, std::process::id())).to_string_lossy().to_string();
    let _ = std::fs::remove_file(&path);
    let ex = Database::open_for_account(&path, "exchange-aaaaaa").unwrap();
    let gm = Database::open_for_account(&path, "gmail-bbbbbb").unwrap();
    ex.insert_account("exchange-aaaaaa", "exchange", Some("adam@work.com"), None, "{}").unwrap();
    gm.insert_account("gmail-bbbbbb", "gmail", Some("Adam@Gmail.com"), None, "{}").unwrap();
    (path, ex, gm)
}

fn folder(id: &str) -> MailFolder {
    MailFolder { id: id.into(), display_name: "Inbox".into(), parent_folder_id: None, unread_item_count: Some(1), total_item_count: Some(2), well_known_name: Some("inbox".into()) }
}

#[test]
fn each_account_only_sees_and_syncs_its_own_folders() {
    let (path, ex, gm) = two_accounts("folders");
    ex.upsert_folder(&folder("AAMk-inbox")).unwrap();
    gm.upsert_folder(&folder("gmail-bbbbbb:INBOX")).unwrap();
    assert_eq!(ex.get_folders().unwrap().iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["AAMk-inbox"]);
    assert_eq!(gm.get_folders().unwrap().iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["gmail-bbbbbb:INBOX"]);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn contacts_and_events_carry_and_filter_by_account_email() {
    let (path, ex, gm) = two_accounts("rows");
    let contact = |id: &str, name: &str| Contact { id: id.into(), display_name: name.into(), ..Default::default() };
    ex.upsert_contact_folder(&ContactFolder { id: "contacts".into(), display_name: "Contacts".into(), parent_folder_id: None }).unwrap();
    ex.upsert_contact(&contact("c-ex", "Exchange Eve")).unwrap();
    gm.upsert_contact(&contact("gmail-bbbbbb:people/c1", "Gmail Gus")).unwrap();
    let ev = |id: &str| CalendarEvent { id: id.into(), subject: "s".into(), start: "2026-10-05T09:00:00".into(), end: "2026-10-05T10:00:00".into(), ..Default::default() };
    ex.upsert_event(&ev("e-ex")).unwrap();
    gm.upsert_event(&ev("gmail-bbbbbb:e1")).unwrap();

    let all = ex.query_contacts_for("all", "first", None).unwrap();
    assert_eq!(all.len(), 2);
    let by_email = |email: &str| ex.query_contacts_for("all", "first", Some(email)).unwrap();
    let g = by_email("adam@gmail.com"); // case-insensitive address match
    assert_eq!((g.len(), g[0].contact.display_name.as_str(), g[0].account_email.as_str(), g[0].account_id.as_str()), (1, "Gmail Gus", "Adam@Gmail.com", "gmail-bbbbbb"));
    assert_eq!(by_email("adam@work.com")[0].contact.display_name, "Exchange Eve");
    assert_eq!(ex.query_contacts_for("all", "first", Some("gmail-bbbbbb")).unwrap().len(), 1, "id works as filter too");
    assert!(by_email("nobody@x.com").is_empty());
    assert_eq!(ex.query_contacts_for("lists", "first", Some("adam@gmail.com")).unwrap().len(), 1);

    let events = ex.get_events_for_month_by_account("2026-10", None).unwrap();
    assert_eq!(events.len(), 2);
    let g: Vec<_> = events.iter().filter(|(_, id, _)| id == "gmail-bbbbbb").collect();
    assert_eq!((g.len(), g[0].0.id.as_str(), g[0].2.as_str()), (1, "gmail-bbbbbb:e1", "Adam@Gmail.com"));
    assert_eq!(ex.get_events_for_month_by_account("2026-10", Some("adam@work.com")).unwrap().len(), 1);
    assert_eq!(ex.get_events_for_month("2026-10").unwrap().len(), 2, "plain month query still spans accounts");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn known_message_ids_reports_only_stored_ones() {
    let (path, _ex, gm) = two_accounts("known");
    let msg = |id: &str| EmailMessage { id: id.into(), from: "a@b.c".into(), subject: "s".into(), received: "2026-10-01T00:00:00Z".into(), body: "".into(), folder_id: None, is_read: true, ..Default::default() };
    gm.insert_email(&msg("gmail-bbbbbb:1")).unwrap();
    gm.insert_email(&msg("gmail-bbbbbb:2")).unwrap();
    let ids: Vec<String> = ["gmail-bbbbbb:1", "gmail-bbbbbb:3", "gmail-bbbbbb:2"].iter().map(|s| s.to_string()).collect();
    let known = known_message_ids(std::path::Path::new(&path), &ids);
    assert_eq!(known.len(), 2);
    assert!(known.contains("gmail-bbbbbb:1") && !known.contains("gmail-bbbbbb:3"));
    assert!(known_message_ids(std::path::Path::new("/nonexistent/x.db"), &ids).is_empty());
    let _ = std::fs::remove_file(&path);
}
