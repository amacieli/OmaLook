//! Graph calls for the compose extras: list headers (OL-053), identities (OL-051).
//! A child module of `graph` so it can use the provider's private token handling.

use super::GraphEmailProvider;
use crate::errors::{OmarchyError, Result};

impl GraphEmailProvider {
    /// The `List-*` / `Mailing-List` headers of one message (Graph `internetMessageHeaders`, which a
    /// message list never carries, hence one lazy request per message).
    pub async fn fetch_list_headers(&self, id: &str) -> Result<Vec<(String, String)>> {
        let token = self.get_token().await?;
        let response = super::super::http_client()
            .get(format!("https://graph.microsoft.com/v1.0/me/messages/{}?$select=internetMessageHeaders", urlencoding::encode(id)))
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| OmarchyError::HttpError(e.to_string()))?;
        let body = Self::check_response(response).await?;
        let json: serde_json::Value = serde_json::from_str(&body).map_err(|e| OmarchyError::HttpError(e.to_string()))?;
        Ok(crate::list_headers::headers_from_graph(&json["internetMessageHeaders"]))
    }
}

impl GraphEmailProvider {
    /// The account's send-as identities from `/me` (`User.Read`, already granted): `proxyAddresses` of the mailbox.
    /// `account_email` is the sign-in address (always an identity).
    pub async fn fetch_identities(&self, account_email: &str) -> Result<Vec<crate::identities::Identity>> {
        let token = self.get_token().await?;
        let response = super::super::http_client()
            .get("https://graph.microsoft.com/v1.0/me?$select=displayName,mail,userPrincipalName,proxyAddresses")
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| OmarchyError::HttpError(e.to_string()))?;
        let body = Self::check_response(response).await?;
        let json: serde_json::Value = serde_json::from_str(&body).map_err(|e| OmarchyError::HttpError(e.to_string()))?;
        Ok(crate::identities::parse_graph_proxies(&json, account_email))
    }
}

/// Exchange takes a MIME message of about 4 MB at most through `sendMail`; stay clear of it and say so.
const MAX_BOUNCE_BYTES: usize = 3_000_000;

impl GraphEmailProvider {
    /// Bounce (OL-052): the original's MIME (`/me/messages/{id}/$value`), rewritten by `crate::bounce::rewrite`, sent as a
    /// MIME `sendMail` (`Content-Type: text/plain`, base64 body; Exchange reads the recipients from the headers).
    pub async fn send_bounce(&self, msg: &crate::compose::OutgoingMessage, from: &str) -> Result<()> {
        use base64::Engine;
        let http = |e: String| OmarchyError::HttpError(e);
        let token = self.get_token().await?;
        let client = super::super::http_client();
        let resp = client
            .get(format!("https://graph.microsoft.com/v1.0/me/messages/{}/$value", urlencoding::encode(&msg.in_reply_to)))
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| http(e.to_string()))?;
        if !resp.status().is_success() {
            return Self::check_response(resp).await.map(|_| ());
        }
        let bytes = resp.bytes().await.map_err(|e| http(e.to_string()))?;
        if bytes.len() > MAX_BOUNCE_BYTES {
            return Err(http(format!("the message is {} MB: too large to bounce through Exchange (limit about 3 MB); forward it instead", bytes.len() / 1_000_000)));
        }
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let out = crate::bounce::rewrite(&bytes, from, &msg.to, &msg.cc, &chrono::Utc::now().to_rfc2822(), &crate::bounce::new_message_id(from, nanos)).map_err(http)?;
        let resp = client
            .post("https://graph.microsoft.com/v1.0/me/sendMail")
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "text/plain")
            .body(base64::engine::general_purpose::STANDARD.encode(&out))
            .send()
            .await
            .map_err(|e| http(e.to_string()))?;
        Self::check_response(resp).await.map(|_| ())
    }
}
