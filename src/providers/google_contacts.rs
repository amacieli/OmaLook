//! Google contacts provider (People API `people.connections`) behind the same
//! `ContactsProvider` trait as Graph.
//!
//! Google "labels" (contact groups) are multi-membership, while the app's contact folders are
//! exclusive. So the default folder "Contacts" fetches every contact once and files each under
//! its first user label (or the default folder when it has none); the label folders themselves
//! only exist for display and fetch nothing. Ids are stored as `<account id>:<id>`.

use super::google_api::GoogleApi;
use crate::accounts::{scoped, unscoped};
use crate::errors::Result;
use crate::models::{Contact, ContactAddress, ContactFolder, ContactPhone};
use async_trait::async_trait;
use log::debug;
use std::collections::HashSet;
use std::sync::Mutex;

const PEOPLE: &str = "https://people.googleapis.com/v1";
const FIELDS: &str = "names,emailAddresses,phoneNumbers,organizations,addresses,memberships,metadata";
const PAGE_SIZE: usize = 100;
/// Raw id of the default folder (same pseudo id the Graph provider uses).
const DEFAULT_RAW: &str = "contacts";

pub struct GoogleContactsProvider {
    account_id: String,
    api: GoogleApi,
    /// Resource names of the user's own labels, learned in `fetch_folders`.
    user_groups: Mutex<HashSet<String>>,
}

fn phone_kind(t: &str) -> String {
    match t.to_ascii_lowercase().as_str() {
        "mobile" | "iphone" => "mobile",
        "home" | "homefax" => "home",
        "work" | "workfax" | "workmobile" | "main" => "work",
        _ => "other",
    }
    .to_string()
}

/// People API `person` → `Contact` (None when it has neither a name nor an email address).
pub(crate) fn parse_person(account_id: &str, user_groups: &HashSet<String>, p: &serde_json::Value) -> Option<Contact> {
    let resource = p["resourceName"].as_str().filter(|s| !s.is_empty())?;
    let name = &p["names"][0];
    let org = &p["organizations"][0];
    let emails: Vec<String> = p["emailAddresses"].as_array().into_iter().flatten()
        .filter_map(|e| e["value"].as_str().map(String::from)).collect();
    let display_name = name["displayName"].as_str().unwrap_or("").to_string();
    if display_name.is_empty() && emails.is_empty() {
        return None;
    }
    let phones = p["phoneNumbers"].as_array().into_iter().flatten()
        .filter_map(|n| Some(ContactPhone { kind: phone_kind(n["type"].as_str().unwrap_or("")), number: n["value"].as_str()?.to_string() }))
        .collect();
    let addresses = p["addresses"].as_array().into_iter().flatten()
        .filter_map(|a| {
            let text = a["formattedValue"].as_str()?.replace('\n', ", ");
            Some(ContactAddress { kind: a["type"].as_str().unwrap_or("other").to_ascii_lowercase(), text })
        })
        .collect();
    let group = p["memberships"].as_array().into_iter().flatten()
        .filter_map(|m| m["contactGroupMembership"]["contactGroupResourceName"].as_str())
        .find(|g| user_groups.contains(*g));
    let folder = scoped(account_id, group.unwrap_or(DEFAULT_RAW));
    // People API exposes no creation time; the source's update time is the closest stamp.
    let stamp = p["metadata"]["sources"][0]["updateTime"].as_str().unwrap_or("").to_string();
    Some(Contact {
        id: scoped(account_id, resource),
        display_name: if display_name.is_empty() { emails[0].clone() } else { display_name },
        given_name: name["givenName"].as_str().unwrap_or("").to_string(),
        surname: name["familyName"].as_str().unwrap_or("").to_string(),
        company: org["name"].as_str().unwrap_or("").to_string(),
        job_title: org["title"].as_str().unwrap_or("").to_string(),
        emails,
        phones,
        addresses,
        folder_id: Some(folder),
        created_at: stamp.clone(),
        modified_at: stamp,
        ..Default::default()
    })
}

impl GoogleContactsProvider {
    pub fn new(account_id: &str) -> Self {
        Self { account_id: account_id.to_string(), api: GoogleApi::new(account_id), user_groups: Mutex::new(HashSet::new()) }
    }

    fn is_default(&self, folder_id: &str) -> bool {
        unscoped(&self.account_id, folder_id) == DEFAULT_RAW
    }

    fn parse_page(&self, json: &serde_json::Value) -> Vec<Contact> {
        let groups = self.user_groups.lock().unwrap_or_else(|p| p.into_inner()).clone();
        json["connections"].as_array().into_iter().flatten().filter_map(|p| parse_person(&self.account_id, &groups, p)).collect()
    }

    fn connections_url(page_size: usize, token: Option<&str>) -> String {
        let mut url = format!(
            "{}/people/me/connections?personFields={}&pageSize={}&sortOrder=LAST_MODIFIED_DESCENDING",
            PEOPLE, FIELDS, page_size
        );
        if let Some(t) = token {
            url.push_str(&format!("&pageToken={}", urlencoding::encode(t)));
        }
        url
    }
}

