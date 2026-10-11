//! Compose: the outgoing-message model and everything that turns it into something a provider
//! can send. Pure functions only (no network, no database), so every piece is unit-tested.
//!
//! * `OutgoingMessage`: what the QML compose pane posts to `/compose/send`.
//! * `clean_html`: the editor's `QTextDocument::toHtml()` output is a full document with
//!   Qt-private CSS; this reduces it to a body fragment, and in system-font mode also removes
//!   every font / size / colour declaration so the recipient's client default applies.
//! * `graph_*`: request bodies for Microsoft Graph (`sendMail`, reply drafts).
//! * `build_mime`: RFC 5322 message for the Gmail API.

use crate::errors::OmarchyError;
use regex::{Captures, Regex};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Address {
    pub email: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Body {
    /// "text" (nothing was formatted) or "html" (the editor's `toHtml()`).
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub content: String,
}

/// A file the user attached while composing. Only the path travels from the UI; the daemon
/// reads the file when the message goes out (the sizes below are re-checked from disk, never
/// trusted from the client).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct OutgoingAttachment {
    pub path: String,
    /// Shown to the recipient. Empty means the file's own name.
    #[serde(default)]
    pub name: String,
    /// What the UI saw when the file was picked (display only).
    #[serde(default)]
    pub size: u64,
    /// Empty means guessed from the file name.
    #[serde(default)]
    pub content_type: String,
}

impl OutgoingAttachment {
    /// The name the recipient sees: the given one, else the file's own, with path separators and
    /// control characters made harmless.
    pub fn display_name(&self) -> String {
        let raw = if self.name.trim().is_empty() {
            std::path::Path::new(&self.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            self.name.trim().to_string()
        };
        let clean: String = raw.chars().map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c }).collect();
        if clean.trim().is_empty() { "attachment".into() } else { clean }
    }

    pub fn mime_type(&self) -> String {
        if self.content_type.trim().is_empty() { guess_content_type(&self.display_name()) } else { self.content_type.trim().to_string() }
    }
}

/// One message as composed in the UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct OutgoingMessage {
    /// "new" | "reply" | "replyAll" | "forward"
    #[serde(default)]
    pub kind: String,
    pub account_id: String,
    /// Id of the message being answered / forwarded (as stored in the cache).
    #[serde(default)]
    pub in_reply_to: String,
    #[serde(default)]
    pub to: Vec<Address>,
    #[serde(default)]
    pub cc: Vec<Address>,
    #[serde(default)]
    pub bcc: Vec<Address>,
    #[serde(default)]
    pub subject: String,
    /// "system" | "html": system mode never sends font / size / colour.
    #[serde(default)]
    pub mode: String,
    pub body: Body,
    #[serde(default)]
    pub attachments: Vec<OutgoingAttachment>,
}

/// Largest body accepted (the HTTP layer reads at most this much anyway).
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Graph accepts at most 150 MB of attachments on one message.
pub const MAX_ATTACHMENT_TOTAL: u64 = 150 * 1024 * 1024;

/// Graph's limit for adding a file in one request; anything this big or bigger goes through an
/// upload session.
pub const SMALL_ATTACHMENT_LIMIT: u64 = 3_000_000;

/// Upload-session chunk: Graph wants a multiple of 320 KiB and no more than 4 MB.
pub const UPLOAD_CHUNK: u64 = 12 * 327_680;

fn email_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^[a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)+$",
        )
        .unwrap()
    })
}

pub fn is_valid_email(email: &str) -> bool {
    email_re().is_match(email.trim())
}

impl OutgoingMessage {
    /// Refuse what could never be sent. The UI checks the same things first; this is the
    /// backstop for any other caller.
    pub fn validate(&self) -> Result<(), String> {
        if self.account_id.trim().is_empty() {
            return Err("no sending account selected".into());
        }
        let all: Vec<&Address> = self.to.iter().chain(&self.cc).chain(&self.bcc).collect();
        if all.is_empty() {
            return Err("add at least one recipient".into());
        }
        if let Some(bad) = all.iter().find(|a| !is_valid_email(&a.email)) {
            return Err(format!("not a valid address: {}", bad.email));
        }
        if self.body.content.len() > MAX_BODY_BYTES {
            return Err("message is too large".into());
        }
        if !matches!(self.body.format.as_str(), "text" | "html") {
            return Err(format!("unknown body format: {:?}", self.body.format));
        }
        self.validate_attachments()
    }

