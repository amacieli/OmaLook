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
