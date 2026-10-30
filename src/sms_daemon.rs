//! SMS sync daemon: mirrors the phone's text messages into the local cache through the
//! KDE Connect daemon (`kdeconnectd`) already running on this machine, over the session bus.
//!
//! How KDE Connect exposes SMS (verified against the live daemon):
//! * `org.kde.kdeconnect.device.conversations` at `/modules/kdeconnect/devices/<id>`.
//! * `requestAllConversationThreads()` makes the phone stream the latest message of every thread
//!   (signals `conversationCreated`, one per thread, over tens of seconds for ~1000 threads).
//! * `requestConversation(threadId, start, end)` (int32 index range, 0..N) streams that thread's
//!   history as `conversationCreated/Updated` signals, then `conversationLoaded(threadId, count)`.
//! * The daemon keeps all this in RAM only, so this cache is the only durable copy.
//!
//! Update path: once a device has been pulled in full, new/changed messages arrive as the
//! same signals and are upserted as they come; a periodic re-request of the thread list is a
//! safety net. `POST /sms/sync` (see `request_sync`) forces a fresh full pass.
//!
//! The daemon is plain blocking threads (zbus's blocking API owns its own executor):
//! one listener thread feeds an mpsc channel, one worker thread writes the database.

use crate::db::sms::{SmsAttachment, SmsMessage};
use crate::db::Database;
use log::{debug, error, info, warn};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::{Array, Structure, Value};
use zbus::MatchRule;

const SERVICE: &str = "org.kde.kdeconnect";
const IFACE: &str = "org.kde.kdeconnect.device.conversations";
const DEVICE_ROOT: &str = "/modules/kdeconnect/devices/";
/// Re-discover devices this often.
const DISCOVER_EVERY: Duration = Duration::from_secs(30);
/// Safety-net refresh of every thread's latest message.
const REFRESH_EVERY: Duration = Duration::from_secs(600);
/// How long a thread may stay silent while its history streams before it is retried later.
const THREAD_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_ATTEMPTS: u32 = 3;
/// Minimum gap between reconnect nudges while a paired phone is missing.
const NUDGE_EVERY: Duration = Duration::from_secs(60);

enum Ev {
    Msg(SmsMessage),
    Loaded { device: String, thread: i64, count: i64 },
    Removed { device: String, thread: i64 },
    /// KDE Connect downloaded an MMS attachment from the phone: (device, local file path, file name).
    Attachment { device: String, path: String, name: String },
    Resync,
}

static CMD: OnceLock<Sender<Ev>> = OnceLock::new();

/// Ask the daemon for a fresh full pass of every device (history is re-pulled; upserts make it cheap).
pub fn request_sync() -> bool {
    CMD.get().map(|tx| tx.send(Ev::Resync).is_ok()).unwrap_or(false)
}

/// Start the SMS daemon thread. Returns immediately.
pub fn start(config_dir: PathBuf) {
    let config_dir_for_ticker = config_dir.clone();
    let (tx, rx) = channel::<Ev>();
    if CMD.set(tx.clone()).is_err() {
        warn!("SMS daemon already started");
        return;
    }
    let r = std::thread::Builder::new().name("sms".into()).spawn(move || {
        let db_path = config_dir.join("messages.db");
        let db = match Database::open(db_path.to_str().unwrap_or("messages.db")) {
            Ok(db) => db,
            Err(e) => return error!("SMS daemon: cannot open database: {}", e),
        };
        Worker { db, tx, rx, conn: None, synced: HashSet::new(), attempts: HashMap::new(), dirty: false, last_nudge: None }.run();
    });
    if let Err(e) = r {
        error!("Cannot spawn SMS daemon thread: {}", e);
    }
    // Sent texts the phone never echoed turn into "send failure" after SEND_TIMEOUT.
    let dir = config_dir_for_ticker;
    let _ = std::thread::Builder::new().name("sms-outbox".into()).spawn(move || {
        let Ok(db) = Database::open(dir.join("messages.db").to_str().unwrap_or("messages.db")) else { return };
        loop {
            std::thread::sleep(Duration::from_secs(10));
            if matches!(db.sms_expire_pending(SEND_TIMEOUT.as_millis() as i64), Ok(n) if n > 0) {
                warn!("SMS: a sent text was never confirmed by the phone; marked as failed");
                crate::sync_state::bump_sms("send timeout");
            }
        }
    });
}