    /// Every attached file must still exist as a readable regular file, and together they must
    /// fit what the provider accepts. Sizes come from the disk.
    pub fn validate_attachments(&self) -> Result<(), String> {
        let mut total: u64 = 0;
        for a in &self.attachments {
            let name = a.display_name();
            let meta = std::fs::metadata(&a.path).map_err(|_| format!("{} is no longer there", name))?;
            if !meta.is_file() {
                return Err(format!("{} is not a file", name));
            }
            if std::fs::File::open(&a.path).is_err() {
                return Err(format!("{} cannot be read", name));
            }
            total += meta.len();
        }
        if total > MAX_ATTACHMENT_TOTAL {
            return Err(format!("attachments total {} MB; the limit is {} MB", total / (1024 * 1024), MAX_ATTACHMENT_TOTAL / (1024 * 1024)));
        }
        Ok(())
    }

    /// True for the kinds that continue an existing conversation and name the message they answer.
    pub fn is_reply(&self) -> bool {
        matches!(self.kind.as_str(), "reply" | "replyAll") && !self.in_reply_to.is_empty()
    }

    pub fn is_forward(&self) -> bool {
        self.kind == "forward" && !self.in_reply_to.is_empty()
    }

    fn system_mode(&self) -> bool {
        self.mode != "html"
    }
}

// ───────────────────────────────────────────────────────────────────── HTML

fn style_attr_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?is)\s+style\s*=\s*"([^"]*)""#).unwrap())
}

/// Declarations dropped in every mode: Qt-private ones, and defaults that say nothing.
fn always_dropped(prop: &str, value: &str) -> bool {
    prop.starts_with("-qt-")
        || (prop == "text-indent" && value == "0px")
        || (prop == "font-weight" && value == "400")
        || (prop == "font-style" && value == "normal")
        || ((prop == "margin-left" || prop == "margin-right") && value == "0px")
}

/// Declarations dropped in system-font mode, where the recipient's client decides how text looks.
fn appearance(prop: &str) -> bool {
    matches!(
        prop,
        "font-family" | "font-size" | "color" | "background-color" | "background" | "font"
    )
}

fn clean_style(style: &str, system: bool) -> String {
    let kept: Vec<String> = style
        .split(';')
        .filter_map(|d| {
            let (p, v) = d.split_once(':')?;
            let (p, v) = (p.trim().to_ascii_lowercase(), v.trim());
            if p.is_empty() || always_dropped(&p, v) || (system && appearance(&p)) {
                None
            } else {
                Some(format!("{}:{}", p, v))
            }
        })
        .collect();
    kept.join("; ")
}

/// Reduce `QTextDocument::toHtml()` output to a fragment fit for a mail body.
pub fn clean_html(raw: &str, system: bool) -> String {
    static BODY_OPEN: OnceLock<Regex> = OnceLock::new();
    let body_open = BODY_OPEN.get_or_init(|| Regex::new(r"(?is)<body([^>]*)>").unwrap());

    let (body_attrs, inner) = match body_open.captures(raw) {
        Some(c) => {
            let start = c.get(0).unwrap().end();
            let end = raw.to_ascii_lowercase().rfind("</body>").unwrap_or(raw.len()).max(start);
            (c.get(1).map(|m| m.as_str().to_string()).unwrap_or_default(), &raw[start..end])
        }
        None => (String::new(), raw),
    };

    let inner = style_attr_re().replace_all(inner, |c: &Captures| {
        let s = clean_style(&c[1], system);
        if s.is_empty() { String::new() } else { format!(" style=\"{}\"", s) }
    });

    // The body's own style carries the editor's default font; keep it only in HTML mode.
    let body_style = style_attr_re()
        .captures(&body_attrs)
        .map(|c| clean_style(&c[1], system))
        .unwrap_or_default();

    let inner = inner.trim();
    if body_style.is_empty() {
        format!("<div>{}</div>", inner)
    } else {
        format!("<div style=\"{}\">{}</div>", body_style, inner)
    }
}

