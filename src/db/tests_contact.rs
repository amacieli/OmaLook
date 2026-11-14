//! Unit tests (moved from db.rs; module body dedented by 4 spaces, otherwise verbatim).

use super::*;

fn c(id: &str, given: &str, sur: &str, company: &str, created: &str) -> Contact {
    Contact {
        id: id.into(), display_name: format!("{} {}", given, sur).trim().to_string(),
        given_name: given.into(), surname: sur.into(), company: company.into(),
        created_at: created.into(), ..Default::default()
    }
}
#[test]
fn the_old_pseudo_default_folder_row_is_removed_and_two_accounts_keep_their_own_folders() {
    let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
    db.upsert_contact_folder(&ContactFolder { id: "contacts".into(), display_name: "Contacts".into(), parent_folder_id: None }).unwrap();
    db.upsert_contact_folder(&ContactFolder { id: "AQM-real-a".into(), display_name: "Contacts".into(), parent_folder_id: None }).unwrap();
    db.init_contacts_ext_schema().unwrap();
    let names: Vec<String> = db.get_contact_folders().unwrap().into_iter().map(|f| f.id).collect();
    assert_eq!(names, vec!["AQM-real-a"], "the colliding pseudo id is gone");
}

#[test]
fn contact_search_filters_by_every_word_folder_and_category_and_hides_deleted_rows() {
    let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
    let mk = |id: &str, raw: serde_json::Value, folder: &str| {
        let mut v = raw; v["id"] = id.into();
        db.put_contact_json(&v, Some(folder), "ok").unwrap();
    };
    mk("1", serde_json::json!({"displayName": "Ada Lovelace", "companyName": "Engines", "categories": ["OL: Family"]}), "f1");
    mk("2", serde_json::json!({"displayName": "Bob Engine", "personalNotes": "likes ada"}), "f2");
    mk("3", serde_json::json!({"displayName": "Gone Person"}), "f1");
    db.set_contact_state("3", "deleted").unwrap();
    let q = |s: Option<&str>, f: Option<&str>, c: Option<&str>| -> Vec<String> {
        db.query_contacts_full("all", "first", None, s, f, c).unwrap().into_iter().map(|r| r.contact.id).collect()
    };
    assert_eq!(q(None, None, None).len(), 2, "deleted row hidden");
    assert_eq!(q(Some("ada"), None, None), vec!["1", "2"], "word matches names and notes");
    assert_eq!(q(Some("ada engines"), None, None), vec!["1"], "every word must match");
    assert_eq!(q(None, Some("f2"), None), vec!["2"]);
    assert_eq!(q(None, None, Some("ol: family")), vec!["1"], "category is exact, ignoring case");
    assert!(q(Some("100%"), None, None).is_empty(), "LIKE wildcards in the query are not wildcards");
}

#[test]
fn category_and_search_values_with_like_wildcards_match_literally() {
    let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
    let mk = |id: &str, raw: serde_json::Value| {
        let mut v = raw; v["id"] = id.into();
        db.put_contact_json(&v, Some("f1"), "ok").unwrap();
    };
    mk("u", serde_json::json!({"displayName": "Mr_Smith", "categories": ["Team_A"]}));
    mk("x", serde_json::json!({"displayName": "MrXSmith", "categories": ["TeamXA"]}));
    mk("p", serde_json::json!({"displayName": "Ann 100% Sure", "categories": ["Team%"]}));
    mk("q", serde_json::json!({"displayName": "Plain Bob", "categories": ["Teams"]}));
    let q = |s: Option<&str>, c: Option<&str>| -> Vec<String> {
        db.query_contacts_full("all", "first", None, s, None, c).unwrap().into_iter().map(|r| r.contact.id).collect()
    };
    assert_eq!(q(None, Some("team_a")), vec!["u"], "`_` in a category is literal, not any character");
    assert_eq!(q(None, Some("team%")), vec!["p"], "`%` in a category is literal, not everything");
    assert_eq!(q(Some("_"), None), vec!["u"], "`_` in a search word matches only the literal underscore");
    assert_eq!(q(Some("100%"), None), vec!["p"], "`%` in a search word matches only the literal percent sign");
    assert!(q(Some("%"), None).iter().all(|id| id == "p"), "a bare `%` must not match every contact");
}

