//! Contacts (People view): the list, detail, folders, directory search, photos, favorites and every
//! write (save, move, categories, delete and undo, folders, merge, quick add, import, export), plus
//! `GET /addresses` (the compose suggestions harvested from mail). Moved from the legacy chain in
//! `main.rs` (R3.1, group G2). Every answer is 200 with JSON, as before; failures are reported
//! inside the JSON.

use crate::http::router::{Ctx, Lock, Pattern, Response, Route};
use crate::Database;
use log::warn;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

const MIB: usize = 1024 * 1024;
const KIB64: usize = 64 * 1024;

pub fn routes() -> Vec<Route> {
    use Pattern::Exact;
    vec![
        Route::new("GET", Exact("/contacts"), Lock::Exclusive, list_contacts),
        Route::new("GET", Exact("/contacts/get"), Lock::None, get_contact),
        Route::new("GET", Exact("/contacts/folders"), Lock::Exclusive, contact_folders),
        Route::new("GET", Exact("/contacts/directory"), Lock::None, directory),
        Route::new("GET", Exact("/contacts/photo"), Lock::Exclusive, photo_get),
        Route::new("GET", Exact("/addresses"), Lock::Exclusive, addresses),
        Route::new("POST", Exact("/contacts/favorite"), Lock::Exclusive, favorite),
        Route::new("POST", Exact("/contacts/merge"), Lock::Exclusive, merge),
        Route::new("POST", Exact("/contacts/quick"), Lock::Exclusive, quick),
        Route::new("POST", Exact("/contacts/export"), Lock::Exclusive, export),
        Route::new("POST", Exact("/contacts/import"), Lock::Exclusive, import),
        Route::new("POST", Exact("/contacts/photo/set"), Lock::Exclusive, photo_set),
        Route::new("POST", Exact("/contacts/save"), Lock::Exclusive, save),
        Route::new("POST", Exact("/contacts/categories"), Lock::Exclusive, categories),
        Route::new("POST", Exact("/contacts/move"), Lock::Exclusive, move_contacts),
        Route::new("POST", Exact("/contacts/folder/create"), Lock::Exclusive, folder_create),
        Route::new("POST", Exact("/contacts/folder/rename"), Lock::Exclusive, folder_rename),
        Route::new("POST", Exact("/contacts/folder/delete"), Lock::Exclusive, folder_delete),
        Route::new("POST", Exact("/contacts/delete"), Lock::Exclusive, delete),
        Route::new("POST", Exact("/contacts/undo"), Lock::Exclusive, undo),
        Route::new("POST", Exact("/contacts/retry"), Lock::Exclusive, retry),
        Route::new("POST", Exact("/contacts/discard"), Lock::Exclusive, discard),
    ]
}

/// Sequence numbers for the outbox ops this process queues (the low 12 bits, as before).
static SEQ: AtomicU32 = AtomicU32::new(0);
fn seq() -> u32 {
    SEQ.fetch_add(1, Ordering::Relaxed) & 0xfff
}

/// `{"ok": false, "error": m}` as a 200 answer.
fn refused(m: &str) -> Response {
    Response::json(serde_json::json!({ "ok": false, "error": m }).to_string())
}

/// The same answer as a bare string (for the helpers that build their own JSON).
fn err_json(m: &str) -> String {
    serde_json::json!({ "ok": false, "error": m }).to_string()
}

/// The request body as UTF-8, read the way the old `read_post_body` did: without a Content-Length
/// header it is an error (the body reader alone would call it empty), and it is capped at `limit`.
fn read_body(c: &Ctx, limit: usize) -> Result<String, Response> {
    if c.req.header("content-length").is_none() {
        return Err(refused("missing Content-Length"));
    }
    c.body_text(limit).map_err(|e| refused(&e))
}

/// A JSON body; an empty or unparsable one is `Null`, as the old `unwrap_or_default` gave.
fn parse(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_default()
}

/// The strings in a JSON array (anything else is empty).
fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

