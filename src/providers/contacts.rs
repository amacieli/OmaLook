//! Microsoft Graph contacts provider (mirrors providers/graph.rs for mail: contact
//! folders play the role of mail folders, contacts the role of messages).
//!
//! Needs the `Contacts.Read` delegated permission.

use crate::auth::AuthManager;
use crate::errors::{OmarchyError, Result};
use crate::models::{Contact, ContactAddress, ContactFolder, ContactPhone};
use async_trait::async_trait;
use log::{debug, error};
use tokio::sync::Mutex;

/// Pseudo folder id for the user's default Contacts folder. Graph does NOT list it under
/// `/me/contactFolders` (that returns only the folders beneath it); its contacts are served
/// from `/me/contacts`. Without this entry an address book with no extra folders syncs nothing.
pub const DEFAULT_FOLDER_ID: &str = "contacts";

fn folder_contacts_url(folder_id: &str) -> String {
    if folder_id == DEFAULT_FOLDER_ID {
        "https://graph.microsoft.com/v1.0/me/contacts".to_string()
    } else {
        format!("https://graph.microsoft.com/v1.0/me/contactFolders/{}/contacts", folder_id)
    }
}

const SELECT: &str = "id,displayName,givenName,surname,companyName,jobTitle,emailAddresses,homePhones,\
                      businessPhones,mobilePhone,homeAddress,businessAddress,otherAddress,\
                      createdDateTime,lastModifiedDateTime,parentFolderId";

#[async_trait]
pub trait ContactsProvider: Send + Sync {
    /// All contact folders ("contact lists"), including the default Contacts folder
    async fn fetch_folders(&self) -> Result<Vec<ContactFolder>>;

    /// The most recently modified contacts of a folder
    async fn fetch_folder_contacts(&self, folder_id: &str, limit: usize) -> Result<Vec<Contact>>;

    /// ALL contacts of a folder via @odata.nextLink, streamed in pages of ~50
    async fn fetch_all_folder_contacts(
        &self,
        folder_id: &str,
        tx: tokio::sync::mpsc::Sender<Vec<Contact>>,
    ) -> Result<usize>;

    async fn is_token_valid(&self) -> Result<bool>;
}

pub struct GraphContactsProvider {
    auth: Mutex<AuthManager>,
}

impl GraphContactsProvider {
    pub fn new(auth: AuthManager) -> Self {
        Self { auth: Mutex::new(auth) }
    }

    async fn get_token(&self) -> Result<String> {
        let mut auth = self.auth.lock().await;
        let token = auth
            .get_token()
            .map_err(|e| OmarchyError::AuthError(format!("Failed to get token: {}", e)))?;
        if token.is_empty() {
            return Err(OmarchyError::AuthError("No access token".to_string()));
        }
        Ok(token)
    }

    async fn get_page(client: &reqwest::Client, token: &str, url: &str) -> Result<serde_json::Value> {
        let response = client
            .get(url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| OmarchyError::HttpError(e.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!("Graph contacts API returned status: {} — {}", status, body);
            let hint = if status.as_u16() == 403 {
                " (Contacts.Read not granted: log this account in again to consent)"
            } else {
                ""
            };
            return Err(OmarchyError::HttpError(format!("Graph API error: {}{} — {}", status, hint, body)));
        }
        let text = response.text().await.map_err(|e| OmarchyError::HttpError(e.to_string()))?;
        serde_json::from_str(&text).map_err(|e| OmarchyError::HttpError(e.to_string()))
    }
}

#[async_trait]
impl ContactsProvider for GraphContactsProvider {
    async fn fetch_folders(&self) -> Result<Vec<ContactFolder>> {
        let token = self.get_token().await?;
        let client = super::http_client();
        let mut url = Some("https://graph.microsoft.com/v1.0/me/contactFolders?$top=100&$select=id,displayName,parentFolderId".to_string());
        let mut folders = vec![ContactFolder {
            id: DEFAULT_FOLDER_ID.to_string(),
            display_name: "Contacts".to_string(),
            parent_folder_id: None,
        }];
        while let Some(u) = url.take() {
            let json = Self::get_page(&client, &token, &u).await?;
            for f in json["value"].as_array().cloned().unwrap_or_default() {
                folders.push(ContactFolder {
                    id: f["id"].as_str().unwrap_or("").to_string(),
                    display_name: f["displayName"].as_str().unwrap_or("").to_string(),
                    parent_folder_id: f["parentFolderId"].as_str().map(|s| s.to_string()),
                });
            }
            url = json["@odata.nextLink"].as_str().map(|s| s.to_string());
        }
        debug!("Fetched {} contact folders", folders.len());
        Ok(folders)
    }

