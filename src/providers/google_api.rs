//! Shared HTTP plumbing for the Google providers (Gmail, Calendar, People).
//!
//! One `GoogleApi` per account and service thread: it owns the account's `AuthManager` (so
//! tokens come from the shared per-account broker) and retries the transient failures Google
//! documents (429, 5xx, per-user rate limits) with exponential backoff.

use crate::auth::AuthManager;
use crate::errors::{OmarchyError, Result};
use log::{debug, error, warn};
use std::time::Duration;
use tokio::sync::Mutex;

const MAX_ATTEMPTS: u32 = 4;

pub struct GoogleApi {
    auth: Mutex<AuthManager>,
    client: reqwest::Client,
}

/// Whether a failed response is worth retrying, from its status and error body.
pub(crate) fn is_retryable(status: u16, body: &str) -> bool {
    match status {
        429 | 500 | 502 | 503 | 504 => true,
        403 => body.contains("rateLimitExceeded") || body.contains("userRateLimitExceeded"),
        _ => false,
    }
}

/// A readable hint for the failures that need action from the user rather than a retry.
pub(crate) fn hint_for(status: u16, body: &str) -> &'static str {
    if status == 403 && (body.contains("accessNotConfigured") || body.contains("SERVICE_DISABLED")) {
        " (this API is not enabled in the Google Cloud project: enable it in APIs & Services → Library)"
    } else if status == 403 && body.contains("insufficientPermissions") {
        " (scope not granted: sign in to this account again)"
    } else {
        ""
    }
}

impl GoogleApi {
    pub fn new(account_id: &str) -> Self {
        Self { auth: Mutex::new(AuthManager::for_account(account_id)), client: reqwest::Client::new() }
    }

    async fn token(&self) -> Result<String> {
        let mut auth = self.auth.lock().await;
        let token = auth
            .get_token()
            .map_err(|e| OmarchyError::AuthError(format!("Failed to get token: {}", e)))?;
        if token.is_empty() {
            return Err(OmarchyError::AuthError("No access token".to_string()));
        }
        Ok(token)
    }

    pub async fn is_token_valid(&self) -> Result<bool> {
        Ok(self.token().await.is_ok())
    }

    pub async fn get(&self, url: &str) -> Result<serde_json::Value> {
        self.send(reqwest::Method::GET, url, None).await
    }

    pub async fn post(&self, url: &str, body: &serde_json::Value) -> Result<serde_json::Value> {
        self.send(reqwest::Method::POST, url, Some(body)).await
    }

    pub async fn patch(&self, url: &str, body: &serde_json::Value) -> Result<serde_json::Value> {
        self.send(reqwest::Method::PATCH, url, Some(body)).await
    }

    pub async fn delete(&self, url: &str) -> Result<serde_json::Value> {
        self.send(reqwest::Method::DELETE, url, None).await
    }

