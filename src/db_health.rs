//! Database health (roadmap R3.9, OL-082): find a damaged `messages.db` and rebuild the cache from it.
//!
//! - At start-up, before anything opens the DB: if the user asked for a rebuild (marker file), or the
//!   file cannot be read as a database at all, the file is moved aside and a fresh one is built with
//!   every readable row of the old one copied over (`rebuild`). The damaged file is kept.
//! - Shortly after start-up a background `PRAGMA quick_check` reads the whole file; damage it finds is
//!   shown in the UI (`GET /sync/health` -> `database`), which offers "rebuild on next start".
//!
//! What a rebuild keeps: every table the new schema has, copied row by row where readable (drafts,
//! outbox, rules, tags, sender settings, SMS, accounts ... and the cached mail too, when readable).
//! What it drops on purpose: the sync positions (`DROPPED_CURSORS`), so every account walks its mail and
//! contacts again from the start and refills whatever was lost.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Sync positions that are never copied: without them each account re-walks from the start, which
/// refetches anything a damaged cache table lost (and a position past lost rows would hide them for good).
pub const DROPPED_CURSORS: &[&str] = &["mail_delta", "mail_history", "contact_sync"];

/// File the UI's "rebuild on next start" leaves in the config dir.
pub const MARKER: &str = "db-rebuild-requested";

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    /// No check has finished yet.
    Unchecked,
    Ok,
    /// `quick_check` (or opening) found damage; the first problems, as SQLite words them.
    Damaged(String),
    /// The user asked for a rebuild at the next start.
    RebuildRequested,
    /// This start rebuilt the DB. `lost` = tables whose rows could not be read (refilled by sync when
    /// they are cache); `kept` = path of the damaged file.
    Rebuilt { lost: Vec<String>, kept: String },
}

static STATE: Mutex<State> = Mutex::new(State::Unchecked);

fn set(s: State) {
    let mut cur = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if *cur != s {
        *cur = s;
        crate::supervisor::bump_health_serial();
    }
}

pub fn state() -> State {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// `database` part of `GET /sync/health`.
pub fn json() -> Value {
    match state() {
        State::Unchecked => json!({ "state": "unchecked" }),
        State::Ok => json!({ "state": "ok" }),
        State::Damaged(d) => json!({ "state": "damaged", "detail": d }),
        State::RebuildRequested => json!({ "state": "rebuild_requested" }),
        // The damaged copy's path stays in the log; the UI only needs to know what was lost.
        State::Rebuilt { lost, .. } => json!({ "state": "rebuilt", "lost": lost }),
    }
}

pub fn marker_path(config_dir: &Path) -> PathBuf {
    config_dir.join(MARKER)
}

/// Ask for a rebuild at the next start (`cancel` withdraws the request).
pub fn request_rebuild(config_dir: &Path, cancel: bool) -> std::io::Result<()> {
    let marker = marker_path(config_dir);
    if cancel {
        match std::fs::remove_file(&marker) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        // Back to what the last check said; a new check will run at the next start anyway.
        if state() == State::RebuildRequested {
            set(State::Unchecked);
        }
        return Ok(());
    }
    std::fs::write(&marker, b"")?;
    set(State::RebuildRequested);
    Ok(())
}

/// `None` = healthy; `Some(text)` = the first problems `PRAGMA quick_check` reports.
pub fn quick_check(conn: &Connection) -> rusqlite::Result<Option<String>> {
    let mut stmt = conn.prepare("PRAGMA quick_check(5)")?;
    let rows: Vec<String> = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?;
    if rows.len() == 1 && rows[0] == "ok" {
        Ok(None)
    } else {
        Ok(Some(rows.join("; ")))
    }
}

/// Whether an open/read error means the file itself is damaged (not "busy" or "no permission").
fn is_damage(e: &rusqlite::Error) -> bool {
    matches!(
        e.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseCorrupt) | Some(rusqlite::ErrorCode::NotADatabase)
    )
}

