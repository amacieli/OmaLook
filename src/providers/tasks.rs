//! Microsoft To Do through Graph (`/me/todo`), read side. Needs `Tasks.ReadWrite` (already in the
//! sign-in scope list) or `Tasks.Read`.

use crate::auth::AuthManager;
use crate::db::tasks::{Task, TaskList};
use crate::errors::{OmarchyError, Result};
use async_trait::async_trait;
use tokio::sync::Mutex;

#[async_trait]
pub trait TasksProvider: Send + Sync {
    async fn fetch_lists(&self) -> Result<Vec<TaskList>>;
    /// EVERY task of a list, completed ones included. Err if any page could not be read: the
    /// caller deletes cached tasks absent from the answer, so a partial answer must fail.
    async fn fetch_list_tasks(&self, list_id: &str) -> Result<Vec<Task>>;
    async fn is_token_valid(&self) -> Result<bool>;
}

pub struct GraphTasksProvider {
    auth: Mutex<AuthManager>,
}

const BASE: &str = "https://graph.microsoft.com/v1.0/me/todo/lists";

impl GraphTasksProvider {
    pub fn new(auth: AuthManager) -> Self {
        Self { auth: Mutex::new(auth) }
    }

    async fn get_token(&self) -> Result<String> {
        let mut auth = self.auth.lock().await;
        let token = auth.get_token().map_err(|e| OmarchyError::AuthError(format!("Failed to get token: {}", e)))?;
        if token.is_empty() {
            return Err(OmarchyError::AuthError("No access token".to_string()));
        }
        Ok(token)
    }

    async fn get_page(client: &reqwest::Client, token: &str, url: &str) -> Result<serde_json::Value> {
        let response = client.get(url).header("Authorization", format!("Bearer {}", token)).send().await
            .map_err(|e| OmarchyError::HttpError(e.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(OmarchyError::HttpError(format!("Graph API error: {} — {}", status, body)));
        }
        let text = response.text().await.map_err(|e| OmarchyError::HttpError(e.to_string()))?;
        serde_json::from_str(&text).map_err(|e| OmarchyError::HttpError(e.to_string()))
    }
}

/// The date part of a Graph `dateTimeTimeZone` (`{"dateTime":"2026-11-10T00:00:00.0000000",...}`).
fn date_of(v: &serde_json::Value) -> String {
    v["dateTime"].as_str().map(|s| s.chars().take(10).collect()).unwrap_or_default()
}

pub(crate) fn parse_lists(json: &serde_json::Value) -> Vec<TaskList> {
    json["value"].as_array().cloned().unwrap_or_default().iter().filter_map(|l| {
        Some(TaskList {
            id: l["id"].as_str()?.to_string(),
            name: l["displayName"].as_str().unwrap_or("").to_string(),
            is_default: l["wellknownListName"].as_str() == Some("defaultList"),
        })
    }).collect()
}

pub(crate) fn parse_tasks(json: &serde_json::Value, list_id: &str) -> Vec<Task> {
    json["value"].as_array().cloned().unwrap_or_default().iter().filter_map(|t| {
        Some(Task {
            id: t["id"].as_str()?.to_string(),
            list_id: list_id.to_string(),
            title: t["title"].as_str().unwrap_or("").to_string(),
            notes: t["body"]["content"].as_str().unwrap_or("").to_string(),
            completed: t["status"].as_str() == Some("completed"),
            due: date_of(&t["dueDateTime"]),
            importance: t["importance"].as_str().unwrap_or("normal").to_string(),
            modified_at: t["lastModifiedDateTime"].as_str().unwrap_or("").to_string(),
            completed_at: date_of(&t["completedDateTime"]),
        })
    }).collect()
}

#[async_trait]
impl TasksProvider for GraphTasksProvider {
    async fn fetch_lists(&self) -> Result<Vec<TaskList>> {
        let token = self.get_token().await?;
        let client = super::http_client();
        let mut url = Some(format!("{}?$top=100", BASE));
        let mut out = Vec::new();
        while let Some(u) = url.take() {
            let json = Self::get_page(&client, &token, &u).await?;
            out.extend(parse_lists(&json));
            url = json["@odata.nextLink"].as_str().map(String::from);
        }
        Ok(out)
    }

    async fn fetch_list_tasks(&self, list_id: &str) -> Result<Vec<Task>> {
        let token = self.get_token().await?;
        let client = super::http_client();
        let mut url = Some(format!("{}/{}/tasks?$top=100", BASE, urlencoding::encode(list_id)));
        let mut out = Vec::new();
        while let Some(u) = url.take() {
            let json = Self::get_page(&client, &token, &u).await?;
            out.extend(parse_tasks(&json, list_id));
            url = json["@odata.nextLink"].as_str().map(String::from);
            tokio::task::yield_now().await;
        }
        Ok(out)
    }

    async fn is_token_valid(&self) -> Result<bool> {
        let mut auth = self.auth.lock().await;
        let token = auth.get_token().map_err(|_| OmarchyError::AuthError("Token check failed".to_string()));
        Ok(token.is_ok() && !token.unwrap_or_default().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_lists_and_tasks_parse() {
        let lists = parse_lists(&serde_json::json!({"value":[
            {"id":"A","displayName":"Tasks","wellknownListName":"defaultList"},
            {"id":"B","displayName":"Home","wellknownListName":"none"}]}));
        assert_eq!(lists.len(), 2);
        assert!(lists[0].is_default && !lists[1].is_default);
        let tasks = parse_tasks(&serde_json::json!({"value":[
            {"id":"1","title":"Pay","status":"notStarted","importance":"high",
             "dueDateTime":{"dateTime":"2026-11-12T00:00:00.0000000","timeZone":"UTC"},"body":{"content":"n"},"lastModifiedDateTime":"2026-11-01T00:00:00Z"},
            {"id":"2","title":"Done","status":"completed","completedDateTime":{"dateTime":"2026-11-03T10:00:00.0000000","timeZone":"UTC"}}]}), "A");
        assert_eq!(tasks[0].due, "2026-11-12");
        assert!(!tasks[0].completed && tasks[1].completed);
        assert_eq!(tasks[1].completed_at, "2026-11-03");
        assert_eq!(tasks[1].importance, "normal");
    }
}
