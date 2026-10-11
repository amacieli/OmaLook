//! Microsoft Graph API email provider implementation

use crate::errors::Result;
use crate::models::{DeltaEnd, DeltaPage, EmailMessage, MailFolder};
use crate::auth::AuthManager;
use async_trait::async_trait;
use log::{debug, error, warn};
use tokio::sync::Mutex;

/// Message fields the list, reading pane and categories need. They are part of the change-feed
/// link, so changing this list requires a fresh walk of every folder
/// (see `email_daemon::FIELDS_VERSION`).
pub(crate) const MSG_SELECT: &str = "id,subject,from,receivedDateTime,bodyPreview,isRead,parentFolderId,\
toRecipients,ccRecipients,bccRecipients,categories,sentDateTime,importance,hasAttachments,conversationId";

pub struct GraphEmailProvider {
    auth: Mutex<AuthManager>,
}

impl GraphEmailProvider {
    pub fn new(auth: AuthManager) -> Self {
        Self {
            auth: Mutex::new(auth),
        }
    }

    /// Internal: get a valid access token
    async fn get_token(&self) -> Result<String> {
        let mut auth = self.auth.lock().await;
        let token = auth.get_token().map_err(|e| {
            crate::errors::OmarchyError::AuthError(format!("Failed to get token: {}", e))
        })?;
        if token.is_empty() {
            return Err(crate::errors::OmarchyError::AuthError("No access token".to_string()));
        }
        Ok(token)
    }

    /// Attach one file to draft `did`: under 3 MB in one request, otherwise through an upload
    /// session in chunks (read from disk one at a time, so a large file is never held whole).
    async fn add_attachment(client: &reqwest::Client, token: &str, did: &str, a: &crate::compose::OutgoingAttachment) -> Result<()> {
        use crate::compose::{content_range, graph_file_attachment_json, graph_upload_session_json, upload_ranges, SMALL_ATTACHMENT_LIMIT, UPLOAD_CHUNK};
        use std::io::{Read, Seek, SeekFrom};
        let http = |e: reqwest::Error| crate::errors::OmarchyError::HttpError(e.to_string());
        let base = format!("https://graph.microsoft.com/v1.0/me/messages/{}/attachments", did);
        let size = std::fs::metadata(&a.path)?.len();

        if size < SMALL_ATTACHMENT_LIMIT {
            let bytes = std::fs::read(&a.path)?;
            let resp = client.post(&base).header("Authorization", format!("Bearer {}", token))
                .json(&graph_file_attachment_json(a, &bytes)).send().await.map_err(http)?;
            return Self::check_response(resp).await.map(|_| ());
        }

        let resp = client.post(format!("{}/createUploadSession", base)).header("Authorization", format!("Bearer {}", token))
            .json(&graph_upload_session_json(a, size)).send().await.map_err(http)?;
        let text = Self::check_response(resp).await?;
        let url = serde_json::from_str::<serde_json::Value>(&text).ok()
            .and_then(|j| j["uploadUrl"].as_str().map(str::to_string))
            .ok_or_else(|| crate::errors::OmarchyError::HttpError("Graph did not return an upload URL".into()))?;
        let mut file = std::fs::File::open(&a.path)?;
        for (first, last) in upload_ranges(size, UPLOAD_CHUNK) {
            let mut buf = vec![0u8; (last - first + 1) as usize];
            file.seek(SeekFrom::Start(first))?;
            file.read_exact(&mut buf)?;
            // The upload URL is pre-authorised: sending the bearer token to it is refused.
            let resp = client.put(&url)
                .header("Content-Type", "application/octet-stream")
                .header("Content-Range", content_range(first, last, size))
                .body(buf).send().await.map_err(http)?;
            Self::check_response(resp).await?;
        }
        Ok(())
    }