/// Can the file be opened and its schema read? `Err` only for damage.
fn readable(db_path: &Path) -> Result<(), String> {
    if !db_path.exists() {
        return Ok(());
    }
    let conn = match Connection::open(db_path) {
        Ok(c) => c,
        Err(e) if is_damage(&e) => return Err(e.to_string()),
        Err(_) => return Ok(()),
    };
    match conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0)) {
        Ok(_) => Ok(()),
        Err(e) if is_damage(&e) => Err(e.to_string()),
        Err(_) => Ok(()),
    }
}

/// Call once at start-up, before anything opens `db_path`. Rebuilds when asked to or when the file
/// cannot be read at all. Never fails start-up: a failed rebuild is logged and the old file stays.
pub fn prepare_at_startup(config_dir: &Path, db_path: &Path) {
    let asked = marker_path(config_dir).exists();
    let damage = readable(db_path).err();
    if !asked && damage.is_none() {
        return;
    }
    if let Some(d) = &damage {
        log::error!("messages.db cannot be read ({}); rebuilding it", d);
    } else {
        log::warn!("messages.db rebuild requested; rebuilding it");
    }
    match rebuild(db_path) {
        Ok(report) => {
            log::warn!(
                "messages.db rebuilt: {} tables copied, unreadable: [{}]; damaged file kept at {}",
                report.copied,
                report.lost.join(", "),
                report.kept.display()
            );
            let _ = std::fs::remove_file(marker_path(config_dir));
            set(State::Rebuilt { lost: report.lost, kept: report.kept.display().to_string() });
        }
        Err(e) => {
            log::error!("messages.db rebuild failed, the old file is still in use: {}", e);
            set(State::Damaged(damage.unwrap_or_else(|| format!("rebuild failed: {}", e))));
        }
    }
}

/// Run `quick_check` on its own connection after `delay`, off every request path. Skipped when this
/// start already rebuilt the DB.
pub fn check_in_background(db_path: PathBuf, delay: std::time::Duration) {
    let spawned = std::thread::Builder::new().name("db-health".into()).spawn(move || {
        std::thread::sleep(delay);
        if matches!(state(), State::Rebuilt { .. } | State::RebuildRequested) {
            return;
        }
        let _t = crate::perf::span("db quick_check");
        let result = Connection::open(&db_path).and_then(|c| {
            c.busy_timeout(std::time::Duration::from_secs(30))?;
            quick_check(&c)
        });
        let verdict = match result {
            Ok(None) => State::Ok,
            Ok(Some(problems)) => {
                log::error!("messages.db quick_check found damage: {}", problems);
                State::Damaged(problems)
            }
            Err(e) if is_damage(&e) => {
                log::error!("messages.db quick_check could not read the file: {}", e);
                State::Damaged(e.to_string())
            }
            // Busy or similar: not a verdict either way.
            Err(e) => {
                log::warn!("messages.db quick_check did not run: {}", e);
                return;
            }
        };
        // A rebuild the user asked for while the check ran stays asked for.
        if state() != State::RebuildRequested {
            set(verdict);
        }
    });
    if let Err(e) = spawned {
        log::warn!("db-health: could not start the check: {}", e);
    }
}

pub struct RebuildReport {
    pub copied: usize,
    pub lost: Vec<String>,
    pub kept: PathBuf,
}

