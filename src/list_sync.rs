//! The two-way list link (docs/PLAN-contacts.md section 7). Graph cannot create Outlook's personal
//! contact groups, so a list is mirrored as a contact category named `OL: <list>`: the members of the
//! list are the contacts carrying it (a contact stands for its first address). Both sides change
//! independently, so the sets are merged three ways against what was in step last time; only
//! differences are written, and only to the contacts that need it.
//!
//! Safety rules (each has a test): nothing runs while the contact cache is incomplete or a contact write
//! is waiting or failing (a missing category would look like a removal in Outlook); the first sync only
//! adds; a write guards against the version it read (a stale copy conflicts, it never wipes); a member
//! that could not be mirrored is left out of the base, so it is never mistaken for a remote removal.

use crate::db::Database;
use crate::db::list_link::LinkContact;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU32, Ordering};

pub const PREFIX: &str = "OL: ";

pub fn category_of(list_name: &str) -> String { format!("{PREFIX}{}", list_name.trim()) }

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Merge {
    pub merged: BTreeSet<String>,
    pub add_local: BTreeSet<String>,
    pub remove_local: BTreeSet<String>,
    pub add_remote: BTreeSet<String>,
    pub remove_remote: BTreeSet<String>,
}

/// Three-way set merge. `base` None = never synced: nothing can have been removed, so the union wins.
/// Removed on one side and unchanged on the other: removed. Added on either side: kept (add wins).
pub fn merge(base: Option<&BTreeSet<String>>, local: &BTreeSet<String>, remote: &BTreeSet<String>) -> Merge {
    let mut merged = BTreeSet::new();
    for x in local.union(remote) {
        let in_base = base.map(|b| b.contains(x)).unwrap_or(false);
        if in_base && !(local.contains(x) && remote.contains(x)) { continue; }
        merged.insert(x.clone());
    }
    // in base, gone from one side: gone from the result (the loop skipped it); also gone from the other side
    Merge {
        add_local: merged.difference(local).cloned().collect(),
        add_remote: merged.difference(remote).cloned().collect(),
        remove_local: local.difference(&merged).cloned().collect(),
        remove_remote: remote.difference(&merged).cloned().collect(),
        merged,
    }
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ListReport {
    pub list: String,
    pub category: String,
    pub added_local: usize,
    pub removed_local: usize,
    pub added_remote: usize,
    pub removed_remote: usize,
    /// Members that could not be mirrored, with the reason.
    pub not_mirrored: Vec<(String, String)>,
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Report {
    pub account: String,
    /// Why nothing ran, if so.
    pub skipped: Option<String>,
    pub lists: Vec<ListReport>,
}

impl Report {
    pub fn wrote_contacts(&self) -> bool { self.lists.iter().any(|l| l.added_remote + l.removed_remote > 0) }
}

static LAST: std::sync::OnceLock<std::sync::Mutex<BTreeMap<String, Report>>> = std::sync::OnceLock::new();

/// The most recent report of every account (for the Lists page).
pub fn last_reports() -> Vec<Report> {
    LAST.get_or_init(Default::default).lock().map(|m| m.values().cloned().collect()).unwrap_or_default()
}

/// Run the link for an account's database, remember the report, wake the contact outbox if contacts were written.
pub fn run_and_record(db: &Database, full_walk_done: bool, policy: CreatePolicy) -> Report {
    let r = sync_account(db, full_walk_done, policy);
    if r.wrote_contacts() || r.lists.iter().any(|l| l.added_remote > 0) { crate::sync_state::request_contact_outbox_run(); }
    if let Ok(mut m) = LAST.get_or_init(Default::default).lock() { m.insert(r.account.clone(), r.clone()); }
    r
}

static SEQ: AtomicU32 = AtomicU32::new(0);
fn seq() -> u32 { 0x800 + (SEQ.fetch_add(1, Ordering::Relaxed) & 0x7ff) }

/// What to do with the contact for a list member that has none in the account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreatePolicy { Skip, Always }

/// Run the link for one account (its database handle). `full_walk_done`: the contact cache is complete.
pub fn sync_account(db: &Database, full_walk_done: bool, policy: CreatePolicy) -> Report {
    let account = db.account_id().to_string();
    let mut report = Report { account: account.clone(), ..Default::default() };
    match db.list_sync_targets() {
        Ok(t) if t.iter().any(|a| a == &account) => {}
        _ => { report.skipped = Some("this account is not a mirror target".into()); return report; }
    }
    if !full_walk_done { report.skipped = Some("the contacts have not been read completely yet".into()); return report; }
    match db.contact_writes_open() {
        Ok(0) => {}
        Ok(n) => { report.skipped = Some(format!("{n} contact write(s) are waiting or need attention")); return report; }
        Err(e) => { report.skipped = Some(format!("database: {e}")); return report; }
    }

    // Lists made by another OmaLook (an `OL: ` category with no local list) are adopted.
    if let Ok(cats) = db.categories_with_prefix(PREFIX) {
        let skip = db.list_link_not_adoptable().unwrap_or_default();
        let have: BTreeSet<String> = db.lists_all().unwrap_or_default().into_iter().map(|l| l.name.trim().to_lowercase()).collect();
        for c in cats {
            let name = c[PREFIX.len()..].trim().to_string();
            if !name.is_empty() && !have.contains(&name.to_lowercase()) && !skip.contains(&c.to_lowercase()) {
                if let Ok((_, Some(_))) = db.list_create(&name) { let _ = db.list_link_log_add(&account, &name, "adopted from the category on the contacts"); }
            }
        }
    }

    for list in db.lists_all().unwrap_or_default() {
        report.lists.push(sync_list(db, &account, &list, policy));
    }
    report
}

fn sync_list(db: &Database, account: &str, list: &crate::db::lists::DlList, policy: CreatePolicy) -> ListReport {
    let category = category_of(&list.name);
    let mut rep = ListReport { list: list.name.clone(), category: category.clone(), ..Default::default() };
    let log = |text: String| { let _ = db.list_link_log_add(account, &list.name, &text); };

    let base = db.list_link_base(list.id, account).ok().flatten();
    let old_category = base.as_ref().map(|b| b.0.clone()).filter(|c| !c.eq_ignore_ascii_case(&category));
    let base_set = base.as_ref().map(|b| b.1.clone());

    // remote side: contacts carrying the category (or the old name of a renamed list)
    let mut carriers: Vec<LinkContact> = db.contacts_with_category(&category).unwrap_or_default();
    if let Some(old) = &old_category {
        for c in db.contacts_with_category(old).unwrap_or_default() {
            if !carriers.iter().any(|x| x.id == c.id) { carriers.push(c); }
        }
    }
    let mut remote: BTreeSet<String> = BTreeSet::new();
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    for c in &carriers {
        match c.address() {
            Some(a) => { names.insert(a.clone(), c.name.clone()); remote.insert(a); }
            None => rep.not_mirrored.push((c.name.clone(), "the contact has no email address".into())),
        }
    }
    let local: BTreeSet<String> = list.members.iter().map(|m| m.email.trim().to_lowercase()).collect();
    for m in &list.members { names.entry(m.email.trim().to_lowercase()).or_insert_with(|| m.name.clone()); }

    let m = merge(base_set.as_ref(), &local, &remote);

    // ---- local side
    for a in &m.add_local {
        if let Ok(crate::db::lists::ListOp::Done) = db.list_member_add(list.id, a, names.get(a).map(String::as_str).unwrap_or("")) {
            rep.added_local += 1;
            log(format!("added {a} (the contact has the category in Exchange)"));
        }
    }
    for a in &m.remove_local {
        if db.list_member_remove(list.id, a).unwrap_or(false) {
            rep.removed_local += 1;
            log(format!("removed {a} (it lost the category in Exchange)"));
        }
    }

    // ---- remote side: one patch per contact
    let mut wanted: BTreeMap<String, Vec<String>> = BTreeMap::new(); // contact id -> categories after
    let current = |c: &LinkContact| c.categories.clone();
    let mut pushed: BTreeSet<String> = BTreeSet::new();
    for a in &m.add_remote {
        let found = db.contacts_by_first_address(a).unwrap_or_default();
        if let Some(c) = found.first() {
            let mut cats = wanted.get(&c.id).cloned().unwrap_or_else(|| current(c));
            if !cats.iter().any(|x| x.eq_ignore_ascii_case(&category)) { cats.push(category.clone()); }
            wanted.insert(c.id.clone(), cats);
            pushed.insert(a.clone());
            rep.added_remote += 1;
            log(format!("tagged the contact {a} in Exchange"));
        } else if policy == CreatePolicy::Always {
            let name = names.get(a).cloned().unwrap_or_default();
            let mut fields = json!({ "emailAddresses": [{ "address": a }], "categories": [category] });
            if !name.is_empty() && name != *a { fields["displayName"] = json!(name); }
            let r = crate::contacts_write::save(db, &json!({ "fields": fields }), seq());
            if r["ok"] == true {
                pushed.insert(a.clone());
                rep.added_remote += 1;
                log(format!("created a contact for {a} in Exchange"));
            } else {
                rep.not_mirrored.push((a.clone(), r["error"].as_str().unwrap_or("could not create the contact").to_string()));
            }
        } else {
            let has_other = db.contacts_by_first_address(a).map(|v| v.is_empty()).unwrap_or(true);
            let _ = has_other;
            rep.not_mirrored.push((a.clone(), "no contact in this account has it as its first address".into()));
        }
    }
    for a in &m.remove_remote {
        for c in carriers.iter().filter(|c| c.address().as_deref() == Some(a.as_str())) {
            let cats: Vec<String> = wanted.get(&c.id).cloned().unwrap_or_else(|| current(c)).into_iter()
                .filter(|x| !x.eq_ignore_ascii_case(&category) && !old_category.as_deref().map(|o| x.eq_ignore_ascii_case(o)).unwrap_or(false))
                .collect();
            wanted.insert(c.id.clone(), cats);
        }
        rep.removed_remote += 1;
        log(format!("took the category off {a} in Exchange"));
    }
    // a renamed list moves its members to the new category name
    if let Some(old) = &old_category {
        for c in carriers.iter().filter(|c| c.has_category(old)) {
            if let Some(a) = c.address() {
                if m.merged.contains(&a) && !m.remove_remote.contains(&a) {
                    let mut cats: Vec<String> = wanted.get(&c.id).cloned().unwrap_or_else(|| current(c)).into_iter().filter(|x| !x.eq_ignore_ascii_case(old)).collect();
                    if !cats.iter().any(|x| x.eq_ignore_ascii_case(&category)) { cats.push(category.clone()); }
                    wanted.insert(c.id.clone(), cats);
                }
            }
        }
        log(format!("the list was renamed: contacts move from '{old}' to '{category}'"));
    }
    for (id, cats) in wanted {
        let r = crate::contacts_write::save(db, &json!({ "id": id, "fields": { "categories": cats } }), seq());
        if r["ok"] != true {
            log(format!("could not update a contact: {}", r["error"].as_str().unwrap_or("?")));
        }
    }

    // base: what is now the same on both sides. A member that could not be mirrored stays out.
    let unmirrored: BTreeSet<&String> = rep.not_mirrored.iter().map(|(a, _)| a).collect();
    let new_base: BTreeSet<String> = m.merged.iter().filter(|a| (remote.contains(*a) || pushed.contains(*a)) && !unmirrored.contains(a)).cloned().collect();
    let _ = db.set_list_link_base(list.id, account, &category, &new_base);
    rep
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::lists::ListOp;
    use crate::models::ContactFolder;

    fn s(v: &[&str]) -> BTreeSet<String> { v.iter().map(|x| x.to_string()).collect() }

    #[test]
    fn merge_matrix() {
        // first sync: union, nothing removed
        let m = merge(None, &s(&["a", "b"]), &s(&["b", "c"]));
        assert_eq!(m.merged, s(&["a", "b", "c"]));
        assert_eq!(m.add_local, s(&["c"]));
        assert_eq!(m.add_remote, s(&["a"]));
        assert!(m.remove_local.is_empty() && m.remove_remote.is_empty());
        // add on both sides
        let b = s(&["x"]);
        let m = merge(Some(&b), &s(&["x", "a"]), &s(&["x", "b"]));
        assert_eq!(m.merged, s(&["x", "a", "b"]));
        // removed locally, unchanged remotely: removed remotely
        let m = merge(Some(&b), &s(&[]), &s(&["x"]));
        assert_eq!(m.merged, s(&[]));
        assert_eq!(m.remove_remote, s(&["x"]));
        // removed remotely, unchanged locally: removed locally
        let m = merge(Some(&b), &s(&["x"]), &s(&[]));
        assert_eq!(m.remove_local, s(&["x"]));
        // removed on both
        let m = merge(Some(&b), &s(&[]), &s(&[]));
        assert!(m.merged.is_empty() && m.remove_local.is_empty() && m.remove_remote.is_empty());
        // nothing changed
        let m = merge(Some(&b), &b, &b);
        assert_eq!(m, Merge { merged: b.clone(), ..Default::default() });
        // removed here, added back there under the same address: it is in base, so the add cannot be told from "unchanged": removed
        // (the address was in base on both sides before; the only add-wins case is a member that was not in base)
        let m = merge(Some(&s(&["x"])), &s(&["y"]), &s(&["x", "y"]));
        assert_eq!(m.merged, s(&["y"]));
    }

    fn setup() -> Database {
        let db = Database::open_for_account(":memory:", "exchange-aaaaaa").unwrap();
        db.upsert_contact_folder(&ContactFolder { id: "f1".into(), display_name: "Contacts".into(), parent_folder_id: None }).unwrap();
        db.set_list_sync_target("exchange-aaaaaa", true).unwrap();
        db
    }
    fn contact(db: &Database, id: &str, email: &str, cats: &[&str]) {
        db.put_contact_json(&json!({"id": id, "@odata.etag": "W/\"1\"", "displayName": id, "emailAddresses": [{"address": email}], "categories": cats}), Some("f1"), "ok").unwrap();
    }
    fn members(db: &Database, list: i64) -> BTreeSet<String> { db.list_members(list).unwrap().unwrap().into_iter().map(|m| m.email).collect() }
    fn patch_of(db: &Database) -> Vec<(String, serde_json::Value)> {
        db.contact_outbox_claim_due(Database::now_ms() + 1).unwrap().into_iter()
            .map(|o| (o.2.clone(), serde_json::from_str::<serde_json::Value>(&o.3).unwrap()["patch"].clone())).collect()
    }

    #[test]
    fn not_a_target_or_not_ready_or_writes_waiting_means_nothing_runs() {
        let db = setup();
        db.set_list_sync_target("exchange-aaaaaa", false).unwrap();
        assert!(sync_account(&db, true, CreatePolicy::Skip).skipped.unwrap().contains("not a mirror target"));
        db.set_list_sync_target("exchange-aaaaaa", true).unwrap();
        assert!(sync_account(&db, false, CreatePolicy::Skip).skipped.unwrap().contains("not been read"));
        contact(&db, "c1", "a@x.org", &[]);
        crate::contacts_write::save(&db, &json!({"id":"c1","fields":{"givenName":"A"}}), 1);
        assert!(sync_account(&db, true, CreatePolicy::Skip).skipped.unwrap().contains("waiting"));
    }

    #[test]
    fn contacts_with_the_category_become_members_and_a_new_list_is_adopted() {
        let db = setup();
        contact(&db, "c1", "a@x.org", &["OL: Family"]);
        contact(&db, "c2", "b@x.org", &["Other"]);
        let r = sync_account(&db, true, CreatePolicy::Skip);
        assert!(r.skipped.is_none());
        let lists = db.lists_all().unwrap();
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].name, "Family");
        assert_eq!(members(&db, lists[0].id), s(&["a@x.org"]));
        assert!(patch_of(&db).is_empty(), "nothing is written back for a list that came from the contacts");
        let (_, base) = db.list_link_base(lists[0].id, "exchange-aaaaaa").unwrap().unwrap();
        assert_eq!(base, s(&["a@x.org"]));
    }

    #[test]
    fn a_local_member_tags_the_contact_without_losing_its_other_categories() {
        let db = setup();
        contact(&db, "c1", "a@x.org", &["Blue"]);
        let (_, id) = db.list_create("Family").unwrap();
        let id = id.unwrap();
        assert!(matches!(db.list_member_add(id, "A@X.org", "Ada").unwrap(), ListOp::Done));
        let r = sync_account(&db, true, CreatePolicy::Skip);
        assert_eq!(r.lists[0].added_remote, 1);
        let p = patch_of(&db);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].0, "c1");
        assert_eq!(p[0].1["categories"], json!(["Blue", "OL: Family"]));
    }

    #[test]
    fn a_member_with_no_contact_is_skipped_or_created_and_never_mistaken_for_a_removal() {
        let db = setup();
        let (_, id) = db.list_create("Family").unwrap();
        let id = id.unwrap();
        db.list_member_add(id, "new@x.org", "New One").unwrap();
        let r = sync_account(&db, true, CreatePolicy::Skip);
        assert_eq!(r.lists[0].not_mirrored.len(), 1);
        assert!(patch_of(&db).is_empty());
        // second run: still a member (it was never in the base, so it is not a remote removal)
        let r = sync_account(&db, true, CreatePolicy::Skip);
        assert_eq!(members(&db, id), s(&["new@x.org"]));
        assert_eq!(r.lists[0].removed_local, 0);
        // always: a contact is created with the address, the name and the category
        let r = sync_account(&db, true, CreatePolicy::Always);
        assert_eq!(r.lists[0].added_remote, 1);
        let due = db.contact_outbox_claim_due(Database::now_ms() + 1).unwrap();
        assert_eq!(due[0].1, "create");
        let p: serde_json::Value = serde_json::from_str(&due[0].3).unwrap();
        assert_eq!(p["patch"]["categories"], json!(["OL: Family"]));
        assert_eq!(p["patch"]["emailAddresses"][0]["address"], json!("new@x.org"));
        assert_eq!(p["patch"]["displayName"], json!("New One"));
    }

    #[test]
    fn a_category_removed_in_exchange_removes_the_member_and_one_removed_here_takes_the_category_off() {
        let db = setup();
        contact(&db, "c1", "a@x.org", &["OL: Family", "Blue"]);
        contact(&db, "c2", "b@x.org", &["OL: Family"]);
        sync_account(&db, true, CreatePolicy::Skip);
        let id = db.lists_all().unwrap()[0].id;
        assert_eq!(members(&db, id), s(&["a@x.org", "b@x.org"]));
        // Outlook takes the category off c2; here a@x.org is removed from the list
        contact(&db, "c2", "b@x.org", &[]);
        db.list_member_remove(id, "a@x.org").unwrap();
        let r = sync_account(&db, true, CreatePolicy::Skip);
        assert_eq!(r.lists[0].removed_local, 1);
        assert_eq!(r.lists[0].removed_remote, 1);
        assert!(members(&db, id).is_empty());
        let p = patch_of(&db);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].1["categories"], json!(["Blue"]), "only this list's category goes, the others stay");
    }

    #[test]
    fn renaming_a_list_moves_its_contacts_to_the_new_category() {
        let db = setup();
        contact(&db, "c1", "a@x.org", &["OL: Family"]);
        sync_account(&db, true, CreatePolicy::Skip);
        let id = db.lists_all().unwrap()[0].id;
        db.list_rename(id, "Relatives").unwrap();
        let r = sync_account(&db, true, CreatePolicy::Skip);
        assert_eq!(r.lists[0].category, "OL: Relatives");
        let p = patch_of(&db);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].1["categories"], json!(["OL: Relatives"]));
        assert_eq!(members(&db, id), s(&["a@x.org"]), "the list keeps its members");
    }

    #[test]
    fn a_deleted_list_does_not_come_back_but_a_new_one_of_the_same_name_may() {
        let db = setup();
        contact(&db, "c1", "a@x.org", &["OL: Family"]);
        sync_account(&db, true, CreatePolicy::Skip);
        let id = db.lists_all().unwrap()[0].id;
        db.list_delete(id).unwrap();
        sync_account(&db, true, CreatePolicy::Skip);
        assert!(db.lists_all().unwrap().is_empty(), "not adopted again");
        let (_, id) = db.list_create("Family").unwrap();
        let r = sync_account(&db, true, CreatePolicy::Skip);
        assert_eq!(members(&db, id.unwrap()), s(&["a@x.org"]), "{r:?}");
    }

    #[test]
    fn an_address_that_is_only_a_second_email_is_reported_not_forced() {
        let db = setup();
        db.put_contact_json(&json!({"id":"c1","displayName":"Two","emailAddresses":[{"address":"first@x.org"},{"address":"second@x.org"}],"categories":[]}), Some("f1"), "ok").unwrap();
        let (_, id) = db.list_create("L").unwrap();
        db.list_member_add(id.unwrap(), "second@x.org", "").unwrap();
        let r = sync_account(&db, true, CreatePolicy::Skip);
        assert_eq!(r.lists[0].not_mirrored[0].0, "second@x.org");
        assert!(patch_of(&db).is_empty());
    }
}
