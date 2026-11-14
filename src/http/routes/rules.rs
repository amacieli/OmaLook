//! Rules and lists (R3.1 group G5): distribution lists (Settings → Lists), OmaLook's own local
//! rules (Settings → Rules, "Rules OmaLook runs"), the mailbox rules of each account (Exchange
//! server rules and Gmail filters), and the Tags page. Moved from the legacy chain in `main.rs`;
//! answers are 200 with JSON as before. Errors inside a body stay in the body (`{"ok":false,...}`).
//!
//! The read-only answers (`GET /rules`, `GET /rules/preview`, `GET /rules/filter-preview`,
//! `GET /local-rules`) run unlocked as they did. The three routes that used to spawn a thread for
//! their work (`GET /rules/run-preview`, `POST /rules/run`, `POST /local-rules/apply`) also run
//! unlocked, because that work ran off the dispatch lock in the legacy chain too; they answer on
//! the connection's own thread instead of a spawned one.

use crate::http::router::{Ctx, Lock, Pattern, Response, Route};
use crate::Database;
use std::path::PathBuf;

pub fn routes() -> Vec<Route> {
    use Pattern::Exact;
    vec![
        // Distribution lists
        Route::new("GET", Exact("/lists"), Lock::Exclusive, lists_all),
        Route::new("GET", Exact("/lists/targets"), Lock::Exclusive, lists_targets),
        Route::new("GET", Exact("/lists/sync/status"), Lock::Exclusive, lists_sync_status),
        Route::new("POST", Exact("/lists/targets/set"), Lock::Exclusive, lists_targets_set),
        Route::new("POST", Exact("/lists/sync/now"), Lock::Exclusive, lists_sync_now),
        Route::new("POST", Exact("/lists/create"), Lock::Exclusive, lists_create),
        Route::new("POST", Exact("/lists/rename"), Lock::Exclusive, lists_rename),
        Route::new("POST", Exact("/lists/delete"), Lock::Exclusive, lists_delete),
        Route::new("POST", Exact("/lists/members/add"), Lock::Exclusive, lists_members_add),
        Route::new("POST", Exact("/lists/members/remove"), Lock::Exclusive, lists_members_remove),
        // OmaLook's local rules
        Route::new("GET", Exact("/local-rules"), Lock::None, local_rules_list),
        Route::new("GET", Exact("/local-rules/preview"), Lock::Exclusive, local_rules_preview),
        Route::new("POST", Exact("/local-rules/save"), Lock::Exclusive, local_rules_save),
        Route::new("POST", Exact("/local-rules/delete"), Lock::Exclusive, local_rules_delete),
        Route::new("POST", Exact("/local-rules/enable"), Lock::Exclusive, local_rules_enable),
        Route::new("POST", Exact("/local-rules/move"), Lock::Exclusive, local_rules_move),
        Route::new("POST", Exact("/local-rules/apply"), Lock::None, local_rules_apply),
        // Mailbox rules (Exchange server rules, Gmail filters)
        Route::new("GET", Exact("/rules"), Lock::None, rules_list),
        Route::new("GET", Exact("/rules/preview"), Lock::None, rules_preview),
        Route::new("GET", Exact("/rules/filter-preview"), Lock::None, rules_filter_preview),
        Route::new("GET", Exact("/rules/run-preview"), Lock::None, rules_run_preview),
        Route::new("POST", Exact("/rules/run"), Lock::None, rules_run),
        Route::new("POST", Exact("/rules/filter-add"), Lock::Exclusive, rules_filter_add),
        Route::new("POST", Exact("/rules/filter-edit"), Lock::Exclusive, rules_filter_edit),
        Route::new("POST", Exact("/rules/filter-delete"), Lock::Exclusive, rules_filter_delete),
        Route::new("POST", Exact("/rules/add"), Lock::Exclusive, rules_add),
        Route::new("POST", Exact("/rules/enable"), Lock::Exclusive, rules_enable),
        Route::new("POST", Exact("/rules/delete"), Lock::Exclusive, rules_delete),
        Route::new("POST", Exact("/rules/move"), Lock::Exclusive, rules_move),
        // Tags page
        Route::new("GET", Exact("/tags"), Lock::Exclusive, tags_overview),
        Route::new("GET", Exact("/tags/similar"), Lock::Exclusive, tags_similar),
        Route::new("POST", Exact("/tags/color"), Lock::Exclusive, tags_color),
        Route::new("POST", Exact("/tags/delete"), Lock::Exclusive, tags_delete),
        Route::new("POST", Exact("/tags/adopt"), Lock::Exclusive, tags_adopt),
    ]
}

fn ok_json(body: String) -> Response {
    Response::json(body)
}

// ── Distribution lists ──────────────────────────────────────────────────────────────────────

fn lists_all(c: &Ctx) -> Response {
    ok_json(lists_call(c, "/lists"))
}
fn lists_targets(c: &Ctx) -> Response {
    ok_json(lists_call(c, "/lists/targets"))
}
fn lists_sync_status(c: &Ctx) -> Response {
    ok_json(lists_call(c, "/lists/sync/status"))
}
fn lists_targets_set(c: &Ctx) -> Response {
    ok_json(lists_call(c, "/lists/targets/set"))
}
fn lists_sync_now(c: &Ctx) -> Response {
    ok_json(lists_call(c, "/lists/sync/now"))
}
fn lists_create(c: &Ctx) -> Response {
    ok_json(lists_call(c, "/lists/create"))
}
fn lists_rename(c: &Ctx) -> Response {
    ok_json(lists_call(c, "/lists/rename"))
}
fn lists_delete(c: &Ctx) -> Response {
    ok_json(lists_call(c, "/lists/delete"))
}
fn lists_members_add(c: &Ctx) -> Response {
    ok_json(lists_call(c, "/lists/members/add"))
}
fn lists_members_remove(c: &Ctx) -> Response {
    ok_json(lists_call(c, "/lists/members/remove"))
}