fn decode_entities(s: &str) -> String {
    static NUM: OnceLock<Regex> = OnceLock::new();
    let num = NUM.get_or_init(|| Regex::new(r"&#(x[0-9a-fA-F]+|[0-9]+);").unwrap());
    let s = num.replace_all(s, |c: &Captures| {
        let t = &c[1];
        let code = if let Some(h) = t.strip_prefix('x') { u32::from_str_radix(h, 16).ok() } else { t.parse().ok() };
        code.and_then(char::from_u32).map(|ch| ch.to_string()).unwrap_or_default()
    });
    s.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Plain-text rendering of HTML, for the text/plain half of a multipart message.
pub fn html_to_text(html: &str) -> String {
    static DROP: OnceLock<Regex> = OnceLock::new();
    static LINK: OnceLock<Regex> = OnceLock::new();
    static BREAK: OnceLock<Regex> = OnceLock::new();
    static ITEM: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    static BLANKS: OnceLock<Regex> = OnceLock::new();
    let drop = DROP.get_or_init(|| Regex::new(r"(?is)<(head|style|script)\b.*?</(head|style|script)>").unwrap());
    let link = LINK.get_or_init(|| Regex::new(r#"(?is)<a\b[^>]*?href\s*=\s*"([^"]*)"[^>]*>(.*?)</a>"#).unwrap());
    let brk = BREAK.get_or_init(|| Regex::new(r"(?i)<br\s*/?>|</(p|div|li|tr|h[1-6]|blockquote)>|<(ul|ol)\b[^>]*>").unwrap());
    let item = ITEM.get_or_init(|| Regex::new(r"(?i)<li\b[^>]*>").unwrap());
    let tag = TAG.get_or_init(|| Regex::new(r"(?s)<[^>]*>").unwrap());
    let blanks = BLANKS.get_or_init(|| Regex::new(r"\n{3,}").unwrap());

    let s = drop.replace_all(html, "");
    let s = link.replace_all(&s, |c: &Captures| {
        let (href, text) = (&c[1], tag.replace_all(&c[2], "").trim().to_string());
        if text.is_empty() || text == href || href.starts_with("mailto:") && href[7..] == text { href.to_string() } else { format!("{} ({})", text, href) }
    });
    let s = item.replace_all(&s, "* ");
    let s = brk.replace_all(&s, "\n");
    let s = tag.replace_all(&s, "");
    let s = decode_entities(&s);
    let lines: Vec<&str> = s.lines().map(|l| l.trim_end()).collect();
    blanks.replace_all(lines.join("\n").trim(), "\n\n").to_string()
}

/// `(html, text)` for sending: html is None when the message is plain text.
pub fn prepare_bodies(msg: &OutgoingMessage) -> (Option<String>, String) {
    if msg.body.format == "text" {
        return (None, msg.body.content.clone());
    }
    let html = clean_html(&msg.body.content, msg.system_mode());
    let text = html_to_text(&html);
    (Some(html), text)
}

// ───────────────────────────────────────────────────────────────── Graph

fn graph_recipients(list: &[Address]) -> serde_json::Value {
    serde_json::Value::Array(
        list.iter()
            .map(|a| {
                let mut ea = serde_json::json!({ "address": a.email.trim() });
                if !a.name.trim().is_empty() {
                    ea["name"] = serde_json::json!(a.name.trim());
                }
                serde_json::json!({ "emailAddress": ea })
            })
            .collect(),
    )
}

/// The Graph `message` resource for this message (also the PATCH body for a reply draft).
pub fn graph_message_json(msg: &OutgoingMessage) -> serde_json::Value {
    let (html, text) = prepare_bodies(msg);
    let body = match html {
        Some(h) => serde_json::json!({ "contentType": "HTML", "content": h }),
        None => serde_json::json!({ "contentType": "Text", "content": text }),
    };
    serde_json::json!({
        "subject": msg.subject,
        "body": body,
        "toRecipients": graph_recipients(&msg.to),
        "ccRecipients": graph_recipients(&msg.cc),
        "bccRecipients": graph_recipients(&msg.bcc),
    })
}

/// Body of `POST /me/sendMail`.
pub fn graph_send_mail_json(msg: &OutgoingMessage) -> serde_json::Value {
    serde_json::json!({ "message": graph_message_json(msg), "saveToSentItems": true })
}

/// Content type from the file name; `application/octet-stream` when unknown.
pub fn guess_content_type(name: &str) -> String {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "txt" | "log" | "md" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "xml" => "application/xml",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "7z" => "application/x-7z-compressed",
        "tar" => "application/x-tar",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "ics" => "text/calendar",
        "eml" => "message/rfc822",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Body of `POST /me/messages/{id}/attachments` (a file under 3 MB, sent inline as base64).
pub fn graph_file_attachment_json(a: &OutgoingAttachment, bytes: &[u8]) -> serde_json::Value {
    serde_json::json!({
        "@odata.type": "#microsoft.graph.fileAttachment",
        "name": a.display_name(),
        "contentType": a.mime_type(),
        "contentBytes": b64(bytes),
    })
}

/// Body of `POST /me/messages/{id}/attachments/createUploadSession` (a file of 3 MB or more).
pub fn graph_upload_session_json(a: &OutgoingAttachment, size: u64) -> serde_json::Value {
    serde_json::json!({
        "AttachmentItem": {
            "attachmentType": "file",
            "name": a.display_name(),
            "size": size,
            "contentType": a.mime_type(),
        }
    })
}

/// Inclusive `(first, last)` byte ranges covering `total` bytes in pieces of `chunk`.
pub fn upload_ranges(total: u64, chunk: u64) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < total {
        let end = (start + chunk).min(total) - 1;
        out.push((start, end));
        start = end + 1;
    }
    out
}

pub fn content_range(first: u64, last: u64, total: u64) -> String {
    format!("bytes {}-{}/{}", first, last, total)
}

// ───────────────────────────────────────────────────────────────── MIME

/// Headers of the message being answered, needed to thread a Gmail reply.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplyHeaders {
    pub message_id: String,
    pub references: String,
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Base64 in 76-column lines, CRLF-terminated (RFC 2045).
fn b64_wrapped(bytes: &[u8]) -> String {
    let enc = b64(bytes);
    let mut out = String::with_capacity(enc.len() + enc.len() / 38);
    for chunk in enc.as_bytes().chunks(76) {
        out.push_str(std::str::from_utf8(chunk).unwrap());
        out.push_str("\r\n");
    }
    out
}

/// RFC 2047 encoded-word text for a header value; ASCII passes through untouched.
fn encode_header_text(s: &str) -> String {
    let s = s.replace(['\r', '\n'], " ");
    if s.is_ascii() {
        return s;
    }
    // Each encoded word is at most 75 chars: keep the UTF-8 payload to 42 bytes, on char boundaries.
    let mut words = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if cur.len() + ch.len_utf8() > 42 {
            words.push(std::mem::take(&mut cur));
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words.iter().map(|w| format!("=?UTF-8?B?{}?=", b64(w.as_bytes()))).collect::<Vec<_>>().join("\r\n ")
}

fn format_address(a: &Address) -> String {
    let email = a.email.trim().replace(['\r', '\n', '<', '>'], "");
    let name = a.name.trim();
    if name.is_empty() {
        return email;
    }
    if name.is_ascii() {
        let quoted = name.replace('\\', "\\\\").replace('"', "\\\"").replace(['\r', '\n'], " ");
        format!("\"{}\" <{}>", quoted, email)
    } else {
        format!("{} <{}>", encode_header_text(name), email)
    }
}

fn address_list(list: &[Address]) -> String {
    list.iter().map(format_address).collect::<Vec<_>>().join(", ")
}

/// Build the RFC 5322 message. `boundary` and `date` are parameters so tests are deterministic.
pub fn build_mime(msg: &OutgoingMessage, from: &str, reply: Option<&ReplyHeaders>, boundary: &str, date: &str) -> String {
    let (html, text) = prepare_bodies(msg);
    let mut h = String::new();
    fn line(h: &mut String, k: &str, v: &str) {
        h.push_str(k);
        h.push_str(": ");
        h.push_str(v);
        h.push_str("\r\n");
    }
    if !from.trim().is_empty() {
        line(&mut h, "From", from.trim());
    }
    if !msg.to.is_empty() {
        line(&mut h, "To", &address_list(&msg.to));
    }
    if !msg.cc.is_empty() {
        line(&mut h, "Cc", &address_list(&msg.cc));
    }
    if !msg.bcc.is_empty() {
        line(&mut h, "Bcc", &address_list(&msg.bcc));
    }
    line(&mut h, "Subject", &encode_header_text(&msg.subject));
    line(&mut h, "Date", date);
    if let Some(r) = reply {
        if !r.message_id.is_empty() {
            line(&mut h, "In-Reply-To", &r.message_id);
            let refs = if r.references.is_empty() { r.message_id.clone() } else { format!("{} {}", r.references, r.message_id) };
            line(&mut h, "References", &refs);
        }
    }
    line(&mut h, "MIME-Version", "1.0");

    match html {
        None => {
            line(&mut h, "Content-Type", "text/plain; charset=UTF-8");
            line(&mut h, "Content-Transfer-Encoding", "base64");
            h.push_str("\r\n");
            h.push_str(&b64_wrapped(text.as_bytes()));
        }
        Some(html) => {
            line(&mut h, "Content-Type", &format!("multipart/alternative; boundary=\"{}\"", boundary));
            h.push_str("\r\n");
            for (ctype, content) in [("text/plain", text.as_str()), ("text/html", html.as_str())] {
                h.push_str(&format!("--{}\r\nContent-Type: {}; charset=UTF-8\r\nContent-Transfer-Encoding: base64\r\n\r\n", boundary, ctype));
                h.push_str(&b64_wrapped(content.as_bytes()));
            }
            h.push_str(&format!("--{}--\r\n", boundary));
        }
    }
    h
}

// ─────────────────────────────────────────────────────────── retry policy

/// Whether a failed send is worth trying again later. Only failures where the message cannot
/// have been delivered qualify: could not reach the server, throttled (429) or unavailable
/// (503), or no usable token yet. Anything else (4xx, other 5xx, a timeout after the request
/// left) is reported, never retried: a retry there could send the message twice.
pub fn is_transient(e: &OmarchyError) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?:Graph|Google) API error: (?:429|503)\b").unwrap());
    match e {
        OmarchyError::AuthError(_) | OmarchyError::TokenError(_) => true,
        OmarchyError::HttpError(m) => {
            re.is_match(m) || (m.contains("error sending request") && !m.to_ascii_lowercase().contains("timed out"))
        }
        _ => false,
    }
}

/// Seconds to wait before attempt number `attempts + 1`.
pub fn retry_delay_secs(attempts: i64) -> i64 {
    match attempts {
        0 | 1 => 5,
        2 => 20,
        _ => 60,
    }
}

/// Attempts (the first included) before a transient failure is given up on.
pub const MAX_ATTEMPTS: i64 = 4;

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(e: &str, n: &str) -> Address { Address { email: e.into(), name: n.into() } }

    fn msg(format: &str, content: &str, mode: &str) -> OutgoingMessage {
        OutgoingMessage {
            kind: "new".into(),
            account_id: "exchange-aaaaaa".into(),
            to: vec![addr("pat@example.com", "Pat Lee")],
            subject: "Q3 numbers".into(),
            mode: mode.into(),
            body: Body { format: format.into(), content: content.into() },
            ..Default::default()
        }
    }

    // What QTextDocument::toHtml() really produced in the compose spike.
    const QT_HTML: &str = "<!DOCTYPE HTML PUBLIC \"-//W3C//DTD HTML 4.0//EN\" \"http://www.w3.org/TR/REC-html40/strict.dtd\">\n<html><head><meta name=\"qrichtext\" content=\"1\" /><meta charset=\"utf-8\" /><style type=\"text/css\">\np, li { white-space: pre-wrap; }\nhr { height: 1px; border-width: 0; }\n</style></head><body style=\" font-family:'Calibri'; font-size:15px; font-weight:400; font-style:normal;\">\n<p align=\"center\" style=\" margin-top:12px; margin-bottom:12px; margin-left:0px; margin-right:0px; -qt-block-indent:0; text-indent:0px;\"><span style=\" font-size:20px; font-weight:700; color:#c00000;\">On</span> Oct 7 &amp; Pat Lee &lt;pat@example.com&gt; wrote:</p>\n<p style=\" margin-top:12px; margin-bottom:12px; margin-left:40px; margin-right:40px; -qt-block-indent:0; text-indent:0px;\">Hi Adam,<br />Thanks</p></body></html>";

    #[test]
    fn validation_catches_what_can_never_be_sent() {
        let ok = msg("text", "hi", "system");
        assert!(ok.validate().is_ok());
        let mut m = ok.clone(); m.to.clear();
        assert_eq!(m.validate().unwrap_err(), "add at least one recipient");
        let mut m = ok.clone(); m.to = vec![addr("not-an-address", "")];
        assert!(m.validate().unwrap_err().contains("not a valid address"));
        let mut m = ok.clone(); m.cc = vec![addr("a@b.co", "")]; m.to.clear();
        assert!(m.validate().is_ok(), "cc alone is enough");
        let mut m = ok.clone(); m.account_id = " ".into();
        assert!(m.validate().is_err());
        let mut m = ok.clone(); m.body.format = "rtf".into();
        assert!(m.validate().is_err());
        assert!(is_valid_email("a.b+c@sub.example.org"));
        assert!(!is_valid_email("a@b"), "a bare host is not an address");
        assert!(!is_valid_email("a b@c.de"));
    }

    #[test]
    fn json_from_the_ui_deserialises() {
        let j = r#"{"kind":"reply","account_id":"a1","in_reply_to":"x","to":[{"email":"p@e.com","name":"P"}],"cc":[],"bcc":[],"subject":"Re: s","mode":"html","body":{"format":"html","content":"<p>x</p>"}}"#;
        let m: OutgoingMessage = serde_json::from_str(j).unwrap();
        assert!(m.is_reply() && !m.is_forward());
        assert_eq!(m.to[0].name, "P");
    }

    #[test]
    fn system_mode_strips_fonts_sizes_colours_but_keeps_emphasis() {
        let out = clean_html(QT_HTML, true);
        assert!(!out.contains("Calibri") && !out.contains("font-size") && !out.contains("color"), "{}", out);
        assert!(!out.contains("-qt-") && !out.contains("<style") && !out.contains("<html"), "{}", out);
        assert!(out.contains("font-weight:700"), "bold survives: {}", out);
        assert!(out.contains("margin-left:40px"), "quote indent survives: {}", out);
        assert!(out.contains("align=\"center\""), "alignment attribute is not a style, left to the UI to flatten");
    }

    #[test]
    fn html_mode_keeps_the_chosen_look() {
        let out = clean_html(QT_HTML, false);
        assert!(out.starts_with("<div style=\"font-family:'Calibri'; font-size:15px\">"), "{}", out);
        assert!(out.contains("color:#c00000") && out.contains("font-size:20px"), "{}", out);
        assert!(!out.contains("-qt-"), "{}", out);
    }

    #[test]
    fn clean_html_copes_with_a_fragment() {
        assert_eq!(clean_html("<p>hi</p>", true), "<div><p>hi</p></div>");
    }

    #[test]
    fn html_to_text_decodes_and_breaks_lines() {
        let t = html_to_text(&clean_html(QT_HTML, true));
        assert_eq!(t, "On Oct 7 & Pat Lee <pat@example.com> wrote:\n\nHi Adam,\nThanks");
        assert_eq!(html_to_text("<ul><li>one</li><li>two</li></ul>"), "* one\n* two");
        assert_eq!(html_to_text("<a href=\"https://x.org/a\">the site</a>"), "the site (https://x.org/a)");
        assert_eq!(html_to_text("<a href=\"https://x.org\">https://x.org</a>"), "https://x.org");
        assert_eq!(html_to_text("caf&#233; &#x41;"), "café A");
    }

    #[test]
    fn plain_text_stays_plain() {
        let (h, t) = prepare_bodies(&msg("text", "a < b & c", "system"));
        assert!(h.is_none());
        assert_eq!(t, "a < b & c");
    }

    #[test]
    fn graph_send_mail_body() {
        let mut m = msg("text", "hello", "system");
        m.cc = vec![addr("c@example.com", "")];
        let j = graph_send_mail_json(&m);
        assert_eq!(j["saveToSentItems"], true);
        assert_eq!(j["message"]["subject"], "Q3 numbers");
        assert_eq!(j["message"]["body"]["contentType"], "Text");
        assert_eq!(j["message"]["body"]["content"], "hello");
        assert_eq!(j["message"]["toRecipients"][0]["emailAddress"]["address"], "pat@example.com");
        assert_eq!(j["message"]["toRecipients"][0]["emailAddress"]["name"], "Pat Lee");
        assert!(j["message"]["ccRecipients"][0]["emailAddress"].get("name").is_none(), "empty name is omitted");
        assert_eq!(j["message"]["bccRecipients"].as_array().unwrap().len(), 0);

        let h = graph_message_json(&msg("html", QT_HTML, "system"));
        assert_eq!(h["body"]["contentType"], "HTML");
        assert!(!h["body"]["content"].as_str().unwrap().contains("Calibri"));
    }

    fn decode_part(mime: &str, ctype: &str) -> String {
        use base64::Engine;
        let start = mime.find(&format!("Content-Type: {}", ctype)).unwrap();
        let body = mime[start..].split("\r\n\r\n").nth(1).unwrap();
        let body = body.split("\r\n--").next().unwrap().replace("\r\n", "");
        String::from_utf8(base64::engine::general_purpose::STANDARD.decode(body).unwrap()).unwrap()
    }

    #[test]
    fn mime_plain_text() {
        let m = msg("text", "héllo wörld", "system");
        let raw = build_mime(&m, "adam@example.com", None, "BND", "Thu, 08 Oct 2026 12:00:00 +0000");
        assert!(raw.starts_with("From: adam@example.com\r\nTo: \"Pat Lee\" <pat@example.com>\r\nSubject: Q3 numbers\r\n"), "{}", raw);
        assert!(raw.contains("Content-Type: text/plain; charset=UTF-8\r\nContent-Transfer-Encoding: base64\r\n\r\n"));
        assert!(!raw.contains("multipart") && !raw.contains("Bcc:") && !raw.contains("In-Reply-To"));
        assert_eq!(decode_part(&raw, "text/plain"), "héllo wörld");
    }

    #[test]
    fn mime_html_is_multipart_with_a_text_alternative_and_threads_replies() {
        let mut m = msg("html", QT_HTML, "system");
        m.bcc = vec![addr("b@example.com", "")];
        let r = ReplyHeaders { message_id: "<m2@x>".into(), references: "<m1@x>".into() };
        let raw = build_mime(&m, "", Some(&r), "BND", "Thu, 08 Oct 2026 12:00:00 +0000");
        assert!(raw.contains("Content-Type: multipart/alternative; boundary=\"BND\"\r\n"));
        assert!(raw.contains("In-Reply-To: <m2@x>\r\nReferences: <m1@x> <m2@x>\r\n"));
        assert!(raw.contains("Bcc: b@example.com\r\n") && !raw.contains("From:"));
        assert!(raw.ends_with("--BND--\r\n"));
        assert!(decode_part(&raw, "text/plain").starts_with("On Oct 7 & Pat Lee"));
        let html = decode_part(&raw, "text/html");
        assert!(html.starts_with("<div>") && !html.contains("Calibri"), "{}", html);
        for l in raw.split("\r\n") { assert!(l.len() <= 998, "line too long"); }
    }

    #[test]
    fn mime_encodes_non_ascii_subject_and_names() {
        let mut m = msg("text", "x", "system");
        m.subject = "Réunion — prévue à 15h, très importante, merci d'avance pour votre réponse rapide".into();
        m.to = vec![addr("j@example.com", "Jürgen Müller")];
        let raw = build_mime(&m, "", None, "BND", "d");
        let subj = raw.lines().find(|l| l.starts_with("Subject:")).unwrap();
        assert!(subj.contains("=?UTF-8?B?"), "{}", subj);
        assert!(raw.contains("To: =?UTF-8?B?"), "{}", raw);
        // every encoded word fits the 75-char limit
        for w in raw.split(|c: char| c.is_whitespace()).filter(|w| w.starts_with("=?UTF-8")) {
            assert!(w.len() <= 75, "{} ({})", w, w.len());
        }
        // header injection is neutralised
        let mut m = msg("text", "x", "system");
        m.subject = "hi\r\nBcc: evil@example.com".into();
        let raw = build_mime(&m, "", None, "BND", "d");
        assert!(!raw.contains("\r\nBcc:"), "{}", raw);
    }

    #[test]
    fn only_failures_that_cannot_have_delivered_are_retried() {
        use OmarchyError::*;
        assert!(is_transient(&HttpError("error sending request for url (https://graph.microsoft.com/v1.0/me/sendMail)".into())));
        assert!(is_transient(&HttpError("Graph API error: 429 Too Many Requests — {}".into())));
        assert!(is_transient(&HttpError("Google API error: 503 Service Unavailable — x".into())));
        assert!(is_transient(&AuthError("Failed to get token".into())));
        assert!(!is_transient(&HttpError("Graph API error: 400 Bad Request — {}".into())));
        assert!(!is_transient(&HttpError("Graph API error: 500 Internal Server Error — {}".into())));
        assert!(!is_transient(&HttpError("Google API error: 403 Forbidden — x".into())));
        assert!(!is_transient(&HttpError("error sending request: operation timed out".into())));
        assert_eq!(retry_delay_secs(1), 5);
        assert!(retry_delay_secs(3) >= retry_delay_secs(2));
    }

    // ---- attachments

    fn file_in(dir: &std::path::Path, name: &str, bytes: &[u8]) -> OutgoingAttachment {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        OutgoingAttachment { path: p.to_string_lossy().into(), ..Default::default() }
    }

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("olook-att-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn attachment_names_are_safe_and_types_are_guessed() {
        let a = OutgoingAttachment { path: "/home/x/Q3 report.PDF".into(), ..Default::default() };
        assert_eq!(a.display_name(), "Q3 report.PDF");
        assert_eq!(a.mime_type(), "application/pdf");
        let b = OutgoingAttachment { path: "/x/y".into(), name: "../../etc/pa\nsswd".into(), content_type: "text/x-custom".into(), ..Default::default() };
        assert_eq!(b.display_name(), ".._.._etc_pa_sswd");
        assert_eq!(b.mime_type(), "text/x-custom");
        assert_eq!(guess_content_type("noext"), "application/octet-stream");
        assert_eq!(guess_content_type("a.b.JPG"), "image/jpeg");
        assert_eq!(OutgoingAttachment { path: "/".into(), ..Default::default() }.display_name(), "attachment");
    }

    #[test]
    fn attachments_are_checked_against_the_disk() {
        let d = tmpdir("val");
        let mut m = msg("text", "hi", "system");
        m.to = vec![addr("p@example.com", "")];
        m.account_id = "a1".into();
        m.attachments = vec![file_in(&d, "ok.txt", b"hello")];
        assert!(m.validate().is_ok());
        // gone before sending
        std::fs::remove_file(&m.attachments[0].path).unwrap();
        assert_eq!(m.validate().unwrap_err(), "ok.txt is no longer there");
        // a folder is not a file
        m.attachments = vec![OutgoingAttachment { path: d.to_string_lossy().into(), ..Default::default() }];
        assert!(m.validate().unwrap_err().contains("is not a file"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn old_payloads_without_attachments_still_load() {
        let m: OutgoingMessage = serde_json::from_str(r#"{"account_id":"a","body":{"format":"text","content":"x"}}"#).unwrap();
        assert!(m.attachments.is_empty());
    }

    #[test]
    fn graph_attachment_bodies() {
        let a = OutgoingAttachment { path: "/x/n.txt".into(), ..Default::default() };
        let j = graph_file_attachment_json(&a, b"hi");
        assert_eq!(j["@odata.type"], "#microsoft.graph.fileAttachment");
        assert_eq!(j["name"], "n.txt");
        assert_eq!(j["contentType"], "text/plain");
        assert_eq!(j["contentBytes"], "aGk=");
        let s = graph_upload_session_json(&a, 5_000_000);
        assert_eq!(s["AttachmentItem"]["attachmentType"], "file");
        assert_eq!(s["AttachmentItem"]["size"], 5_000_000);
    }

    #[test]
    fn upload_ranges_tile_the_file_exactly() {
        assert_eq!(upload_ranges(0, 10), vec![]);
        assert_eq!(upload_ranges(10, 10), vec![(0, 9)]);
        assert_eq!(upload_ranges(25, 10), vec![(0, 9), (10, 19), (20, 24)]);
        assert_eq!(content_range(10, 19, 25), "bytes 10-19/25");
        let r = upload_ranges(10_000_000, UPLOAD_CHUNK);
        assert_eq!(r.first().unwrap().0, 0);
        assert_eq!(r.last().unwrap().1, 9_999_999);
        assert!(r.windows(2).all(|w| w[1].0 == w[0].1 + 1));
        assert_eq!(UPLOAD_CHUNK % 327_680, 0);
        assert!(UPLOAD_CHUNK <= 4 * 1024 * 1024);
    }
}
