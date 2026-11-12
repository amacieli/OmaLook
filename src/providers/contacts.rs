//! Microsoft Graph contacts provider (mirrors providers/graph.rs for mail: contact
//! folders play the role of mail folders, contacts the role of messages).
//!
//! Reading needs `Contacts.Read`; creating, changing and deleting needs `Contacts.ReadWrite`
//! (the full scope tier; an account on the base tier gets a 403, which the callers turn into
//! "sign in again to allow editing"). Facts below were checked live on 2026-11-11
//! (tools/spike/contacts_spike.py, docs/PLAN-contacts.md section 2).

use crate::auth::AuthManager;
use crate::errors::{OmarchyError, Result};
use crate::models::{Contact, ContactAddress, ContactDeltaPage, ContactFolder, ContactPhone};
use async_trait::async_trait;
use log::{debug, error};
use tokio::sync::Mutex;

/// Old pseudo id of the user's default Contacts folder. Graph does NOT list that folder under
/// `/me/contactFolders` (it returns only the folders beneath it), and the pseudo id collided
/// between two Graph accounts in one database. The default folder now carries its real id
/// (read from `/me/contactFolders/contacts`), so every folder id is unique across accounts.
/// The constant stays so old cached rows and the URL helper still make sense.
pub const DEFAULT_FOLDER_ID: &str = "contacts";

fn folder_contacts_url(folder_id: &str) -> String {
    if folder_id == DEFAULT_FOLDER_ID {
        "https://graph.microsoft.com/v1.0/me/contacts".to_string()
    } else {
        format!("https://graph.microsoft.com/v1.0/me/contactFolders/{}/contacts", folder_id)
    }
}

const GRAPH: &str = "https://graph.microsoft.com/v1.0";

/// How a folder delta read ends: the link to ask next time.
pub type DeltaLink = String;

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

    // ---- change tracking and writes (Graph only; the defaults refuse) -------------------------

    /// True when `fetch_delta` works (the daemon then syncs by delta).
    fn supports_delta(&self) -> bool { false }

    /// Delta read of one folder: with `link` None the whole folder (every contact, full data);
    /// with a stored link only the changes since. Each page goes to `tx` as it arrives; the
    /// result is the link to store. A stale link is an Err (the caller starts again without one).
    async fn fetch_delta(&self, _folder_id: &str, _link: Option<&str>, _tx: tokio::sync::mpsc::Sender<ContactDeltaPage>) -> Result<DeltaLink> {
        Err(OmarchyError::HttpError("contact delta not supported".into()))
    }

    /// One contact, fresh from the server, as the provider's own JSON (with `@odata.etag`).
    async fn fetch_contact(&self, _id: &str) -> Result<serde_json::Value> {
        Err(OmarchyError::HttpError("fetch_contact not supported".into()))
    }

    /// Create a contact in a folder; returns the stored contact (JSON).
    async fn create_contact(&self, _folder_id: &str, _body: &serde_json::Value) -> Result<serde_json::Value> {
        Err(OmarchyError::HttpError("create_contact not supported".into()))
    }

    /// Change a contact (PATCH of only the keys in `body`). `if_match` is the `@odata.etag` the caller saw:
    /// a changed contact answers 412. Returns the stored contact.
    async fn update_contact(&self, _id: &str, _body: &serde_json::Value, _if_match: Option<&str>) -> Result<serde_json::Value> {
        Err(OmarchyError::HttpError("update_contact not supported".into()))
    }

    /// Delete a contact (to Deleted Items, or for good when `permanent`).
    async fn delete_contact(&self, _id: &str, _permanent: bool) -> Result<()> {
        Err(OmarchyError::HttpError("delete_contact not supported".into()))
    }

    async fn create_folder(&self, _parent_id: Option<&str>, _name: &str) -> Result<ContactFolder> {
        Err(OmarchyError::HttpError("create_folder not supported".into()))
    }
    async fn rename_folder(&self, _id: &str, _name: &str) -> Result<()> {
        Err(OmarchyError::HttpError("rename_folder not supported".into()))
    }
    async fn delete_folder(&self, _id: &str) -> Result<()> {
        Err(OmarchyError::HttpError("delete_folder not supported".into()))
    }

    /// The contact's photo bytes (None when it has none).
    async fn get_photo(&self, _id: &str) -> Result<Option<Vec<u8>>> {
        Err(OmarchyError::HttpError("get_photo not supported".into()))
    }
    async fn set_photo(&self, _id: &str, _jpeg: &[u8]) -> Result<()> {
        Err(OmarchyError::HttpError("set_photo not supported".into()))
    }

    /// People in the organisation's directory matching what was typed (Graph: `/me/people` and `/users`).
    async fn directory_search(&self, _query: &str) -> Result<Vec<crate::models::DirectoryEntry>> {
        Err(OmarchyError::HttpError("directory search not supported".into()))
    }

    /// Read the MAPI single-value properties named by `ids` (`"String 0x3A23"`) of a contact.
    async fn get_extended(&self, _id: &str, _ids: &[&str]) -> Result<Vec<(String, String)>> {
        Err(OmarchyError::HttpError("get_extended not supported".into()))
    }
}