fn signed_in(account: &str) -> bool {
    crate::scheduler::is_running(account)
}

fn open_db(c: &Ctx) -> Result<Database, String> {
    Database::open(&c.db_path()).map_err(|e| format!("database: {}", e))
}

/// A write to the People view's address book: opens the account's database, runs `f` on it, wakes the
/// outbox worker. `account` is the account the write belongs to.
fn contacts_write_call(config_dir: &PathBuf, account: &str, f: impl FnOnce(&Database) -> Value) -> String {
    if account.is_empty() {
        return err_json("no contact account selected");
    }
    if !signed_in(account) {
        return err_json("that account is not signed in, so it cannot save contacts");
    }
    let path = config_dir.join("messages.db");
    let db = match Database::open_for_account(path.to_str().unwrap_or("messages.db"), account) {
        Ok(db) => db,
        Err(e) => return err_json(&format!("database: {}", e)),
    };
    let out = f(&db);
    crate::sync_state::request_contact_outbox_run();
    out.to_string()
}

/// The account a contact belongs to (its row says so).
fn account_of_contact(config_dir: &PathBuf, id: &str) -> String {
    let path = config_dir.join("messages.db");
    Database::open(path.to_str().unwrap_or("messages.db"))
        .ok()
        .and_then(|db| db.get_contact_record(id).ok().flatten())
        .map(|r| r.account_id)
        .unwrap_or_default()
}

// ---- GET /contacts ----------------------------------------------------------------------------

/// GET /contacts?view=all|favorites|lists|duplicates&sort=first|last|company|recent&q=&folder=&category=&account=
/// Answers the rows as a JSON array (`[]` when the database cannot be read).
fn list_contacts(c: &Ctx) -> Response {
    let view = c.query("view").unwrap_or_else(|| "all".to_string());
    let sort = c.query("sort").unwrap_or_else(|| "first".to_string());
    let dup_view = view == "duplicates";
    let body = match Database::open(&c.db_path()) {
        Ok(db) => match db.query_contacts_full(
            if dup_view { "all" } else { view.as_str() },
            &sort,
            c.query("account").as_deref(),
            c.query("q").as_deref(),
            c.query("folder").as_deref(),
            c.query("category").as_deref(),
        ) {
            Ok(rows) if dup_view => serde_json::to_string(&crate::contacts_api::duplicates_view(&db, rows)).unwrap_or_else(|_| "[]".to_string()),
            Ok(rows) => serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string()),
            Err(e) => {
                warn!("GET /contacts: query failed: {}", e);
                "[]".to_string()
            }
        },
        Err(e) => {
            warn!("GET /contacts: DB open failed: {}", e);
            "[]".to_string()
        }
    };
    Response::json(body)
}

/// POST /contacts/favorite?id=ID&value=true|false — the local favorite flag. Answers `ok` or `error`.
fn favorite(c: &Ctx) -> Response {
    let id = c.query("id").unwrap_or_default();
    let value = c.query("value").as_deref() == Some("true");
    let ok = Database::open(&c.db_path()).and_then(|db| db.set_contact_favorite(&id, value)).is_ok();
    Response::json(if ok { "ok" } else { "error" })
}

// ---- GET /contacts/get, /folders, /directory, /photo ------------------------------------------

/// GET /contacts/get?id=ID[&live=1] — the whole contact. `live=1` also asks the provider for the
/// extra MAPI fields and merges them in.
fn get_contact(c: &Ctx) -> Response {
    let id = c.query("id").unwrap_or_default();
    let live = c.query("live").as_deref() == Some("1");
    Response::json(match open_db(c) {
        Ok(db) if live => {
            // the editor wants the extra MAPI fields too: one request to the server, merged into the answer
            let mut d = crate::contacts_api::detail(&db, &id);
            let account = d["account_id"].as_str().unwrap_or("").to_string();
            if d["ok"] == true && !account.is_empty() && !id.starts_with("local-") && signed_in(&account) {
                let provider = crate::scheduler::contacts_provider_for(&account);
                if let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    if let Ok(list) = rt.block_on(provider.get_extended(&id, &crate::contacts_write::ext_ids())) {
                        d["contact"]["singleValueExtendedProperties"] = Value::Array(list.into_iter().map(|(i, v)| serde_json::json!({ "id": i, "value": v })).collect());
                    }
                }
            }
            d.to_string()
        }
        Ok(db) => crate::contacts_api::detail(&db, &id).to_string(),
        Err(e) => err_json(&e),
    })
}