    /// Internal: handle non-2xx responses with full body logging
    async fn check_response(response: reqwest::Response) -> Result<String> {
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!("Graph API returned status: {}", status);
            error!("Graph API error body: {}", body);
            return Err(crate::errors::OmarchyError::HttpError(format!(
                "Graph API error: {} — {}",
                status, body
            )));
        }
        let text = response.text().await
            .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;
        Ok(text)
    }

    /// What kind of calendar message `id` is, with its event (answer, time, place) in the same
    /// request. Ordinary mail comes back as `InviteInfo::none()`.
    pub async fn fetch_invite_info(&self, id: &str) -> Result<crate::invites::InviteInfo> {
        let token = self.get_token().await?;
        let client = reqwest::Client::new();
        let response = client
            .get(format!(
                "https://graph.microsoft.com/v1.0/me/messages/{}?{}",
                urlencoding::encode(id),
                crate::invites::INFO_QUERY
            ))
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;
        let body = Self::check_response(response).await?;
        let json: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;
        Ok(crate::invites::InviteInfo::from_graph(&json))
    }

    /// Answer a meeting request on its calendar event: `op` is "accept" | "tentativelyAccept" |
    /// "decline". Exchange updates the calendar and sends the organizer a reply (silently, no
    /// comment, as the user chose). Graph answers 202 with no body.
    pub async fn respond_to_event(&self, event_id: &str, op: &str) -> Result<()> {
        let token = self.get_token().await?;
        let client = reqwest::Client::new();
        let response = client
            .post(format!(
                "https://graph.microsoft.com/v1.0/me/events/{}/{}",
                urlencoding::encode(event_id),
                op
            ))
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .body(r#"{"sendResponse": true}"#)
            .send()
            .await
            .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;
        Self::check_response(response).await.map(|_| ())
    }

    /// Full body of one message: (content type "html"|"text", content).
    pub async fn fetch_message_body(&self, id: &str) -> Result<(String, String)> {
        let token = self.get_token().await?;
        let client = reqwest::Client::new();
        let response = client
            .get(format!(
                "https://graph.microsoft.com/v1.0/me/messages/{}?$select=id,body",
                urlencoding::encode(id)
            ))
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;
        let body = Self::check_response(response).await?;
        let json: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;
        let content_type = if json["body"]["contentType"].as_str().map(|t| t.eq_ignore_ascii_case("html")).unwrap_or(false) {
            "html"
        } else {
            "text"
        };
        Ok((content_type.to_string(), json["body"]["content"].as_str().unwrap_or("").to_string()))
    }

    /// Attachments of one message (no file contents). `body` is the message HTML when known,
    /// so pictures the HTML shows by `cid:` count as inline even if the server did not say so.
    /// Only base properties are selected: Graph rejects `contentId` in `$select` on this
    /// collection (400), and `isInline` is reliable in practice.
    pub async fn fetch_attachments(&self, id: &str, body: Option<&str>) -> Result<Vec<crate::attachments::Attachment>> {
        let token = self.get_token().await?;
        let client = reqwest::Client::new();
        let response = client
            .get(format!(
                "https://graph.microsoft.com/v1.0/me/messages/{}/attachments?$select=id,name,size,contentType,isInline&$top=200",
                urlencoding::encode(id)
            ))
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;
        let text = Self::check_response(response).await?;
        let json: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;
        Ok(crate::attachments::from_graph(&json, body))
    }

    /// Download one attachment into `dir` under its (sanitised, collision-free) name and return
    /// the path. Streams to a `.part` file and renames on success, so a failed download leaves
    /// nothing behind.
    pub async fn download_attachment(&self, msg_id: &str, att: &crate::attachments::Attachment, dir: &std::path::Path) -> Result<std::path::PathBuf> {
        use std::io::Write;
        let err = |m: String| crate::errors::OmarchyError::HttpError(m);
        if !att.saveable() {
            return Err(err(format!("{} is a cloud link; there is no file to download", att.name)));
        }
        let token = self.get_token().await?;
        let client = reqwest::Client::new();
        let mut response = client
            .get(format!(
                "https://graph.microsoft.com/v1.0/me/messages/{}/attachments/{}/$value",
                urlencoding::encode(msg_id), urlencoding::encode(&att.id)
            ))
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| err(e.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            return Err(err(format!("Graph API error: {}", status)));
        }
        let (final_path, reserved) = crate::attachments::create_unique(dir, &att.name).map_err(|e| err(format!("cannot write to {}: {}", dir.display(), e)))?;
        drop(reserved);
        let part = final_path.with_extension(format!("{}part", final_path.extension().map(|e| format!("{}.", e.to_string_lossy())).unwrap_or_default()));
        let result: std::result::Result<(), String> = async {
            let mut f = std::fs::File::create(&part).map_err(|e| e.to_string())?;
            while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
                f.write_all(&chunk).map_err(|e| e.to_string())?;
            }
            f.flush().map_err(|e| e.to_string())?;
            std::fs::rename(&part, &final_path).map_err(|e| e.to_string())
        }.await;
        match result {
            Ok(()) => Ok(final_path),
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                let _ = std::fs::remove_file(&final_path);
                Err(err(format!("download of {} failed: {}", att.name, e)))
            }
        }
    }

    /// `Name <addr>, addr2` from a Graph recipient list; None when there are none.
    pub(crate) fn recipients_text(list: &serde_json::Value) -> Option<String> {
        let parts: Vec<String> = list.as_array()?.iter().filter_map(|r| {
            let addr = r["emailAddress"]["address"].as_str().unwrap_or("").trim();
            let name = r["emailAddress"]["name"].as_str().unwrap_or("").trim();
            if addr.is_empty() {
                return if name.is_empty() { None } else { Some(name.to_string()) };
            }
            if name.is_empty() || name.eq_ignore_ascii_case(addr) { Some(addr.to_string()) } else { Some(format!("{} <{}>", name, addr)) }
        }).collect();
        if parts.is_empty() { None } else { Some(parts.join(", ")) }
    }

    /// Parse a Graph API message JSON value into an EmailMessage
    pub(crate) fn parse_message(msg: &serde_json::Value) -> EmailMessage {
        let id = msg["id"].as_str().unwrap_or("").to_string();
        let subject = msg["subject"].as_str().unwrap_or("(no subject)").to_string();
        let received = msg["receivedDateTime"]
            .as_str()
            .unwrap_or(&chrono::Utc::now().to_rfc3339())
            .to_string();
        let from = msg["from"]["emailAddress"]["address"]
            .as_str()
            .unwrap_or("unknown@example.com")
            .to_string();
        let preview = msg["bodyPreview"].as_str().unwrap_or("").to_string();
        let parent_folder_id = msg["parentFolderId"].as_str().map(|s| s.to_string());

        let is_read = msg["isRead"].as_bool().unwrap_or(false);

        let non_empty = |v: &serde_json::Value| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
        EmailMessage {
            id, from, subject, received, body: preview, folder_id: parent_folder_id, is_read,
            meta: true,
            from_name: non_empty(&msg["from"]["emailAddress"]["name"]),
            to_text: Self::recipients_text(&msg["toRecipients"]),
            cc_text: Self::recipients_text(&msg["ccRecipients"]),
            bcc_text: Self::recipients_text(&msg["bccRecipients"]),
            categories: Some(msg["categories"].as_array().map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect()).unwrap_or_default()),
            sent_at: non_empty(&msg["sentDateTime"]),
            importance: non_empty(&msg["importance"]).map(|s| s.to_lowercase()),
            has_attachments: msg["hasAttachments"].as_bool(),
            conversation_id: non_empty(&msg["conversationId"]),
        }
    }
}

