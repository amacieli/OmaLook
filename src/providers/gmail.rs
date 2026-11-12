//! Gmail provider (Gmail REST API v1) behind the same `EmailProvider` trait as Graph.
//!
//! Mapping: labels play the role of folders (system INBOX/SENT/DRAFT/TRASH/SPAM plus the
//! user's own labels); a message is stored once, under the first folder that lists it.
//! Gmail ids are only unique per mailbox, so every id leaving this module is
//! `<account id>:<gmail id>` (see `accounts::scoped`).
//!
//! Pagination mirrors Graph: ids are listed page by page (`nextPageToken`), ids already in the
//! cache are skipped *before* the expensive per-message fetch, and each page's new messages
//! are streamed to the daemon through the channel.

use super::google_api::GoogleApi;
use crate::accounts::{scoped, unscoped};
use crate::errors::Result;
use crate::models::{EmailMessage, MailFolder};
use async_trait::async_trait;
use base64::Engine;
use log::{debug, warn};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

mod extras;
const BASE: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
const PAGE_SIZE: usize = 100;
/// Concurrent `messages.get` calls (Gmail allows ~50/s per user; this stays far below).
const FETCH_CONCURRENCY: usize = 10;

/// System labels shown as folders, with the Graph-style well-known name that drives ordering.
const SYSTEM_FOLDERS: &[(&str, &str, &str)] = &[
    ("INBOX", "Inbox", "inbox"),
    ("DRAFT", "Drafts", "drafts"),
    ("SENT", "Sent", "sentitems"),
    ("TRASH", "Trash", "deleteditems"),
    ("SPAM", "Spam", "junkemail"),
];

pub struct GmailProvider {
    account_id: String,
    api: Arc<GoogleApi>,
    /// Cache database, used only to skip messages that are already stored.
    db_path: Option<PathBuf>,
    /// User labels by raw id -> name, from `labels.list`. A message's tags are its user labels;
    /// the map turns ids into the names the rest of the app uses.
    labels: Arc<Mutex<HashMap<String, String>>>,
}

// ───────────────────────────────────────────────────────────── pure parsing

/// `"Name" <a@b.com>` → `Name`; None when the header has no display name.
pub(crate) fn name_of(from_header: &str) -> Option<String> {
    let h = from_header.trim();
    let l = h.rfind('<')?;
    let name = h[..l].trim().trim_matches('"').trim();
    if name.is_empty() { None } else { Some(name.to_string()) }
}

/// Header value with runs of whitespace (folded lines) collapsed; None when empty.
fn header_text(msg: &serde_json::Value, name: &str) -> Option<String> {
    header(msg, name)
        .map(|v| v.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
}

/// `Importance: high` / `X-Priority: 1` → "high" | "low" | "normal".
fn importance_of(msg: &serde_json::Value) -> String {
    if let Some(i) = header(msg, "Importance").map(|v| v.trim().to_lowercase()) {
        if i == "high" || i == "low" { return i; }
    }
    match header(msg, "X-Priority").and_then(|v| v.trim().chars().next()) {
        Some('1') | Some('2') => "high".into(),
        Some('4') | Some('5') => "low".into(),
        _ => "normal".into(),
    }
}

/// `"Name" <a@b.com>` / `a@b.com` → `a@b.com`
pub(crate) fn address_of(from_header: &str) -> String {
    let h = from_header.trim();
    match (h.rfind('<'), h.rfind('>')) {
        (Some(l), Some(r)) if l < r => h[l + 1..r].trim().to_string(),
        _ => h.trim_matches('"').to_string(),
    }
}

fn header<'a>(msg: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    msg["payload"]["headers"].as_array()?.iter().find_map(|h| {
        h["name"].as_str().filter(|n| n.eq_ignore_ascii_case(name)).and_then(|_| h["value"].as_str())
    })
}

/// Gmail message resource (format=metadata) → `EmailMessage` stored under `folder_id`.
pub(crate) fn parse_message(account_id: &str, folder_id: &str, msg: &serde_json::Value, labels: &HashMap<String, String>) -> Option<EmailMessage> {
    let raw_id = msg["id"].as_str().filter(|s| !s.is_empty())?;
    let received = msg["internalDate"]
        .as_str()
        .and_then(|ms| ms.parse::<i64>().ok())
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
    let unread = msg["labelIds"].as_array().map(|l| l.iter().any(|x| x == "UNREAD")).unwrap_or(false);
    Some(EmailMessage {
        id: scoped(account_id, raw_id),
        from: header(msg, "From").map(address_of).filter(|s| !s.is_empty()).unwrap_or_else(|| "unknown@example.com".into()),
        subject: header(msg, "Subject").map(str::trim).filter(|s| !s.is_empty()).unwrap_or("(no subject)").to_string(),
        received,
        body: msg["snippet"].as_str().unwrap_or("").to_string(),
        folder_id: Some(folder_id.to_string()),
        is_read: !unread,
        meta: true,
        from_name: header(msg, "From").and_then(name_of),
        to_text: header_text(msg, "To"),
        cc_text: header_text(msg, "Cc"),
        bcc_text: header_text(msg, "Bcc"),
        // Tags = the user's own labels on the message (system labels are folders / flags).
        categories: Some(msg["labelIds"].as_array().into_iter().flatten()
            .filter_map(|l| l.as_str())
            .filter_map(|id| labels.get(id).cloned())
            .collect()),
        sent_at: header(msg, "Date")
            .and_then(|d| chrono::DateTime::parse_from_rfc2822(d.trim()).ok())
            .map(|d| d.with_timezone(&chrono::Utc).format("%Y-%m-%dT%H:%M:%SZ").to_string()),
        importance: Some(importance_of(msg)),
        // A mixed multipart message is how mail with attachments is built.
        has_attachments: msg["payload"]["mimeType"].as_str().map(|m| m.starts_with("multipart/mixed")),
        conversation_id: msg["threadId"].as_str().map(String::from),
    })
}

