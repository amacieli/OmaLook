//! Unit tests (moved from db.rs; module body dedented by 4 spaces, otherwise verbatim).

use super::*;

#[test]
fn all_accounts_rows_sum_shared_kinds_only() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE folders (id TEXT PRIMARY KEY, display_name TEXT, unread_item_count INTEGER,
        total_item_count INTEGER, well_known_name TEXT, account_id TEXT);").unwrap();
    let rows = [
        ("a:INBOX", 2, 10, "inbox", "a"),
        ("b:INBOX", 3, 20, "inbox", "b"),
        ("a:SENT", 0, 5, "sentitems", "a"),       // only one account has Sent: no row
        ("a:Work", 1, 1, "", "a"),                // custom folder: never aggregated
        ("b:Work", 1, 1, "", "b"),
    ];
    for (id, u, t, wk, acct) in rows {
        conn.execute("INSERT INTO folders VALUES (?1, ?1, ?2, ?3, ?4, ?5)",
            params![id, u, t, if wk.is_empty() { None } else { Some(wk) }, acct]).unwrap();
    }
    let got = all_accounts_folders(&conn).unwrap();
    assert_eq!(got, vec![AllAccountsFolder { well_known_name: "inbox".into(), accounts: 2,
        unread_item_count: 5, total_item_count: 30 }]);
    assert_eq!(all_accounts_label("inbox"), "Inbox - all accounts");
    assert_eq!(all_accounts_label("sentitems"), "Sent - all accounts");
    assert_eq!(all_accounts_label("deleteditems"), "Deleted - all accounts");
}