// ── MMS attachments (OL-032) ──────────────────────────────────────────────
// KDE Connect only lists an MMS part's metadata; the bytes are fetched on demand with
// `requestAttachmentFile(part_id, unique_id)` and arrive later as the `attachmentReceived(path, name)`
// signal. The worker copies each arrival into our own cache so it outlives kdeconnectd's.

/// How long a requested attachment may stay missing before the request counts as failed.
const ATT_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a failure is remembered before the next poll asks the phone again.
const ATT_RETRY_AFTER: Duration = Duration::from_secs(30);

enum AttState { Pending(Instant), Failed(String, Instant) }

fn att_map() -> &'static std::sync::Mutex<HashMap<(String, String), AttState>> {
    static M: OnceLock<std::sync::Mutex<HashMap<(String, String), AttState>>> = OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn safe_name(s: &str) -> String {
    let n: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' }).collect();
    if n.is_empty() || n.chars().all(|c| c == '.') { "_".into() } else { n }
}

/// Where a device's attachment is kept (`<cache>/omalook/sms-attachments/<device>/<unique id>`).
pub fn attachment_path(device: &str, unique_id: &str) -> PathBuf {
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir)
        .join("omalook").join("sms-attachments").join(safe_name(device)).join(safe_name(unique_id))
}

/// State of one attachment for the UI, without ever blocking: `{state: "ready", path}`, `{state: "pending"}`
/// or `{state: "failed", error}`. The first call for a missing file asks the phone for it.
pub fn attachment_status(device: &str, part_id: i64, unique_id: &str) -> serde_json::Value {
    let file = attachment_path(device, unique_id);
    if file.is_file() {
        return serde_json::json!({ "state": "ready", "path": file.to_string_lossy() });
    }
    // kdeconnectd saves a downloaded part as `<cache>/kdeconnect.daemon/<device name>/<unique id>` and does not
    // always announce it with `attachmentReceived`, so look there too (verified live: the file appears < 2 s).
    if let Some(found) = find_in_kdeconnect_cache(unique_id) {
        let dest = file.clone();
        if let Some(dir) = dest.parent() { let _ = std::fs::create_dir_all(dir); }
        let tmp = dest.with_extension("part");
        if std::fs::copy(&found, &tmp).and_then(|_| std::fs::rename(&tmp, &dest)).is_ok() {
            att_map().lock().unwrap_or_else(|e| e.into_inner()).remove(&(device.to_string(), unique_id.to_string()));
            return serde_json::json!({ "state": "ready", "path": dest.to_string_lossy() });
        }
    }
    let key = (device.to_string(), unique_id.to_string());
    let mut map = att_map().lock().unwrap_or_else(|e| e.into_inner());
    match map.get(&key) {
        Some(AttState::Pending(at)) if at.elapsed() < ATT_TIMEOUT => return serde_json::json!({ "state": "pending" }),
        Some(AttState::Pending(_)) => {
            map.insert(key, AttState::Failed("the phone did not send the picture (is it reachable?)".into(), Instant::now()));
            return serde_json::json!({ "state": "failed", "error": "the phone did not send the picture (is it reachable?)" });
        }
        Some(AttState::Failed(e, at)) if at.elapsed() < ATT_RETRY_AFTER => return serde_json::json!({ "state": "failed", "error": e }),
        _ => {}
    }
    match request_attachment(device, part_id, unique_id) {
        Ok(()) => { map.insert(key, AttState::Pending(Instant::now())); serde_json::json!({ "state": "pending" }) }
        Err(e) => { map.insert(key, AttState::Failed(e.clone(), Instant::now())); serde_json::json!({ "state": "failed", "error": e }) }
    }
}

fn find_in_kdeconnect_cache(unique_id: &str) -> Option<PathBuf> {
    if unique_id.is_empty() || unique_id.contains('/') || unique_id == "." || unique_id == ".." { return None; }
    let root = dirs::cache_dir()?.join("kdeconnect.daemon");
    std::fs::read_dir(root).ok()?.flatten()
        .map(|d| d.path().join(unique_id))
        .find(|p| p.is_file() && p.metadata().map(|m| m.len() > 0).unwrap_or(false))
}

fn request_attachment(device: &str, part_id: i64, unique_id: &str) -> Result<(), String> {
    let conn = Connection::session().map_err(|e| format!("session bus: {}", e))?;
    conn.call_method(Some(SERVICE), format!("{}{}", DEVICE_ROOT, device), Some(IFACE), "requestAttachmentFile", &(part_id, unique_id))
        .map(|_| ())
        .map_err(|e| format!("KDE Connect refused the request: {}", e))
}