    /// Send a finished RFC 822 file as a message through Gmail's resumable upload (`messages.send`).
    ///
    /// The file is read in 8 MiB pieces, so memory use does not grow with the message. Starting
    /// the session cannot duplicate anything, so it is retried like any other request and its
    /// failures read `Google API error: ...` (the outbox retries those it deems transient). Once
    /// bytes are going up, a failure is reported as `Google upload error: ...`, which the outbox
    /// never retries: the message may have been taken, and a second copy is worse than a report.
    /// A dropped chunk is resumed from what Google says it has.
    pub async fn upload_message(&self, path: &std::path::Path, thread_id: Option<&str>) -> Result<serde_json::Value> {
        use std::io::{Read, Seek, SeekFrom};
        const CHUNK: u64 = 8 * 1024 * 1024; // a multiple of 256 KiB, as Google requires
        let total = std::fs::metadata(path).map_err(|e| OmarchyError::HttpError(format!("cannot read the prepared message: {}", e)))?.len();
        let init_body = match thread_id {
            Some(t) => serde_json::json!({ "threadId": t }),
            None => serde_json::json!({}),
        };
        let init_url = "https://gmail.googleapis.com/upload/gmail/v1/users/me/messages/send?uploadType=resumable";
        let mut attempt = 0;
        let session = loop {
            attempt += 1;
            let token = self.token().await?;
            let resp = self.client.post(init_url).bearer_auth(token)
                .header("X-Upload-Content-Type", "message/rfc822")
                .header("X-Upload-Content-Length", total.to_string())
                .json(&init_body).send().await;
            match resp {
                Ok(r) if r.status().is_success() => {
                    match r.headers().get("location").and_then(|v| v.to_str().ok()) {
                        Some(l) => break l.to_string(),
                        None => return Err(OmarchyError::HttpError("Google API error: upload session had no location".into())),
                    }
                }
                Ok(r) => {
                    let status = r.status();
                    let text = r.text().await.unwrap_or_default();
                    if attempt < MAX_ATTEMPTS && is_retryable(status.as_u16(), &text) {
                        tokio::time::sleep(Duration::from_secs(1 << (attempt - 1))).await;
                        continue;
                    }
                    return Err(OmarchyError::HttpError(format!("Google API error: {}{} — {}", status, hint_for(status.as_u16(), &text), text)));
                }
                Err(e) if attempt < MAX_ATTEMPTS => {
                    warn!("Google upload start failed ({}), retrying", e);
                    tokio::time::sleep(Duration::from_secs(1 << (attempt - 1))).await;
                }
                Err(e) => return Err(OmarchyError::HttpError(format!("Google API error: {}", e))),
            }
        };

        let mut file = std::fs::File::open(path).map_err(|e| OmarchyError::HttpError(format!("cannot read the prepared message: {}", e)))?;
        let up_err = |m: String| OmarchyError::HttpError(format!("Google upload error: {}", m));
        let mut offset: u64 = 0;
        let mut failures = 0;
        loop {
            let len = (total - offset).min(CHUNK);
            let mut buf = vec![0u8; len as usize];
            file.seek(SeekFrom::Start(offset)).map_err(|e| up_err(e.to_string()))?;
            file.read_exact(&mut buf).map_err(|e| up_err(e.to_string()))?;
            let range = if len == 0 { format!("bytes */{}", total) } else { format!("bytes {}-{}/{}", offset, offset + len - 1, total) };
            let token = self.token().await?;
            let resp = self.client.put(&session).bearer_auth(token).header("Content-Range", range).body(buf).send().await;
            let (status, headers, text) = match resp {
                Ok(r) => { let s = r.status(); let h = r.headers().clone(); (s.as_u16(), h, r.text().await.unwrap_or_default()) }
                Err(e) => (0, reqwest::header::HeaderMap::new(), e.to_string()),
            };
            match status {
                200 | 201 => {
                    return serde_json::from_str(&text).map_err(|e| up_err(format!("bad JSON from Google: {}", e)));
                }
                308 => {
                    // "Range: bytes=0-N" is what Google holds; none yet means start over.
                    offset = headers.get("range").and_then(|v| v.to_str().ok()).and_then(|v| v.rsplit('-').next()).and_then(|n| n.trim().parse::<u64>().ok()).map(|n| n + 1).unwrap_or(0);
                    failures = 0;
                }
                s if (s == 0 || s == 429 || (500..600).contains(&s)) && failures < MAX_ATTEMPTS => {
                    failures += 1;
                    tokio::time::sleep(Duration::from_secs(1 << (failures - 1))).await;
                    // Ask what Google has; a finished upload answers 200/201 with the message.
                    let token = self.token().await?;
                    if let Ok(r) = self.client.put(&session).bearer_auth(token).header("Content-Range", format!("bytes */{}", total)).header("Content-Length", "0").body("").send().await {
                        let st = r.status().as_u16();
                        let h = r.headers().clone();
                        let t = r.text().await.unwrap_or_default();
                        if st == 200 || st == 201 {
                            return serde_json::from_str(&t).map_err(|e| up_err(format!("bad JSON from Google: {}", e)));
                        }
                        if st == 308 {
                            offset = h.get("range").and_then(|v| v.to_str().ok()).and_then(|v| v.rsplit('-').next()).and_then(|n| n.trim().parse::<u64>().ok()).map(|n| n + 1).unwrap_or(0);
                        }
                    }
                }
                s => {
                    error!("Google upload {}: {}", s, text);
                    return Err(up_err(format!("{}{} — {}", s, hint_for(s, &text), text)));
                }
            }
        }
    }

    async fn send(&self, method: reqwest::Method, url: &str, body: Option<&serde_json::Value>) -> Result<serde_json::Value> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let token = self.token().await?;
            let mut req = self.client.request(method.clone(), url).bearer_auth(token);
            if let Some(b) = body {
                req = req.json(b);
            }
            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) if attempt < MAX_ATTEMPTS => {
                    warn!("Google API request failed ({}), retrying", e);
                    tokio::time::sleep(Duration::from_secs(1 << (attempt - 1))).await;
                    continue;
                }
                Err(e) => return Err(OmarchyError::HttpError(e.to_string())),
            };
            let status = resp.status();
            let text = resp.text().await.map_err(|e| OmarchyError::HttpError(e.to_string()))?;
            if status.is_success() {
                if text.trim().is_empty() {
                    return Ok(serde_json::Value::Null);
                }
                return serde_json::from_str(&text).map_err(|e| OmarchyError::HttpError(format!("bad JSON from Google: {}", e)));
            }
            if attempt < MAX_ATTEMPTS && is_retryable(status.as_u16(), &text) {
                let wait = Duration::from_secs(1 << (attempt - 1));
                debug!("Google API {} — retrying in {:?}", status, wait);
                tokio::time::sleep(wait).await;
                continue;
            }
            error!("Google API {} for {}: {}", status, url.split('?').next().unwrap_or(url), text);
            return Err(OmarchyError::HttpError(format!("Google API error: {}{} — {}", status, hint_for(status.as_u16(), &text), text)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_transient_failures_are_retried() {
        assert!(is_retryable(429, ""));
        assert!(is_retryable(503, ""));
        assert!(is_retryable(403, r#"{"error":{"errors":[{"reason":"userRateLimitExceeded"}]}}"#));
        assert!(!is_retryable(403, r#"{"error":{"errors":[{"reason":"insufficientPermissions"}]}}"#));
        assert!(!is_retryable(401, ""));
        assert!(!is_retryable(404, ""));
    }

    #[test]
    fn disabled_api_gets_an_actionable_hint() {
        assert!(hint_for(403, r#"{"status":"PERMISSION_DENIED","reason":"SERVICE_DISABLED"}"#).contains("not enabled"));
        assert!(hint_for(403, "accessNotConfigured").contains("Library"));
        assert_eq!(hint_for(404, ""), "");
    }
}