/// GET /contacts/folders?account=ID — the contact folders.
fn contact_folders(c: &Ctx) -> Response {
    let account = c.query("account");
    Response::json(match open_db(c) {
        Ok(db) => crate::contacts_api::folders(&db, account.as_deref()).to_string(),
        Err(e) => err_json(&e),
    })
}

/// GET /contacts/directory?account=ID&q=TEXT — people the account's directory knows.
fn directory(c: &Ctx) -> Response {
    let account = c.query("account").unwrap_or_default();
    let q = c.query("q").unwrap_or_default();
    if account.is_empty() {
        return refused("no account selected");
    }
    if !signed_in(&account) {
        return refused("that account is not signed in");
    }
    let provider = crate::scheduler::contacts_provider_for(&account);
    let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        return refused("could not start");
    };
    Response::json(match rt.block_on(provider.directory_search(&q)) {
        Ok(list) => serde_json::json!({ "ok": true, "people": list }).to_string(),
        Err(e) => err_json(&crate::contacts_outbox::short_error(&e)),
    })
}

/// GET /contacts/photo?id=ID — the path of the contact's photo on disk, fetched in the background the first time.
fn photo_get(c: &Ctx) -> Response {
    let id = c.query("id").unwrap_or_default();
    Response::json(contact_photo_path(c.config_dir, &id))
}

fn photo_cache_file(id: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    let base = std::env::var("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache"));
    base.join("omalook").join("contact-photos").join(format!("{:016x}.jpg", h.finish()))
}

/// The photo's path on disk; a fetch in the background the first time (`pending`).
fn contact_photo_path(config_dir: &PathBuf, id: &str) -> String {
    let file = photo_cache_file(id);
    let none_marker = file.with_extension("none");
    let fresh = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|a| a < std::time::Duration::from_secs(24 * 3600)).unwrap_or(false);
    if file.exists() && fresh(&file) {
        return serde_json::json!({ "ok": true, "path": file.to_string_lossy() }).to_string();
    }
    if none_marker.exists() && fresh(&none_marker) {
        return serde_json::json!({ "ok": true, "path": "" }).to_string();
    }
    let account = account_of_contact(config_dir, id);
    if account.is_empty() || id.starts_with("local-") || !signed_in(&account) {
        return serde_json::json!({ "ok": true, "path": "" }).to_string();
    }
    let id = id.to_string();
    std::thread::spawn(move || {
        let provider = crate::scheduler::contacts_provider_for(&account);
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match rt.block_on(provider.get_photo(&id)) {
            Ok(Some(bytes)) if !bytes.is_empty() => {
                let _ = std::fs::write(&file, bytes);
                let _ = std::fs::remove_file(&none_marker);
            }
            Ok(_) => {
                let _ = std::fs::write(&none_marker, b"");
            }
            Err(_) => {}
        }
    });
    serde_json::json!({ "ok": true, "path": "", "pending": true }).to_string()
}

/// POST /contacts/photo/set `{id, handle}` — a JPEG file becomes the contact's photo (at once, not queued).
/// The handle must have been issued for this file (`/files/register`, purpose photo).
fn photo_set(c: &Ctx) -> Response {
    let body = match read_body(c, KIB64) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let req = parse(&body);
    Response::json(set_photo(c.config_dir, req["id"].as_str().unwrap_or(""), req["handle"].as_str().unwrap_or("")))
}