/// JSON body for the distribution-list endpoints; `op` is the request path.
fn lists_call(c: &Ctx, op: &str) -> String {
    use crate::db::lists::ListOp;
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let db = match Database::open(c.config_dir.join("messages.db").to_str().unwrap_or("messages.db")) {
        Ok(db) => db,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let status = |op: ListOp| match op {
        ListOp::Done => "done", ListOp::Exists => "exists", ListOp::Invalid => "invalid", ListOp::Missing => "missing",
    };
    let id: i64 = c.query("id").and_then(|v| v.parse().ok()).unwrap_or(0);
    let name = c.query("name").unwrap_or_default();
    let email = c.query("email").unwrap_or_default();
    let r: Result<String, crate::errors::OmarchyError> = match op {
        "/lists/targets/set" => {
            let account = c.query("account").unwrap_or_default();
            let on = c.query("on").as_deref() == Some("true");
            if account.is_empty() { Ok(serde_json::json!({ "ok": false }).to_string()) }
            else { db.set_list_sync_target(&account, on).map(|_| serde_json::json!({ "ok": true }).to_string()) }
        }
        "/lists/targets" => db.list_sync_targets().map(|v| serde_json::json!(v).to_string()),
        "/lists/sync/status" => db.list_link_log(60).map(|log| serde_json::json!({ "reports": crate::list_sync::last_reports(), "log": log }).to_string()),
        "/lists/sync/now" => Ok(serde_json::json!({ "ok": true }).to_string()),
        "/lists/create" => db.list_create(&name).map(|(op, id)| serde_json::json!({ "ok": op == ListOp::Done, "status": status(op), "id": id }).to_string()),
        "/lists/rename" => db.list_rename(id, &name).map(|op| serde_json::json!({ "ok": op == ListOp::Done, "status": status(op) }).to_string()),
        "/lists/delete" => db.list_delete(id).map(|ok| serde_json::json!({ "ok": ok }).to_string()),
        "/lists/members/add" => db.list_member_add(id, &email, &name).map(|op| serde_json::json!({ "ok": op == ListOp::Done || op == ListOp::Exists, "status": status(op) }).to_string()),
        "/lists/members/remove" => db.list_member_remove(id, &email).map(|ok| serde_json::json!({ "ok": ok }).to_string()),
        _ => db.lists_all().map(|l| serde_json::to_string(&l).unwrap_or_else(|_| "[]".into())),
    };
    // a change to the lists (or to where they are mirrored) is followed by a run of the link
    let changed = ["/lists/targets/set", "/lists/create", "/lists/rename", "/lists/delete", "/lists/members/add", "/lists/members/remove", "/lists/sync/now"];
    if changed.contains(&op) {
        lists_link_run(c.config_dir.clone());
    }
    r.unwrap_or_else(|e| err(&e.to_string()))
}

/// Run the two-way list link for every mirror account that is signed in (off the request thread).
fn lists_link_run(config_dir: PathBuf) {
    std::thread::spawn(move || {
        let path = config_dir.join("messages.db");
        let path = path.to_str().unwrap_or("messages.db").to_string();
        let targets = match Database::open(&path).and_then(|d| d.list_sync_targets()) { Ok(t) => t, Err(_) => return };
        let create = crate::settings::read_lists_create_contacts(&config_dir.join("settings.toml"));
        for account in targets {
            if crate::accounts::provider_of(&account) == "gmail" { continue; }
            if !crate::scheduler::GLOBAL.get().map(|s| s.is_running(&account)).unwrap_or(false) { continue; }
            if let Ok(db) = Database::open_for_account(&path, &account) {
                let walked = db.flag_is_set(crate::contacts_daemon::FULL_FLAG).unwrap_or(false);
                let policy = if create { crate::list_sync::CreatePolicy::Always } else { crate::list_sync::CreatePolicy::Skip };
                crate::list_sync::run_and_record(&db, walked, policy);
            }
        }
    });
}

// ── Local rules ─────────────────────────────────────────────────────────────────────────────

fn local_rules_list(c: &Ctx) -> Response {
    ok_json(local_rules_call(c, "/local-rules"))
}
fn local_rules_preview(c: &Ctx) -> Response {
    ok_json(local_rules_call(c, "/local-rules/preview"))
}
fn local_rules_save(c: &Ctx) -> Response {
    ok_json(local_rules_call(c, "/local-rules/save"))
}
fn local_rules_delete(c: &Ctx) -> Response {
    ok_json(local_rules_call(c, "/local-rules/delete"))
}
fn local_rules_enable(c: &Ctx) -> Response {
    ok_json(local_rules_call(c, "/local-rules/enable"))
}
fn local_rules_move(c: &Ctx) -> Response {
    ok_json(local_rules_call(c, "/local-rules/move"))
}
fn local_rules_apply(c: &Ctx) -> Response {
    ok_json(local_rules_call(c, "/local-rules/apply"))
}

/// JSON body for the `/local-rules` routes (Settings -> Rules, OL-040 phase 4d): rules that are exactly what a
/// provider can store (see `crate::local_rules`), run by OmaLook on arrival for accounts without a server
/// engine and, on request, over existing Inbox mail. `op` is the request path.
fn local_rules_call(c: &Ctx, op: &str) -> String {
    use crate::local_rules::{has_local_engine, LocalRule};
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let path = c.config_dir.join("messages.db");
    let db = match Database::open(path.to_str().unwrap_or("messages.db")) {
        Ok(d) => d,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let limit = crate::settings::read_rules_confirm_over(&c.config_dir.join("settings.toml"));
    let id = c.query("id").unwrap_or_default();
    // the rule from `rule=<json>`; a blank id means a new rule
    let parsed = || -> Result<LocalRule, String> {
        let raw = c.query("rule").ok_or("need rule=<json>")?;
        let mut v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| format!("rule is not valid JSON: {e}"))?;
        if v["id"].as_str().unwrap_or("").is_empty() {
            let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
            v["id"] = serde_json::json!(format!("lr-{:x}", n));
        }
        serde_json::from_value::<LocalRule>(v).map_err(|e| format!("rule has the wrong shape: {e}"))
    };
    let named = |db: &Database| db.local_rules().map(|r| r.into_iter().find(|x| x.id == id));
    let r: Result<String, String> = (|| {
        if op == "/local-rules/preview" {
            let mut rule = parsed()?;
            rule.validate()?;
            rule.enabled = true; // a preview shows what the rule would do if it were on
            let d = db.dry_run(&[rule]).map_err(|e| e.to_string())?;
            return Ok(serde_json::json!({ "ok": true, "preview": d }).to_string());
        }
        if op == "/local-rules/save" {
            let mut rule = parsed()?;
            rule.validate()?;
            let existing = db.local_rules().map_err(|e| e.to_string())?;
            match existing.iter().find(|x| x.id == rule.id) {
                Some(old) => rule.sequence = old.sequence,
                None => rule.sequence = existing.iter().map(|x| x.sequence).max().unwrap_or(0) + 1,
            }
            db.save_local_rule(&rule).map_err(|e| e.to_string())?;
            return Ok(serde_json::json!({ "ok": true, "id": rule.id }).to_string());
        }
        if op == "/local-rules/delete" {
            if c.query("confirm").as_deref() != Some("yes") { return Err("confirm=yes is required".into()); }
            let gone = db.delete_local_rule(&id).map_err(|e| e.to_string())?;
            return Ok(serde_json::json!({ "ok": gone, "error": if gone { "" } else { "no such rule" } }).to_string());
        }
        if op == "/local-rules/enable" {
            let mut rule = named(&db).map_err(|e| e.to_string())?.ok_or("no such rule")?;
            rule.enabled = c.query("on").as_deref() == Some("true");
            db.save_local_rule(&rule).map_err(|e| e.to_string())?;
            return Ok(serde_json::json!({ "ok": true }).to_string());
        }
        if op == "/local-rules/move" {
            let up = match c.query("dir").as_deref() { Some("up") => true, Some("down") => false, _ => return Err("dir must be up or down".into()) };
            let moved = db.move_local_rule(&id, up).map_err(|e| e.to_string())?;
            return Ok(serde_json::json!({ "ok": moved, "error": if moved { "" } else { "cannot move that way" } }).to_string());
        }
        if op == "/local-rules/apply" {
            // This rule alone over existing Inbox mail. Over the limit it writes nothing and says how many it would
            // change; the caller repeats with confirm=yes after the user agrees.
            let rule = named(&db).map_err(|e| e.to_string())?.ok_or("no such rule")?;
            if !rule.enabled { return Err("turn the rule on before applying it to existing mail".into()); }
            let confirmed = c.query("confirm").as_deref() == Some("yes");
            let out = db.apply_local_rules(&[rule], limit, confirmed).map_err(|e| e.to_string())?;
            if matches!(out, crate::db::local_rules::ApplyOutcome::Applied { messages, .. } if messages > 0) { crate::sync_state::request_read_push(); }
            return Ok(serde_json::json!({ "ok": true, "outcome": out, "limit": limit }).to_string());
        }
        // GET /local-rules
        let stats = db.local_rule_stats().unwrap_or_default();
        let accounts: Vec<serde_json::Value> = db.accounts_basic().unwrap_or_default().into_iter().map(|(id, provider, email)| {
            let on_arrival = has_local_engine(crate::accounts::provider_of(&id));
            serde_json::json!({ "id": id, "provider": provider, "email": email, "on_arrival": on_arrival })
        }).collect();
        let rules: Vec<serde_json::Value> = db.local_rules().map_err(|e| e.to_string())?.into_iter().map(|r| {
            let applied = stats.get(&r.id).copied().unwrap_or(0);
            let mut v = serde_json::to_value(&r).unwrap_or_default();
            v["applied"] = serde_json::json!(applied);
            v
        }).collect();
        let tags: Vec<String> = db.tags_overview().unwrap_or_default().into_iter().map(|t| t.name).collect();
        Ok(serde_json::json!({ "ok": true, "rules": rules, "accounts": accounts, "tags": tags, "limit": limit }).to_string())
    })();
    r.unwrap_or_else(|m| err(&m))
}

// ── Mailbox rules ───────────────────────────────────────────────────────────────────────────

/// Marks one manual rule run as going (key = account|rule id); dropped when the run ends, even on a panic.
struct RunGuard(String);
static RUNNING_RULES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
impl RunGuard {
    fn take(key: &str) -> Option<RunGuard> {
        let mut g = RUNNING_RULES.lock().unwrap_or_else(|e| e.into_inner());
        if g.iter().any(|k| k == key) { return None }
        g.push(key.to_string());
        Some(RunGuard(key.to_string()))
    }
}
impl Drop for RunGuard {
    fn drop(&mut self) {
        let mut g = RUNNING_RULES.lock().unwrap_or_else(|e| e.into_inner());
        g.retain(|k| k != &self.0);
    }
}

fn rules_list(c: &Ctx) -> Response {
    ok_json(rules_list_call(c))
}
fn rules_preview(c: &Ctx) -> Response {
    ok_json(rules_account_call(c, "/rules/preview"))
}
fn rules_filter_preview(c: &Ctx) -> Response {
    ok_json(rules_account_call(c, "/rules/filter-preview"))
}
fn rules_run_preview(c: &Ctx) -> Response {
    ok_json(rules_account_call(c, "/rules/run-preview"))
}
fn rules_run(c: &Ctx) -> Response {
    ok_json(rules_account_call(c, "/rules/run"))
}
fn rules_filter_add(c: &Ctx) -> Response {
    ok_json(rules_account_call(c, "/rules/filter-add"))
}
fn rules_filter_edit(c: &Ctx) -> Response {
    ok_json(rules_account_call(c, "/rules/filter-edit"))
}
fn rules_filter_delete(c: &Ctx) -> Response {
    ok_json(rules_account_call(c, "/rules/filter-delete"))
}
fn rules_add(c: &Ctx) -> Response {
    ok_json(rules_account_call(c, "/rules/add"))
}
fn rules_enable(c: &Ctx) -> Response {
    ok_json(rules_account_call(c, "/rules/enable"))
}
fn rules_delete(c: &Ctx) -> Response {
    ok_json(rules_account_call(c, "/rules/delete"))
}
fn rules_move(c: &Ctx) -> Response {
    ok_json(rules_account_call(c, "/rules/move"))
}

/// The per-account answers and writes under `/rules` (everything but the list). `op` is the request path.
fn rules_account_call(c: &Ctx, op: &str) -> String {
    use crate::db::rules::RuleChange;
    use crate::rules::{Match, NewRule};
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let path = c.config_dir.join("messages.db");
    let Some(account) = c.query("account").filter(|a| !a.is_empty()) else { return err("need account=<id>") };
    let db = match Database::open_for_account(path.to_str().unwrap_or("messages.db"), &account) {
        Ok(d) => d,
        Err(e) => return err(&format!("database: {}", e)),
    };
    if !db.accounts_basic().unwrap_or_default().iter().any(|(id, _, _)| *id == account) { return err("no such account") }
    let id = c.query("id").unwrap_or_default();
    // The Exchange routes (everything but filter-*) are Graph rule changes and runs; Gmail accounts have their own.
    let provider_of = db.accounts_basic().unwrap_or_default().into_iter().find(|(a, _, _)| *a == account).map(|(_, p, _)| p).unwrap_or_default();
    let is_filter = op.starts_with("/rules/filter-");
    if !is_filter && !provider_of.eq_ignore_ascii_case("exchange") {
        return err("this account's rules are not Exchange rules (Gmail accounts use filters)");
    }
    let new_rule = || -> Result<NewRule, String> {
        let matcher = match (c.query("address"), c.query("domain")) {
            (Some(a), None) => Match::Address(a),
            (None, Some(d)) => Match::Domain(d),
            _ => return Err("need exactly one of address=<email> or domain=<domain>".into()),
        };
        Ok(NewRule {
            tag: c.query("tag").unwrap_or_default().trim().to_string(), matcher,
            enabled: c.query("enabled").as_deref() != Some("false"),
            stop: c.query("stop").as_deref() != Some("false"),
        })
    };
    let queued = |r: Result<i64, String>| match r {
        Ok(op) => { crate::sync_state::request_read_push(); serde_json::json!({ "ok": true, "op": op }).to_string() }
        Err(m) => err(&m),
    };
    // Gmail filters (OL-040 phase 5b): the rule is a local-rule JSON, written to Gmail only when Gmail can hold it
    // exactly (crate::gmail_filters). Everything is queued; the daemon does the checked write.
    if op == "/rules/filter-preview" || matches!(op, "/rules/filter-add" | "/rules/filter-edit" | "/rules/filter-delete") {
        let rule = || -> Result<crate::local_rules::LocalRule, String> {
            let raw = c.query("rule").ok_or("need rule=<json>")?;
            let mut v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| format!("rule is not valid JSON: {e}"))?;
            if v["id"].as_str().unwrap_or("").is_empty() { v["id"] = serde_json::json!("gmail-filter"); }
            if v["name"].as_str().unwrap_or("").is_empty() { v["name"] = serde_json::json!("Gmail filter"); }
            let mut r: crate::local_rules::LocalRule = serde_json::from_value(v).map_err(|e| format!("rule has the wrong shape: {e}"))?;
            r.applies_to = vec![account.clone()];
            r.enabled = true;
            Ok(r)
        };
        if op == "/rules/filter-preview" {
            return match rule() {
                Err(m) => err(&m),
                Ok(r) => {
                    let (writable, reason) = match crate::gmail_filters::writable(&r) { Ok(()) => (true, String::new()), Err(m) => (false, m) };
                    match db.dry_run(&[r]) {
                        Ok(p) => serde_json::json!({ "ok": true, "writable": writable, "reason": reason, "preview": p }).to_string(),
                        Err(e) => err(&e.to_string()),
                    }
                }
            };
        }
        use crate::db::gmail_filters::FilterChange;
        let change = if op == "/rules/filter-add" {
            rule().map(FilterChange::Add)
        } else if op == "/rules/filter-edit" {
            rule().map(|r| FilterChange::Edit { id: id.clone(), rule: r })
        } else if op == "/rules/filter-delete" {
            if c.query("confirm").as_deref() != Some("yes") { Err("confirm=yes is required".to_string()) } else { Ok(FilterChange::Delete { id: id.clone() }) }
        } else { Err("no such route".to_string()) };
        return match change { Ok(c) => queued(db.queue_filter_change(&c)), Err(m) => err(&m) };
    }
    // Manual "run now" of the account's Exchange tag rules over cached Inbox mail (all of them, or `id`).
    // Count first (run-preview); `run` writes nothing over the limit until confirm=yes. Tags only, added never removed.
    if op == "/rules/run-preview" || op == "/rules/run" {
        let only = if id.is_empty() { None } else { Some(id.as_str()) };
        let (rules, left_out) = match db.exchange_rules_to_run(&account, only) { Ok(x) => x, Err(m) => return err(&m) };
        if rules.is_empty() { return err("no enabled tag rules to run on this account") }
        let limit = crate::settings::read_rules_confirm_over(&c.config_dir.join("settings.toml"));
        return if op == "/rules/run-preview" {
            match db.dry_run(&rules) {
                Ok(p) => serde_json::json!({ "ok": true, "preview": p, "rules": rules.len(), "left_out": left_out, "limit": limit }).to_string(),
                Err(e) => err(&e.to_string()),
            }
        } else {
            let confirmed = c.query("confirm").as_deref() == Some("yes");
            // one run per (account, rule) at a time; different rules run side by side
            let _running = match RunGuard::take(&format!("{}|{}", account, id)) { Some(g) => g, None => return err("that run is already going") };
            match db.apply_local_rules(&rules, limit, confirmed) {
                Ok(out) => {
                    if matches!(out, crate::db::local_rules::ApplyOutcome::Applied { messages, .. } if messages > 0) { crate::sync_state::request_read_push(); }
                    serde_json::json!({ "ok": true, "outcome": out, "rules": rules.len(), "left_out": left_out, "limit": limit }).to_string()
                }
                Err(e) => err(&e.to_string()),
            }
        };
    }
    if op == "/rules/preview" {
        match new_rule().and_then(|n| db.rule_preview(&n)) {
            Ok(p) => serde_json::json!({ "ok": true, "preview": p }).to_string(),
            Err(m) => err(&m),
        }
    } else if op == "/rules/add" {
        match new_rule() { Ok(n) => queued(db.queue_rule_change(&RuleChange::Add(n))), Err(m) => err(&m) }
    } else if op == "/rules/enable" {
        queued(db.queue_rule_change(&RuleChange::Enable { id, on: c.query("on").as_deref() == Some("true") }))
    } else if op == "/rules/delete" {
        if c.query("confirm").as_deref() != Some("yes") { err("confirm=yes is required") }
        else { queued(db.queue_rule_change(&RuleChange::Delete { id })) }
    } else if op == "/rules/move" {
        match c.query("dir").as_deref() {
            Some("up") => queued(db.queue_rule_change(&RuleChange::Move { id, up: true })),
            Some("down") => queued(db.queue_rule_change(&RuleChange::Move { id, up: false })),
            _ => err("dir must be up or down"),
        }
    } else { err("no such route") }
}