/// Folders from `labels.list` (`counts` supplies per-label totals from `labels.get`).
pub(crate) fn parse_labels(account_id: &str, labels: &serde_json::Value, counts: &dyn Fn(&str) -> (Option<i32>, Option<i32>)) -> Vec<MailFolder> {
    let mut out = Vec::new();
    for l in labels["labels"].as_array().cloned().unwrap_or_default() {
        let (Some(id), Some(name)) = (l["id"].as_str(), l["name"].as_str()) else { continue };
        let folder = if let Some((_, display, wk)) = SYSTEM_FOLDERS.iter().find(|(sid, _, _)| *sid == id) {
            Some((display.to_string(), Some(wk.to_string())))
        } else if l["type"].as_str() == Some("user") {
            Some((name.to_string(), None))
        } else {
            None // CATEGORY_*, STARRED, IMPORTANT, UNREAD, CHAT ... are views, not folders
        };
        if let Some((display_name, well_known_name)) = folder {
            let (unread, total) = counts(id);
            out.push(MailFolder {
                id: scoped(account_id, id),
                display_name,
                parent_folder_id: None,
                unread_item_count: unread,
                total_item_count: total,
                well_known_name,
            });
        }
    }
    out
}

/// Best body of a `format=full` message: HTML if present, else plain text.
pub(crate) fn extract_body(payload: &serde_json::Value) -> (String, String) {
    fn walk(p: &serde_json::Value, html: &mut Option<String>, text: &mut Option<String>) {
        let mime = p["mimeType"].as_str().unwrap_or("");
        if let Some(data) = p["body"]["data"].as_str() {
            let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(data.trim_end_matches('='))
                .ok()
                .map(|b| String::from_utf8_lossy(&b).into_owned());
            if let Some(d) = decoded {
                if mime == "text/html" && html.is_none() { *html = Some(d); }
                else if mime == "text/plain" && text.is_none() { *text = Some(d); }
            }
        }
        for part in p["parts"].as_array().into_iter().flatten() {
            walk(part, html, text);
        }
    }
    let (mut html, mut text) = (None, None);
    walk(payload, &mut html, &mut text);
    match (html, text) {
        (Some(h), _) => ("html".into(), h),
        (None, Some(t)) => ("text".into(), t),
        _ => ("text".into(), String::new()),
    }
}

/// An attachment of a received message, as listed by `format=full`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PartInfo {
    /// MIME part id (`"2"`, `"1.2"`): stable for the message, used as the attachment's id.
    pub part_id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
    /// Present for parts too big to be inlined in the message resource.
    pub attachment_id: Option<String>,
    /// Present for small parts: the content itself, base64url.
    pub data: Option<String>,
    /// A picture that belongs to the HTML body (a signature logo), not a file the sender attached.
    pub inline: bool,
    /// The part's Content-ID without brackets (an inline picture is referred to by it).
    pub content_id: Option<String>,
}

fn header_of<'a>(p: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    p["headers"].as_array().into_iter().flatten()
        .find(|h| h["name"].as_str().map(|n| n.eq_ignore_ascii_case(name)).unwrap_or(false))
        .and_then(|h| h["value"].as_str())
}

/// `Content-Disposition: inline`, or no disposition but a Content-ID on a picture: the way mail
/// clients embed images in the text. (Gmail itself puts a Content-ID on every attachment, so an
/// ID alone proves nothing.)
fn part_is_inline(p: &serde_json::Value) -> bool {
    match header_of(p, "Content-Disposition") {
        Some(d) => d.trim_start().to_ascii_lowercase().starts_with("inline"),
        None => header_of(p, "Content-ID").is_some() && p["mimeType"].as_str().unwrap_or("").starts_with("image/"),
    }
}