#[async_trait]
impl super::EmailProvider for GraphEmailProvider {
    /// Fetch emails from a specific folder (defaults to inbox)
    async fn fetch_inbox(&self, limit: usize) -> Result<Vec<EmailMessage>> {
        self.fetch_folder_messages("inbox", limit).await
    }

    async fn fetch_folder_messages(&self, folder_id: &str, limit: usize) -> Result<Vec<EmailMessage>> {
        debug!("Fetching {} emails from Graph API folder: {}", limit, folder_id);
        let token = self.get_token().await?;
        let client = reqwest::Client::new();

        // Use well-known names directly OR folder IDs
        let url = format!(
            "https://graph.microsoft.com/v1.0/me/mailFolders/{}/messages?\
             $top={}&$select={sel}\
             &$orderby=receivedDateTime desc",
            folder_id, limit, sel = MSG_SELECT
        );

        let response = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;

        let body = Self::check_response(response).await?;
        let json: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;

        let mut emails = Vec::new();
        if let Some(values) = json["value"].as_array() {
            for msg in values {
                emails.push(Self::parse_message(msg));
            }
        }

        debug!("Fetched {} emails from folder {}", emails.len(), folder_id);
        Ok(emails)
    }

    /// Fetch ALL messages in a folder, following pagination via @odata.nextLink.
    /// Returns results in batches via a channel so callers can process incrementally
    /// without blocking. Each page is ~50 items.
    async fn fetch_all_folder_messages(
        &self,
        folder_id: &str,
        tx: tokio::sync::mpsc::Sender<Vec<EmailMessage>>,
    ) -> Result<usize> {
        let token = self.get_token().await?;
        let client = reqwest::Client::new();

        let mut next_url = Some(format!(
            "https://graph.microsoft.com/v1.0/me/mailFolders/{}/messages?\
             $top=50&$select={sel}\
             &$orderby=receivedDateTime desc",
            folder_id, sel = MSG_SELECT
        ));

        let mut total = 0usize;

        while let Some(url) = next_url.take() {
            let response = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", token))
                .send()
                .await
                .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;

            let body = Self::check_response(response).await?;
            let json: serde_json::Value = serde_json::from_str(&body)
                .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;

            let mut batch = Vec::new();
            if let Some(values) = json["value"].as_array() {
                for msg in values {
                    batch.push(Self::parse_message(msg));
                }
            }

            total += batch.len();
            // debug!("Fetched page of {} messages from folder {} (total so far: {})", batch.len(), folder_id, total);

            if !batch.is_empty() {
                // Send batch to caller — if receiver is gone, stop pagination
                if tx.send(batch).await.is_err() {
                    debug!("Pagination receiver dropped, stopping for folder {}", folder_id);
                    break;
                }
            }

            // Follow next page link if present
            next_url = json["@odata.nextLink"].as_str().map(|s| s.to_string());

            // Yield to Tokio scheduler between pages so UI stays responsive
            tokio::task::yield_now().await;
        }

        debug!("Completed full sync of folder {}: {} messages total", folder_id, total);
        Ok(total)
    }

