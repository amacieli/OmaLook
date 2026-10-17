//! The tool catalogue. One table is the single source of truth for the MCP server (what to
//! advertise, what to allow) and for the Settings → MCP page (what can be switched).
//!
//! A tool with `implemented: false` is in the catalogue so policy and the Settings page know
//! about it (drafts, for one), but it is never advertised or callable until its handler exists.

use super::policy::{Class, Domain};
use super::Ctx;
use serde_json::{json, Value};

pub struct ToolSpec {
    pub name: &'static str,
    pub domain: Domain,
    pub class: Class,
    pub description: &'static str,
    pub implemented: bool,
}

use Class::{R, W1, W2, W3};
use Domain::{Accounts, Calendar, Contacts, Mail, Sms};

const fn t(name: &'static str, domain: Domain, class: Class, description: &'static str, implemented: bool) -> ToolSpec {
    ToolSpec { name, domain, class, description, implemented }
}

/// Every tool OmaLook can ever offer over MCP. Settings (and anything under `/settings`, sign-in,
/// sign-out, account removal) is deliberately absent.
pub static TOOLS: &[ToolSpec] = &[
    // accounts
    t("accounts_list", Accounts, R, "List the OmaLook accounts MCP is allowed to use (id, provider, email, signed_in).", true),
    // mail: read
    t("mail_list_folders", Mail, R, "List mail folders.", true),
    t("mail_list_messages", Mail, R, "List cached messages, newest first, with a short stored preview of each. Filter by folder (id or kind such as inbox), unread_only, account. Paged: pass next_cursor back as cursor.", true),
    t("mail_search", Mail, R, "Search cached mail. Keywords: from: to: cc: subject: body: cat: is:unread is:read is:important has:attachment after: before: date: (2026-10-01, today, 7d, 2w), \"phrases\", -negation. body: searches the stored preview only; fullbody: is not available.", true),
    t("mail_get_message", Mail, R, "Headers and details of one message (from, to, cc, categories, importance, attachments flag).", true),
    t("mail_get_preview", Mail, R, "The stored preview of one message (up to 2000 characters). The full body is not available over MCP.", true),
    t("mail_list_attachments", Mail, R, "List a message's non-inline attachments, if OmaLook has already fetched the list (otherwise says so).", true),
    t("mail_list_categories", Mail, R, "List categories / labels.", true),
    t("mail_get_action_status", Mail, R, "Status of a queued archive/move/trash action.", false),
    t("mail_get_send_status", Mail, R, "Status of a queued send.", false),
    t("mail_list_drafts", Mail, R, "List drafts.", false),
    t("mail_get_draft", Mail, R, "Read one draft.", false),
    // mail: reversible writes
    t("mail_mark_read", Mail, W1, "Mark messages read or unread.", false),
    t("mail_set_categories", Mail, W1, "Set categories / labels on messages.", false),
    t("mail_create_category", Mail, W1, "Create a category / label.", false),
    t("mail_archive", Mail, W1, "Archive messages (queued, with an undo window).", false),
    t("mail_move", Mail, W1, "Move messages to a folder (queued, with an undo window).", false),
    t("mail_trash", Mail, W1, "Move messages to trash (queued, with an undo window).", false),
    t("mail_undo_action", Mail, W1, "Undo a queued archive/move/trash while its window is open.", false),
    t("mail_cancel_send", Mail, W1, "Cancel a queued send while its undo delay is running.", false),
    t("mail_create_draft", Mail, W1, "Create a draft (new, reply, reply-all or forward). Does not send.", false),
    t("mail_update_draft", Mail, W1, "Change a draft. Does not send.", false),
    t("mail_delete_draft", Mail, W1, "Delete a draft.", false),
    // mail: outbound / hard to take back
    t("mail_send", Mail, W2, "Send a message (goes through the outbox and its undo delay).", false),
    t("mail_send_draft", Mail, W2, "Send an existing draft.", false),
    t("mail_save_attachment", Mail, W2, "Save an attachment into the MCP download folder.", false),
    // mail: destructive
    t("mail_delete_permanent", Mail, W3, "Permanently delete messages. Cannot be undone.", false),
    // calendar
    t("calendar_list_events", Calendar, R, "List calendar events overlapping a date range (from and to as YYYY-MM-DD; default today + 14 days; at most 366 days).", true),
    t("calendar_get_event", Calendar, R, "Details of one event.", true),
    t("calendar_respond_invite", Calendar, W2, "Accept, tentatively accept or decline a meeting invite (tells the organiser).", false),
    // contacts
    t("contacts_list", Contacts, R, "List contacts (view: all or favorites; sort: first, last, company, recent).", true),
    t("contacts_search", Contacts, R, "Search contacts by name, company, job title, email or phone.", true),
    t("contacts_set_favorite", Contacts, W1, "Mark or unmark a contact as favorite.", false),
    // sms
    t("sms_list_threads", Sms, R, "List SMS threads, optionally filtered by a query.", true),
    t("sms_get_messages", Sms, R, "Messages of one SMS thread, newest first. Use next_before as before to go further back.", true),
    t("sms_list_contacts", Sms, R, "Contacts known to the SMS module.", true),
    t("sms_unread_counts", Sms, R, "Unread SMS message and thread counts.", true),
    t("sms_mark_read", Sms, W1, "Mark an SMS thread read or unread (local state only).", false),
    t("sms_sync", Sms, W1, "Ask the phone for a fresh SMS pull.", false),
    t("sms_retry", Sms, W1, "Retry a failed SMS send.", false),
    t("sms_send", Sms, W2, "Send an SMS.", false),
];