pub struct GraphContactsProvider {
    auth: Mutex<AuthManager>,
}

/// The status code inside an error made by this provider (`"Graph API error: 412 ..."`).
pub fn error_status(e: &OmarchyError) -> Option<u16> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"(?:Graph|Google) API error: (\d{3})").unwrap());
    match e {
        OmarchyError::HttpError(m) => re.captures(m).and_then(|c| c[1].parse().ok()),
        _ => None,
    }
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

    /// One request; a non-2xx answer is `HttpError("Graph API error: <status> <hint> — <body>")`.
    /// `body` is sent as JSON; `extra` are added headers. An empty answer is `Value::Null`.
    async fn request(
        client: &reqwest::Client,
        token: &str,
        method: reqwest::Method,
        url: &str,
        body: Option<&serde_json::Value>,
        extra: &[(&str, String)],
    ) -> Result<serde_json::Value> {
        let mut rb = client.request(method, url).header("Authorization", format!("Bearer {}", token));
        for (k, v) in extra {
            rb = rb.header(*k, v);
        }
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let response = rb.send().await.map_err(|e| OmarchyError::HttpError(e.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            error!("Graph contacts API returned status: {} — {}", status, text);
            let hint = if status.as_u16() == 403 {
                " (Contacts.ReadWrite not granted: log this account in again to consent)"
            } else {
                ""
            };
            return Err(OmarchyError::HttpError(format!("Graph API error: {}{} — {}", status.as_u16(), hint, text)));
        }
        let text = response.text().await.map_err(|e| OmarchyError::HttpError(e.to_string()))?;
        if text.trim().is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_str(&text).map_err(|e| OmarchyError::HttpError(e.to_string()))
    }

    async fn get_page(client: &reqwest::Client, token: &str, url: &str) -> Result<serde_json::Value> {
        Self::request(client, token, reqwest::Method::GET, url, None, &[]).await
    }

    /// The real id of the default Contacts folder.
    async fn default_folder_id(client: &reqwest::Client, token: &str) -> Result<String> {
        let json = Self::get_page(client, token, &format!("{}/me/contactFolders/contacts?$select=id", GRAPH)).await?;
        json["id"].as_str().map(|s| s.to_string()).ok_or_else(|| OmarchyError::HttpError("the default contacts folder has no id".into()))
    }

    /// `folder` and everything beneath it (depth-limited), appended to `out`.
    async fn collect_children(client: &reqwest::Client, token: &str, parent: &str, depth: u8, out: &mut Vec<ContactFolder>) -> Result<()> {
        if depth == 0 {
            return Ok(());
        }
        let mut url = Some(format!("{}/me/contactFolders/{}/childFolders?$top=100&$select=id,displayName,parentFolderId", GRAPH, parent));
        while let Some(u) = url.take() {
            let json = Self::get_page(client, token, &u).await?;
            for f in json["value"].as_array().cloned().unwrap_or_default() {
                let id = f["id"].as_str().unwrap_or("").to_string();
                if id.is_empty() {
                    continue;
                }
                out.push(ContactFolder {
                    id: id.clone(),
                    display_name: f["displayName"].as_str().unwrap_or("").to_string(),
                    parent_folder_id: f["parentFolderId"].as_str().map(|s| s.to_string()),
                });
                Box::pin(Self::collect_children(client, token, &id, depth - 1, out)).await?;
            }
            url = json["@odata.nextLink"].as_str().map(|s| s.to_string());
        }
        Ok(())
    }
}

