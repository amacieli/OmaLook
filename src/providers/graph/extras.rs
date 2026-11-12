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