fn temp_db_path(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("omalook-test-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_file(&dir);
    dir.to_string_lossy().to_string()
}

#[test]
fn create_account_generates_pattern_ids_even_for_first_account() {
    let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
    let a = db.create_account("Exchange", Some("a@x.com"), None, "{}").unwrap();
    let b = db.create_account("exchange", Some("b@x.com"), None, "{}").unwrap();
    for acc in [&a, &b] {
        assert!(acc.id.starts_with("exchange-") && acc.id != DEFAULT_ACCOUNT, "{}", acc.id);
        assert_eq!(acc.id.len(), "exchange-".len() + 6);
    }
    assert_ne!(a.id, b.id);
}

#[test]
fn duplicate_mailbox_is_rejected_case_insensitively() {
    let db = Database::open(":memory:").unwrap();
    db.create_account("gmail", Some("Me@Gmail.com"), None, "{}").unwrap();
    assert!(db.create_account("gmail", Some("me@gmail.com"), None, "{}").is_err());
    // same address on another provider type is a different account
    assert!(db.create_account("outlook", Some("me@gmail.com"), None, "{}").is_ok());
}

#[test]
fn accounts_start_enabled_and_can_be_toggled() {
    let db = Database::open(":memory:").unwrap();
    let a = db.create_account("exchange", Some("a@x.com"), None, "{}").unwrap();
    assert!(a.enabled);
    db.set_account_enabled(&a.id, false).unwrap();
    assert!(!db.list_accounts().unwrap()[0].enabled);
    db.set_account_enabled(&a.id, true).unwrap();
    assert!(db.list_accounts().unwrap()[0].enabled);
}

#[test]
fn enabled_column_is_added_to_an_existing_accounts_table() {
    let path = temp_db_path("enabled-col");
    {
        let c = Connection::open(&path).unwrap();
        c.execute_batch("CREATE TABLE accounts (id TEXT PRIMARY KEY, provider TEXT NOT NULL, email TEXT, display_name TEXT,
            config TEXT NOT NULL DEFAULT '{}', created_at DATETIME DEFAULT CURRENT_TIMESTAMP, UNIQUE(provider, email));
            INSERT INTO accounts (id, provider, email) VALUES ('exchange-aaaaaa','exchange','a@x.com');").unwrap();
    }
    let db = Database::open(&path).unwrap();
    assert!(db.list_accounts().unwrap()[0].enabled, "existing accounts stay signed in");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn open_does_not_invent_accounts() {
    let db = Database::open(":memory:").unwrap();
    assert!(db.list_accounts().unwrap().is_empty(), "fresh installs must not show a phantom account");
    db.ensure_legacy_account().unwrap();
    assert_eq!(db.list_accounts().unwrap()[0].id, "exchange-primary");
}

#[test]
fn read_state_toggle_push_and_reconcile() {
    let db = Database::open_for_account(":memory:", "a1").unwrap();
    let mk = |id: &str, read: bool| EmailMessage {
        id: id.into(), from: "x@y".into(), subject: "s".into(), received: "2026-10-01T00:00:00Z".into(),
        body: "b".into(), folder_id: Some("f".into()), is_read: read, ..Default::default()
    };
    db.insert_email(&mk("m1", false)).unwrap();
    db.insert_email(&mk("m2", true)).unwrap();
    db.insert_email(&mk("m3", false)).unwrap();
    db.conn.execute("INSERT INTO folders (id, display_name, unread_item_count, total_item_count) VALUES ('f','F',2,3)", []).unwrap();
    let unread = |id: &str| -> bool { !db.conn.query_row("SELECT is_read FROM messages WHERE id=?1", [id], |r| r.get::<_, bool>(0)).unwrap() };
    let folder_unread = || -> i32 { db.conn.query_row("SELECT unread_item_count FROM folders WHERE id='f'", [], |r| r.get(0)).unwrap() };

    // insert keeps the provider's flag
    assert!(unread("m1") && !unread("m2"));

    // toggle: changes state, adjusts the folder count, queues a push; repeat is a no-op
    assert!(db.set_message_read("m1", true).unwrap());
    assert!(!db.set_message_read("m1", true).unwrap());
    assert_eq!(folder_unread(), 1);
    assert_eq!(db.pending_reads().unwrap(), vec![("m1".to_string(), true)]);

    // reconcile leaves the pending row alone, fixes the others (m2 is unread upstream, m3 read)
    let n = db.reconcile_read_state("f", &["m1".to_string(), "m2".to_string()]).unwrap();
    assert_eq!(n, 2);
    assert!(!unread("m1") && unread("m2") && !unread("m3"));

    // a flip after the push started keeps the row pending; a matching clear releases it
    db.clear_read_pending("m1", false).unwrap();
    assert_eq!(db.pending_reads().unwrap().len(), 1);
    db.clear_read_pending("m1", true).unwrap();
    assert!(db.pending_reads().unwrap().is_empty());
}

#[test]
fn queued_actions_hide_the_row_undo_restores_and_finish_removes_it() {
    use crate::models::MessageAction;
    let db = Database::open_for_account(":memory:", "a1").unwrap();
    let mk = |id: &str, read: bool| EmailMessage {
        id: id.into(), from: "x@y.z".into(), subject: "s".into(), received: "2026-01-01T00:00:00Z".into(),
        body: "b".into(), folder_id: Some("f".into()), is_read: read, ..Default::default()
    };
    db.insert_email(&mk("m1", false)).unwrap();
    db.insert_email(&mk("m2", true)).unwrap();
    db.conn.execute("INSERT INTO folders (id, display_name, unread_item_count, total_item_count) VALUES ('f','F',1,2)", []).unwrap();
    let unread = || -> i32 { db.conn.query_row("SELECT unread_item_count FROM folders WHERE id='f'", [], |r| r.get(0)).unwrap() };
    let ids = vec!["m1".to_string(), "m2".to_string()];

    // queue: both queued once, unread count drops for the unread one only, repeat is a no-op
    assert_eq!(db.queue_message_actions(&ids, &MessageAction::Archive, 100).unwrap(), 2);
    assert_eq!(db.queue_message_actions(&ids, &MessageAction::Trash, 100).unwrap(), 0);
    assert_eq!(unread(), 0);

    // not due yet / due
    assert!(db.due_actions(99).unwrap().is_empty());
    let due = db.due_actions(100).unwrap();
    assert_eq!(due.len(), 2);
    assert!(due.iter().all(|(_, f, a)| f == "f" && *a == MessageAction::Archive));

    // undo one: it comes back and the count follows
    assert_eq!(db.cancel_message_actions(&["m1".to_string()]).unwrap(), 1);
    assert_eq!(unread(), 1);
    assert_eq!(db.due_actions(100).unwrap().len(), 1);

    // the provider did m2: its row is gone; m1 is untouched
    db.finish_message_action("m2").unwrap();
    let left: i32 = db.conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0)).unwrap();
    assert_eq!(left, 1);
    // finish never deletes a message that is not queued
    db.finish_message_action("m1").unwrap();
    let left: i32 = db.conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0)).unwrap();
    assert_eq!(left, 1);
}