#[async_trait]
impl ContactsProvider for GraphContactsProvider {
    async fn fetch_folders(&self) -> Result<Vec<ContactFolder>> {
        let token = self.get_token().await?;
        let client = super::http_client();
        let default_id = Self::default_folder_id(&client, &token).await?;
        let mut folders = vec![ContactFolder { id: default_id.clone(), display_name: "Contacts".to_string(), parent_folder_id: None }];
        Self::collect_children(&client, &token, &default_id, 5, &mut folders).await?;
        // Folders beside the default one (listed by /me/contactFolders) are its children in Graph's model.
        debug!("Fetched {} contact folders", folders.len());
        Ok(folders)
    }

    async fn fetch_folder_contacts(&self, folder_id: &str, limit: usize) -> Result<Vec<Contact>> {
        let token = self.get_token().await?;
        let client = super::http_client();
        let url = format!("{}?$top={}&$orderby=lastModifiedDateTime desc", folder_contacts_url(folder_id), limit);
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
        let mut next_url = Some(format!("{}?$top=50&$orderby=lastModifiedDateTime desc", folder_contacts_url(folder_id)));
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

    fn supports_delta(&self) -> bool { true }

    async fn fetch_delta(&self, folder_id: &str, link: Option<&str>, tx: tokio::sync::mpsc::Sender<ContactDeltaPage>) -> Result<DeltaLink> {
        let token = self.get_token().await?;
        let client = super::http_client();
        // $select / $top / $filter are refused on a contact delta (checked live); the page size is a header.
        let mut next = Some(match link {
            Some(l) => l.to_string(),
            None => format!("{}/me/contactFolders/{}/contacts/delta", GRAPH, folder_id),
        });
        let prefer = [("Prefer", "odata.maxpagesize=100".to_string())];
        loop {
            let url = next.take().ok_or_else(|| OmarchyError::HttpError("contact delta ended without a link".into()))?;
            let json = Self::request(&client, &token, reqwest::Method::GET, &url, None, &prefer).await?;
            let page = parse_delta_page(&json);
            if (!page.upserts.is_empty() || !page.removed.is_empty()) && tx.send(page).await.is_err() {
                return Err(OmarchyError::HttpError("contact delta: receiver dropped".into()));
            }
            if let Some(n) = json["@odata.nextLink"].as_str() {
                next = Some(n.to_string());
                tokio::task::yield_now().await;
            } else if let Some(d) = json["@odata.deltaLink"].as_str() {
                return Ok(d.to_string());
            } else {
                return Err(OmarchyError::HttpError("contact delta page has neither a next nor a delta link".into()));
            }
        }
    }

    async fn fetch_contact(&self, id: &str) -> Result<serde_json::Value> {
        let token = self.get_token().await?;
        let client = super::http_client();
        Self::get_page(&client, &token, &format!("{}/me/contacts/{}", GRAPH, id)).await
    }

    async fn create_contact(&self, folder_id: &str, body: &serde_json::Value) -> Result<serde_json::Value> {
        let token = self.get_token().await?;
        let client = super::http_client();
        let url = format!("{}/me/contactFolders/{}/contacts", GRAPH, folder_id);
        Self::request(&client, &token, reqwest::Method::POST, &url, Some(body), &[]).await
    }

    async fn update_contact(&self, id: &str, body: &serde_json::Value, if_match: Option<&str>) -> Result<serde_json::Value> {
        let token = self.get_token().await?;
        let client = super::http_client();
        let url = format!("{}/me/contacts/{}", GRAPH, id);
        let extra: Vec<(&str, String)> = if_match.map(|e| vec![("If-Match", e.to_string())]).unwrap_or_default();
        Self::request(&client, &token, reqwest::Method::PATCH, &url, Some(body), &extra).await
    }

    async fn delete_contact(&self, id: &str, permanent: bool) -> Result<()> {
        let token = self.get_token().await?;
        let client = super::http_client();
        let r = if permanent {
            Self::request(&client, &token, reqwest::Method::POST, &format!("{}/me/contacts/{}/permanentDelete", GRAPH, id), None, &[]).await
        } else {
            Self::request(&client, &token, reqwest::Method::DELETE, &format!("{}/me/contacts/{}", GRAPH, id), None, &[]).await
        };
        match r {
            Ok(_) => Ok(()),
            // Already gone is the state that was asked for.
            Err(e) if error_status(&e) == Some(404) => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn create_folder(&self, parent_id: Option<&str>, name: &str) -> Result<ContactFolder> {
        let token = self.get_token().await?;
        let client = super::http_client();
        let url = match parent_id {
            Some(p) => format!("{}/me/contactFolders/{}/childFolders", GRAPH, p),
            None => format!("{}/me/contactFolders", GRAPH),
        };
        let json = Self::request(&client, &token, reqwest::Method::POST, &url, Some(&serde_json::json!({ "displayName": name })), &[]).await?;
        Ok(ContactFolder {
            id: json["id"].as_str().unwrap_or("").to_string(),
            display_name: json["displayName"].as_str().unwrap_or(name).to_string(),
            parent_folder_id: json["parentFolderId"].as_str().map(|s| s.to_string()),
        })
    }

    async fn rename_folder(&self, id: &str, name: &str) -> Result<()> {
        let token = self.get_token().await?;
        let client = super::http_client();
        Self::request(&client, &token, reqwest::Method::PATCH, &format!("{}/me/contactFolders/{}", GRAPH, id), Some(&serde_json::json!({ "displayName": name })), &[]).await?;
        Ok(())
    }

    async fn delete_folder(&self, id: &str) -> Result<()> {
        let token = self.get_token().await?;
        let client = super::http_client();
        match Self::request(&client, &token, reqwest::Method::DELETE, &format!("{}/me/contactFolders/{}", GRAPH, id), None, &[]).await {
            Ok(_) => Ok(()),
            Err(e) if error_status(&e) == Some(404) => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn get_photo(&self, id: &str) -> Result<Option<Vec<u8>>> {
        let token = self.get_token().await?;
        let client = super::http_client();
        let resp = client
            .get(format!("{}/me/contacts/{}/photo/$value", GRAPH, id))
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| OmarchyError::HttpError(e.to_string()))?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(OmarchyError::HttpError(format!("Graph API error: {} — photo", resp.status().as_u16())));
        }
        Ok(Some(resp.bytes().await.map_err(|e| OmarchyError::HttpError(e.to_string()))?.to_vec()))
    }

    async fn set_photo(&self, id: &str, jpeg: &[u8]) -> Result<()> {
        let token = self.get_token().await?;
        let client = super::http_client();
        let resp = client
            .put(format!("{}/me/contacts/{}/photo/$value", GRAPH, id))
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "image/jpeg")
            .body(jpeg.to_vec())
            .send()
            .await
            .map_err(|e| OmarchyError::HttpError(e.to_string()))?;
        if !resp.status().is_success() {
            let st = resp.status().as_u16();
            let t = resp.text().await.unwrap_or_default();
            return Err(OmarchyError::HttpError(format!("Graph API error: {} — {}", st, t)));
        }
        Ok(())
    }

    async fn directory_search(&self, query: &str) -> Result<Vec<crate::models::DirectoryEntry>> {
        let q = query.trim();
        if q.chars().count() < 2 {
            return Ok(vec![]);
        }
        let token = self.get_token().await?;
        let client = super::http_client();
        let enc = |t: &str| -> String { t.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{:02X}", b) }).collect() };
        // people the user works with first (ranked by Graph), then the directory
        let people_url = format!("{}/me/people?$search={}&$top=15", GRAPH, enc(&format!("\"{}\"", q.replace('"', ""))));
        let lit = q.replace('\'', "''");
        let filter = format!("startswith(displayName,'{lit}') or startswith(mail,'{lit}') or startswith(surname,'{lit}') or startswith(givenName,'{lit}')");
        let users_url = format!("{}/users?$filter={}&$top=15&$select=displayName,mail,jobTitle,companyName,department", GRAPH, enc(&filter));
        let people = Self::get_page(&client, &token, &people_url).await.unwrap_or(serde_json::Value::Null);
        let users = Self::get_page(&client, &token, &users_url).await;
        if people.is_null() && users.is_err() {
            return Err(users.unwrap_err());
        }
        Ok(parse_directory(&people, &users.unwrap_or(serde_json::Value::Null)))
    }

    async fn get_extended(&self, id: &str, ids: &[&str]) -> Result<Vec<(String, String)>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let token = self.get_token().await?;
        let client = super::http_client();
        let filter = ids.iter().map(|i| format!("id eq '{}'", i)).collect::<Vec<_>>().join(" or ");
        let url = format!("{}/me/contacts/{}?$expand=singleValueExtendedProperties($filter={})&$select=id", GRAPH, id, filter);
        let json = Self::get_page(&client, &token, &url).await?;
        Ok(json["singleValueExtendedProperties"]
            .as_array()
            .map(|a| a.iter().filter_map(|p| Some((p["id"].as_str()?.to_string(), p["value"].as_str()?.to_string()))).collect())
            .unwrap_or_default())
    }
}

/// People and directory answers merged: one entry per address, people first, no one without an address.
pub fn parse_directory(people: &serde_json::Value, users: &serde_json::Value) -> Vec<crate::models::DirectoryEntry> {
    let mut out: Vec<crate::models::DirectoryEntry> = vec![];
    let mut push = |name: &str, email: &str, title: &str, company: &str| {
        let e = email.trim().to_lowercase();
        if e.is_empty() || out.iter().any(|x| x.email.to_lowercase() == e) { return; }
        out.push(crate::models::DirectoryEntry { name: name.trim().to_string(), email: email.trim().to_string(), title: title.trim().to_string(), company: company.trim().to_string() });
    };
    for p in people["value"].as_array().into_iter().flatten() {
        let email = p["scoredEmailAddresses"].as_array().and_then(|a| a.first()).and_then(|e| e["address"].as_str()).unwrap_or("");
        push(p["displayName"].as_str().unwrap_or(""), email, p["jobTitle"].as_str().unwrap_or(""), p["companyName"].as_str().unwrap_or(""));
    }
    for u in users["value"].as_array().into_iter().flatten() {
        push(u["displayName"].as_str().unwrap_or(""), u["mail"].as_str().unwrap_or(""), u["jobTitle"].as_str().unwrap_or(""), u["companyName"].as_str().unwrap_or(""));
    }
    out
}

/// What a delta page holds: contacts to store, ids to drop (`"@removed"`).
fn parse_delta_page(json: &serde_json::Value) -> ContactDeltaPage {
    let mut page = ContactDeltaPage::default();
    for item in json["value"].as_array().into_iter().flatten() {
        let id = item["id"].as_str().unwrap_or("");
        if id.is_empty() {
            continue;
        }
        if item.get("@removed").is_some() {
            page.removed.push(id.to_string());
        } else {
            page.upserts.push(parse_contact(item));
        }
    }
    page
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
        raw: {
            let mut keep = c.clone();
            if let Some(o) = keep.as_object_mut() {
                o.remove("@odata.context");
            }
            keep.to_string()
        },
        change_key: text(&c["changeKey"]),
        categories: c["categories"].as_array().map(|a| a.iter().map(text).filter(|s| !s.is_empty()).collect()).unwrap_or_default(),
    }
}

/// Lower-case text the People search looks in: every name part, address (with its display name), phone,
/// IM address, postal address, company/work field, personal field, note and category of the contact.
pub fn search_text(c: &serde_json::Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    for k in [
        "displayName", "title", "givenName", "middleName", "surname", "generation", "nickName", "fileAs", "initials",
        "companyName", "department", "jobTitle", "officeLocation", "profession", "manager", "assistantName",
        "spouseName", "personalNotes", "businessHomePage", "mobilePhone", "yomiGivenName", "yomiSurname", "yomiCompanyName",
    ] {
        let t = text(&c[k]);
        if !t.is_empty() {
            parts.push(t);
        }
    }
    for e in c["emailAddresses"].as_array().into_iter().flatten() {
        parts.push(text(&e["address"]));
        parts.push(text(&e["name"]));
    }
    for k in ["businessPhones", "homePhones", "imAddresses", "children", "categories"] {
        for v in c[k].as_array().into_iter().flatten() {
            parts.push(text(v));
        }
    }
    for k in ["homeAddress", "businessAddress", "otherAddress"] {
        parts.push(format_address(&c[k]));
    }
    parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n").to_lowercase()
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

    #[test]
    fn keeps_the_whole_contact_the_version_and_the_categories() {
        let c = parse_contact(&json!({
            "@odata.context": "x", "@odata.etag": "W/\"abc\"", "id": "1", "changeKey": "CK1", "displayName": "A",
            "categories": ["OL: Family", "", "Friends"], "nickName": "Al", "spouseName": "B"
        }));
        let raw: serde_json::Value = serde_json::from_str(&c.raw).unwrap();
        assert_eq!(raw["nickName"], "Al");
        assert_eq!(raw["@odata.etag"], "W/\"abc\"");
        assert!(raw.get("@odata.context").is_none(), "the response envelope is not kept");
        assert_eq!(c.change_key, "CK1");
        assert_eq!(c.categories, vec!["OL: Family", "Friends"]);
    }

    #[test]
    fn search_text_covers_names_addresses_phones_notes_and_categories() {
        let t = search_text(&json!({
            "displayName": "Ada Lovelace", "emailAddresses": [{"name": "Countess A", "address": "Ada@X.com"}],
            "businessPhones": ["555-0103"], "homeAddress": {"city": "London"}, "personalNotes": "Met at the Engine",
            "categories": ["OL: Family"], "companyName": "Analytical", "imAddresses": ["ada@im"]
        }));
        for needle in ["ada lovelace", "ada@x.com", "countess a", "555-0103", "london", "met at the engine", "ol: family", "analytical", "ada@im"] {
            assert!(t.contains(needle), "missing {needle:?} in {t:?}");
        }
        assert_eq!(t, t.to_lowercase());
    }

    #[test]
    fn delta_pages_split_into_contacts_and_removals() {
        let page = parse_delta_page(&json!({"value": [
            {"id": "a", "displayName": "A"},
            {"id": "b", "@removed": {"reason": "deleted"}},
            {"displayName": "no id"}
        ]}));
        assert_eq!(page.upserts.len(), 1);
        assert_eq!(page.removed, vec!["b"]);
    }

    #[test]
    fn directory_answers_merge_people_first_without_duplicates_or_addressless_entries() {
        let people = serde_json::json!({ "value": [
            { "displayName": "Ann Lee", "jobTitle": "CFO", "scoredEmailAddresses": [{ "address": "ann@acme.test" }] },
            { "displayName": "No Mail", "scoredEmailAddresses": [] } ] });
        let users = serde_json::json!({ "value": [
            { "displayName": "Ann Lee (dir)", "mail": "ANN@acme.test", "jobTitle": "x" },
            { "displayName": "Bo Ray", "mail": "bo@acme.test", "companyName": "Acme" },
            { "displayName": "Room 4", "mail": null } ] });
        let d = parse_directory(&people, &users);
        assert_eq!(d.iter().map(|e| e.email.as_str()).collect::<Vec<_>>(), vec!["ann@acme.test", "bo@acme.test"]);
        assert_eq!((d[0].name.as_str(), d[0].title.as_str()), ("Ann Lee", "CFO"), "the people answer wins");
        assert_eq!(d[1].company, "Acme");
        assert!(parse_directory(&serde_json::Value::Null, &serde_json::Value::Null).is_empty());
    }

    #[test]
    fn status_codes_are_read_back_out_of_an_error() {
        let e = OmarchyError::HttpError("Graph API error: 412 — {\"error\":{}}".into());
        assert_eq!(error_status(&e), Some(412));
        assert_eq!(error_status(&OmarchyError::HttpError("timed out".into())), None);
    }
}