/// Copy a downloaded attachment into our cache and clear its pending entry. Returns true when stored.
fn store_attachment(device: &str, src: &str, name: &str) -> bool {
    let mut map = att_map().lock().unwrap_or_else(|e| e.into_inner());
    // The phone names the file after the part's unique id; if it does not, a lone pending request claims it.
    let key = if map.contains_key(&(device.to_string(), name.to_string())) {
        Some((device.to_string(), name.to_string()))
    } else {
        let mut pend = map.iter().filter(|(k, v)| k.0 == device && matches!(v, AttState::Pending(_))).map(|(k, _)| k.clone());
        match (pend.next(), pend.next()) { (Some(k), None) => Some(k), _ => None }
    };
    let unique = key.as_ref().map(|k| k.1.clone()).unwrap_or_else(|| name.to_string());
    let dest = attachment_path(device, &unique);
    if let Some(dir) = dest.parent() { let _ = std::fs::create_dir_all(dir); }
    let tmp = dest.with_extension("part");
    let ok = std::fs::copy(src, &tmp).and_then(|_| std::fs::rename(&tmp, &dest)).is_ok();
    if ok { if let Some(k) = key { map.remove(&k); } } else { warn!("SMS: cannot keep attachment {} from {}", unique, src); }
    ok
}

/// How long a sent text may wait for the phone's echo before it is shown as failed.
const SEND_TIMEOUT: Duration = Duration::from_secs(120);

fn address_value(addr: &str) -> Value<'static> {
    // KDE Connect wants ConversationAddress structs, D-Bus type "(s)"; plain strings are silently dropped.
    Value::Structure(zbus::zvariant::StructureBuilder::new().add_field(addr.to_string()).build())
}

/// Hand a text to the phone through KDE Connect. `thread` (a real thread id) replies in that
/// conversation; otherwise it goes to `addresses` as a new one. Returns once the daemon has
/// accepted it: the phone's stored copy follows as a sent message 10-60 s later (see
/// `Database::sms_reconcile_pending`), so this success is "queued", not "delivered".
pub fn send_text(device: &str, thread: Option<i64>, addresses: &[String], text: &str) -> Result<(), String> {
    let conn = Connection::session().map_err(|e| format!("no session bus: {}", e))?;
    let ids: Vec<String> = conn.call_method(Some(SERVICE), "/modules/kdeconnect", Some("org.kde.kdeconnect.daemon"), "devices", &(true, true))
        .and_then(|m| m.body())
        .map_err(|_| "KDE Connect is not running".to_string())?;
    if !ids.iter().any(|d| d == device) { return Err("phone not reachable".into()); }
    let path = format!("{}{}", DEVICE_ROOT, device);
    let atts: Vec<Value> = vec![];
    let r = match thread {
        Some(t) if t > 0 => conn.call_method(Some(SERVICE), path.as_str(), Some(IFACE), "replyToConversation", &(t, text, atts)),
        _ => {
            if addresses.is_empty() { return Err("no recipient".into()); }
            let addrs: Vec<Value> = addresses.iter().map(|a| address_value(a)).collect();
            conn.call_method(Some(SERVICE), path.as_str(), Some(IFACE), "sendWithoutConversation", &(addrs, text, atts))
        }
    };
    r.map(|_| ()).map_err(|e| format!("KDE Connect refused the text: {}", e))
}

/// Package ids / notification texts of apps that are SMS clients.
fn is_sms_app(internal_id: &str, app: &str) -> bool {
    let id = internal_id.to_lowercase();
    let a = app.to_lowercase();
    ["com.textra", "com.google.android.apps.messaging", "com.android.mms", "com.samsung.android.messaging", "com.oneplus.mms", "com.oplus.mms"]
        .iter().any(|p| id.contains(p)) || ["messages", "messaging", "textra", "sms"].iter().any(|n| a == *n)
}

