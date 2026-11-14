//! Core routes: sync polls, perf marks, folders, accounts list, tasks.

use crate::http::router::{Ctx, Lock, Pattern, Response, Route};
use log::warn;

pub fn routes() -> Vec<Route> {
    use Pattern::Exact;
    vec![
        Route::new("GET", Exact("/sync/health"), Lock::None, sync_health),
        Route::new("GET", Exact("/sync/serial"), Lock::None, sync_serial),
        Route::new("POST", Exact("/perf"), Lock::None, perf),
        Route::new("GET", Exact("/folders"), Lock::Exclusive, folders),
        Route::new("GET", Exact("/accounts"), Lock::Exclusive, accounts),
        Route::new("GET", Exact("/tasks"), Lock::Exclusive, tasks),
        Route::new("POST", Exact("/settings/db/rebuild"), Lock::Exclusive, db_rebuild),
    ]
}

/// POST /settings/db/rebuild `{"cancel": false}` — rebuild messages.db at the next start (R3.9); the UI
/// offers it when `/sync/health` says the database is damaged. `{"cancel": true}` withdraws the request.
/// The rebuild keeps the old file and every readable row (see `db_health`), so this destroys nothing.
fn db_rebuild(c: &Ctx) -> Response {
    let body = match c.json_body(4 * 1024) {
        Ok(v) => v,
        Err(e) => return Response::json(serde_json::json!({ "ok": false, "error": e }).to_string()),
    };
    let cancel = body["cancel"].as_bool().unwrap_or(false);
    match crate::db_health::request_rebuild(c.config_dir, cancel) {
        Ok(()) => Response::json(serde_json::json!({ "ok": true, "requested": !cancel, "restart_required": !cancel }).to_string()),
        Err(e) => {
            warn!("POST /settings/db/rebuild: {}", e);
            Response::json(serde_json::json!({ "ok": false, "error": "could not save the request" }).to_string())
        }
    }
}

/// GET /sync/health — per-thread supervisor state (restarts, last panic text). No DB access.
fn sync_health(_: &Ctx) -> Response {
    let mut v = crate::supervisor::health_json();
    v["database"] = crate::db_health::json();
    Response::json(v.to_string())
}

/// GET /sync/serial — cheap change counters the UI polls (no DB access).
fn sync_serial(_: &Ctx) -> Response {
    Response::json(
        serde_json::json!({
            "mail": crate::sync_state::mail_serial(),
            "sms": crate::sync_state::sms_serial(),
            "mcp_pending": crate::mcp::confirm::serial(),
            "sync_health": crate::supervisor::health_serial(),
        })
        .to_string(),
    )
}

/// POST /perf?m=label — a mark from the QML side (shares the backend's perf timeline).
fn perf(c: &Ctx) -> Response {
    if let Some(m) = c.query("m") {
        crate::perf::mark(&format!("UI: {}", m));
    }
    Response::new(200, "", &[], b"ok".to_vec())
}

/// GET /folders[?account=<id or email>] — cached folders; every row names its account (id +
/// address). Without `account`, "<Kind> - all accounts" rows come first: one per folder kind two
/// or more accounts have, with an empty account_id so move targets skip them.
fn folders(c: &Ctx) -> Response {
    let conn = match rusqlite::Connection::open(c.config_dir.join("messages.db")) {
        Ok(conn) => conn,
        Err(e) => {
            warn!("GET /folders: DB open failed: {}", e);
            return Response::json("[]");
        }
    };
    let account = c.query("account");
    let rows = match folder_rows(&conn, account.as_deref()) {
        Ok(rows) => rows,
        Err(e) => {
            warn!("GET /folders: query failed: {}", e);
            Vec::new()
        }
    };
    let mut all_rows: Vec<serde_json::Value> = Vec::new();
    if account.is_none() {
        if let Ok(aggs) = crate::db::all_accounts_folders(&conn) {
            for a in aggs {
                all_rows.push(serde_json::json!({
                    "id": format!("all:{}", a.well_known_name),
                    "display_name": crate::db::all_accounts_label(&a.well_known_name),
                    "unread_item_count": a.unread_item_count,
                    "well_known_name": a.well_known_name,
                    "total_item_count": a.total_item_count,
                    "account_id": "",
                    "account_email": "All accounts",
                    "all_accounts": true,
                }));
            }
        }
    }
    all_rows.extend(rows);
    Response::json(serde_json::Value::Array(all_rows).to_string())
}

fn folder_rows(conn: &rusqlite::Connection, account: Option<&str>) -> rusqlite::Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT f.id, f.display_name, f.unread_item_count, f.well_known_name, f.total_item_count, \
                COALESCE(f.account_id, ''), COALESCE(a.email, '') \
         FROM folders f LEFT JOIN accounts a ON a.id = f.account_id \
         WHERE (?1 IS NULL OR f.account_id = ?1 OR lower(a.email) = lower(?1)) \
         ORDER BY f.account_id ASC, f.sort_order ASC, f.display_name ASC",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![account], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "display_name": row.get::<_, String>(1)?,
                "unread_item_count": row.get::<_, Option<i32>>(2)?.unwrap_or(0),
                "well_known_name": row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                "total_item_count": row.get::<_, Option<i32>>(4)?.unwrap_or(0),
                "account_id": row.get::<_, String>(5)?,
                "account_email": row.get::<_, String>(6)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// GET /accounts — configured accounts and whether each is signed in.
fn accounts(c: &Ctx) -> Response {
    Response::json(crate::account_ops::accounts_json(c.config_dir))
}

/// GET /tasks[?account=ID] — cached To Do tasks of every account (JSON array).
fn tasks(c: &Ctx) -> Response {
    let body = match crate::Database::open(&c.db_path()) {
        Ok(db) => match db.query_tasks(c.query("account").as_deref()) {
            Ok(rows) => serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string()),
            Err(e) => {
                warn!("GET /tasks: query failed: {}", e);
                "[]".to_string()
            }
        },
        Err(e) => {
            warn!("GET /tasks: DB open failed: {}", e);
            "[]".to_string()
        }
    };
    Response::json(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::request::{read_request, test_stream, Limits};

    fn ctx_run(dir: &std::path::PathBuf, line: &str, f: fn(&Ctx) -> Response) -> Response {
        let raw = format!("{}\r\nHost: 127.0.0.1:1\r\nContent-Length: 0\r\n\r\n", line);
        let req = read_request(&mut test_stream(raw.into_bytes()), &Limits::default()).unwrap();
        f(&Ctx::new(dir, &req))
    }

    #[test]
    fn serial_and_folders_answer_json_without_a_db() {
        let dir = std::env::temp_dir().join(format!("omalook-core-routes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let r = ctx_run(&dir, "GET /sync/serial HTTP/1.1", sync_serial);
        assert_eq!(r.status(), 200);
        let v: serde_json::Value = serde_json::from_slice(r.body()).unwrap();
        assert!(v.get("mail").is_some() && v.get("mcp_pending").is_some());
        // A DB with no folders table: a JSON array, not a panic (was `.unwrap()` on prepare).
        let r = ctx_run(&dir, "GET /folders?account=x HTTP/1.1", folders);
        assert_eq!((r.status(), r.body()), (200, &b"[]"[..]));
        let r = ctx_run(&dir, "POST /perf?m=hello HTTP/1.1", perf);
        assert_eq!((r.status(), r.body()), (200, &b"ok"[..]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