fn set_photo(config_dir: &PathBuf, id: &str, handle: &str) -> String {
    let Some(path) = crate::app::files::resolve(crate::app::files::Purpose::Photo, handle) else {
        return err_json("pick the picture again");
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return err_json("cannot read that file"),
    };
    if bytes.len() > 4 * 1024 * 1024 {
        return err_json("the picture is larger than 4 MB");
    }
    if !(bytes.len() > 3 && bytes[0] == 0xFF && bytes[1] == 0xD8) {
        return err_json("only JPEG pictures can be set (a .jpg file)");
    }
    let account = account_of_contact(config_dir, id);
    if account.is_empty() || id.starts_with("local-") {
        return err_json("that contact is not saved yet");
    }
    if !signed_in(&account) {
        return err_json("that account is not signed in");
    }
    let provider = crate::scheduler::contacts_provider_for(&account);
    let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        return err_json("could not start");
    };
    match rt.block_on(provider.set_photo(id, &bytes)) {
        Ok(_) => {
            let f = photo_cache_file(id);
            let _ = std::fs::write(&f, &bytes);
            let _ = std::fs::remove_file(f.with_extension("none"));
            serde_json::json!({ "ok": true, "path": f.to_string_lossy() }).to_string()
        }
        Err(e) => err_json(&crate::contacts_outbox::short_error(&e)),
    }
}

// ---- GET /addresses ---------------------------------------------------------------------------

/// The harvest is cached for a minute: the scan covers the newest messages and is reused for a minute.
static ADDRESS_CACHE: std::sync::OnceLock<std::sync::Mutex<Option<(std::time::Instant, String)>>> = std::sync::OnceLock::new();
const ADDRESS_SCAN_MESSAGES: usize = 40_000;
const ADDRESS_MAX_RETURNED: usize = 6_000;

/// GET /addresses → `[{email, name, score}]` seen in mail, best first.
fn addresses(c: &Ctx) -> Response {
    Response::json(addresses_json(c.config_dir))
}

fn addresses_json(config_dir: &PathBuf) -> String {
    let cache = ADDRESS_CACHE.get_or_init(|| std::sync::Mutex::new(None));
    if let Some((at, body)) = cache.lock().ok().and_then(|g| g.clone()) {
        if at.elapsed() < std::time::Duration::from_secs(60) {
            return body;
        }
    }
    let body = match Database::open(config_dir.join("messages.db").to_str().unwrap_or("messages.db"))
        .and_then(|db| db.harvest_addresses(ADDRESS_SCAN_MESSAGES))
    {
        Ok(v) => {
            let rows: Vec<Value> = v
                .iter()
                .take(ADDRESS_MAX_RETURNED)
                .map(|s| serde_json::json!({ "email": s.email, "name": s.name, "score": s.score }))
                .collect();
            Value::Array(rows).to_string()
        }
        Err(e) => return serde_json::json!({ "ok": false, "error": e.to_string() }).to_string(),
    };
    if let Ok(mut g) = cache.lock() {
        *g = Some((std::time::Instant::now(), body.clone()));
    }
    body
}

// ---- POST /contacts/* writes -------------------------------------------------------------------

/// POST /contacts/merge `{ids}` — merge duplicates into the first.
fn merge(c: &Ctx) -> Response {
    let body = match read_body(c, MIB) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let ids = strings(&parse(&body)["ids"]);
    let account = ids.first().map(|i| account_of_contact(c.config_dir, i)).unwrap_or_default();
    Response::json(contacts_write_call(c.config_dir, &account, |db| crate::contacts_write::merge(db, &ids, seq())))
}

/// POST /contacts/quick `{account_id, name, address}` — add a sender to the contacts.
fn quick(c: &Ctx) -> Response {
    let body = match read_body(c, KIB64) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let req = parse(&body);
    let account = req["account_id"].as_str().unwrap_or("").to_string();
    let (name, address) = (req["name"].as_str().unwrap_or("").to_string(), req["address"].as_str().unwrap_or("").to_string());
    Response::json(contacts_write_call(c.config_dir, &account, |db| crate::contacts_api::quick_add(db, &name, &address, seq())))
}