/// Best-effort read sync: KDE Connect has no "mark read" call, so ask the phone's SMS app
/// through the thread's notification, using the app's own "Mark as read" action. Dismissing the
/// notification alone does not mark the thread read (tested with Textra). Runs on its own
/// thread; the outcome is only logged. `needles` = contact names / numbers that identify the thread.
pub fn sync_read_to_phone(device: String, needles: Vec<String>) {
    let _ = std::thread::Builder::new().name("sms-readsync".into()).spawn(move || {
        let Ok(conn) = Connection::session() else { return };
        let base = format!("{}{}/notifications", DEVICE_ROOT, device);
        let ids: Vec<String> = match conn.call_method(Some(SERVICE), base.as_str(), Some("org.kde.kdeconnect.device.notifications"), "activeNotifications", &())
            .and_then(|m| m.body()) { Ok(v) => v, Err(_) => return };
        let digits = |s: &str| s.chars().filter(|c| c.is_ascii_digit()).collect::<String>();
        for id in ids {
            let path = format!("{}/{}", base, id);
            let get = |prop: &str| -> Option<zbus::zvariant::OwnedValue> {
                conn.call_method(Some(SERVICE), path.as_str(), Some("org.freedesktop.DBus.Properties"), "Get",
                                 &("org.kde.kdeconnect.device.notifications.notification", prop)).ok()?.body().ok()
            };
            let s = |prop: &str| get(prop).and_then(|v| String::try_from(v).ok()).unwrap_or_default();
            let (app, title, internal) = (s("appName"), s("title"), s("internalId"));
            if !is_sms_app(&internal, &app) { continue; }
            let t = title.to_lowercase();
            let hit = needles.iter().any(|n| {
                let n = n.trim();
                !n.is_empty() && (t.contains(&n.to_lowercase()) || (digits(n).len() >= 7 && digits(&title).ends_with(&digits(n)[digits(n).len().saturating_sub(10)..])))
            });
            if !hit { continue; }
            for action in ["Mark as read", "Mark as Read", "Mark read"] {
                let r = conn.call_method(Some(SERVICE), base.as_str(), Some("org.kde.kdeconnect.device.notifications"), "sendAction", &(internal.as_str(), action));
                info!("SMS read-sync: action '{}' on notification {} ({} / {}): {}", action, id, app, title, if r.is_ok() { "sent" } else { "refused" });
            }
        }
    });
}

// ── D-Bus payload parsing ─────────────────────────────────────────────────

/// Parse the `(isa(s)xiixixa(xsss))` conversation tuple (already unwrapped from the `v`).
/// Fields: event, body, addresses, date_ms, type, read, thread, uid, sub_id, attachments.
pub fn parse_message(device_id: &str, v: &Value) -> Option<SmsMessage> {
    let v = match v { Value::Value(inner) => inner.as_ref(), other => other };
    let s: &Structure = match v { Value::Structure(s) => s, _ => return None };
    let f = s.fields();
    if f.len() != 10 { return None; }
    let int = |v: &Value| -> Option<i64> {
        match v { Value::I32(n) => Some(*n as i64), Value::I64(n) => Some(*n), Value::U32(n) => Some(*n as i64),
                  Value::U64(n) => Some(*n as i64), _ => None }
    };
    let text = |v: &Value| -> Option<String> { match v { Value::Str(s) => Some(s.to_string()), _ => None } };
    let event = int(&f[0])? as i32;
    let addresses = match &f[2] {
        Value::Array(a) => array_items(a).iter().filter_map(|e| match e {
            Value::Structure(s) => s.fields().first().and_then(text),
            other => text(other),
        }).collect(),
        _ => vec![],
    };
    let attachments: Vec<SmsAttachment> = match &f[9] {
        Value::Array(a) => array_items(a).iter().filter_map(|e| match e {
            Value::Structure(s) => { let g = s.fields(); Some(SmsAttachment {
                part_id: int(g.first()?)?, mime: text(g.get(1)?)?,
                // KDE Connect order: part id, mime, base64 preview (a small PNG), unique id.
                preview: text(g.get(2)?)?, unique_id: text(g.get(3)?)? }) }
            _ => None,
        }).collect(),
        _ => vec![],
    };
    let direction = int(&f[4])? as i32;
    Some(SmsMessage {
        device_id: device_id.to_string(),
        uid: int(&f[7])?,
        // SMS and MMS ids come from separate Android tables and can collide; the MULTI_TARGET
        // event bit or any attachment marks an MMS.
        mms: !attachments.is_empty() || (event & 0x2) != 0,
        thread_id: int(&f[6])?,
        body: text(&f[1])?,
        addresses,
        date_ms: int(&f[3])?,
        direction,
        event,
        sub_id: int(&f[8])?,
        attachments,
        phone_read: int(&f[5])? != 0,
    })
}

fn array_items<'a>(a: &'a Array<'a>) -> Vec<&'a Value<'a>> {
    a.iter().collect()
}