pub fn find(name: &str) -> Option<&'static ToolSpec> {
    TOOLS.iter().find(|t| t.name == name)
}

/// JSON Schema for a tool's arguments.
pub fn input_schema(name: &str) -> Value {
    let account = json!({ "type": "string", "description": "Limit to one account (id or email address). Default: every account enabled for MCP." });
    let limit = json!({ "type": "integer", "minimum": 1, "maximum": 100, "description": "Page size (default 25)." });
    let cursor = json!({ "type": "string", "description": "next_cursor from the previous page." });
    let id = json!({ "type": "string", "description": "The item's id." });
    let obj = |props: Value, required: &[&str]| json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false });
    match name {
        "accounts_list" => obj(json!({}), &[]),
        "mail_list_folders" | "mail_list_categories" => obj(json!({ "account": account }), &[]),
        "mail_list_messages" => obj(json!({
            "account": account, "limit": limit, "cursor": cursor,
            "folder": { "type": "string", "description": "A folder id from mail_list_folders, or a kind: inbox, sentitems, drafts, archive, deleteditems, junkemail." },
            "unread_only": { "type": "boolean" },
        }), &[]),
        "mail_search" => obj(json!({
            "query": { "type": "string", "description": "Search pattern, e.g. from:stripe is:unread after:30d" },
            "account": account, "limit": limit, "cursor": cursor,
            "folder": { "type": "string", "description": "Folder id or kind, as for mail_list_messages." },
            "unread_only": { "type": "boolean" },
        }), &["query"]),
        "mail_get_message" | "mail_get_preview" | "mail_list_attachments" | "calendar_get_event" => obj(json!({ "id": id }), &["id"]),
        "calendar_list_events" => obj(json!({
            "from": { "type": "string", "description": "First day, YYYY-MM-DD (default today)." },
            "to": { "type": "string", "description": "Last day, YYYY-MM-DD, inclusive (default from + 14 days)." },
            "account": account, "limit": limit, "cursor": cursor,
        }), &[]),
        "contacts_list" => obj(json!({
            "account": account, "limit": limit, "cursor": cursor,
            "view": { "type": "string", "enum": ["all", "favorites"] },
            "sort": { "type": "string", "enum": ["first", "last", "company", "recent"] },
        }), &[]),
        "contacts_search" => obj(json!({
            "query": { "type": "string" }, "account": account, "limit": limit, "cursor": cursor,
            "view": { "type": "string", "enum": ["all", "favorites"] },
        }), &["query"]),
        "sms_list_threads" => obj(json!({
            "query": { "type": "string", "description": "Match contact name, phone number or message text." },
            "device": { "type": "string", "description": "Phone id (default: the paired phone)." }, "limit": limit, "cursor": cursor,
        }), &[]),
        "sms_get_messages" => obj(json!({
            "thread": { "type": "integer", "description": "Thread id from sms_list_threads." },
            "device": { "type": "string" }, "limit": limit,
            "before": { "type": "integer", "description": "Only messages older than this date_ms (next_before from the previous page)." },
        }), &["thread"]),
        "sms_list_contacts" => obj(json!({ "query": { "type": "string" }, "device": { "type": "string" } }), &[]),
        "sms_unread_counts" => obj(json!({}), &[]),
        _ => json!({ "type": "object" }),
    }
}