/// JSON body for `GET /rules`: every account's cached mailbox rules, summarised and grouped.
fn rules_list_call(c: &Ctx) -> String {
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let path = c.config_dir.join("messages.db");
    let db = match Database::open(path.to_str().unwrap_or("messages.db")) {
        Ok(d) => d,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let colors: std::collections::HashMap<(String, String), String> = rusqlite::Connection::open(&path).ok()
        .and_then(|c| c.prepare("SELECT account_id, name, color FROM category_defs WHERE delete_pending = 0").ok().map(|mut st| {
            st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
                .map(|it| it.flatten().map(|(a, n, c)| ((a, n.to_lowercase()), c)).collect()).unwrap_or_default()
        })).unwrap_or_default();
    let tags = db.tags_overview().unwrap_or_default();
    let mut accounts: Vec<serde_json::Value> = Vec::new();
    for (id, provider, email) in db.accounts_basic().unwrap_or_default() {
        let state = db.rules_state(&id).ok().flatten();
        let raw = db.rules_raw(&id).unwrap_or_default();
        let mut order: Vec<(i64, String, bool)> = raw.iter().filter(|_| !provider.eq_ignore_ascii_case("gmail")).map(|r| (r["sequence"].as_i64().unwrap_or(0), r["id"].as_str().unwrap_or_default().to_string(),
            crate::rules::writable(r).is_err())).collect();
        order.sort();
        let order: Vec<serde_json::Value> = order.into_iter().map(|(_, rid, locked)| serde_json::json!({ "id": rid, "locked": locked })).collect();
        let ops = db.recent_rule_ops(&id, 86_400).unwrap_or_default();
        let is_gmail = provider.eq_ignore_ascii_case("gmail");
        // Gmail filters are not Graph rules: they are summarised by crate::gmail_filters, not grouped as tag rules
        let filters: Vec<crate::gmail_filters::FilterSummary> = if is_gmail { db.filter_summaries(&id) } else { Vec::new() };
        let summaries: Vec<crate::rules::RuleSummary> = if is_gmail { Vec::new() } else { raw.iter().map(crate::rules::summarize).collect() };
        let total = if is_gmail { filters.len() } else { summaries.len() };
        let org = crate::rules::organise(summaries);
        let groups: Vec<serde_json::Value> = org.groups.iter().map(|g| {
            let color = colors.get(&(id.clone(), g.tag.to_lowercase())).cloned()
                .or_else(|| tags.iter().find(|t| crate::db::tags::tag_key(&t.name) == crate::db::tags::tag_key(&g.tag)).map(|t| t.color.clone()))
                .unwrap_or_else(|| crate::models::fallback_category_color(&g.tag).to_string());
            serde_json::json!({ "tag": g.tag, "color": color, "rules": g.rules })
        }).collect();
        accounts.push(serde_json::json!({
            "account_id": id, "provider": provider, "email": email, "total": total,
            "state": state.as_ref().map(|s| s.state.clone()).unwrap_or_else(|| "none".into()),
            "error": state.as_ref().map(|s| s.error.clone()).unwrap_or_default(),
            "fetched_at": state.as_ref().map(|s| s.fetched_at).unwrap_or(0),
            "groups": groups, "other": org.other, "system": org.system,
            "can_write": (provider == "exchange" || is_gmail) && state.as_ref().map(|s| s.state == "ok").unwrap_or(false),
            "filters": filters,
            "order": order, "ops": ops,
            "tag_pushes_pending": db.tag_pushes_pending(&id).unwrap_or(0),
        }));
    }
    serde_json::json!({ "accounts": accounts }).to_string()
}

// ── Tags page ───────────────────────────────────────────────────────────────────────────────

fn tags_overview(c: &Ctx) -> Response {
    ok_json(tags_call(c, "/tags"))
}
fn tags_similar(c: &Ctx) -> Response {
    ok_json(tags_call(c, "/tags/similar"))
}
fn tags_color(c: &Ctx) -> Response {
    ok_json(tags_call(c, "/tags/color"))
}
fn tags_delete(c: &Ctx) -> Response {
    ok_json(tags_call(c, "/tags/delete"))
}
fn tags_adopt(c: &Ctx) -> Response {
    ok_json(tags_call(c, "/tags/adopt"))
}

/// JSON body for the Tags page endpoints; `op` is the request path.
///   GET  /tags                                → {tags:[{name,color,total,accounts:[{account_id,email,provider,defined,pending,color_pending,color,messages}]}], palette}
///   POST /tags/color?name=N&color=KEY[&account=A]   recolour on every account that has it (or A); pushed by the daemon
///   POST /tags/delete?name=N&confirm=yes[&account=A] strip from messages, then delete on the provider
///   POST /tags/adopt?name=N&to=A              make the tag on another account (same colour)
///   GET  /tags/similar?name=N                 → {similar:[names that look like N]}
fn tags_call(c: &Ctx, op: &str) -> String {
    let err = |m: &str| serde_json::json!({ "ok": false, "error": m }).to_string();
    let path = c.config_dir.join("messages.db");
    let path = path.to_str().unwrap_or("messages.db");
    let overview = match Database::open(path).and_then(|d| d.tags_overview()) {
        Ok(v) => v,
        Err(e) => return err(&format!("database: {}", e)),
    };
    let name = c.query("name").unwrap_or_default().trim().to_string();
    let key = crate::db::tags::tag_key(&name);
    let only_account = c.query("account");
    let row = overview.iter().find(|t| crate::db::tags::tag_key(&t.name) == key);
    let for_account = |id: &str| only_account.as_deref().map(|a| a == id).unwrap_or(true);

    if op == "/tags/color" {
        let Some(c_pal) = c.query("color").and_then(|k| crate::models::palette_color(&k)) else { return err("color must be a palette key") };
        let Some(row) = row else { return err("no such tag") };
        let mut changed: Vec<String> = Vec::new();
        for a in row.accounts.iter().filter(|a| a.defined && for_account(&a.account_id)) {
            match Database::open_for_account(path, &a.account_id).and_then(|d| d.tag_set_color(&name, c_pal.hex)) {
                Ok(true) => changed.push(a.account_id.clone()),
                Ok(false) => {}
                Err(e) => return err(&format!("database: {}", e)),
            }
        }
        if changed.is_empty() { return err("no account has this tag in its list (add it to one first)") }
        crate::sync_state::request_read_push();
        serde_json::json!({ "ok": true, "accounts": changed }).to_string()
    } else if op == "/tags/delete" {
        let Some(row) = row else { return err("no such tag") };
        let counts: Vec<serde_json::Value> = row.accounts.iter().filter(|a| (a.defined || a.messages > 0) && for_account(&a.account_id))
            .map(|a| serde_json::json!({ "account": a.account_id, "messages": a.messages })).collect();
        if c.query("confirm").as_deref() != Some("yes") {
            return serde_json::json!({ "ok": false, "error": "confirm=yes is required", "accounts": counts }).to_string();
        }
        let mut stripped = 0usize;
        for a in row.accounts.iter().filter(|a| (a.defined || a.messages > 0) && for_account(&a.account_id)) {
            match Database::open_for_account(path, &a.account_id).and_then(|d| d.tag_delete(&name)) {
                Ok((n, _)) => stripped += n,
                Err(e) => return err(&format!("database: {}", e)),
            }
        }
        crate::sync_state::request_read_push();
        serde_json::json!({ "ok": true, "messages": stripped, "accounts": counts }).to_string()
    } else if op == "/tags/adopt" {
        let Some(to) = c.query("to") else { return err("need to=<account id>") };
        let Some(row) = row else { return err("no such tag") };
        if name.chars().count() > 64 || name.chars().any(|ch| ch.is_control() || ch == ';' || ch == ',') { return err("not a usable tag name") }
        let Some(target) = row.accounts.iter().find(|a| a.account_id == to) else { return err("no such account") };
        if target.defined { return serde_json::json!({ "ok": true, "created": false }).to_string(); }
        let hex = crate::models::CATEGORY_PALETTE.iter().find(|c| c.hex == row.color || c.gmail_bg == row.color)
            .map(|c| c.hex).unwrap_or_else(|| crate::models::fallback_category_color(&name));
        match Database::open_for_account(path, &to).and_then(|d| d.add_pending_category(&row.name, hex)) {
            Ok(created) => { crate::sync_state::request_read_push(); serde_json::json!({ "ok": true, "created": created }).to_string() }
            Err(e) => err(&format!("database: {}", e)),
        }
    } else if op == "/tags/similar" {
        match Database::open(path).and_then(|d| d.tags_similar(&name)) {
            Ok(v) => serde_json::json!({ "similar": v }).to_string(),
            Err(e) => err(&format!("database: {}", e)),
        }
    } else {
        let palette: Vec<serde_json::Value> = crate::models::CATEGORY_PALETTE.iter()
            .map(|c| serde_json::json!({ "key": c.key, "label": c.label, "hex": c.hex })).collect();
        serde_json::json!({ "tags": overview, "palette": palette }).to_string()
    }
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

    fn post(target: &str) -> String {
        format!("POST {} HTTP/1.1\r\nHost: 127.0.0.1:1\r\nContent-Length: 0\r\n\r\n", target)
    }

    /// Percent-encode everything but unreserved characters (JSON in a query string).
    fn enc(s: &str) -> String {
        s.bytes().map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{:02X}", b),
        }).collect()
    }

    fn temp_dir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("omalook-rules-{}-{}", test, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn json_of(r: &Response) -> serde_json::Value {
        serde_json::from_slice(r.body()).expect("JSON body")
    }

    const RULE: &str = r#"{"id":"lr-test","name":"Tag Carol","enabled":true,"applies_to":[],"conditions":[{"kind":"from_addresses","value":["carol@example.com"]}],"exceptions":[],"actions":{"tags":["Project X"],"stop":false}}"#;

    #[test]
    fn the_table_lists_each_moved_path_with_its_lock() {
        let routes = routes();
        let mut seen: Vec<(&str, &str, Lock)> = routes.iter().map(|r| (r.method, r.pattern_str(), r.lock)).collect();
        seen.sort_by_key(|(m, p, _)| (*m, *p));
        let none = [
            ("GET", "/local-rules"), ("POST", "/local-rules/apply"), ("GET", "/rules"), ("GET", "/rules/preview"),
            ("GET", "/rules/filter-preview"), ("GET", "/rules/run-preview"), ("POST", "/rules/run"),
        ];
        assert_eq!(seen.len(), 34);
        for (m, p, lock) in &seen {
            let want = if none.contains(&(*m, *p)) { Lock::None } else { Lock::Exclusive };
            assert_eq!(*lock, want, "{} {}", m, p);
        }
        assert!(seen.iter().any(|(m, p, _)| (*m, *p) == ("GET", "/lists")));
        assert!(!seen.iter().any(|(_, p, _)| *p == "/addresses"), "/addresses stays in main.rs");
    }

    #[test]
    fn empty_database_answers_the_lists_rules_and_tags() {
        let dir = temp_dir("empty");
        let rules = json_of(&run(&dir, get("/rules"), rules_list));
        assert_eq!(rules, serde_json::json!({ "accounts": [] }));
        let local = json_of(&run(&dir, get("/local-rules"), local_rules_list));
        assert_eq!(local["ok"], true);
        assert_eq!(local["rules"], serde_json::json!([]));
        assert_eq!(local["accounts"], serde_json::json!([]));
        let tags = json_of(&run(&dir, get("/tags"), tags_overview));
        assert_eq!(tags["tags"], serde_json::json!([]));
        assert!(!tags["palette"].as_array().unwrap().is_empty());
        let lists = json_of(&run(&dir, get("/lists"), lists_all));
        assert_eq!(lists, serde_json::json!([]));
    }

    #[test]
    fn local_rules_save_refuses_bad_json_with_the_old_message() {
        let dir = temp_dir("bad-json");
        let r = run(&dir, post("/local-rules/save?rule=notjson"), local_rules_save);
        assert_eq!(r.status(), 200);
        let v = json_of(&r);
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().starts_with("rule is not valid JSON: "));
        let v = json_of(&run(&dir, post("/local-rules/save"), local_rules_save));
        assert_eq!(v["error"], "need rule=<json>");
    }

    #[test]
    fn local_rule_save_list_enable_move_delete() {
        let dir = temp_dir("local-rule");
        let save = json_of(&run(&dir, post(&format!("/local-rules/save?rule={}", enc(RULE))), local_rules_save));
        assert_eq!(save, serde_json::json!({ "ok": true, "id": "lr-test" }));
        let list = json_of(&run(&dir, get("/local-rules"), local_rules_list));
        assert_eq!(list["rules"][0]["id"], "lr-test");
        assert_eq!(list["rules"][0]["enabled"], true);
        let off = json_of(&run(&dir, post("/local-rules/enable?id=lr-test&on=false"), local_rules_enable));
        assert_eq!(off, serde_json::json!({ "ok": true }));
        let list = json_of(&run(&dir, get("/local-rules"), local_rules_list));
        assert_eq!(list["rules"][0]["enabled"], false);
        // one rule: nothing to swap with
        let mv = json_of(&run(&dir, post("/local-rules/move?id=lr-test&dir=up"), local_rules_move));
        assert_eq!(mv, serde_json::json!({ "ok": false, "error": "cannot move that way" }));
        let mv = json_of(&run(&dir, post("/local-rules/move?id=lr-test&dir=sideways"), local_rules_move));
        assert_eq!(mv["error"], "dir must be up or down");
        let del = json_of(&run(&dir, post("/local-rules/delete?id=lr-test"), local_rules_delete));
        assert_eq!(del["error"], "confirm=yes is required");
        let del = json_of(&run(&dir, post("/local-rules/delete?id=lr-test&confirm=yes"), local_rules_delete));
        assert_eq!(del["ok"], true);
        let list = json_of(&run(&dir, get("/local-rules"), local_rules_list));
        assert_eq!(list["rules"], serde_json::json!([]));
    }

    #[test]
    fn local_rule_preview_and_apply_refuse_unknown_or_off_rules() {
        let dir = temp_dir("local-preview");
        let r = json_of(&run(&dir, get(&format!("/local-rules/preview?rule={}", enc(RULE))), local_rules_preview));
        assert_eq!(r["ok"], true);
        let r = json_of(&run(&dir, post("/local-rules/apply?id=nope"), local_rules_apply));
        assert_eq!(r, serde_json::json!({ "ok": false, "error": "no such rule" }));
    }

    #[test]
    fn tags_refuse_unknown_tags_and_bad_colours() {
        let dir = temp_dir("tags");
        let v = json_of(&run(&dir, post("/tags/color?name=Nope&color=nope"), tags_color));
        assert_eq!(v, serde_json::json!({ "ok": false, "error": "color must be a palette key" }));
        let key = crate::models::CATEGORY_PALETTE[0].key;
        let v = json_of(&run(&dir, post(&format!("/tags/color?name=Nope&color={}", key)), tags_color));
        assert_eq!(v["error"], "no such tag");
        let v = json_of(&run(&dir, post("/tags/delete?name=Nope"), tags_delete));
        assert_eq!(v["error"], "no such tag");
        let v = json_of(&run(&dir, post("/tags/adopt?name=Nope"), tags_adopt));
        assert_eq!(v["error"], "need to=<account id>");
        let v = json_of(&run(&dir, get("/tags/similar?name=Project"), tags_similar));
        assert!(v["similar"].is_array());
    }

    #[test]
    fn list_create_members_rename_remove_delete() {
        let dir = temp_dir("lists");
        let created = json_of(&run(&dir, post("/lists/create?name=Team"), lists_create));
        assert_eq!(created["status"], "done");
        let id = created["id"].as_i64().expect("id");
        let lists = json_of(&run(&dir, get("/lists"), lists_all));
        assert_eq!(lists[0]["name"], "Team");
        let add = json_of(&run(&dir, post(&format!("/lists/members/add?id={}&email=alice%40example.com&name=Alice", id)), lists_members_add));
        assert_eq!(add, serde_json::json!({ "ok": true, "status": "done" }));
        let again = json_of(&run(&dir, post(&format!("/lists/members/add?id={}&email=alice%40example.com", id)), lists_members_add));
        assert_eq!(again["status"], "exists");
        let lists = json_of(&run(&dir, get("/lists"), lists_all));
        assert_eq!(lists[0]["members"][0]["email"], "alice@example.com");
        let ren = json_of(&run(&dir, post(&format!("/lists/rename?id={}&name=Crew", id)), lists_rename));
        assert_eq!(ren, serde_json::json!({ "ok": true, "status": "done" }));
        let rm = json_of(&run(&dir, post(&format!("/lists/members/remove?id={}&email=alice%40example.com", id)), lists_members_remove));
        assert_eq!(rm, serde_json::json!({ "ok": true }));
        let lists = json_of(&run(&dir, get("/lists"), lists_all));
        assert_eq!(lists[0]["name"], "Crew");
        assert_eq!(lists[0]["members"], serde_json::json!([]));
        let del = json_of(&run(&dir, post(&format!("/lists/delete?id={}", id)), lists_delete));
        assert_eq!(del, serde_json::json!({ "ok": true }));
        assert_eq!(json_of(&run(&dir, get("/lists"), lists_all)), serde_json::json!([]));
    }

    #[test]
    fn list_targets_and_status_answer_on_an_empty_database() {
        let dir = temp_dir("list-targets");
        let v = json_of(&run(&dir, post("/lists/targets/set?on=true"), lists_targets_set));
        assert_eq!(v, serde_json::json!({ "ok": false }));
        assert_eq!(json_of(&run(&dir, get("/lists/targets"), lists_targets)), serde_json::json!([]));
        let st = json_of(&run(&dir, get("/lists/sync/status"), lists_sync_status));
        assert!(st["log"].is_array());
        assert_eq!(json_of(&run(&dir, post("/lists/sync/now"), lists_sync_now)), serde_json::json!({ "ok": true }));
    }

    #[test]
    fn rules_refuse_missing_and_unknown_accounts() {
        let dir = temp_dir("rules-errors");
        let v = json_of(&run(&dir, get("/rules/preview"), rules_preview));
        assert_eq!(v, serde_json::json!({ "ok": false, "error": "need account=<id>" }));
        let v = json_of(&run(&dir, post("/rules/add?tag=T&address=a@b.c"), rules_add));
        assert_eq!(v["error"], "need account=<id>");
        let v = json_of(&run(&dir, get("/rules/run-preview?account=nope"), rules_run_preview));
        assert_eq!(v, serde_json::json!({ "ok": false, "error": "no such account" }));
        let v = json_of(&run(&dir, post("/rules/filter-delete?account=nope&id=f"), rules_filter_delete));
        assert_eq!(v["error"], "no such account");
        let v = json_of(&run(&dir, post("/rules/run?account=nope"), rules_run));
        assert_eq!(v["error"], "no such account");
    }
}