    fn supports_delta(&self) -> bool {
        true
    }

    /// Graph `messages/delta`: the first call (no link) enumerates the folder newest-first, 500
    /// per page; later calls with the stored deltaLink return only what changed (new, read-flag
    /// changes, `@removed`). Pages are streamed to `tx` as they arrive.
    async fn fetch_delta(
        &self,
        folder_id: &str,
        link: Option<String>,
        tx: tokio::sync::mpsc::Sender<DeltaPage>,
    ) -> Result<DeltaEnd> {
        let client = reqwest::Client::new();
        let mut url = link.clone().unwrap_or_else(|| {
            format!(
                "https://graph.microsoft.com/v1.0/me/mailFolders/{}/messages/delta?\
                 $select={sel}",
                folder_id, sel = MSG_SELECT
            )
        });
        loop {
            let token = self.get_token().await?;
            let mut attempt = 0;
            let body = loop {
                let response = client
                    .get(&url)
                    .header("Authorization", format!("Bearer {}", token))
                    .header("Prefer", "odata.maxpagesize=500")
                    .send()
                    .await
                    .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;
                let status = response.status();
                if status.is_success() {
                    break response
                        .text()
                        .await
                        .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;
                }
                // A rejected stored link (expired state, malformed) means: start over.
                if link.is_some() && matches!(status.as_u16(), 400 | 404 | 410) {
                    debug!("Delta link for {} rejected ({}); resetting", folder_id, status);
                    return Ok(DeltaEnd::Reset);
                }
                // Throttled / briefly unavailable: honour Retry-After a few times.
                if matches!(status.as_u16(), 429 | 503 | 504) && attempt < 3 {
                    attempt += 1;
                    let wait = response
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok())
                        .unwrap_or(2 * attempt as u64)
                        .min(30);
                    debug!("Delta request throttled ({}); retrying in {}s", status, wait);
                    tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                    continue;
                }
                return Err(crate::errors::OmarchyError::HttpError(
                    Self::check_response(response).await.err().map(|e| e.to_string()).unwrap_or_default(),
                ));
            };
            let json: serde_json::Value = serde_json::from_str(&body)
                .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;
            let mut page = DeltaPage::default();
            for item in json["value"].as_array().into_iter().flatten() {
                if item.get("@removed").is_some() {
                    if let Some(id) = item["id"].as_str() {
                        page.removed.push(id.to_string());
                    }
                } else {
                    page.upserts.push(Self::parse_message(item));
                }
            }
            if let Some(done) = json["@odata.deltaLink"].as_str() {
                if tx.send(page).await.is_err() {
                    return Err(crate::errors::OmarchyError::HttpError("delta receiver dropped".into()));
                }
                return Ok(DeltaEnd::Done(done.to_string()));
            }
            match json["@odata.nextLink"].as_str() {
                Some(next) => {
                    page.next_link = Some(next.to_string());
                    url = next.to_string();
                    if tx.send(page).await.is_err() {
                        return Err(crate::errors::OmarchyError::HttpError("delta receiver dropped".into()));
                    }
                }
                None => {
                    return Err(crate::errors::OmarchyError::HttpError(
                        "delta response had neither nextLink nor deltaLink".into(),
                    ))
                }
            }
            tokio::task::yield_now().await;
        }
    }

    async fn fetch_unread_ids(&self, folder_id: &str) -> Result<Vec<String>> {
        let token = self.get_token().await?;
        let client = reqwest::Client::new();
        let mut ids = Vec::new();
        let mut next_url = Some(format!(
            "https://graph.microsoft.com/v1.0/me/mailFolders/{}/messages?\
             $filter=isRead eq false&$select=id&$top=1000",
            folder_id
        ));
        while let Some(url) = next_url.take() {
            let response = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", token))
                .send()
                .await
                .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;
            let body = Self::check_response(response).await?;
            let json: serde_json::Value = serde_json::from_str(&body)
                .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;
            if let Some(values) = json["value"].as_array() {
                ids.extend(values.iter().filter_map(|m| m["id"].as_str().map(String::from)));
            }
            next_url = json["@odata.nextLink"].as_str().map(String::from);
        }
        Ok(ids)
    }

    async fn set_message_read(&self, id: &str, is_read: bool) -> Result<()> {
        let token = self.get_token().await?;
        let client = reqwest::Client::new();
        let response = client
            .patch(format!("https://graph.microsoft.com/v1.0/me/messages/{}", id))
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .body(format!("{{\"isRead\": {}}}", is_read))
            .send()
            .await
            .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;
        Self::check_response(response).await.map(|_| ())
    }

    async fn fetch_categories(&self) -> Result<Vec<crate::models::CategoryDef>> {
        let token = self.get_token().await?;
        let client = reqwest::Client::new();
        let response = client
            .get("https://graph.microsoft.com/v1.0/me/outlook/masterCategories")
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;
        let body = Self::check_response(response).await?;
        let json: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;
        Ok(json["value"].as_array().into_iter().flatten().filter_map(|c| {
            let name = c["displayName"].as_str()?.to_string();
            let preset = c["color"].as_str().unwrap_or("none").to_string();
            Some(crate::models::CategoryDef {
                color: crate::models::graph_preset_hex(&preset).to_string(),
                name,
                provider_color: Some(preset),
            })
        }).collect())
    }

    async fn create_category(&self, name: &str, color: &crate::models::PaletteColor) -> Result<crate::models::CategoryDef> {
        let token = self.get_token().await?;
        let client = reqwest::Client::new();
        let response = client
            .post("https://graph.microsoft.com/v1.0/me/outlook/masterCategories")
            .header("Authorization", format!("Bearer {}", token))
            .json(&serde_json::json!({ "displayName": name, "color": color.graph_preset }))
            .send()
            .await
            .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;
        Self::check_response(response).await?;
        Ok(crate::models::CategoryDef { name: name.to_string(), color: color.hex.to_string(), provider_color: Some(color.graph_preset.to_string()) })
    }

    async fn set_message_categories(&self, id: &str, wanted: &[String], _had: &[String]) -> Result<()> {
        let token = self.get_token().await?;
        let client = reqwest::Client::new();
        let response = client
            .patch(format!("https://graph.microsoft.com/v1.0/me/messages/{}", id))
            .header("Authorization", format!("Bearer {}", token))
            .json(&serde_json::json!({ "categories": wanted }))
            .send()
            .await
            .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;
        Self::check_response(response).await.map(|_| ())
    }

    async fn apply_message_action(
        &self,
        id: &str,
        _from_folder: &str,
        action: &crate::models::MessageAction,
    ) -> Result<()> {
        use crate::models::MessageAction;
        let token = self.get_token().await?;
        let client = reqwest::Client::new();
        let base = format!("https://graph.microsoft.com/v1.0/me/messages/{}", id);
        // `archive` and `deleteditems` are well-known folder names Graph accepts as a destination.
        let dest = match action {
            MessageAction::Archive => "archive".to_string(),
            MessageAction::Trash => "deleteditems".to_string(),
            MessageAction::Move(d) => d.clone(),
            MessageAction::Delete => String::new(),
        };
        let request = if *action == MessageAction::Delete {
            client.delete(&base)
        } else {
            client.post(format!("{}/move", base)).json(&serde_json::json!({ "destinationId": dest }))
        };
        let response = request
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;
        Self::check_response(response).await.map(|_| ())
    }

    async fn send_message(&self, msg: &crate::compose::OutgoingMessage, _from: &str) -> Result<()> {
        use crate::compose::{graph_message_json, graph_send_mail_json};
        let token = self.get_token().await?;
        let client = reqwest::Client::new();
        let call = |method: reqwest::Method, path: String, body: Option<serde_json::Value>| {
            let mut req = client
                .request(method, format!("https://graph.microsoft.com/v1.0{}", path))
                .header("Authorization", format!("Bearer {}", token));
            if let Some(b) = body {
                req = req.json(&b);
            }
            async move {
                let resp = req.send().await.map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;
                Self::check_response(resp).await
            }
        };
        let send_new = || call(reqwest::Method::POST, "/me/sendMail".into(), Some(graph_send_mail_json(msg)));

        let has_files = !msg.attachments.is_empty();
        let threaded = msg.is_reply() || msg.is_forward();
        if !threaded && !has_files {
            return send_new().await.map(|_| ());
        }
        let draft_id_of = |text: &str| {
            serde_json::from_str::<serde_json::Value>(text)
                .ok()
                .and_then(|j| j["id"].as_str().map(str::to_string))
                .ok_or_else(|| crate::errors::OmarchyError::HttpError("Graph did not return a draft id".into()))
        };

        // Reply / forward: let Graph build the draft so the conversation stays threaded, then
        // replace its body and recipients with what the user wrote, and send it.
        let mut existing: Option<String> = None;
        if threaded {
            let action = match msg.kind.as_str() {
                "replyAll" => "createReplyAll",
                "forward" => "createForward",
                _ => "createReply",
            };
            let orig = urlencoding::encode(&msg.in_reply_to).into_owned();
            match call(reqwest::Method::POST, format!("/me/messages/{}/{}", orig, action), Some(serde_json::json!({}))).await {
                Ok(text) => existing = Some(draft_id_of(&text)?),
                // The original is gone: send what was written as a new message rather than lose it.
                Err(crate::errors::OmarchyError::HttpError(m)) if m.contains("Graph API error: 404") => {
                    warn!("Graph: message {} no longer exists; sending as a new message", msg.in_reply_to);
                    if !has_files {
                        return send_new().await.map(|_| ());
                    }
                }
                Err(e) => return Err(e),
            }
        }
        // A new message with files is built as a draft too: attachments can only be added to a
        // message that exists (sendMail's inline list is capped at a few MB).
        let fresh = existing.is_none();
        let draft_id = match existing {
            Some(id) => id,
            None => draft_id_of(&call(reqwest::Method::POST, "/me/messages".into(), Some(graph_message_json(msg))).await?)?,
        };
        let did = urlencoding::encode(&draft_id).into_owned();

        let finish = async {
            if !fresh {
                call(reqwest::Method::PATCH, format!("/me/messages/{}", did), Some(graph_message_json(msg))).await?;
            }
            for a in &msg.attachments {
                Self::add_attachment(&client, &token, &did, a).await?;
            }
            call(reqwest::Method::POST, format!("/me/messages/{}/send", did), None).await
        };
        match finish.await {
            Ok(_) => Ok(()),
            Err(e) => {
                // Do not leave a stray draft behind.
                if let Err(d) = call(reqwest::Method::DELETE, format!("/me/messages/{}", did), None).await {
                    warn!("Graph: could not delete draft {} after a failed send: {}", draft_id, d);
                }
                Err(e)
            }
        }
    }

    /// Fetch all mail folders from Graph API
    async fn fetch_folders(&self) -> Result<Vec<MailFolder>> {
        debug!("Fetching mail folders from Graph API");
        let token = self.get_token().await?;
        let client = reqwest::Client::new();

        // Fetch top-level folders including well-known names
        let url = "https://graph.microsoft.com/v1.0/me/mailFolders?\
                   $top=100&$select=id,displayName,parentFolderId,unreadItemCount,totalItemCount"
            .to_string();

        let response = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;

        let body = Self::check_response(response).await?;
        let json: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| crate::errors::OmarchyError::HttpError(e.to_string()))?;

        let mut folders = Vec::new();
        if let Some(values) = json["value"].as_array() {
            for f in values {
                folders.push(MailFolder {
                    id:               f["id"].as_str().unwrap_or("").to_string(),
                    display_name:     f["displayName"].as_str().unwrap_or("").to_string(),
                    parent_folder_id: f["parentFolderId"].as_str().map(|s| s.to_string()),
                    unread_item_count: f["unreadItemCount"].as_i64().map(|n| n as i32),
                    total_item_count:  f["totalItemCount"].as_i64().map(|n| n as i32),
                    // wellKnownName is NOT a v1.0 API property — infer from displayName
                    well_known_name: infer_well_known_name(
                        f["displayName"].as_str().unwrap_or("")
                    ),
                });
            }
        }

        debug!("Fetched {} folders", folders.len());
        Ok(folders)
    }

    async fn is_token_valid(&self) -> Result<bool> {
        let mut auth = self.auth.lock().await;
        let token = auth.get_token().map_err(|_| {
            crate::errors::OmarchyError::AuthError("Token check failed".to_string())
        });
        Ok(token.is_ok() && !token.unwrap_or_default().is_empty())
    }
}