/// Run an implemented tool. The caller has already applied the policy.
pub fn call(ctx: &Ctx, name: &str, args: &Value) -> Result<Value, String> {
    use super::read as r;
    match name {
        "accounts_list" => accounts_list(ctx),
        "mail_list_folders" => r::mail_list_folders(ctx, args),
        "mail_list_messages" => r::mail_list_messages(ctx, args),
        "mail_search" => r::mail_search(ctx, args),
        "mail_get_message" => r::mail_get_message(ctx, args),
        "mail_get_preview" => r::mail_get_preview(ctx, args),
        "mail_list_attachments" => r::mail_list_attachments(ctx, args),
        "mail_list_categories" => r::mail_list_categories(ctx, args),
        "calendar_list_events" => r::calendar_list_events(ctx, args),
        "calendar_get_event" => r::calendar_get_event(ctx, args),
        "contacts_list" => r::contacts_list(ctx, args),
        "contacts_search" => r::contacts_search(ctx, args),
        "sms_list_threads" => r::sms_list_threads(ctx, args),
        "sms_get_messages" => r::sms_get_messages(ctx, args),
        "sms_list_contacts" => r::sms_list_contacts(ctx, args),
        "sms_unread_counts" => r::sms_unread_counts(ctx, args),
        _ => Err(format!("tool '{}' is not implemented yet", name)),
    }
}

fn accounts_list(ctx: &Ctx) -> Result<Value, String> {
    let policy = ctx.policy();
    let all: Value = serde_json::from_str(&crate::account_ops::accounts_json(&ctx.config_dir)).map_err(|e| e.to_string())?;
    let rows: Vec<Value> = all
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|r| r.get("id").and_then(|v| v.as_str()).map(|id| policy.account_allowed(id)).unwrap_or(false))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({ "accounts": rows }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn tool_names_are_unique_and_prefixed_by_domain() {
        let mut seen = HashSet::new();
        for t in TOOLS {
            assert!(seen.insert(t.name), "duplicate tool {}", t.name);
            assert!(t.name.starts_with(&format!("{}_", t.domain.key())), "{} not prefixed by its domain", t.name);
        }
    }

    #[test]
    fn accounts_has_no_write_tools_and_nothing_touches_settings() {
        assert!(TOOLS.iter().filter(|t| t.domain == Domain::Accounts).all(|t| t.class == R));
        assert!(TOOLS.iter().all(|t| !t.name.contains("settings") && !t.name.contains("login") && !t.name.contains("logout")));
    }

    #[test]
    fn drafts_are_in_the_catalogue_but_not_yet_callable() {
        for n in ["mail_create_draft", "mail_update_draft", "mail_delete_draft", "mail_send_draft", "mail_list_drafts", "mail_get_draft"] {
            let t = find(n).unwrap_or_else(|| panic!("{} missing", n));
            assert!(!t.implemented);
        }
        assert_eq!(find("mail_create_draft").unwrap().class, W1);
        assert_eq!(find("mail_send_draft").unwrap().class, W2);
    }

    #[test]
    fn sends_are_never_below_w2() {
        for n in ["mail_send", "mail_send_draft", "sms_send", "calendar_respond_invite", "mail_save_attachment"] {
            assert!(matches!(find(n).unwrap().class, W2 | W3), "{} must be W2 or stricter", n);
        }
    }
}