    async fn fetch_folder_contacts(&self, folder_id: &str, limit: usize) -> Result<Vec<Contact>> {
        let token = self.get_token().await?;
        let client = super::http_client();
        let url = format!(
            "{}?$top={}&$select={}&$orderby=lastModifiedDateTime desc",
            folder_contacts_url(folder_id), limit, SELECT
        );
        let json = Self::get_page(&client, &token, &url).await?;
        Ok(parse_page(&json))
    }

    async fn fetch_all_folder_contacts(
        &self,
        folder_id: &str,
        tx: tokio::sync::mpsc::Sender<Vec<Contact>>,
    ) -> Result<usize> {
        let token = self.get_token().await?;
        let client = super::http_client();
        let mut next_url = Some(format!(
            "{}?$top=50&$select={}&$orderby=lastModifiedDateTime desc",
            folder_contacts_url(folder_id), SELECT
        ));
        let mut total = 0usize;
        while let Some(url) = next_url.take() {
            let json = Self::get_page(&client, &token, &url).await?;
            let batch = parse_page(&json);
            total += batch.len();
            if !batch.is_empty() && tx.send(batch).await.is_err() {
                debug!("Contacts pagination receiver dropped, stopping folder {}", folder_id);
                break;
            }
            next_url = json["@odata.nextLink"].as_str().map(|s| s.to_string());
            tokio::task::yield_now().await;
        }
        debug!("Completed full contacts sync of folder {}: {} contacts", folder_id, total);
        Ok(total)
    }