/// Move the DB (and its WAL/SHM) aside, create a fresh one with the current schema, and copy every
/// readable row of every table the new schema has, except the sync positions (`DROPPED_CURSORS`),
/// SQLite's own tables and full-text index tables (rebuilt by the insert triggers).
pub fn rebuild(db_path: &Path) -> Result<RebuildReport, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let kept = PathBuf::from(format!("{}.damaged-{}", db_path.display(), stamp));
    if db_path.exists() {
        std::fs::rename(db_path, &kept).map_err(|e| format!("could not move the old file aside: {}", e))?;
    }
    // The WAL holds committed rows not yet in the main file: it moves with it, under the name SQLite looks for.
    for suffix in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{}", db_path.display(), suffix));
        if side.exists() {
            let _ = std::fs::rename(&side, format!("{}{}", kept.display(), suffix));
        }
    }
    let path_str = db_path.to_str().ok_or("database path is not UTF-8")?;
    crate::Database::open(path_str).map_err(|e| format!("could not create the new database: {}", e))?;

    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;
    if !kept.exists() {
        return Ok(RebuildReport { copied: 0, lost: Vec::new(), kept });
    }
    conn.execute("ATTACH DATABASE ?1 AS old", params![kept.to_string_lossy()]).map_err(|e| format!("could not open the old file: {}", e))?;

    let tables = copyable_tables(&conn).map_err(|e| e.to_string())?;
    let old_tables: Vec<String> = match conn
        .prepare("SELECT name FROM old.sqlite_master WHERE type = 'table'")
        .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>())
    {
        Ok(v) => v,
        // Not even the old schema is readable: nothing can be copied; sync refills the cache.
        Err(e) => {
            log::error!("rebuild: the old schema is unreadable: {}", e);
            let _ = conn.execute("DETACH DATABASE old", []);
            return Ok(RebuildReport { copied: 0, lost: tables, kept });
        }
    };
    let mut copied = 0;
    let mut lost = Vec::new();
    for t in tables {
        if !old_tables.contains(&t) {
            continue;
        }
        match copy_table(&conn, &t) {
            Ok(()) => copied += 1,
            Err(e) => {
                log::warn!("rebuild: rows of {} could not be read: {}", t, e);
                lost.push(t);
            }
        }
    }
    let _ = conn.execute("DETACH DATABASE old", []);
    Ok(RebuildReport { copied, lost, kept })
}

/// Tables of the new (main) schema that a rebuild copies.
fn copyable_tables(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let virtuals: Vec<String> = conn
        .prepare("SELECT name FROM main.sqlite_master WHERE type = 'table' AND sql LIKE 'CREATE VIRTUAL TABLE%'")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    let all: Vec<String> = conn
        .prepare("SELECT name FROM main.sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(all
        .into_iter()
        .filter(|t| !DROPPED_CURSORS.contains(&t.as_str()))
        .filter(|t| !virtuals.iter().any(|v| t == v || t.starts_with(&format!("{}_", v))))
        .collect())
}