/// The Content-ID of a part, brackets removed.
fn part_content_id(p: &serde_json::Value) -> Option<String> {
    let v = header_of(p, "Content-ID")?.trim().trim_start_matches('<').trim_end_matches('>').trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// Every part with a file name (so signature logos without one are not attachments), in order.
/// An inline picture without a file name is included too (named after its type), so a reply can
/// take the picture its quoted HTML refers to by Content-ID.
pub(crate) fn attachment_parts(payload: &serde_json::Value) -> Vec<PartInfo> {
    fn walk(p: &serde_json::Value, out: &mut Vec<PartInfo>) {
        let content_id = part_content_id(p);
        let inline = part_is_inline(p);
        let mut filename = p["filename"].as_str().unwrap_or("").to_string();
        if filename.is_empty() && inline && content_id.is_some() {
            let sub = p["mimeType"].as_str().unwrap_or("").strip_prefix("image/").unwrap_or("bin");
            filename = format!("picture.{}", if sub == "jpeg" { "jpg" } else { sub });
        }
        if !filename.is_empty() {
            out.push(PartInfo {
                part_id: p["partId"].as_str().unwrap_or("").to_string(),
                filename,
                mime_type: p["mimeType"].as_str().unwrap_or("application/octet-stream").to_string(),
                size: p["body"]["size"].as_u64().unwrap_or(0),
                attachment_id: p["body"]["attachmentId"].as_str().map(String::from),
                data: p["body"]["data"].as_str().map(String::from),
                inline,
                content_id,
            });
        }
        for c in p["parts"].as_array().into_iter().flatten() {
            walk(c, out);
        }
    }
    let mut out = Vec::new();
    walk(payload, &mut out);
    out
}

/// A scratch directory under the cache that is deleted when dropped, for the originals of a
/// forward and the finished message file.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> std::io::Result<Self> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let dir = dirs::cache_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join("omalook").join("outgoing-tmp").join(format!("{:x}-{}", nanos, std::process::id()));
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ─────────────────────────────────────────────────────────────── provider

impl GmailProvider {
    pub fn new(account_id: &str, db_path: Option<PathBuf>) -> Self {
        Self { account_id: account_id.to_string(), api: Arc::new(GoogleApi::new(account_id)), db_path, labels: Arc::new(Mutex::new(HashMap::new())) }
    }

    /// Remember user labels (raw id -> name) from a `labels.list` response.
    fn remember_labels(&self, labels: &serde_json::Value) {
        let map: HashMap<String, String> = labels["labels"].as_array().into_iter().flatten()
            .filter(|l| l["type"].as_str() == Some("user"))
            .filter_map(|l| Some((l["id"].as_str()?.to_string(), l["name"].as_str()?.to_string())))
            .collect();
        *self.labels.lock().unwrap_or_else(|p| p.into_inner()) = map;
    }

    /// Load the label names once if nothing has fetched folders yet.
    async fn ensure_labels(&self) {
        if !self.labels.lock().unwrap_or_else(|p| p.into_inner()).is_empty() { return; }
        match self.api.get(&format!("{}/labels", BASE)).await {
            Ok(json) => self.remember_labels(&json),
            Err(e) => warn!("Gmail: could not read labels: {}", e),
        }
    }

    fn label_id_for(&self, name: &str) -> Option<String> {
        self.labels.lock().unwrap_or_else(|p| p.into_inner()).iter()
            .find(|(_, n)| n.eq_ignore_ascii_case(name)).map(|(id, _)| id.clone())
    }

    fn label_of(&self, folder_id: &str) -> String {
        let raw = unscoped(&self.account_id, folder_id);
        if raw.eq_ignore_ascii_case("inbox") { "INBOX".to_string() } else { raw.to_string() }
    }

    /// Subset of `ids` (scoped) that is already cached.
    fn known(&self, ids: &[String]) -> HashSet<String> {
        match &self.db_path {
            Some(p) => crate::db::known_message_ids(p, ids),
            None => HashSet::new(),
        }
    }

    /// One page of message ids for a label: (raw ids, next page token).
    async fn list_page(&self, label: &str, unread_only: bool, page_token: Option<&str>, max: usize) -> Result<(Vec<String>, Option<String>)> {
        let mut url = format!("{}/messages?labelIds={}&maxResults={}&includeSpamTrash=true", BASE, urlencoding::encode(label), max);
        if unread_only {
            url.push_str("&labelIds=UNREAD");
        }
        if let Some(t) = page_token {
            url.push_str(&format!("&pageToken={}", urlencoding::encode(t)));
        }
        let json = self.api.get(&url).await?;
        let ids = json["messages"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str().map(String::from)).collect();
        Ok((ids, json["nextPageToken"].as_str().map(String::from)))
    }

    /// Metadata for each raw id, a few requests at a time. A message that fails is skipped
    /// (it is retried next sync, because it never became "known").
    async fn fetch_metadata(&self, folder_id: &str, raw_ids: Vec<String>, keep_gone: bool) -> Vec<EmailMessage> {
        let mut out = Vec::with_capacity(raw_ids.len());
        for chunk in raw_ids.chunks(FETCH_CONCURRENCY) {
            let mut set = tokio::task::JoinSet::new();
            for id in chunk {
                let api = Arc::clone(&self.api);
                let raw_id = id.clone();
                let url = format!(
                    "{}/messages/{}?format=metadata&metadataHeaders=From&metadataHeaders=Subject&metadataHeaders=To&metadataHeaders=Cc&metadataHeaders=Bcc&metadataHeaders=Date&metadataHeaders=Importance&metadataHeaders=X-Priority",
                    BASE, urlencoding::encode(id)
                );
                set.spawn(async move { (raw_id, api.get(&url).await) });
            }
            let labels = self.labels.lock().unwrap_or_else(|p| p.into_inner()).clone();
            while let Some(res) = set.join_next().await {
                match res {
                    Ok((_, Ok(json))) => out.extend(parse_message(&self.account_id, folder_id, &json, &labels)),
                    // Gone from the mailbox: report it without details so the repopulate stops asking.
                    Ok((raw, Err(e))) if keep_gone && e.to_string().contains("404") => {
                        debug!("Gmail: message {} no longer exists on the server", raw);
                        out.push(EmailMessage { id: scoped(&self.account_id, &raw), meta: false, ..Default::default() });
                    }
                    Ok((_, Err(e))) => warn!("Gmail: skipping a message that could not be fetched: {}", e),
                    Err(e) => warn!("Gmail: fetch task failed: {}", e),
                }
            }
        }
        out
    }

    /// New (not yet cached) messages among `raw_ids`.
    async fn new_messages(&self, folder_id: &str, raw_ids: Vec<String>) -> Vec<EmailMessage> {
        let scoped_ids: Vec<String> = raw_ids.iter().map(|r| scoped(&self.account_id, r)).collect();
        let known = self.known(&scoped_ids);
        let fresh: Vec<String> = raw_ids.into_iter().filter(|r| !known.contains(&scoped(&self.account_id, r))).collect();
        if fresh.is_empty() { return Vec::new(); }
        self.fetch_metadata(folder_id, fresh, false).await
    }

    /// The files attached to a received message (no inline pictures), for the forward chips:
    /// `(MIME part id, name, size)`.
    pub async fn list_attachments(&self, id: &str) -> Result<Vec<(String, String, u64)>> {
        let raw = unscoped(&self.account_id, id);
        let msg = self.api.get(&format!("{}/messages/{}?format=full", BASE, urlencoding::encode(raw))).await?;
        Ok(attachment_parts(&msg["payload"]).into_iter().filter(|p| !p.inline).map(|p| (p.part_id, p.filename, p.size)).collect())
    }

    /// Threading for a reply: the original's Message-ID / References and its threadId. A forward
    /// starts a new conversation. An original that is gone gives no threading (nothing is lost).
    async fn reply_context(&self, msg: &crate::compose::OutgoingMessage) -> (Option<crate::compose::ReplyHeaders>, Option<String>) {
        let (mut reply, mut thread_id) = (None, None);
        if msg.is_reply() {
            let raw = unscoped(&self.account_id, &msg.in_reply_to);
            let url = format!("{}/messages/{}?format=metadata&metadataHeaders=Message-ID&metadataHeaders=References", BASE, urlencoding::encode(raw));
            match self.api.get(&url).await {
                Ok(m) => {
                    reply = Some(crate::compose::ReplyHeaders {
                        message_id: header(&m, "Message-ID").unwrap_or_default().trim().to_string(),
                        references: header(&m, "References").unwrap_or_default().trim().to_string(),
                    });
                    thread_id = m["threadId"].as_str().map(str::to_string);
                }
                Err(e) => warn!("Gmail: cannot thread reply to {}: {}", msg.in_reply_to, e),
            }
        }
        (reply, thread_id)
    }

    /// Download the kept attachments of a message being forwarded (`keep` = MIME part ids) into
    /// `dir` and return them as attachments to send. Parts that no longer exist are an error:
    /// silently forwarding less than the user ticked would be worse than refusing.
    async fn fetch_originals(&self, raw_id: &str, keep: &[String], dir: &std::path::Path) -> Result<Vec<crate::compose::OutgoingAttachment>> {
        use crate::errors::OmarchyError;
        if keep.is_empty() {
            return Ok(Vec::new());
        }
        let msg = self.api.get(&format!("{}/messages/{}?format=full", BASE, urlencoding::encode(raw_id))).await?;
        let parts = attachment_parts(&msg["payload"]);
        let mut out = Vec::new();
        for id in keep {
            let part = parts.iter().find(|p| &p.part_id == id)
                .ok_or_else(|| OmarchyError::HttpError(format!("the original's attachment {} is no longer there", id)))?;
            out.push(self.download_part(raw_id, part, dir).await?);
        }
        Ok(out)
    }

    /// The pictures of the original that the quoted HTML refers to by `cid:` (a reply or forward
    /// keeps them). A picture the original no longer has is logged and left out; the body then
    /// drops that reference rather than sending it broken.
    async fn fetch_inline_originals(&self, raw_id: &str, cids: &[String], dir: &std::path::Path) -> Result<Vec<crate::compose::OutgoingAttachment>> {
        let msg = self.api.get(&format!("{}/messages/{}?format=full", BASE, urlencoding::encode(raw_id))).await?;
        let parts = attachment_parts(&msg["payload"]);
        let mut out = Vec::new();
        for cid in cids {
            let Some(part) = parts.iter().find(|p| p.content_id.as_deref() == Some(cid.as_str())) else {
                warn!("Gmail: the original has no picture {}; the reference is dropped", cid);
                continue;
            };
            let mut a = self.download_part(raw_id, part, dir).await?;
            a.inline_cid = cid.clone();
            out.push(a);
        }
        Ok(out)
    }

    /// One part of a received message, written under `dir` (the bytes, fetched when Google did
    /// not inline them).
    async fn download_part(&self, raw_id: &str, part: &PartInfo, dir: &std::path::Path) -> Result<crate::compose::OutgoingAttachment> {
        use crate::errors::OmarchyError;
        let b64 = match (&part.data, &part.attachment_id) {
            (Some(d), _) => d.clone(),
            (None, Some(aid)) => {
                let a = self.api.get(&format!("{}/messages/{}/attachments/{}", BASE, urlencoding::encode(raw_id), urlencoding::encode(aid))).await?;
                a["data"].as_str().unwrap_or("").to_string()
            }
            (None, None) => String::new(),
        };
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(b64.trim_end_matches('=')).map_err(|e| OmarchyError::HttpError(format!("bad attachment data from Google: {}", e)))?;
        let slot = dir.join(format!("orig-{}", part.part_id.replace('.', "_")));
        std::fs::create_dir_all(&slot)?;
        let path = slot.join("file");
        std::fs::write(&path, &bytes)?;
        Ok(crate::compose::OutgoingAttachment {
            path: path.to_string_lossy().into_owned(),
            name: part.filename.clone(),
            size: bytes.len() as u64,
            content_type: part.mime_type.clone(),
            source_path: String::new(),
            inline_cid: String::new(),
            mtime: 0,
        })
    }

    /// Full body of one message: (content type "html"|"text", content).
    pub async fn fetch_message_body(&self, id: &str) -> Result<(String, String)> {
        let raw = unscoped(&self.account_id, id);
        let json = self.api.get(&format!("{}/messages/{}?format=full", BASE, urlencoding::encode(raw))).await?;
        Ok(extract_body(&json["payload"]))
    }
}

#[async_trait]
impl super::EmailProvider for GmailProvider {
    async fn fetch_body(&self, id: &str) -> Result<(String, String)> {
        self.fetch_message_body(id).await
    }

    async fn fetch_inbox(&self, limit: usize) -> Result<Vec<EmailMessage>> {
        self.fetch_folder_messages("INBOX", limit).await
    }

    async fn fetch_folder_messages(&self, folder_id: &str, limit: usize) -> Result<Vec<EmailMessage>> {
        let label = self.label_of(folder_id);
        let scoped_folder = scoped(&self.account_id, &label);
        let (ids, _) = self.list_page(&label, false, None, limit.clamp(1, 500)).await?;
        Ok(self.new_messages(&scoped_folder, ids).await)
    }

    async fn fetch_all_folder_messages(&self, folder_id: &str, tx: tokio::sync::mpsc::Sender<Vec<EmailMessage>>) -> Result<usize> {
        let label = self.label_of(folder_id);
        let scoped_folder = scoped(&self.account_id, &label);
        let (mut total, mut token) = (0usize, None::<String>);
        loop {
            let (ids, next) = self.list_page(&label, false, token.as_deref(), PAGE_SIZE).await?;
            let batch = self.new_messages(&scoped_folder, ids).await;
            total += batch.len();
            if !batch.is_empty() && tx.send(batch).await.is_err() {
                debug!("Gmail pagination receiver dropped, stopping");
                break;
            }
            match next {
                Some(t) => token = Some(t),
                None => break,
            }
            tokio::task::yield_now().await;
        }
        debug!("Gmail label {}: {} new messages", label, total);
        Ok(total)
    }

    async fn fetch_all_ids(&self, folder_id: &str) -> Result<Vec<String>> {
        let label = self.label_of(folder_id);
        let (mut all, mut token) = (Vec::new(), None::<String>);
        loop {
            let (ids, next) = self.list_page(&label, false, token.as_deref(), 500).await?;
            all.extend(ids.into_iter().map(|r| scoped(&self.account_id, &r)));
            match next {
                Some(t) => token = Some(t),
                None => return Ok(all),
            }
        }
    }

    async fn fetch_unread_ids(&self, folder_id: &str) -> Result<Vec<String>> {
        let label = self.label_of(folder_id);
        let (mut all, mut token) = (Vec::new(), None::<String>);
        loop {
            // Any failed page aborts: a partial list must not be treated as authoritative.
            let (ids, next) = self.list_page(&label, true, token.as_deref(), 500).await?;
            all.extend(ids.into_iter().map(|r| scoped(&self.account_id, &r)));
            match next {
                Some(t) => token = Some(t),
                None => return Ok(all),
            }
        }
    }

    async fn search_message_ids(&self, folder_id: Option<&str>, text: &str, limit: usize) -> Result<(Vec<String>, bool)> {
        let q = crate::search::gmail_q(text);
        if q.is_empty() {
            return Ok((Vec::new(), false));
        }
        let mut base = format!("{}/messages?q={}&maxResults=500", BASE, urlencoding::encode(&q));
        if let Some(f) = folder_id {
            base.push_str(&format!("&labelIds={}", urlencoding::encode(&self.label_of(f))));
        }
        let (mut ids, mut token, mut more) = (Vec::<String>::new(), None::<String>, false);
        loop {
            let url = match &token {
                Some(t) => format!("{}&pageToken={}", base, urlencoding::encode(t)),
                None => base.clone(),
            };
            let json = self.api.get(&url).await?;
            ids.extend(json["messages"].as_array().into_iter().flatten()
                .filter_map(|m| m["id"].as_str().map(|r| scoped(&self.account_id, r))));
            token = json["nextPageToken"].as_str().map(String::from);
            if ids.len() >= limit {
                more = token.is_some() || ids.len() > limit;
                ids.truncate(limit);
                break;
            }
            if token.is_none() { break; }
        }
        Ok((ids, more))
    }

    async fn set_message_read(&self, id: &str, is_read: bool) -> Result<()> {
        let raw = unscoped(&self.account_id, id);
        let body = if is_read {
            serde_json::json!({"removeLabelIds": ["UNREAD"]})
        } else {
            serde_json::json!({"addLabelIds": ["UNREAD"]})
        };
        self.api.post(&format!("{}/messages/{}/modify", BASE, urlencoding::encode(raw)), &body).await.map(|_| ())
    }

    async fn apply_message_action(
        &self,
        id: &str,
        from_folder: &str,
        action: &crate::models::MessageAction,
    ) -> Result<()> {
        use crate::models::MessageAction;
        let raw = urlencoding::encode(unscoped(&self.account_id, id)).into_owned();
        match action {
            // Gmail has no archive folder: archiving is leaving the inbox.
            MessageAction::Archive => self
                .api
                .post(&format!("{}/messages/{}/modify", BASE, raw), &serde_json::json!({"removeLabelIds": ["INBOX"]}))
                .await
                .map(|_| ()),
            MessageAction::Trash => self.api.post(&format!("{}/messages/{}/trash", BASE, raw), &serde_json::json!({})).await.map(|_| ()),
            // A folder is a label: moving swaps the old label for the new one.
            MessageAction::Move(dest) => {
                let mut body = serde_json::json!({"addLabelIds": [unscoped(&self.account_id, dest)]});
                if !from_folder.is_empty() {
                    body["removeLabelIds"] = serde_json::json!([unscoped(&self.account_id, from_folder)]);
                }
                self.api.post(&format!("{}/messages/{}/modify", BASE, raw), &body).await.map(|_| ())
            }
            // gmail.modify cannot delete for good (that needs the full-mail scope); Gmail empties
            // Trash itself after 30 days.
            MessageAction::Delete => Err(crate::errors::OmarchyError::HttpError(
                "Google API error: 403 — permanent delete needs the full-mail scope; Gmail empties Trash after 30 days".into(),
            )),
        }
    }

    async fn save_draft(&self, msg: &crate::compose::OutgoingMessage, remote_id: Option<&str>, from: &str, _attachments_changed: bool) -> Result<crate::providers::RemoteDraft> {
        use crate::compose::{check_attachments, write_mime};
        // A draft is one MIME message: the whole of it is rewritten on each save (drafts.update),
        // so the attachment flag is not needed. Encoded to a file first, as a send is.
        let (reply, thread_id) = self.reply_context(msg).await;
        let boundary = format!("=_omalook_{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        let date = chrono::Utc::now().to_rfc2822();
        let scratch = Scratch::new()?;
        let mut files = msg.attachments.clone();
        if let (true, Some(keep)) = (msg.is_forward(), &msg.forwarded_keep) {
            let orig = unscoped(&self.account_id, &msg.in_reply_to);
            files.extend(self.fetch_originals(orig, keep, &scratch.0).await?);
        }
        // A reply's or forward's quoted pictures (cid:) come from the original, as a send does.
        if (msg.is_reply() || msg.is_forward()) && msg.body.format == "html" {
            let have = msg.inline_cids();
            let wanted: Vec<String> = crate::compose::referenced_cids(&msg.body.content).into_iter().filter(|c| !have.contains(c)).collect();
            if !wanted.is_empty() {
                let orig = unscoped(&self.account_id, &msg.in_reply_to);
                files.extend(self.fetch_inline_originals(orig, &wanted, &scratch.0).await?);
            }
        }
        let sizes: Vec<(String, u64)> = files.iter().map(|a| (a.display_name(), std::fs::metadata(&a.path).map(|m| m.len()).unwrap_or(0))).collect();
        if let Some(e) = check_attachments("gmail", msg.body.content.len(), &sizes).error() {
            return Err(crate::errors::OmarchyError::HttpError(e));
        }
        let eml = scratch.0.join("draft.eml");
        {
            let mut w = std::io::BufWriter::new(std::fs::File::create(&eml)?);
            write_mime(&mut w, msg, &files, from, reply.as_ref(), &boundary, &date)?;
            std::io::Write::flush(&mut w)?;
        }
        // JSON `raw` (drafts.create / drafts.update take no resumable upload here): fine up to the
        // sizes the draft worker pushes; larger drafts are only pushed on close or send.
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(std::fs::read(&eml)?);
        let mut message = serde_json::json!({ "raw": raw });
        if let Some(t) = &thread_id {
            message["threadId"] = serde_json::json!(t);
        }
        let resp = match remote_id {
            Some(id) => {
                let body = serde_json::json!({ "id": id, "message": message });
                self.api.put(&format!("{}/drafts/{}", BASE, urlencoding::encode(id)), &body).await?
            }
            None => self.api.post(&format!("{}/drafts", BASE), &serde_json::json!({ "message": message })).await?,
        };
        let id = resp["id"].as_str().ok_or_else(|| crate::errors::OmarchyError::HttpError("Gmail did not return a draft id".into()))?;
        Ok(crate::providers::RemoteDraft { id: id.to_string(), version: resp["message"]["id"].as_str().map(str::to_string) })
    }

    async fn draft_version(&self, remote_id: &str) -> Result<Option<String>> {
        let url = format!("{}/drafts/{}?format=minimal", BASE, urlencoding::encode(remote_id));
        match self.api.get(&url).await {
            Ok(d) => Ok(d["message"]["id"].as_str().map(str::to_string)),
            Err(crate::errors::OmarchyError::HttpError(m)) if m.contains("Google API error: 404") => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn delete_draft(&self, remote_id: &str) -> Result<()> {
        match self.api.delete(&format!("{}/drafts/{}", BASE, urlencoding::encode(remote_id))).await {
            Ok(_) => Ok(()),
            Err(crate::errors::OmarchyError::HttpError(m)) if m.contains("Google API error: 404") => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn send_draft(&self, msg: &crate::compose::OutgoingMessage, remote_id: &str, from: &str) -> Result<()> {
        // Update the draft to what was sent, then drafts.send (which moves it to Sent: no copy is left).
        self.save_draft(msg, Some(remote_id), from, true).await?;
        self.api.post(&format!("{}/drafts/send", BASE), &serde_json::json!({ "id": remote_id })).await.map(|_| ())
    }

    async fn send_message(&self, msg: &crate::compose::OutgoingMessage, from: &str) -> Result<()> {
        use crate::compose::{build_mime, ReplyHeaders};
        // A reply joins the original's thread: it needs the original's Message-ID / References
        // (for the headers) and its threadId. A forward starts a new conversation, as Gmail's does.
        let (mut reply, mut thread_id) = (None::<ReplyHeaders>, None::<String>);
        if msg.is_reply() {
            let raw = unscoped(&self.account_id, &msg.in_reply_to);
            let url = format!(
                "{}/messages/{}?format=metadata&metadataHeaders=Message-ID&metadataHeaders=References",
                BASE, urlencoding::encode(raw)
            );
            match self.api.get(&url).await {
                Ok(m) => {
                    reply = Some(ReplyHeaders {
                        message_id: header(&m, "Message-ID").unwrap_or_default().trim().to_string(),
                        references: header(&m, "References").unwrap_or_default().trim().to_string(),
                    });
                    thread_id = m["threadId"].as_str().map(str::to_string);
                }
                // The original is gone: send as a new message rather than lose what was written.
                Err(e) => warn!("Gmail: cannot thread reply to {}: {}", msg.in_reply_to, e),
            }
        }
        let boundary = format!("=_omalook_{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        let date = chrono::Utc::now().to_rfc2822();

        // Pictures the body refers to that are not attached: a reply or forward quotes the
        // original's, so they come from the original.
        let wanted: Vec<String> = if (msg.is_reply() || msg.is_forward()) && msg.body.format == "html" {
            let have = msg.inline_cids();
            crate::compose::referenced_cids(&msg.body.content).into_iter().filter(|c| !have.contains(c)).collect()
        } else {
            Vec::new()
        };
        // The user's files, plus (forward only) the originals they chose to keep.
        let keeps_originals = msg.is_forward() && msg.forwarded_keep.as_ref().map(|k| !k.is_empty()).unwrap_or(false);
        let scratch = if msg.attachments.is_empty() && !keeps_originals && wanted.is_empty() {
            None
        } else {
            Some(Scratch::new()?)
        };
        let mut files = msg.attachments.clone();
        if let (Some(dir), true, Some(keep)) = (&scratch, msg.is_forward(), &msg.forwarded_keep) {
            let orig = unscoped(&self.account_id, &msg.in_reply_to);
            files.extend(self.fetch_originals(orig, keep, &dir.0).await?);
        }
        if let (Some(dir), false) = (&scratch, wanted.is_empty()) {
            let orig = unscoped(&self.account_id, &msg.in_reply_to);
            files.extend(self.fetch_inline_originals(orig, &wanted, &dir.0).await?);
        }

        let Some(scratch) = scratch else {
            // No files: the small JSON `raw` path, as before.
            let mime = build_mime(msg, from, reply.as_ref(), &boundary, &date);
            let mut body = serde_json::json!({ "raw": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mime.as_bytes()) });
            if let Some(t) = thread_id {
                body["threadId"] = serde_json::json!(t);
            }
            return self.api.post(&format!("{}/messages/send", BASE), &body).await.map(|_| ());
        };

        // With files: the limit is on the *encoded* message, and the originals only now have a
        // known size, so check again here, with Gmail's own numbers.
        let sizes: Vec<(String, u64)> = files.iter().map(|a| (a.display_name(), std::fs::metadata(&a.path).map(|m| m.len()).unwrap_or(0))).collect();
        if let Some(e) = crate::compose::check_attachments("gmail", msg.body.content.len(), &sizes).error() {
            return Err(crate::errors::OmarchyError::HttpError(e));
        }
        // Encode to a file (never the whole message in memory), then upload from the file.
        // Not through the JSON `raw` field: it is twice as slow and accepts what then bounces.
        let eml = scratch.0.join("message.eml");
        {
            let mut w = std::io::BufWriter::new(std::fs::File::create(&eml)?);
            crate::compose::write_mime(&mut w, msg, &files, from, reply.as_ref(), &boundary, &date)?;
            std::io::Write::flush(&mut w)?;
        }
        self.api.upload_message(&eml, thread_id.as_deref()).await.map(|_| ())
    }

    async fn fetch_folders(&self) -> Result<Vec<MailFolder>> {
        let labels = self.api.get(&format!("{}/labels", BASE)).await?;
        self.remember_labels(&labels);
        // Counts need one `labels.get` per shown label; fetch them up front.
        let mut counts = std::collections::HashMap::new();
        for l in labels["labels"].as_array().into_iter().flatten() {
            let Some(id) = l["id"].as_str() else { continue };
            let shown = SYSTEM_FOLDERS.iter().any(|(s, _, _)| *s == id) || l["type"].as_str() == Some("user");
            if !shown { continue; }
            match self.api.get(&format!("{}/labels/{}", BASE, urlencoding::encode(id))).await {
                Ok(d) => {
                    counts.insert(id.to_string(), (d["messagesUnread"].as_i64().map(|n| n as i32), d["messagesTotal"].as_i64().map(|n| n as i32)));
                }
                Err(e) => warn!("Gmail: no counts for label {}: {}", id, e),
            }
        }
        Ok(parse_labels(&self.account_id, &labels, &|id| counts.get(id).copied().unwrap_or((None, None))))
    }

    async fn fetch_categories(&self) -> Result<Vec<crate::models::CategoryDef>> {
        let labels = self.api.get(&format!("{}/labels", BASE)).await?;
        self.remember_labels(&labels);
        Ok(labels["labels"].as_array().into_iter().flatten()
            .filter(|l| l["type"].as_str() == Some("user"))
            .filter_map(|l| {
                let name = l["name"].as_str()?.to_string();
                let bg = l["color"]["backgroundColor"].as_str().map(String::from);
                Some(crate::models::CategoryDef {
                    color: bg.clone().unwrap_or_else(|| crate::models::fallback_category_color(&name).to_string()),
                    name,
                    provider_color: bg,
                })
            })
            .collect())
    }

    async fn create_category(&self, name: &str, color: &crate::models::PaletteColor) -> Result<crate::models::CategoryDef> {
        let body = serde_json::json!({
            "name": name,
            "labelListVisibility": "labelShow",
            "messageListVisibility": "show",
            "color": { "backgroundColor": color.gmail_bg, "textColor": color.gmail_fg },
        });
        let created = self.api.post(&format!("{}/labels", BASE), &body).await?;
        if let (Some(id), Some(n)) = (created["id"].as_str(), created["name"].as_str()) {
            self.labels.lock().unwrap_or_else(|p| p.into_inner()).insert(id.to_string(), n.to_string());
        }
        Ok(crate::models::CategoryDef { name: name.to_string(), color: color.gmail_bg.to_string(), provider_color: Some(color.gmail_bg.to_string()) })
    }

    async fn update_category_color(&self, name: &str, color: &crate::models::PaletteColor) -> Result<crate::models::CategoryDef> {
        self.ensure_labels().await;
        let id = self.label_id_for(name)
            .ok_or_else(|| crate::errors::OmarchyError::HttpError(format!("label '{}' not found", name)))?;
        let url = format!("{}/labels/{}", BASE, urlencoding::encode(&id));
        self.api.patch(&url, &serde_json::json!({ "color": { "backgroundColor": color.gmail_bg, "textColor": color.gmail_fg } })).await?;
        let now = self.api.get(&url).await?;
        match now["color"]["backgroundColor"].as_str() {
            Some(bg) if bg.eq_ignore_ascii_case(color.gmail_bg) => Ok(crate::models::CategoryDef {
                name: name.to_string(), color: color.gmail_bg.to_string(), provider_color: Some(bg.to_string()) }),
            other => Err(crate::errors::OmarchyError::HttpError(format!("label '{}' kept colour {:?} after the change", name, other))),
        }
    }

    async fn fetch_mail_filters(&self) -> Result<Vec<serde_json::Value>> {
        // Empty list comes back as 204 with no body (phase 0), which the client reads as Null.
        let r = self.api.get(&format!("{}/settings/filters", BASE)).await?;
        Ok(r["filter"].as_array().cloned().unwrap_or_default())
    }

    async fn create_mail_filter(&self, body: &serde_json::Value) -> Result<serde_json::Value> {
        self.api.post(&format!("{}/settings/filters", BASE), body).await
    }

    async fn delete_mail_filter(&self, id: &str) -> Result<()> {
        match self.api.delete(&format!("{}/settings/filters/{}", BASE, urlencoding::encode(id))).await {
            Ok(_) => Ok(()),
            Err(e) if e.to_string().contains(" 404") => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn label_names(&self) -> Result<HashMap<String, String>> {
        let labels = self.api.get(&format!("{}/labels", BASE)).await?;
        self.remember_labels(&labels);
        Ok(self.labels.lock().unwrap_or_else(|p| p.into_inner()).clone())
    }

    async fn label_id_for_tag(&self, name: &str) -> Result<Option<String>> {
        let labels = self.api.get(&format!("{}/labels", BASE)).await?;
        self.remember_labels(&labels);
        Ok(self.label_id_for(name))
    }

    async fn delete_category(&self, name: &str) -> Result<()> {
        self.ensure_labels().await;
        let Some(id) = self.label_id_for(name) else { return Ok(()) };
        match self.api.delete(&format!("{}/labels/{}", BASE, urlencoding::encode(&id))).await {
            Ok(_) => {}
            Err(e) if e.to_string().contains(" 404") => {}
            Err(e) => return Err(e),
        }
        self.labels.lock().unwrap_or_else(|p| p.into_inner()).remove(&id);
        Ok(())
    }

    async fn set_message_categories(&self, id: &str, wanted: &[String], had: &[String]) -> Result<()> {
        self.ensure_labels().await;
        let lookup = |names: &[String], skip: &[String]| -> Vec<String> {
            names.iter()
                .filter(|n| !skip.iter().any(|s| s.eq_ignore_ascii_case(n)))
                .filter_map(|n| self.label_id_for(n))
                .collect()
        };
        let add = lookup(wanted, had);
        let remove = lookup(had, wanted);
        if add.is_empty() && remove.is_empty() { return Ok(()); }
        let raw = unscoped(&self.account_id, id);
        self.api
            .post(&format!("{}/messages/{}/modify", BASE, urlencoding::encode(raw)),
                  &serde_json::json!({"addLabelIds": add, "removeLabelIds": remove}))
            .await
            .map(|_| ())
    }

    fn repopulates_meta_per_message(&self) -> bool {
        true
    }

    async fn fetch_message_meta(&self, ids: &[String]) -> Vec<EmailMessage> {
        self.ensure_labels().await;
        let raw: Vec<String> = ids.iter().map(|i| unscoped(&self.account_id, i).to_string()).collect();
        self.fetch_metadata("", raw, true).await
    }

    async fn is_token_valid(&self) -> Result<bool> {
        self.api.is_token_valid().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn from_header_forms() {
        assert_eq!(address_of("Ada Lovelace <ada@example.com>"), "ada@example.com");
        assert_eq!(address_of("\"Doe, J\" <j@x.org>"), "j@x.org");
        assert_eq!(address_of("plain@x.org"), "plain@x.org");
        assert_eq!(address_of("<only@x.org>"), "only@x.org");
    }

    #[test]
    fn metadata_becomes_a_scoped_message() {
        let msg = json!({
            "id": "18f3a", "labelIds": ["INBOX", "UNREAD"], "snippet": "hello there", "internalDate": "1790000000000",
            "payload": {"headers": [{"name": "From", "value": "Ada <ada@example.com>"}, {"name": "subject", "value": " Lunch? "}]}
        });
        let m = parse_message("gmail-aaaaaa", "gmail-aaaaaa:INBOX", &msg, &HashMap::new()).unwrap();
        assert_eq!(m.id, "gmail-aaaaaa:18f3a");
        assert_eq!((m.from.as_str(), m.subject.as_str(), m.body.as_str()), ("ada@example.com", "Lunch?", "hello there"));
        assert!(!m.is_read);
        assert_eq!(m.folder_id.as_deref(), Some("gmail-aaaaaa:INBOX"));
        assert!(m.received.ends_with('Z') && m.received.starts_with("2026-"), "{}", m.received);
        // read when UNREAD is absent; missing headers get safe defaults; no id → no message
        let read = parse_message("a-1", "f", &json!({"id": "x", "labelIds": ["INBOX"]}), &HashMap::new()).unwrap();
        assert!(read.is_read);
        assert_eq!((read.subject.as_str(), read.from.as_str()), ("(no subject)", "unknown@example.com"));
        assert!(parse_message("a-1", "f", &json!({}), &HashMap::new()).is_none());
    }

    #[test]
    fn same_gmail_id_in_two_accounts_stays_distinct() {
        let msg = json!({"id": "same"});
        let a = parse_message("gmail-aaaaaa", "f", &msg, &HashMap::new()).unwrap();
        let b = parse_message("gmail-bbbbbb", "f", &msg, &HashMap::new()).unwrap();
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn labels_map_to_folders_and_hide_views() {
        let labels = json!({"labels": [
            {"id": "INBOX", "name": "INBOX", "type": "system"},
            {"id": "SENT", "name": "SENT", "type": "system"},
            {"id": "STARRED", "name": "STARRED", "type": "system"},
            {"id": "CATEGORY_PROMOTIONS", "name": "CATEGORY_PROMOTIONS", "type": "system"},
            {"id": "Label_7", "name": "Receipts/2026", "type": "user"},
        ]});
        let f = parse_labels("gmail-aaaaaa", &labels, &|id| if id == "INBOX" { (Some(3), Some(120)) } else { (None, None) });
        let ids: Vec<_> = f.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, ["gmail-aaaaaa:INBOX", "gmail-aaaaaa:SENT", "gmail-aaaaaa:Label_7"]);
        assert_eq!(f[0].well_known_name.as_deref(), Some("inbox"));
        assert_eq!((f[0].unread_item_count, f[0].total_item_count), (Some(3), Some(120)));
        assert_eq!(f[2].display_name, "Receipts/2026");
        assert_eq!(f[2].well_known_name, None);
    }

    #[test]
    fn body_prefers_html_and_walks_nested_parts() {
        let enc = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s);
        let payload = json!({"mimeType": "multipart/mixed", "parts": [
            {"mimeType": "multipart/alternative", "parts": [
                {"mimeType": "text/plain", "body": {"data": enc("plain")}},
                {"mimeType": "text/html", "body": {"data": enc("<b>rich</b>")}},
            ]},
            {"mimeType": "application/pdf", "body": {"attachmentId": "x"}},
        ]});
        assert_eq!(extract_body(&payload), ("html".to_string(), "<b>rich</b>".to_string()));
        let only_text = json!({"mimeType": "text/plain", "body": {"data": enc("just text ✓")}});
        assert_eq!(extract_body(&only_text), ("text".to_string(), "just text ✓".to_string()));
        assert_eq!(extract_body(&json!({})).1, "");
    }

    #[test]
    fn gmail_message_details_are_parsed() {
        let msg = json!({
            "id": "18f3a", "threadId": "t77", "labelIds": ["INBOX", "Label_12", "Label_99", "IMPORTANT"], "snippet": "x",
            "internalDate": "1790000000000",
            "payload": {"mimeType": "multipart/mixed", "headers": [
                {"name": "From", "value": "\"Ada Lovelace\" <ada@example.com>"},
                {"name": "Subject", "value": "Lunch?"},
                {"name": "To", "value": "Bob <bob@x.org>,\r\n cy@x.org"},
                {"name": "Cc", "value": "dee@x.org"},
                {"name": "Date", "value": "Tue, 6 Oct 2026 12:00:00 +0000"},
                {"name": "X-Priority", "value": "1 (Highest)"}
            ]}
        });
        let mut labels = HashMap::new();
        labels.insert("Label_12".to_string(), "Work".to_string());
        let m = parse_message("gmail-aaaaaa", "gmail-aaaaaa:INBOX", &msg, &labels).unwrap();
        assert!(m.meta);
        assert_eq!(m.from_name.as_deref(), Some("Ada Lovelace"));
        assert_eq!(m.to_text.as_deref(), Some("Bob <bob@x.org>, cy@x.org"));
        assert_eq!(m.cc_text.as_deref(), Some("dee@x.org"));
        assert_eq!(m.bcc_text, None);
        assert_eq!(m.categories, Some(vec!["Work".to_string()]), "only known user labels become tags");
        assert_eq!(m.importance.as_deref(), Some("high"));
        assert_eq!(m.has_attachments, Some(true));
        assert_eq!(m.conversation_id.as_deref(), Some("t77"));
        assert_eq!(m.sent_at.as_deref(), Some("2026-10-06T12:00:00Z"));
        assert_eq!(name_of("a@b.com"), None);
        assert_eq!(name_of("<a@b.com>"), None);
    }

    #[test]
    fn attachment_parts_lists_named_parts_only_with_their_ids() {
        let payload = serde_json::json!({
            "mimeType": "multipart/mixed", "filename": "", "partId": "",
            "parts": [
                {"partId": "0", "mimeType": "multipart/alternative", "filename": "", "parts": [
                    {"partId": "0.0", "mimeType": "text/plain", "filename": "", "body": {"size": 5, "data": "aGVsbG8"}},
                    {"partId": "0.1", "mimeType": "image/png", "filename": "", "body": {"size": 9, "attachmentId": "logo"}}
                ]},
                {"partId": "1", "mimeType": "application/pdf", "filename": "report.pdf", "headers": [{"name": "Content-Disposition", "value": "attachment; filename=\"report.pdf\""}, {"name": "Content-ID", "value": "<f_abc>"}], "body": {"size": 2048, "attachmentId": "ATT1"}},
                {"partId": "3", "mimeType": "image/png", "filename": "image001.png", "headers": [{"name": "Content-Disposition", "value": "inline; filename=\"image001.png\""}, {"name": "Content-ID", "value": "<logo@x>"}], "body": {"size": 7, "attachmentId": "LOGO"}},
                {"partId": "4", "mimeType": "image/jpeg", "filename": "pic.jpg", "headers": [{"name": "Content-ID", "value": "<pic@x>"}], "body": {"size": 7, "attachmentId": "PIC"}},
                {"partId": "2", "mimeType": "text/plain", "filename": "note.txt", "body": {"size": 2, "data": "aGk"}}
            ]
        });
        let parts = attachment_parts(&payload);
        assert_eq!(parts.len(), 4, "the unnamed logo is not an attachment");
        assert_eq!((parts[0].part_id.as_str(), parts[0].filename.as_str(), parts[0].attachment_id.as_deref()), ("1", "report.pdf", Some("ATT1")));
        // Gmail gives every attachment a Content-ID: only `inline` (or an ID with no disposition, on a picture) marks an embedded image
        assert!(!parts[0].inline, "an attachment with a Content-ID is still an attachment");
        assert!(parts[1].inline && parts[2].inline, "embedded pictures are inline");
        assert_eq!(parts[1].content_id.as_deref(), Some("logo@x"), "Content-ID without the brackets");
        assert_eq!((parts[3].part_id.as_str(), parts[3].data.as_deref(), parts[3].inline), ("2", Some("aGk"), false));
    }
}