/// POST /contacts/export `{ids, format}` — the contacts to a file in the downloads folder.
fn export(c: &Ctx) -> Response {
    let body = match read_body(c, MIB) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let req = parse(&body);
    let ids = strings(&req["ids"]);
    let format = req["format"].as_str().unwrap_or("vcf").to_string();
    let dir = crate::attachments::download_dir(&c.config_dir.join("settings.toml"));
    Response::json(match open_db(c) {
        Ok(db) => crate::contacts_api::export(&db, &ids, &format, &dir).to_string(),
        Err(e) => err_json(&e),
    })
}

/// POST /contacts/import `{handle, account_id, folder_id}` — a vCard or CSV file into one folder.
/// SECURITY: the handle must have been issued for an import (`/files/register`, purpose import).
fn import(c: &Ctx) -> Response {
    let body = match read_body(c, KIB64) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let req = parse(&body);
    let Some(path) = crate::app::files::resolve(crate::app::files::Purpose::Import, req["handle"].as_str().unwrap_or("")) else {
        return refused("pick the file again");
    };
    let (account, folder) = (req["account_id"].as_str().unwrap_or("").to_string(), req["folder_id"].as_str().unwrap_or("").to_string());
    Response::json(contacts_write_call(c.config_dir, &account, |db| {
        let folder = if folder.is_empty() { db.default_contact_folder().ok().flatten().unwrap_or_default() } else { folder.clone() };
        if folder.is_empty() {
            return serde_json::json!({ "ok": false, "error": "this account's contact folder is not known yet" });
        }
        crate::contacts_api::import(db, &path, &folder, seq())
    }))
}

/// POST /contacts/save — create or update one contact (the body is the contact, as the editor sends it).
fn save(c: &Ctx) -> Response {
    let body = match read_body(c, MIB) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let req: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return refused(&format!("unreadable contact: {}", e)),
    };
    let id = req["id"].as_str().unwrap_or("").to_string();
    let account = if id.is_empty() { req["account_id"].as_str().unwrap_or("").to_string() } else { account_of_contact(c.config_dir, &id) };
    Response::json(contacts_write_call(c.config_dir, &account, |db| crate::contacts_write::save(db, &req, seq())))
}

/// POST /contacts/categories `{ids, add, remove}`; POST /contacts/move `{ids, folder_id}` (ids of
/// other accounts are skipped). Both answer `{ok, changed, skipped_other_account}` or `{ok:false,error}`.
fn categories(c: &Ctx) -> Response {
    set_or_move(c, false)
}

fn move_contacts(c: &Ctx) -> Response {
    set_or_move(c, true)
}

fn set_or_move(c: &Ctx, moving: bool) -> Response {
    let body = match read_body(c, MIB) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let req = parse(&body);
    let ids = strings(&req["ids"]);
    let folder = req["folder_id"].as_str().unwrap_or("").to_string();
    let target_account = if moving {
        Database::open(&c.db_path()).ok().and_then(|d| d.contact_folder_account(&folder).ok().flatten()).unwrap_or_default()
    } else {
        String::new()
    };
    let mut by: BTreeMap<String, Vec<String>> = Default::default();
    let mut elsewhere = 0;
    for i in ids {
        let a = account_of_contact(c.config_dir, &i);
        if moving && a != target_account {
            elsewhere += 1;
            continue;
        }
        by.entry(a).or_default().push(i);
    }
    let (add, remove) = (strings(&req["add"]), strings(&req["remove"]));
    let mut changed = 0;
    let mut last_err = String::new();
    for (acct, ids) in by {
        let out = contacts_write_call(c.config_dir, &acct, |db| {
            if moving {
                crate::contacts_write::move_contacts(db, &ids, &folder, seq())
            } else {
                crate::contacts_write::set_categories(db, &ids, &add, &remove, seq())
            }
        });
        let r: Value = serde_json::from_str(&out).unwrap_or_default();
        if r["ok"] == true {
            changed += r["moved"].as_i64().or(r["changed"].as_i64()).unwrap_or(0);
        } else {
            last_err = r["error"].as_str().unwrap_or("").to_string();
        }
    }
    Response::json(if changed == 0 && !last_err.is_empty() {
        err_json(&last_err)
    } else {
        serde_json::json!({ "ok": true, "changed": changed, "skipped_other_account": elsewhere }).to_string()
    })
}

