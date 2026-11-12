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