fn device_of_path(path: &str) -> Option<String> {
    path.strip_prefix(DEVICE_ROOT).map(|r| r.split('/').next().unwrap_or("").to_string()).filter(|d| !d.is_empty())
}

// ── Worker ────────────────────────────────────────────────────────────────

struct Worker {
    db: Database,
    tx: Sender<Ev>,
    rx: Receiver<Ev>,
    conn: Option<Connection>,
    /// Devices whose full pull finished in this process (a restart re-checks incomplete threads).
    synced: HashSet<String>,
    attempts: HashMap<(String, i64), u32>,
    /// Cache changed since the UI serial was last bumped (one bump per batch, not per message).
    dirty: bool,
    /// When kdeconnectd was last told to look for its phone again (see `nudge_unreachable`).
    last_nudge: Option<Instant>,
}

impl Worker {
    fn run(mut self) {
        info!("SMS daemon started");
        let mut last_refresh = Instant::now();
        loop {
            if self.conn.is_none() {
                match self.connect() {
                    Ok(c) => self.conn = Some(c),
                    Err(e) => {
                        debug!("SMS daemon: session bus not ready: {}", e);
                        self.pump(Duration::from_secs(15));
                        continue;
                    }
                }
            }
            let devices = self.discover();
            self.nudge_unreachable();
            for (id, name) in &devices {
                let _ = self.db.sms_touch_device(id, name);
                if !self.synced.contains(id) {
                    self.full_sync(id);
                }
            }
            if last_refresh.elapsed() >= REFRESH_EVERY {
                last_refresh = Instant::now();
                for (id, _) in &devices {
                    self.call(id, "requestAllConversationThreads", &());
                }
            }
            self.pump(DISCOVER_EVERY);
        }
    }

    fn connect(&self) -> Result<Connection, String> {
        let conn = Connection::session().map_err(|e| e.to_string())?;
        // One listener for every device (path namespace), feeding the worker.
        let rule = MatchRule::builder()
            .msg_type(zbus::MessageType::Signal)
            .sender(SERVICE).map_err(|e| e.to_string())?
            .interface(IFACE).map_err(|e| e.to_string())?
            .build();
        let iter = MessageIterator::for_match_rule(rule, &conn, Some(4096)).map_err(|e| e.to_string())?;
        let tx = self.tx.clone();
        std::thread::Builder::new().name("sms-signals".into()).spawn(move || listen(iter, tx)).map_err(|e| e.to_string())?;
        Ok(conn)
    }

    /// A paired phone that is not reachable (Wi-Fi roam, phone slept, network change missed) stays that way until
    /// kdeconnectd is told to announce itself again. That is what the KDE Connect app's refresh button does
    /// (`daemon.forceOnNetworkChange`); do the same, at most once per NUDGE_EVERY, only while a paired device is missing.
    fn nudge_unreachable(&mut self) {
        if self.last_nudge.map(|t| t.elapsed() < NUDGE_EVERY).unwrap_or(false) { return; }
        let Some(conn) = self.conn.as_ref() else { return };
        let list = |reachable: bool| -> Option<Vec<String>> {
            conn.call_method(Some(SERVICE), "/modules/kdeconnect", Some("org.kde.kdeconnect.daemon"), "devices", &(reachable, true))
                .and_then(|m| m.body()).ok()
        };
        let (Some(paired), Some(reachable)) = (list(false), list(true)) else { return };
        let missing: Vec<&String> = paired.iter().filter(|d| !reachable.contains(d)).collect();
        if missing.is_empty() { return; }
        self.last_nudge = Some(Instant::now());
        match conn.call_method(Some(SERVICE), "/modules/kdeconnect", Some("org.kde.kdeconnect.daemon"), "forceOnNetworkChange", &()) {
            Ok(_) => info!("SMS: {} paired device(s) not reachable; asked kdeconnectd to reconnect", missing.len()),
            Err(e) => warn!("SMS: could not ask kdeconnectd to reconnect: {}", e),
        }
    }