fn folder_create(c: &Ctx) -> Response {
    folder_call(c, "create")
}

fn folder_rename(c: &Ctx) -> Response {
    folder_call(c, "rename")
}

fn folder_delete(c: &Ctx) -> Response {
    folder_call(c, "delete")
}

/// POST /contacts/folder/{create,rename,delete} — done at once, not queued: folders are rare and the
/// answer matters.
fn folder_call(c: &Ctx, action: &str) -> Response {
    let body = match read_body(c, KIB64) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let req = parse(&body);
    Response::json(contacts_folder_call(c.config_dir, action, &req))
}

fn contacts_folder_call(config_dir: &PathBuf, action: &str, req: &Value) -> String {
    let account = req["account_id"].as_str().unwrap_or("").to_string();
    if account.is_empty() {
        return err_json("no contact account selected");
    }
    if !signed_in(&account) {
        return err_json("that account is not signed in");
    }
    let name = req["name"].as_str().unwrap_or("").trim().to_string();
    let id = req["id"].as_str().unwrap_or("").to_string();
    let parent = req["parent_id"].as_str().filter(|p| !p.is_empty()).map(String::from);
    let path = config_dir.join("messages.db");
    let db = match Database::open_for_account(path.to_str().unwrap_or("messages.db"), &account) {
        Ok(d) => d,
        Err(e) => return err_json(&format!("database: {}", e)),
    };
    let provider = crate::scheduler::contacts_provider_for(&account);
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => return err_json(&e.to_string()),
    };
    let out: Result<Value, crate::errors::OmarchyError> = rt.block_on(async {
        match action {
            "create" => {
                if name.is_empty() {
                    return Ok(serde_json::json!({ "ok": false, "error": "the folder needs a name" }));
                }
                let f = provider.create_folder(parent.as_deref(), &name).await?;
                let _ = db.upsert_contact_folder(&f);
                Ok(serde_json::json!({ "ok": true, "id": f.id }))
            }
            "rename" => {
                if name.is_empty() || id.is_empty() {
                    return Ok(serde_json::json!({ "ok": false, "error": "the folder needs a name" }));
                }
                provider.rename_folder(&id, &name).await?;
                let old = db.list_contact_folders_ext(Some(&account)).unwrap_or_default().into_iter().find(|f| f["id"] == id.as_str());
                let _ = db.upsert_contact_folder(&crate::models::ContactFolder {
                    id: id.clone(),
                    display_name: name.clone(),
                    parent_folder_id: old.and_then(|f| f["parent_id"].as_str().map(String::from)),
                });
                Ok(serde_json::json!({ "ok": true }))
            }
            _ => {
                if id.is_empty() {
                    return Ok(serde_json::json!({ "ok": false, "error": "no folder given" }));
                }
                provider.delete_folder(&id).await?;
                let _ = db.delete_contact_folder(&id);
                Ok(serde_json::json!({ "ok": true }))
            }
        }
    });
    match out {
        Ok(v) => v.to_string(),
        Err(e) => err_json(&crate::contacts_outbox::short_error(&e)),
    }
}

