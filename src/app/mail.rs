//! Mail read service (R3.5): what the MCP mail tools answer, from the local cache only. Nothing
//! here calls a provider. Callers pass the accounts they may see; this module does no policy.

use super::{clip, Page, LIST_PREVIEW, MAX_TEXT, NOT_FOUND};
use crate::attachments::Attachment;
use crate::db::{categories_from_json, Database, McpMessageRow};
use serde::Serialize;

const FULLBODY: &str = "fullbody: searches the provider and is not available over MCP; use body: for the cached preview";

#[derive(Debug, Serialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
    pub unread: i64,
    pub total: i64,
    pub kind: String,
    pub account_id: String,
    pub account_email: String,
}

pub fn folders(db: &Database, accounts: &[String]) -> Result<Vec<Folder>, String> {
    let rows = db.mcp_folders(accounts).map_err(|e| e.to_string())?;
    Ok(rows.into_iter().map(|r| Folder {
        id: r.id, name: r.display_name, unread: r.unread, total: r.total, kind: r.well_known,
        account_id: r.account_id, account_email: r.account_email,
    }).collect())
}

#[derive(Debug, Serialize)]
pub struct Message {
    pub id: String,
    pub subject: String,
    pub from_email: String,
    pub from_name: String,
    pub received_at: String,
    pub is_read: bool,
    pub account_id: String,
    pub account_email: String,
    pub folder_id: String,
    pub importance: String,
    pub has_attachments: bool,
    pub categories: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bcc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
}

/// `detail`: add recipients and times. `preview`: characters of the stored preview to include
/// (0 = none). A row whose stored body is NULL is left out of a list that needs the preview.
fn message_of(r: McpMessageRow, detail: bool, preview: usize) -> Option<Message> {
    let preview_text = if preview > 0 { Some(clip(r.body.as_deref()?, preview)) } else { None };
    Some(Message {
        categories: categories_from_json(r.categories.as_deref()),
        from_name: r.from_name.unwrap_or_default(),
        importance: r.importance.unwrap_or_default(),
        has_attachments: r.has_attachments.unwrap_or(false),
        preview: preview_text,
        to: detail.then(|| r.to_text.unwrap_or_default()),
        cc: detail.then(|| r.cc_text.unwrap_or_default()),
        bcc: detail.then(|| r.bcc_text.unwrap_or_default()),
        sent_at: detail.then(|| r.sent_at.unwrap_or_default()),
        conversation_id: detail.then(|| r.conversation_id.unwrap_or_default()),
        id: r.id, subject: r.subject, from_email: r.from_email, received_at: r.received_at, is_read: r.is_read,
        account_id: r.account_id, account_email: r.account_email, folder_id: r.folder_id,
    })
}

/// What the caller asked for in a message list. `search` uses the mail search syntax.
pub struct MailQuery<'a> {
    pub folder: Option<&'a str>,
    pub unread_only: bool,
    pub search: Option<&'a str>,
    pub limit: i64,
    pub offset: i64,
}

pub fn list_messages(db: &Database, accounts: &[String], q: &MailQuery) -> Result<Page<Message>, String> {
    let parsed = q.search.map(crate::search::parse).transpose()?;
    if parsed.as_ref().is_some_and(|p| !p.fullbody.is_empty()) {
        return Err(FULLBODY.into());
    }
    let rows = db
        .mcp_messages(accounts, q.folder, q.unread_only, parsed.as_ref(), q.limit, q.offset)
        .map_err(|e| e.to_string())?;
    let items: Vec<Message> = rows.into_iter().filter_map(|r| message_of(r, false, LIST_PREVIEW)).collect();
    Ok(Page::of(items, q.limit, q.offset))
}

/// The message must exist and sit in an account `allowed` accepts. A queued provider action
/// hides it, as does a missing id; both answer the same way.
pub fn require_visible(db: &Database, allowed: &dyn Fn(&str) -> bool, id: &str) -> Result<(), String> {
    db.mcp_message_account(id).ok().flatten().filter(|a| allowed(a)).map(|_| ()).ok_or_else(|| NOT_FOUND.to_string())
}

pub fn get_message(db: &Database, allowed: &dyn Fn(&str) -> bool, id: &str) -> Result<Message, String> {
    require_visible(db, allowed, id)?;
    let row = db.mcp_message(id).ok().flatten().ok_or_else(|| NOT_FOUND.to_string())?;
    message_of(row, true, 0).ok_or_else(|| NOT_FOUND.to_string())
}

#[derive(Debug, Serialize)]
pub struct Preview {
    pub id: String,
    pub preview: String,
    pub note: &'static str,
}

/// The stored preview only; full bodies are not offered over MCP.
pub fn preview(db: &Database, allowed: &dyn Fn(&str) -> bool, id: &str) -> Result<Preview, String> {
    require_visible(db, allowed, id)?;
    let body = db.mcp_message_body(id).ok().flatten().ok_or_else(|| NOT_FOUND.to_string())?;
    Ok(Preview { id: id.to_string(), preview: clip(&body, MAX_TEXT), note: "Stored preview only, not the full message." })
}

/// The cached attachment list without inline parts, or `None` when OmaLook has not fetched the
/// list yet. Never goes to the provider.
pub fn attachments(db: &Database, id: &str) -> Result<Option<Vec<Attachment>>, String> {
    let list = db.cached_attachments(id).map_err(|e| e.to_string())?;
    Ok(list.map(|l| l.into_iter().filter(|a| !a.inline).collect()))
}

#[derive(Debug, Serialize)]
pub struct Category {
    pub account_id: String,
    pub name: String,
    pub color: String,
}

pub fn categories(db: &Database, accounts: &[String]) -> Result<Vec<Category>, String> {
    let rows = db.mcp_category_defs(accounts).map_err(|e| e.to_string())?;
    Ok(rows.into_iter().map(|r| Category { account_id: r.account_id, name: r.name, color: r.color }).collect())
}