/// One table, all-or-nothing: the columns both schemas have, rows the new one already holds kept.
fn copy_table(conn: &Connection, table: &str) -> rusqlite::Result<()> {
    let cols = |schema: &str| -> rusqlite::Result<Vec<String>> {
        conn.prepare(&format!("SELECT name FROM {}.pragma_table_info(?1)", schema))?
            .query_map(params![table], |r| r.get::<_, String>(0))?
            .collect()
    };
    let new_cols = cols("main")?;
    let old_cols = cols("old")?;
    let common: Vec<String> = new_cols.into_iter().filter(|c| old_cols.contains(c)).map(|c| quote(&c)).collect();
    if common.is_empty() {
        return Ok(());
    }
    let list = common.join(", ");
    conn.execute_batch("BEGIN")?;
    let r = conn.execute(
        &format!("INSERT OR IGNORE INTO main.{t} ({list}) SELECT {list} FROM old.{t}", t = quote(table), list = list),
        [],
    );
    match r {
        Ok(_) => conn.execute_batch("COMMIT"),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// SQL identifier quoting (names come from sqlite_master, never from a request).
fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Exposed for the health route's tests and the rebuild report: has `table` rows in `db_path`?
pub fn row_count(db_path: &Path, table: &str) -> Option<i64> {
    let conn = Connection::open(db_path).ok()?;
    conn.query_row(&format!("SELECT COUNT(*) FROM {}", quote(table)), [], |r| r.get(0)).optional().ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("omalook-dbhealth-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn seed(db: &Path) {
        let d = crate::Database::open(db.to_str().unwrap()).unwrap();
        drop(d);
        let c = Connection::open(db).unwrap();
        c.execute_batch(
            "INSERT INTO sender_prefs (email) VALUES ('a@example.com');
             INSERT INTO mail_delta (account_id, folder_id, link) VALUES ('acct', 'inbox', 'https://x/delta?token=1');",
        )
        .unwrap();
    }

    #[test]
    fn a_healthy_file_passes_quick_check() {
        let d = dir("ok");
        let db = d.join("messages.db");
        seed(&db);
        assert_eq!(quick_check(&Connection::open(&db).unwrap()).unwrap(), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rebuild_keeps_local_rows_drops_sync_positions_and_keeps_the_old_file() {
        let d = dir("rebuild");
        let db = d.join("messages.db");
        seed(&db);
        let r = rebuild(&db).unwrap();
        assert!(r.kept.exists(), "the damaged file is kept");
        assert!(r.lost.is_empty(), "{:?}", r.lost);
        assert_eq!(row_count(&db, "sender_prefs"), Some(1));
        assert_eq!(row_count(&db, "mail_delta"), Some(0), "sync positions are not copied");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_that_is_not_a_database_is_rebuilt_at_startup() {
        let d = dir("garbage");
        let db = d.join("messages.db");
        std::fs::write(&db, vec![0x42u8; 8192]).unwrap();
        assert!(readable(&db).is_err());
        prepare_at_startup(&d, &db);
        assert!(readable(&db).is_ok());
        assert!(std::fs::read_dir(&d).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().contains(".damaged-")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_requested_rebuild_happens_once() {
        let d = dir("asked");
        let db = d.join("messages.db");
        seed(&db);
        request_rebuild(&d, false).unwrap();
        assert!(marker_path(&d).exists());
        prepare_at_startup(&d, &db);
        assert!(!marker_path(&d).exists(), "the request is used up");
        assert_eq!(row_count(&db, "sender_prefs"), Some(1));
        request_rebuild(&d, false).unwrap();
        request_rebuild(&d, true).unwrap();
        assert!(!marker_path(&d).exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn damaged_pages_are_found_and_a_rebuild_salvages_the_rest() {
        let d = dir("pages");
        let db = d.join("messages.db");
        seed(&db);
        {
            let c = Connection::open(&db).unwrap();
            c.execute_batch("CREATE TABLE filler (x TEXT); BEGIN;").unwrap();
            for i in 0..3000 {
                c.execute("INSERT INTO filler (x) VALUES (?1)", params![format!("{:0>200}", i)]).unwrap();
            }
            c.execute_batch("COMMIT; PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;").unwrap();
        }
        // Scribble over pages in the second half of the file (the filler's), not the schema page.
        let mut bytes = std::fs::read(&db).unwrap();
        let len = bytes.len();
        assert!(len > 200 * 4096, "file is {} bytes", len);
        for b in &mut bytes[len / 2..len / 2 + 8 * 4096] { *b = 0xA5; }
        std::fs::write(&db, &bytes).unwrap();
        let problems = quick_check(&Connection::open(&db).unwrap());
        assert!(matches!(&problems, Ok(Some(_)) | Err(_)), "damage must be reported: {:?}", problems);
        let r = rebuild(&db).unwrap();
        assert!(readable(&db).is_ok());
        assert_eq!(quick_check(&Connection::open(&db).unwrap()).unwrap(), None, "the new file is healthy");
        assert_eq!(row_count(&db, "sender_prefs"), Some(1), "a table on undamaged pages survives (lost: {:?})", r.lost);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn full_text_and_sqlite_tables_are_not_copied() {
        let d = dir("tables");
        let db = d.join("messages.db");
        seed(&db);
        let c = Connection::open(&db).unwrap();
        let t = copyable_tables(&c).unwrap();
        assert!(t.contains(&"messages".to_string()) && t.contains(&"drafts".to_string()));
        assert!(!t.iter().any(|n| n.starts_with("messages_fts") || n.starts_with("sqlite_") || n == "mail_delta"), "{:?}", t);
        let _ = std::fs::remove_dir_all(&d);
    }
}