    /// Reachable, paired devices that expose the conversations interface: (id, name).
    fn discover(&mut self) -> Vec<(String, String)> {
        let Some(conn) = self.conn.as_ref() else { return vec![] };
        let ids: Vec<String> = match conn.call_method(Some(SERVICE), "/modules/kdeconnect", Some("org.kde.kdeconnect.daemon"),
                                                      "devices", &(true, true)).and_then(|m| m.body()) {
            Ok(v) => v,
            Err(e) => {
                debug!("SMS daemon: kdeconnectd not available ({}); will retry", e);
                // The daemon may have restarted: rebuild the connection and its listener.
                self.conn = None;
                return vec![];
            }
        };
        let mut out = vec![];
        for id in ids {
            let path = format!("{}{}", DEVICE_ROOT, id);
            // The SMS plugin answers activeConversations; a device without it errors out.
            let has_sms = conn.call_method(Some(SERVICE), path.as_str(), Some(IFACE), "activeConversations", &()).is_ok();
            if !has_sms { continue; }
            let name = conn.call_method(Some(SERVICE), path.as_str(), Some("org.freedesktop.DBus.Properties"), "Get",
                                        &("org.kde.kdeconnect.device", "name"))
                .and_then(|m| m.body::<zbus::zvariant::OwnedValue>())
                .ok().and_then(|v| String::try_from(v).ok()).unwrap_or_default();
            out.push((id, name));
        }
        out
    }

    fn call<B: serde::Serialize + zbus::zvariant::DynamicType>(&self, device: &str, method: &str, body: &B) -> bool {
        let Some(conn) = self.conn.as_ref() else { return false };
        match conn.call_method(Some(SERVICE), format!("{}{}", DEVICE_ROOT, device), Some(IFACE), method, body) {
            Ok(_) => true,
            Err(e) => { warn!("SMS {} failed: {}", method, e); false }
        }
    }

    fn active_thread_ids(&self, device: &str) -> Vec<i64> {
        let Some(conn) = self.conn.as_ref() else { return vec![] };
        let reply = conn.call_method(Some(SERVICE), format!("{}{}", DEVICE_ROOT, device), Some(IFACE), "activeConversations", &());
        let Ok(reply) = reply else { return vec![] };
        let Ok(items) = reply.body::<Vec<Value>>() else { return vec![] };
        items.iter().filter_map(|v| parse_message(device, v)).map(|m| m.thread_id).collect()
    }

    /// Pull the device's whole SMS history into the cache: thread list first, then every
    /// thread's messages. Threads that fail are retried on later passes (up to MAX_ATTEMPTS).
    fn full_sync(&mut self, device: &str) {
        info!("SMS[{}]: full history pull starting", device);
        let started = Instant::now();
        self.call(device, "requestAllConversationThreads", &());

        // The phone streams thread heads slowly; wait until the count stops growing.
        let mut last = 0usize;
        let mut stable = 0;
        let deadline = Instant::now() + Duration::from_secs(300);
        while Instant::now() < deadline && stable < 3 {
            self.pump(Duration::from_secs(3));
            let n = self.active_thread_ids(device).len();
            if n == last && n > 0 { stable += 1 } else { stable = 0; last = n }
        }
        let mut threads: Vec<i64> = self.active_thread_ids(device);
        threads.extend(self.db.sms_known_threads(device).unwrap_or_default());
        threads.sort_unstable();
        threads.dedup();
        info!("SMS[{}]: {} threads known; fetching history", device, threads.len());

        let mut pending = 0usize;
        for (i, &t) in threads.iter().enumerate() {
            if self.db.sms_thread_is_complete(device, t).unwrap_or(false) { continue; }
            let tries = self.attempts.entry((device.to_string(), t)).or_insert(0);
            if *tries >= MAX_ATTEMPTS { pending += 1; continue; }
            *tries += 1;
            if !self.call(device, "requestConversation", &(t, 0i32, 100_000i32)) { pending += 1; continue; }
            if !self.wait_loaded(device, t) { pending += 1; }
            if i % 50 == 49 {
                info!("SMS[{}]: {}/{} threads, {} messages cached", device, i + 1, threads.len(),
                      self.db.sms_message_count(device).unwrap_or(0));
            }
        }
        if pending == 0 {
            let _ = self.db.sms_mark_full_sync(device);
            self.synced.insert(device.to_string());
            info!("SMS[{}]: full pull done in {}s, {} messages cached", device, started.elapsed().as_secs(),
                  self.db.sms_message_count(device).unwrap_or(0));
        } else {
            warn!("SMS[{}]: pull finished with {} thread(s) incomplete; will retry", device, pending);
            // Don't spin: the next discover pass re-enters here; threads past MAX_ATTEMPTS stop it.
            if self.attempts.iter().filter(|((d, _), n)| d == device && **n >= MAX_ATTEMPTS).count() >= pending {
                self.synced.insert(device.to_string());
            }
        }
        self.flush_serial("full sync");
        crate::sync_state::bump_sms("full sync");
    }