/// Infer well-known folder name from displayName.
///
/// The Graph API v1.0 `mailFolder` resource does NOT include a `wellKnownName` property
/// (it was added in beta only, and even there it's not reliable for all locales).
/// Instead, well-known names are used as path segments in URLs (e.g. /me/mailFolders/inbox).
/// We infer sort order from the English display name; for non-English mailboxes the
/// sort_order fallback of 999 keeps custom folders at the bottom, which is acceptable.
/// See: https://learn.microsoft.com/en-us/graph/api/resources/mailfolder?view=graph-rest-1.0
fn infer_well_known_name(display_name: &str) -> Option<String> {
    match display_name.to_lowercase().as_str() {
        "inbox"                 => Some("inbox".to_string()),
        "drafts"                => Some("drafts".to_string()),
        "sent items"            => Some("sentitems".to_string()),
        "deleted items"         => Some("deleteditems".to_string()),
        "junk email"            => Some("junkemail".to_string()),
        "archive"               => Some("archive".to_string()),
        "outbox"                => Some("outbox".to_string()),
        "conversation history"  => Some("conversationhistory".to_string()),
        "clutter"               => Some("clutter".to_string()),
        _                       => None,
    }
}

#[cfg(test)]
mod detail_parse_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn graph_message_details_are_parsed() {
        let m = json!({
            "id": "AAMk1", "subject": "Lunch?", "receivedDateTime": "2026-10-09T12:00:00Z", "bodyPreview": "hi",
            "isRead": false, "parentFolderId": "F1", "importance": "High", "hasAttachments": true,
            "sentDateTime": "2026-10-09T11:59:58Z", "conversationId": "conv1", "categories": ["Red category", "Work"],
            "from": {"emailAddress": {"name": "Ada Lovelace", "address": "ada@x.org"}},
            "toRecipients": [{"emailAddress": {"name": "Bob", "address": "bob@x.org"}}, {"emailAddress": {"name": "", "address": "cy@x.org"}}],
            "ccRecipients": [{"emailAddress": {"name": "dee@x.org", "address": "dee@x.org"}}],
            "bccRecipients": []
        });
        let e = GraphEmailProvider::parse_message(&m);
        assert!(e.meta);
        assert_eq!(e.from, "ada@x.org");
        assert_eq!(e.from_name.as_deref(), Some("Ada Lovelace"));
        assert_eq!(e.to_text.as_deref(), Some("Bob <bob@x.org>, cy@x.org"));
        assert_eq!(e.cc_text.as_deref(), Some("dee@x.org"));
        assert_eq!(e.bcc_text, None);
        assert_eq!(e.categories, Some(vec!["Red category".to_string(), "Work".to_string()]));
        assert_eq!(e.importance.as_deref(), Some("high"));
        assert_eq!(e.has_attachments, Some(true));
        assert_eq!(e.conversation_id.as_deref(), Some("conv1"));
        assert_eq!(e.sent_at.as_deref(), Some("2026-10-09T11:59:58Z"));
        // a bare message still parses, with empty (not missing) categories
        let bare = GraphEmailProvider::parse_message(&json!({"id": "x"}));
        assert_eq!(bare.categories, Some(vec![]));
        assert!(MSG_SELECT.contains("categories") && MSG_SELECT.contains("bccRecipients"));
    }
}
