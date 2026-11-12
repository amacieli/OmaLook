//! Gmail calls for the compose extras: list headers (OL-053), identities (OL-051), raw source (OL-052).

use super::{unscoped, GmailProvider, BASE};
use crate::errors::Result;

impl GmailProvider {
    /// `List-Id` / `List-Post` / `Mailing-List` of one message (`format=metadata`).
    pub async fn fetch_list_headers(&self, id: &str) -> Result<Vec<(String, String)>> {
        let raw = unscoped(&self.account_id, id);
        let url = format!(
            "{}/messages/{}?format=metadata&metadataHeaders=List-Post&metadataHeaders=List-Id&metadataHeaders=Mailing-List",
            BASE, urlencoding::encode(raw)
        );
        let m = self.api.get(&url).await?;
        Ok(crate::list_headers::headers_from_gmail(&m))
    }
}

impl GmailProvider {
    /// The account's send-as identities (`users.settings.sendAs`, scope `gmail.settings.basic`): the primary address
    /// and the verified aliases. An account that signed in before that scope was added gets a 403, which is passed on.
    pub async fn fetch_identities(&self, account_email: &str) -> Result<Vec<crate::identities::Identity>> {
        let json = self.api.get(&format!("{}/settings/sendAs", BASE)).await?;
        Ok(crate::identities::ensure_primary(crate::identities::parse_gmail_send_as(&json), account_email))
    }
}

impl GmailProvider {
    /// Bounce (OL-052): the original's raw source (`format=raw`), rewritten by `crate::bounce::rewrite`, sent with
    /// `messages.send` (uploaded from a file, whatever the size). Not threaded: it is a new message to new people.
    pub async fn send_bounce(&self, msg: &crate::compose::OutgoingMessage, from: &str) -> Result<()> {
        use base64::Engine;
        let id = unscoped(&self.account_id, &msg.in_reply_to);
        let m = self.api.get(&format!("{}/messages/{}?format=raw", BASE, urlencoding::encode(id))).await?;
        let raw = m["raw"].as_str().unwrap_or_default();
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(raw.trim_end_matches('='))
            .map_err(|e| crate::errors::OmarchyError::HttpError(format!("the original message could not be read: {}", e)))?;
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let out = crate::bounce::rewrite(&bytes, from, &msg.to, &msg.cc, &chrono::Utc::now().to_rfc2822(), &crate::bounce::new_message_id(from, nanos))
            .map_err(crate::errors::OmarchyError::HttpError)?;
        let scratch = super::Scratch::new()?;
        let eml = scratch.0.join("bounce.eml");
        std::fs::write(&eml, &out)?;
        self.api.upload_message(&eml, None).await.map(|_| ())
    }
}