    /// Pump events until `conversationLoaded` arrives for the requested thread (or it times out),
    /// then record the thread complete. `conversationLoaded` is also sent for each thread head by
    /// `requestAllConversationThreads` (count 1), so it only counts here, after our own request.
    fn wait_loaded(&mut self, device: &str, thread: i64) -> bool {
        let mut deadline = Instant::now() + THREAD_TIMEOUT;
        let mut loaded: Option<i64> = None;
        loop {
            // After the signal, linger briefly: a stray head-load for this thread may precede the real one.
            let left = if loaded.is_some() { Duration::from_millis(150) }
                       else { deadline.saturating_duration_since(Instant::now()) };
            if left.is_zero() { break; }
            match self.rx.recv_timeout(left) {
                Ok(Ev::Loaded { device: d, thread: t, count }) => {
                    if d == device && t == thread { loaded = Some(count); }
                }
                Ok(ev) => {
                    // Big threads stream for minutes: the timeout is idle time, not total time.
                    if matches!(&ev, Ev::Msg(m) if m.device_id == device && m.thread_id == thread) {
                        deadline = Instant::now() + THREAD_TIMEOUT;
                    }
                    self.handle(ev)
                }
                Err(_) => break,
            }
        }
        self.flush_serial("history");
        match loaded {
            Some(count) => {
                if let Err(e) = self.db.sms_thread_loaded(device, thread, count) {
                    warn!("SMS: cannot record thread {} load: {}", thread, e);
                }
                true
            }
            None => false,
        }
    }

    fn flush_serial(&mut self, why: &str) {
        if self.dirty {
            self.dirty = false;
            crate::sync_state::bump_sms(why);
        }
    }

    /// Process events for `dur`.
    fn pump(&mut self, dur: Duration) {
        let end = Instant::now() + dur;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() { self.flush_serial("message"); return; }
            match self.rx.recv_timeout(left) {
                Ok(Ev::Resync) => {
                    info!("SMS: resync requested");
                    self.synced.clear();
                    self.attempts.clear();
                    return;
                }
                Ok(ev) => self.handle(ev),
                Err(RecvTimeoutError::Timeout) => { self.flush_serial("message"); return }
                Err(RecvTimeoutError::Disconnected) => { std::thread::sleep(left); return; }
            }
        }
    }

    fn handle(&mut self, ev: Ev) {
        match ev {
            Ev::Msg(m) => {
                // The phone's copy of a text we sent replaces our pending row.
                if m.direction == 2 && matches!(self.db.sms_reconcile_pending(&m), Ok(n) if n > 0) { self.dirty = true; }
                match self.db.sms_upsert(&m) {
                Ok(true) => self.dirty = true,
                Ok(false) => {}
                Err(e) => warn!("SMS: cannot store message {}: {}", m.uid, e),
                }
            }
            // Loaded outside a history request is a thread-head load; nothing to record.
            Ev::Loaded { .. } => {}
            Ev::Removed { device, thread } => {
                if self.db.sms_delete_thread(&device, thread).is_ok() { self.dirty = true; }
            }
            Ev::Attachment { device, path, name } => {
                if store_attachment(&device, &path, &name) { self.dirty = true; }
            }
            Ev::Resync => {}
        }
    }
}