/// POST /contacts/delete `{ids, permanent}` — each contact is hidden at once and deleted on the provider later.
fn delete(c: &Ctx) -> Response {
    let body = match read_body(c, MIB) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let req = parse(&body);
    let permanent = req["permanent"].as_bool().unwrap_or(false);
    let ids = strings(&req["ids"]);
    // one account at a time: group by the account each contact belongs to
    let mut by: BTreeMap<String, Vec<String>> = Default::default();
    for i in ids {
        by.entry(account_of_contact(c.config_dir, &i)).or_default().push(i);
    }
    let mut deleted = 0;
    let mut last_err = String::new();
    for (acct, list) in by {
        let r: Value = serde_json::from_str(&contacts_write_call(c.config_dir, &acct, |db| crate::contacts_write::delete_ext(db, &list, seq(), permanent))).unwrap_or_default();
        if r["ok"] == true {
            deleted += r["deleted"].as_i64().unwrap_or(0);
        } else {
            last_err = r["error"].as_str().unwrap_or("").to_string();
        }
    }
    Response::json(if deleted == 0 && !last_err.is_empty() {
        err_json(&last_err)
    } else {
        serde_json::json!({ "ok": true, "deleted": deleted }).to_string()
    })
}

/// POST /contacts/undo — put back what the newest write of the most recent account changed.
fn undo(c: &Ctx) -> Response {
    // the account whose newest write is the most recent
    let mut best: Option<(i64, String)> = None;
    if let Ok(db) = Database::open(&c.db_path()) {
        for (t, a) in db.contact_outbox_newest_per_account().unwrap_or_default() {
            if best.as_ref().map(|b| t > b.0).unwrap_or(true) {
                best = Some((t, a));
            }
        }
    }
    Response::json(match best {
        Some((_, a)) => contacts_write_call(c.config_dir, &a, |db| crate::contacts_write::undo(db, seq())),
        None => err_json("nothing to undo"),
    })
}

/// POST /contacts/retry?id=ID — requeue the contact's last failed write.
fn retry(c: &Ctx) -> Response {
    retry_or_discard(c, true)
}

/// POST /contacts/discard?id=ID — drop the contact's last failed write.
fn discard(c: &Ctx) -> Response {
    retry_or_discard(c, false)
}