    async fn is_token_valid(&self) -> Result<bool> {
        let mut auth = self.auth.lock().await;
        let token = auth.get_token().map_err(|_| OmarchyError::AuthError("Token check failed".to_string()));
        Ok(token.is_ok() && !token.unwrap_or_default().is_empty())
    }
}

fn parse_page(json: &serde_json::Value) -> Vec<Contact> {
    json["value"]
        .as_array()
        .map(|v| v.iter().map(parse_contact).filter(|c| !c.id.is_empty()).collect())
        .unwrap_or_default()
}

fn text(v: &serde_json::Value) -> String {
    v.as_str().unwrap_or("").trim().to_string()
}

/// One-line postal address: "street, city, state zip, country" (empty parts skipped).
/// Newlines inside fields collapse to single spaces.
pub fn format_address(a: &serde_json::Value) -> String {
    let clean = |v: &serde_json::Value| text(v).split_whitespace().collect::<Vec<_>>().join(" ");
    let state_zip = [clean(&a["state"]), clean(&a["postalCode"])]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    [clean(&a["street"]), clean(&a["city"]), state_zip, clean(&a["countryOrRegion"])]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn parse_contact(c: &serde_json::Value) -> Contact {
    let given = text(&c["givenName"]);
    let surname = text(&c["surname"]);
    let emails: Vec<String> = c["emailAddresses"]
        .as_array()
        .map(|a| a.iter().map(|e| text(&e["address"])).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();

    let mut phones = Vec::new();
    let mut add_phones = |kind: &str, v: &serde_json::Value| match v {
        serde_json::Value::Array(list) => {
            for n in list.iter().map(text).filter(|s| !s.is_empty()) {
                phones.push(ContactPhone { kind: kind.to_string(), number: n });
            }
        }
        other => {
            let n = text(other);
            if !n.is_empty() {
                phones.push(ContactPhone { kind: kind.to_string(), number: n });
            }
        }
    };
    add_phones("mobile", &c["mobilePhone"]);
    add_phones("home", &c["homePhones"]);
    add_phones("work", &c["businessPhones"]);

    let addresses: Vec<ContactAddress> = [("home", "homeAddress"), ("work", "businessAddress"), ("other", "otherAddress")]
        .iter()
        .map(|(kind, key)| ContactAddress { kind: kind.to_string(), text: format_address(&c[*key]) })
        .filter(|a| !a.text.is_empty())
        .collect();

    // Display name: Graph's own, else "given surname", else first email.
    let mut display = text(&c["displayName"]);
    if display.is_empty() {
        display = format!("{} {}", given, surname).trim().to_string();
    }
    if display.is_empty() {
        display = emails.first().cloned().unwrap_or_else(|| "(no name)".to_string());
    }

    Contact {
        id: text(&c["id"]),
        display_name: display,
        given_name: given,
        surname,
        company: text(&c["companyName"]),
        job_title: text(&c["jobTitle"]),
        emails,
        phones,
        addresses,
        folder_id: c["parentFolderId"].as_str().map(|s| s.to_string()),
        created_at: text(&c["createdDateTime"]),
        modified_at: text(&c["lastModifiedDateTime"]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_a_full_graph_contact() {
        let c = parse_contact(&json!({
            "id": "AAA=", "displayName": "Ada Lovelace", "givenName": "Ada", "surname": "Lovelace",
            "companyName": "Analytical Engines", "jobTitle": "Countess",
            "emailAddresses": [{"name": "Ada", "address": "ada@x.com"}, {"address": "ada@home.org"}, {"address": ""}],
            "mobilePhone": "555-0100", "homePhones": ["555-0101", "555-0102"], "businessPhones": ["555-0103"],
            "homeAddress": {"street": "1 Main St\nApt 2", "city": "London", "state": "", "postalCode": "N1", "countryOrRegion": "UK"},
            "businessAddress": {"street": "", "city": "", "state": "", "postalCode": "", "countryOrRegion": ""},
            "createdDateTime": "2026-01-01T00:00:00Z", "lastModifiedDateTime": "2026-02-01T00:00:00Z",
            "parentFolderId": "F1"
        }));
        assert_eq!(c.emails, vec!["ada@x.com", "ada@home.org"], "blank addresses dropped");
        let phones: Vec<_> = c.phones.iter().map(|p| format!("{}:{}", p.kind, p.number)).collect();
        assert_eq!(phones, vec!["mobile:555-0100", "home:555-0101", "home:555-0102", "work:555-0103"]);
        assert_eq!(c.addresses.len(), 1, "empty business address dropped");
        assert_eq!(c.addresses[0].text, "1 Main St Apt 2, London, N1, UK");
        assert_eq!(c.folder_id.as_deref(), Some("F1"));
        assert_eq!((c.company.as_str(), c.job_title.as_str()), ("Analytical Engines", "Countess"));
    }

    #[test]
    fn display_name_falls_back_to_name_parts_then_email_then_placeholder() {
        let n = |v| parse_contact(&v).display_name;
        assert_eq!(n(json!({"id":"1","givenName":"Bob","surname":"Jones"})), "Bob Jones");
        assert_eq!(n(json!({"id":"2","emailAddresses":[{"address":"x@y.com"}]})), "x@y.com");
        assert_eq!(n(json!({"id":"3"})), "(no name)");
    }

    #[test]
    fn address_formatting_skips_empty_parts() {
        assert_eq!(format_address(&json!({"city":"Paris","countryOrRegion":"FR"})), "Paris, FR");
        assert_eq!(format_address(&json!({"state":"CA","postalCode":"94105"})), "CA 94105");
        assert_eq!(format_address(&json!({})), "");
        assert_eq!(format_address(&serde_json::Value::Null), "");
    }

    #[test]
    fn default_folder_is_read_from_me_contacts_and_others_from_their_folder() {
        assert_eq!(folder_contacts_url(DEFAULT_FOLDER_ID), "https://graph.microsoft.com/v1.0/me/contacts");
        assert_eq!(
            folder_contacts_url("AAMk=="),
            "https://graph.microsoft.com/v1.0/me/contactFolders/AAMk==/contacts"
        );
    }

    #[test]
    fn graph_nulls_are_tolerated() {
        // Real Graph returns null for unset jobTitle/companyName and empty address objects.
        let c = parse_contact(&json!({
            "id": "1", "displayName": "Rick", "givenName": "Rick", "surname": "H", "jobTitle": null, "companyName": null,
            "homePhones": [], "businessPhones": [], "mobilePhone": null, "emailAddresses": [],
            "homeAddress": {}, "businessAddress": {}, "otherAddress": {}
        }));
        assert_eq!((c.company.as_str(), c.job_title.as_str()), ("", ""));
        assert!(c.phones.is_empty() && c.addresses.is_empty() && c.emails.is_empty());
    }

    #[test]
    fn contacts_without_an_id_are_dropped() {
        let page = json!({"value": [{"displayName": "No Id"}, {"id": "ok", "displayName": "Has Id"}]});
        assert_eq!(parse_page(&page).len(), 1);
    }
}