/// Signal listener thread: turns bus signals into events.
fn listen(iter: MessageIterator, tx: Sender<Ev>) {
    for msg in iter {
        let Ok(msg) = msg else { continue };
        let Ok(header) = msg.header() else { continue };
        let (Ok(Some(path)), Ok(Some(member))) = (header.path(), header.member()) else { continue };
        let Some(device) = device_of_path(path.as_str()) else { continue };
        let ev = match member.as_str() {
            "conversationCreated" | "conversationUpdated" => {
                match msg.body::<Value>().ok().and_then(|v| parse_message(&device, &v)) {
                    Some(m) => Ev::Msg(m),
                    None => { debug!("SMS: unparseable {} signal", member); continue }
                }
            }
            "conversationLoaded" => match msg.body::<(i64, u64)>() {
                Ok((thread, count)) => Ev::Loaded { device, thread, count: count as i64 },
                Err(_) => continue,
            },
            "attachmentReceived" => match msg.body::<(String, String)>() {
                Ok((path, name)) => Ev::Attachment { device, path, name },
                Err(_) => continue,
            },
            "conversationRemoved" => match msg.body::<i64>() {
                Ok(thread) => Ev::Removed { device, thread },
                Err(_) => continue,
            },
            _ => continue,
        };
        if tx.send(ev).is_err() { return; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_names_cannot_escape_the_cache() {
        let p = attachment_path("dev/../x", "../../etc/passwd");
        assert!(p.starts_with(dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("omalook").join("sms-attachments")));
        assert!(p.components().all(|c| !matches!(c, std::path::Component::ParentDir)));
        assert_eq!(safe_name(".."), "_");
    }

    #[test]
    fn a_stored_attachment_reports_ready_and_a_missing_one_never_blocks() {
        let dev = format!("test-dev-{}", std::process::id());
        let file = attachment_path(&dev, "PART_1.jpg");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"x").unwrap();
        let r = attachment_status(&dev, 1, "PART_1.jpg");
        assert_eq!(r["state"], "ready");
        // Not stored: the call returns at once (pending when the bus took the request, failed when not).
        let m = attachment_status(&dev, 2, "MISSING.jpg");
        assert!(m["state"] == "pending" || m["state"] == "failed");
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }
    use zbus::zvariant::StructureBuilder;

    fn tuple(event: i32, body: &str, addrs: &[&str], date: i64, typ: i32, read: i32, thread: i64, uid: i32, atts: usize) -> Value<'static> {
        let mut addr_arr = Array::new(zbus::zvariant::Signature::from_str_unchecked("(s)"));
        for a in addrs {
            addr_arr.append(Value::Structure(StructureBuilder::new().add_field(a.to_string()).build())).unwrap();
        }
        let mut att_arr = Array::new(zbus::zvariant::Signature::from_str_unchecked("(xsss)"));
        for i in 0..atts {
            att_arr.append(Value::Structure(StructureBuilder::new()
                .add_field(i as i64).add_field("image/jpeg".to_string()).add_field("pv".to_string()).add_field("PART_1.jpg".to_string()).build())).unwrap();
        }
        Value::Structure(StructureBuilder::new()
            .add_field(event).add_field(body.to_string()).append_field(Value::Array(addr_arr))
            .add_field(date).add_field(typ).add_field(read).add_field(thread).add_field(uid).add_field(7i64)
            .append_field(Value::Array(att_arr)).build())
    }

    #[test]
    fn parses_a_received_text() {
        let v = tuple(1, "hello", &["+15550001", "+15550002"], 1_700_000_000_000, 1, 0, 42, 99, 0);
        let m = parse_message("dev", &v).unwrap();
        assert_eq!((m.uid, m.thread_id, m.direction, m.phone_read, m.mms), (99, 42, 1, false, false));
        assert_eq!(m.addresses, vec!["+15550001", "+15550002"]);
        assert_eq!(m.body, "hello");
        assert_eq!(m.date_ms, 1_700_000_000_000);
    }

    #[test]
    fn attachments_and_multitarget_mark_mms() {
        assert!(parse_message("d", &tuple(1, "", &["+1"], 1, 1, 1, 1, 1, 2)).unwrap().mms);
        // Field order is part id, mime, base64 preview, unique id (it was once read as id = preview).
        let a = &parse_message("d", &tuple(1, "", &["+1"], 1, 1, 1, 1, 1, 1)).unwrap().attachments[0];
        assert_eq!((a.preview.as_str(), a.unique_id.as_str()), ("pv", "PART_1.jpg"));
        assert!(parse_message("d", &tuple(3, "g", &["+1", "+2"], 1, 1, 1, 1, 1, 0)).unwrap().mms);
        assert!(!parse_message("d", &tuple(1, "t", &["+1"], 1, 2, 1, 1, 1, 0)).unwrap().mms);
    }

    #[test]
    fn rejects_malformed_payloads() {
        assert!(parse_message("d", &Value::I32(3)).is_none());
        assert!(parse_message("d", &Value::Structure(StructureBuilder::new().add_field(1i32).build())).is_none());
    }

    #[test]
    fn device_id_comes_from_the_object_path() {
        assert_eq!(device_of_path("/modules/kdeconnect/devices/abc123"), Some("abc123".into()));
        assert_eq!(device_of_path("/modules/kdeconnect/devices/abc123/sms"), Some("abc123".into()));
        assert_eq!(device_of_path("/modules/other"), None);
    }
}