fn retry_or_discard(c: &Ctx, retry: bool) -> Response {
    let contact = c.query("id").unwrap_or_default();
    let account = account_of_contact(c.config_dir, &contact);
    Response::json(contacts_write_call(c.config_dir, &account, |db| match db.contact_last_error(&contact) {
        Ok(Some((op, _, _))) => {
            if retry {
                let _ = db.contact_outbox_retry(&op);
            } else {
                let _ = db.contact_outbox_discard(&op);
            }
            serde_json::json!({ "ok": true })
        }
        _ => serde_json::json!({ "ok": false, "error": "nothing to retry" }),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::request::{read_request, test_stream, Limits};

    /// One raw request, read the way the server reads it, through one handler.
    fn run(dir: &PathBuf, raw: String, f: fn(&Ctx) -> Response) -> Response {
        let req = read_request(&mut test_stream(raw.into_bytes()), &Limits::default()).expect("request parses");
        f(&Ctx::new(dir, &req))
    }

    fn get(target: &str) -> String {
        format!("GET {} HTTP/1.1\r\nHost: 127.0.0.1:1\r\n\r\n", target)
    }

    fn post(target: &str, body: &str) -> String {
        format!("POST {} HTTP/1.1\r\nHost: 127.0.0.1:1\r\nContent-Length: {}\r\n\r\n{}", target, body.len(), body)
    }

    fn temp_dir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("omalook-contacts-{}-{}", test, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn json_of(r: &Response) -> Value {
        serde_json::from_slice(r.body()).expect("JSON body")
    }

    #[test]
    fn the_table_lists_each_moved_path_with_its_lock() {
        let routes = routes();
        let mut seen: Vec<(&str, &str)> = routes.iter().map(|r| (r.method, r.pattern_str())).collect();
        seen.sort();
        assert_eq!(seen.len(), 22, "{:?}", seen);
        assert!(routes.iter().all(|r| matches!(r.pattern, Pattern::Exact(_))));
        let unlocked: Vec<&str> = routes.iter().filter(|r| r.lock == Lock::None).map(|r| r.pattern_str()).collect();
        assert_eq!(unlocked, vec!["/contacts/get", "/contacts/directory"]);
        assert!(routes.iter().filter(|r| r.lock == Lock::Exclusive).count() == 20);
    }

    #[test]
    fn import_and_photo_set_refuse_a_path_or_an_unissued_handle() {
        let dir = temp_dir("refuse");
        let file = dir.join("unregistered.vcf");
        std::fs::write(&file, b"BEGIN:VCARD\r\nEND:VCARD\r\n").unwrap();
        // A path with no handle is not an import: the body carries handles only now.
        let body = serde_json::json!({ "account_id": "acct-a", "folder_id": "f", "path": file.to_string_lossy() }).to_string();
        let r = run(&dir, post("/contacts/import", &body), import);
        assert_eq!(r.status(), 200);
        assert_eq!(json_of(&r), serde_json::json!({ "ok": false, "error": "pick the file again" }));
        // A forged handle is refused the same way.
        let body = serde_json::json!({ "account_id": "acct-a", "folder_id": "f", "handle": "fh-00000000000000000000000000000000" }).to_string();
        let r = run(&dir, post("/contacts/import", &body), import);
        assert_eq!(json_of(&r), serde_json::json!({ "ok": false, "error": "pick the file again" }));

        let body = serde_json::json!({ "id": "c1", "path": file.to_string_lossy() }).to_string();
        let r = run(&dir, post("/contacts/photo/set", &body), photo_set);
        assert_eq!(r.status(), 200);
        assert_eq!(json_of(&r), serde_json::json!({ "ok": false, "error": "pick the picture again" }));
        let body = serde_json::json!({ "id": "c1", "handle": "fh-00000000000000000000000000000000" }).to_string();
        let r = run(&dir, post("/contacts/photo/set", &body), photo_set);
        assert_eq!(json_of(&r), serde_json::json!({ "ok": false, "error": "pick the picture again" }));
    }

    #[test]
    fn a_missing_content_length_is_refused_as_before() {
        let dir = temp_dir("no-length");
        let raw = "POST /contacts/import HTTP/1.1\r\nHost: 127.0.0.1:1\r\n\r\n{\"path\":\"x\"}".to_string();
        let r = run(&dir, raw, import);
        assert_eq!(json_of(&r), serde_json::json!({ "ok": false, "error": "missing Content-Length" }));
    }

    #[test]
    fn get_contacts_on_an_empty_database_is_an_empty_list() {
        let dir = temp_dir("empty-list");
        let r = run(&dir, get("/contacts?view=all&sort=first"), list_contacts);
        assert_eq!(r.status(), 200);
        assert_eq!(r.body(), b"[]");
        let text = String::from_utf8(r.as_bytes().to_vec()).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n"));
        let r = run(&dir, get("/contacts?view=duplicates"), list_contacts);
        assert_eq!(r.body(), b"[]");
    }

    #[test]
    fn favorite_without_an_id_answers_ok_as_before() {
        let dir = temp_dir("favorite");
        let r = run(&dir, post("/contacts/favorite?value=true", ""), favorite);
        assert_eq!(r.status(), 200);
        assert_eq!(r.body(), b"ok");
    }

    #[test]
    fn save_with_a_bad_body_names_the_contact_error() {
        let dir = temp_dir("save-bad");
        let r = run(&dir, post("/contacts/save", "{"), save);
        assert!(json_of(&r)["error"].as_str().unwrap().starts_with("unreadable contact: "));
        let r = run(&dir, post("/contacts/delete", "{\"ids\":[]}"), delete);
        assert_eq!(json_of(&r), serde_json::json!({ "ok": true, "deleted": 0 }));
    }

    #[test]
    fn undo_with_nothing_to_undo_says_so() {
        let dir = temp_dir("undo");
        let r = run(&dir, post("/contacts/undo", ""), undo);
        assert_eq!(json_of(&r), serde_json::json!({ "ok": false, "error": "nothing to undo" }));
    }
}