fn ids(rows: &[ContactRow]) -> Vec<&str> { rows.iter().map(|r| r.contact.id.as_str()).collect() }

fn seeded() -> Database {
    let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
    db.upsert_contact_folder(&ContactFolder { id: "f1".into(), display_name: "Zeta list".into(), parent_folder_id: None }).unwrap();
    db.upsert_contact_folder(&ContactFolder { id: "f2".into(), display_name: "Alpha list".into(), parent_folder_id: None }).unwrap();
    let mut a = c("a", "Bob", "Zimmer", "", "2026-01-01T00:00:00Z"); a.folder_id = Some("f1".into());
    let mut b = c("b", "alice", "Young", "Acme", "2026-03-01T00:00:00Z"); b.folder_id = Some("f2".into());
    let d = c("d", "", "", "Zorg", "2026-02-01T00:00:00Z");           // no first/last name
    db.upsert_contact(&a).unwrap(); db.upsert_contact(&b).unwrap(); db.upsert_contact(&d).unwrap();
    db
}

#[test]
fn sorts_by_each_key_with_missing_values_last() {
    let db = seeded();
    assert_eq!(ids(&db.query_contacts("all", "first").unwrap()), vec!["b", "a", "d"], "case-insensitive, empty last");
    assert_eq!(ids(&db.query_contacts("all", "last").unwrap()), vec!["b", "a", "d"]);
    assert_eq!(ids(&db.query_contacts("all", "company").unwrap()), vec!["b", "d", "a"]);
    assert_eq!(ids(&db.query_contacts("all", "recent").unwrap()), vec!["b", "d", "a"]);
}

#[test]
fn favorites_view_and_sync_never_clears_a_favorite() {
    let db = seeded();
    db.set_contact_favorite("a", true).unwrap();
    assert_eq!(ids(&db.query_contacts("favorites", "first").unwrap()), vec!["a"]);
    // a sync re-upserts the same contact with a changed name: favorite must survive
    let mut a = c("a", "Robert", "Zimmer", "", "2026-01-01T00:00:00Z"); a.folder_id = Some("f1".into());
    db.upsert_contact(&a).unwrap();
    let fav = db.query_contacts("favorites", "first").unwrap();
    assert_eq!(fav.len(), 1);
    assert_eq!(fav[0].contact.given_name, "Robert");
    assert!(fav[0].is_favorite);
}

#[test]
fn lists_view_groups_by_folder_alphabetically() {
    let db = seeded();
    let rows = db.query_contacts("lists", "first").unwrap();
    // Alpha list (b), Contacts (d: no folder), Zeta list (a)
    assert_eq!(ids(&rows), vec!["b", "d", "a"]);
    assert_eq!(rows[0].folder_name, "Alpha list");
    assert_eq!(rows[1].folder_name, "Contacts");
}

#[test]
fn list_fields_round_trip() {
    let db = Database::open(":memory:").unwrap();
    let mut x = c("x", "A", "B", "Co", "2026-01-01T00:00:00Z");
    x.emails = vec!["a@x.com".into(), "b@x.com".into()];
    x.phones = vec![ContactPhone { kind: "mobile".into(), number: "555-1".into() }, ContactPhone { kind: "home".into(), number: "555-2".into() }];
    x.addresses = vec![ContactAddress { kind: "home".into(), text: "1 Main St, Town, ST 12345, US".into() }];
    db.upsert_contact(&x).unwrap();
    let got = &db.query_contacts("all", "first").unwrap()[0].contact;
    assert_eq!(got, &x);
}

#[test]
fn contacts_are_stamped_with_account_and_deleted_with_it() {
    let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
    db.upsert_contact(&c("x", "A", "B", "", "2026-01-01T00:00:00Z")).unwrap();
    let acct: String = db.conn.query_row("SELECT account_id FROM contacts WHERE id='x'", [], |r| r.get(0)).unwrap();
    assert_eq!(acct, "exchange-aaaaaa");
    db.delete_account("exchange-aaaaaa").unwrap();
    assert!(db.query_contacts("all", "first").unwrap().is_empty());
}