#[async_trait]
impl super::ContactsProvider for GoogleContactsProvider {
    async fn fetch_folders(&self) -> Result<Vec<ContactFolder>> {
        let mut folders = vec![ContactFolder {
            id: scoped(&self.account_id, DEFAULT_RAW),
            display_name: "Contacts".to_string(),
            parent_folder_id: None,
        }];
        let (mut groups, mut token) = (HashSet::new(), None::<String>);
        loop {
            let mut url = format!("{}/contactGroups?pageSize=200", PEOPLE);
            if let Some(t) = &token {
                url.push_str(&format!("&pageToken={}", urlencoding::encode(t)));
            }
            let json = self.api.get(&url).await?;
            for g in json["contactGroups"].as_array().into_iter().flatten() {
                if g["groupType"].as_str() != Some("USER_CONTACT_GROUP") { continue; }
                let (Some(rn), Some(name)) = (g["resourceName"].as_str(), g["name"].as_str()) else { continue };
                groups.insert(rn.to_string());
                folders.push(ContactFolder { id: scoped(&self.account_id, rn), display_name: name.to_string(), parent_folder_id: None });
            }
            match json["nextPageToken"].as_str() {
                Some(t) => token = Some(t.to_string()),
                None => break,
            }
        }
        *self.user_groups.lock().unwrap_or_else(|p| p.into_inner()) = groups;
        debug!("Fetched {} Google contact folders", folders.len());
        Ok(folders)
    }

    async fn fetch_folder_contacts(&self, folder_id: &str, limit: usize) -> Result<Vec<Contact>> {
        if !self.is_default(folder_id) {
            return Ok(Vec::new()); // label folders are filled while syncing the default folder
        }
        let json = self.api.get(&Self::connections_url(limit.clamp(1, 1000), None)).await?;
        Ok(self.parse_page(&json))
    }

    async fn fetch_all_folder_contacts(&self, folder_id: &str, tx: tokio::sync::mpsc::Sender<Vec<Contact>>) -> Result<usize> {
        if !self.is_default(folder_id) {
            return Ok(0);
        }
        let (mut total, mut token) = (0usize, None::<String>);
        loop {
            let json = self.api.get(&Self::connections_url(PAGE_SIZE, token.as_deref())).await?;
            let batch = self.parse_page(&json);
            total += batch.len();
            if !batch.is_empty() && tx.send(batch).await.is_err() {
                debug!("Google contacts pagination receiver dropped, stopping");
                break;
            }
            match json["nextPageToken"].as_str() {
                Some(t) => token = Some(t.to_string()),
                None => break,
            }
            tokio::task::yield_now().await;
        }
        debug!("Completed full Google contacts sync: {}", total);
        Ok(total)
    }

    async fn is_token_valid(&self) -> Result<bool> {
        self.api.is_token_valid().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn groups() -> HashSet<String> {
        ["contactGroups/abc".to_string()].into_iter().collect()
    }

    #[test]
    fn person_maps_to_a_scoped_contact() {
        let p = json!({
            "resourceName": "people/c123",
            "names": [{"displayName": "Ada Lovelace", "givenName": "Ada", "familyName": "Lovelace"}],
            "emailAddresses": [{"value": "ada@example.com"}, {"value": "ada@work.org"}],
            "phoneNumbers": [{"value": "+1 555 0100", "type": "mobile"}, {"value": "555 0101", "type": "workFax"}],
            "organizations": [{"name": "Analytical Engines", "title": "Programmer"}],
            "addresses": [{"formattedValue": "1 Main St\nLondon", "type": "home"}],
            "memberships": [
                {"contactGroupMembership": {"contactGroupResourceName": "contactGroups/myContacts"}},
                {"contactGroupMembership": {"contactGroupResourceName": "contactGroups/abc"}}],
            "metadata": {"sources": [{"updateTime": "2026-09-01T10:00:00Z"}]},
        });
        let c = parse_person("gmail-aaaaaa", &groups(), &p).unwrap();
        assert_eq!(c.id, "gmail-aaaaaa:people/c123");
        assert_eq!((c.given_name.as_str(), c.surname.as_str(), c.company.as_str(), c.job_title.as_str()), ("Ada", "Lovelace", "Analytical Engines", "Programmer"));
        assert_eq!(c.emails, ["ada@example.com", "ada@work.org"]);
        assert_eq!(c.phones[0], ContactPhone { kind: "mobile".into(), number: "+1 555 0100".into() });
        assert_eq!(c.phones[1].kind, "work");
        assert_eq!(c.addresses[0], ContactAddress { kind: "home".into(), text: "1 Main St, London".into() });
        assert_eq!(c.folder_id.as_deref(), Some("gmail-aaaaaa:contactGroups/abc"), "first USER label wins");
        assert_eq!(c.modified_at, "2026-09-01T10:00:00Z");
    }

    #[test]
    fn contact_without_a_user_label_goes_to_the_default_folder() {
        let p = json!({"resourceName": "people/c9", "names": [{"displayName": "Bob"}],
            "memberships": [{"contactGroupMembership": {"contactGroupResourceName": "contactGroups/myContacts"}}]});
        assert_eq!(parse_person("gmail-aaaaaa", &groups(), &p).unwrap().folder_id.as_deref(), Some("gmail-aaaaaa:contacts"));
    }

    #[test]
    fn nameless_contacts_fall_back_to_email_or_are_dropped() {
        let only_email = json!({"resourceName": "people/c1", "emailAddresses": [{"value": "x@y.com"}]});
        assert_eq!(parse_person("a-1", &groups(), &only_email).unwrap().display_name, "x@y.com");
        assert!(parse_person("a-1", &groups(), &json!({"resourceName": "people/c2"})).is_none());
        assert!(parse_person("a-1", &groups(), &json!({})).is_none());
    }

    #[test]
    fn same_google_contact_in_two_accounts_stays_distinct() {
        let p = json!({"resourceName": "people/c1", "names": [{"displayName": "A"}]});
        assert_ne!(parse_person("gmail-aaaaaa", &groups(), &p).unwrap().id, parse_person("gmail-bbbbbb", &groups(), &p).unwrap().id);
    }
}