#[test]
fn details_arrive_with_new_rows_and_a_delta_walk_fills_old_ones() {
    let db = Database::open_for_account(":memory:", "a1").unwrap();
    // an old row (no details), as stored before the fields existed
    db.insert_email(&EmailMessage { id: "old".into(), from: "x@y.z".into(), subject: "s".into(), received: "2026-01-01T00:00:00Z".into(),
        body: "b".into(), folder_id: Some("f".into()), is_read: true, ..Default::default() }).unwrap();
    assert_eq!(db.meta_remaining().unwrap(), 1);
    // a new row with details
    db.insert_email(&EmailMessage { id: "new".into(), from: "a@b.c".into(), subject: "t".into(), received: "2026-02-01T00:00:00Z".into(),
        body: "b".into(), folder_id: Some("f".into()), is_read: false, meta: true, from_name: Some("Ann".into()),
        to_text: Some("Bob <bob@x.y>".into()), cc_text: Some("c@x.y".into()), categories: Some(vec!["Red".into()]),
        importance: Some("high".into()), has_attachments: Some(true), ..Default::default() }).unwrap();
    assert_eq!(db.meta_remaining().unwrap(), 1, "new rows do not need the repopulate");
    let get = |id: &str| -> (Option<String>, Option<String>, Option<String>, i64) {
        db.conn.query_row("SELECT from_name, to_text, categories, meta_ok FROM messages WHERE id=?1", [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap()
    };
    assert_eq!(get("new"), (Some("Ann".into()), Some("Bob <bob@x.y>".into()), Some("[\"Red\"]".into()), 1));

    // a delta walk carrying details fills the old row
    let page = crate::models::DeltaPage {
        upserts: vec![EmailMessage { id: "old".into(), from: "x@y.z".into(), subject: "s".into(), received: "2026-01-01T00:00:00Z".into(),
            body: "b".into(), folder_id: Some("f".into()), is_read: true, meta: true, from_name: Some("Xavier".into()),
            categories: Some(vec![]), ..Default::default() }],
        removed: vec![], next_link: None,
    };
    let (changed, _) = db.apply_delta_page("f", &page, None).unwrap();
    assert_eq!(changed, 1);
    assert_eq!(get("old").0, Some("Xavier".into()));
    assert_eq!(db.meta_remaining().unwrap(), 0);

    // a category edited here and not yet pushed survives a feed page that says otherwise
    assert_eq!(db.change_message_categories(&["old".into()], true, "Blue").unwrap(), 1);
    assert_eq!(db.change_message_categories(&["old".into()], true, "blue").unwrap(), 0, "case-insensitive duplicate");
    let page2 = crate::models::DeltaPage {
        upserts: vec![EmailMessage { id: "old".into(), from: "x@y.z".into(), subject: "s".into(), received: "2026-01-01T00:00:00Z".into(),
            body: "b".into(), folder_id: Some("f".into()), is_read: true, meta: true, categories: Some(vec!["Green".into()]), ..Default::default() }],
        removed: vec![], next_link: None,
    };
    db.apply_delta_page("f", &page2, None).unwrap();
    assert_eq!(get("old").2, Some("[\"Blue\"]".into()));
    let pend = db.pending_category_pushes().unwrap();
    assert_eq!(pend, vec![("old".to_string(), vec!["Blue".to_string()], vec!["Green".to_string()])]);
    // pushing releases it; an edit made meanwhile keeps it pending
    db.finish_category_push("old", &["Blue".to_string()]).unwrap();
    assert!(db.pending_category_pushes().unwrap().is_empty());
    db.change_message_categories(&["old".into()], false, "Blue").unwrap();
    db.finish_category_push("old", &["Other".to_string()]).unwrap();
    assert_eq!(db.pending_category_pushes().unwrap().len(), 1);
    db.revert_category_push("old").unwrap();
    assert_eq!(get("old").2, Some("[\"Other\"]".into()));
}

#[test]
fn category_defs_pending_and_replace() {
    let db = Database::open_for_account(":memory:", "a1").unwrap();
    assert!(db.add_pending_category("Work", "#0078d4").unwrap());
    assert!(!db.add_pending_category("Work", "#e74856").unwrap(), "same name: no second row");
    db.replace_category_defs(&[crate::models::CategoryDef { name: "Home".into(), color: "#10893e".into(), provider_color: Some("preset4".into()) }]).unwrap();
    assert_eq!(db.pending_categories().unwrap(), vec![("Work".to_string(), "#0078d4".to_string())], "pending survives a refresh");
    db.finish_pending_category("Work", Some("preset7")).unwrap();
    assert!(db.pending_categories().unwrap().is_empty());
    let n: i64 = db.conn.query_row("SELECT COUNT(*) FROM category_defs", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 2);
    db.set_master_list_available(false).unwrap();
    assert_eq!(db.conn.query_row("SELECT master_list FROM category_state", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    assert!(!db.flag_is_set("k").unwrap());
    db.set_flag("k").unwrap();
    assert!(db.flag_is_set("k").unwrap());
}

#[test]
fn sender_prefs_add_list_set_remove() {
    let db = Database::open_for_account(":memory:", "a1").unwrap();
    assert_eq!(db.add_sender("  Bob@Example.COM ").unwrap(), SenderAdd::Added);
    assert_eq!(db.add_sender("bob@example.com").unwrap(), SenderAdd::AlreadyListed);
    for bad in ["", "bob", "bob@", "@x.com", "a b@x.com", "a@b@c.com", "a@nodot", "a@.com", "a@x."] {
        assert_eq!(db.add_sender(bad).unwrap(), SenderAdd::Invalid, "{bad}");
    }
    db.add_sender("amy@x.org").unwrap();
    let l = db.list_senders().unwrap();
    assert_eq!(l.iter().map(|s| s.email.as_str()).collect::<Vec<_>>(), vec!["amy@x.org", "bob@example.com"]);
    assert!(l.iter().all(|s| !s.always_html && !s.always_images));

    assert!(db.set_sender_pref("BOB@example.com", "html", true).unwrap());
    assert!(db.set_sender_pref("bob@example.com", "images", true).unwrap());
    assert!(db.set_sender_pref("bob@example.com", "html", false).unwrap());
    assert!(!db.set_sender_pref("bob@example.com", "bogus", true).unwrap());
    assert!(!db.set_sender_pref("nobody@x.com", "html", true).unwrap());
    let bob = db.list_senders().unwrap().into_iter().find(|s| s.email == "bob@example.com").unwrap();
    assert_eq!((bob.always_html, bob.always_images), (false, true));

    assert!(db.remove_sender("Amy@x.org").unwrap());
    assert!(!db.remove_sender("amy@x.org").unwrap());
    assert_eq!(db.list_senders().unwrap().len(), 1);
}

#[test]
fn insert_account_uses_given_id_and_rejects_duplicates() {
    let db = Database::open(":memory:").unwrap();
    db.insert_account("exchange-abc123", "exchange", Some("a@x.com"), None, "{}").unwrap();
    assert!(db.insert_account("exchange-abc123", "exchange", Some("b@x.com"), None, "{}").is_err());
    assert!(db.insert_account("exchange-def456", "exchange", Some("a@x.com"), None, "{}").is_err());
}

#[test]
fn writes_are_stamped_with_the_handle_account() {
    let db = Database::open_for_account(":memory:", "gmail-123abc").unwrap();
    db.insert_email(&EmailMessage {
        id: "m1".into(), from: "x@y".into(), subject: "s".into(), received: "2026-10-01T00:00:00Z".into(),
        body: "b".into(), folder_id: Some("f".into()), is_read: false, ..Default::default()
    }).unwrap();
    db.upsert_event(&CalendarEvent {
        id: "e1".into(), subject: "s".into(), body: "".into(), start: "2026-10-01T09:00:00".into(),
        end: "2026-10-01T10:00:00".into(), is_all_day: false, time_zone: "UTC".into(), ..Default::default()
    }).unwrap();
    let m: String = db.conn.query_row("SELECT account_id FROM messages WHERE id='m1'", [], |r| r.get(0)).unwrap();
    let e: String = db.conn.query_row("SELECT account_id FROM calendar_events WHERE id='e1'", [], |r| r.get(0)).unwrap();
    assert_eq!((m.as_str(), e.as_str()), ("gmail-123abc", "gmail-123abc"));
}

#[test]
fn existing_rows_are_backfilled_to_the_legacy_account() {
    // Build a pre-multi-account database (no accounts table, no account_id).
    let path = temp_db_path("backfill");
    {
        let c = Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE messages (id TEXT PRIMARY KEY, subject TEXT NOT NULL, from_email TEXT NOT NULL, from_name TEXT,
                body TEXT NOT NULL, received_at DATETIME NOT NULL, is_read BOOLEAN DEFAULT 0,
                cached_at DATETIME DEFAULT CURRENT_TIMESTAMP, folder_id TEXT);
             CREATE TABLE folders (id TEXT PRIMARY KEY, display_name TEXT NOT NULL, parent_folder_id TEXT,
                unread_item_count INTEGER DEFAULT 0, total_item_count INTEGER DEFAULT 0, well_known_name TEXT,
                sort_order INTEGER DEFAULT 999, cached_at DATETIME DEFAULT CURRENT_TIMESTAMP);
             CREATE TABLE calendar_events (id TEXT PRIMARY KEY, subject TEXT NOT NULL, body TEXT NOT NULL DEFAULT '',
                start_at TEXT NOT NULL, end_at TEXT NOT NULL, is_all_day BOOLEAN DEFAULT 0, time_zone TEXT,
                cached_at DATETIME DEFAULT CURRENT_TIMESTAMP);
             INSERT INTO messages (id, subject, from_email, body, received_at) VALUES ('m1','s','a@b','x','2026-01-01'),('m2','s','a@b','x','2026-01-02');
             INSERT INTO folders (id, display_name) VALUES ('f1','Inbox');
             INSERT INTO calendar_events (id, subject, start_at, end_at) VALUES ('e1','s','2026-01-01T09:00:00','2026-01-01T10:00:00');",
        ).unwrap();
    }
    let db = Database::open(&path).unwrap();
    for (table, n) in [("messages", 2), ("folders", 1), ("calendar_events", 1)] {
        let c: i32 = db.conn.query_row(&format!("SELECT COUNT(*) FROM {} WHERE account_id = 'exchange-primary'", table), [], |r| r.get(0)).unwrap();
        let nulls: i32 = db.conn.query_row(&format!("SELECT COUNT(*) FROM {} WHERE account_id IS NULL", table), [], |r| r.get(0)).unwrap();
        assert_eq!((c, nulls), (n, 0), "{}", table);
    }
    let trig: i32 = db.conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name='messages_au'", [], |r| r.get(0)).unwrap();
    assert_eq!(trig, 1, "FTS update trigger must be restored after backfill");
    let accts = db.list_accounts().unwrap();
    assert_eq!(accts.len(), 1);
    assert_eq!((accts[0].id.as_str(), accts[0].provider.as_str()), ("exchange-primary", "exchange"));

    // Re-opening is idempotent.
    drop(db);
    let db = Database::open(&path).unwrap();
    assert_eq!(db.list_accounts().unwrap().len(), 1);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn delete_account_removes_only_its_data() {
    let a = Database::open_for_account(":memory:", "gmail-aaaaaa").unwrap();
    let ev = |id: &str| CalendarEvent { id: id.into(), subject: "s".into(), body: "".into(), start: "2026-10-01T09:00:00".into(),
        end: "2026-10-01T10:00:00".into(), is_all_day: false, time_zone: "UTC".into(), ..Default::default() };
    a.upsert_event(&ev("e1")).unwrap();
    a.conn.execute("INSERT INTO calendar_events (id, subject, start_at, end_at, account_id) VALUES ('e2','s','2026-10-01T09:00:00','2026-10-01T10:00:00','gmail-bbbbbb')", []).unwrap();
    a.delete_account("gmail-aaaaaa").unwrap();
    let left: Vec<String> = a.conn.prepare("SELECT id FROM calendar_events").unwrap()
        .query_map([], |r| r.get(0)).unwrap().filter_map(|r| r.ok()).collect();
    assert_eq!(left, vec!["e2"]);
    assert!(a.list_accounts().unwrap().is_empty());
}
