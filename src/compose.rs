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
    /// Distribution list id (see `db::lists`), 0 for a plain address. A list entry carries no
    /// address of its own: `expand_lists` replaces it with the members before the message is
    /// validated and queued.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub list: i64,
}

fn is_zero(n: &i64) -> bool { *n == 0 }

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
    /// Set once the file has been copied into the outgoing spool (`attach_spool`): `path` is then
    /// the spool copy and this is the file the user picked. Empty for a message not yet queued.
    #[serde(default)]
    pub source_path: String,
    /// Content-ID of an inline picture (no angle brackets). Set for a picture the body shows, which
    /// refers to it as `cid:<inline_cid>`. Empty for an ordinary file. See `settle_inline`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub inline_cid: String,
    /// Modification time (unix seconds) of the picked file. With `size` and the original path it
    /// tells a draft's spooled copy is still the file the user picked (`attach_spool`). 0 = unknown.
    #[serde(default)]
    pub mtime: i64,
}

impl OutgoingAttachment {
    /// True for a picture that is part of the HTML body (not a file listed beside it).
    pub fn is_inline(&self) -> bool {
        !self.inline_cid.is_empty()
    }

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
    /// Forward only: ids of the original message's attachments the user kept (Gmail: the MIME
    /// `partId`). `None` means the UI did not say, and Gmail sends no originals. Graph keeps them
    /// all until phase 3 teaches it to drop the others.
    #[serde(default)]
    pub forwarded_keep: Option<Vec<String>>,
    /// Local id of the draft this message was sent from (docs/PLAN-compose.md Phase B2). Empty
    /// for a message never saved as a draft. Its provider copy is then sent, not copied.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub draft_id: String,
}

/// Largest body accepted (the HTTP layer reads at most this much anyway).
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Graph accepts at most 150 MB of attachments on one message.
pub const MAX_ATTACHMENT_TOTAL: u64 = 150 * 1024 * 1024;

/// Gmail's upload endpoints refuse a message above 35 MiB (`413 Media is too large. Limit:
/// 36700160`, measured in the phase-0 spike of docs/PLAN-compose-attachments.md). The size
/// counted is the encoded RFC 822 message, not the raw files.
pub const GMAIL_HARD_ENCODED: u64 = 36_700_160;

/// Above this encoded size Gmail still accepts the message but some recipients' servers may not
/// (Gmail's documented limit is 25 MB). A warning, never a block.
pub const GMAIL_WARN_ENCODED: u64 = 25_000_000;

/// What one provider accepts on one message. `basis` says what the numbers measure: the raw
/// files (Graph) or the encoded message (Gmail, where base64 and line breaks add about a third).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AttachmentLimits {
    pub provider: String,
    pub basis: &'static str,
    pub hard: u64,
    pub warn: Option<u64>,
}

pub fn attachment_limits(provider: &str) -> AttachmentLimits {
    match provider {
        "gmail" => AttachmentLimits { provider: "gmail".into(), basis: "encoded", hard: GMAIL_HARD_ENCODED, warn: Some(GMAIL_WARN_ENCODED) },
        other => AttachmentLimits { provider: other.into(), basis: "raw", hard: MAX_ATTACHMENT_TOTAL, warn: None },
    }
}

/// Size of one file as a base64 MIME part: 4 bytes per 3, a CRLF every 76 characters, and a
/// few hundred bytes of part headers.
pub fn encoded_part_size(raw: u64) -> u64 {
    let b64 = raw.div_ceil(3) * 4;
    b64 + b64.div_ceil(76) * 2 + 400
}

/// Result of measuring a set of files against a provider's limits.
#[derive(Debug, Clone, PartialEq)]
pub struct AttachmentCheck {
    pub total_raw: u64,
    /// The figure compared with the limits (raw total for Graph, estimated message size for Gmail).
    pub counted: u64,
    pub hard: u64,
    pub warn: Option<u64>,
    pub biggest: Option<(String, u64)>,
}

impl AttachmentCheck {
    pub fn over_hard(&self) -> bool {
        self.counted > self.hard
    }
    pub fn over_warn(&self) -> bool {
        self.warn.map(|w| self.counted > w).unwrap_or(false)
    }

    /// The message to show when Send must be refused: exact numbers, biggest file named.
    pub fn error(&self) -> Option<String> {
        if !self.over_hard() {
            return None;
        }
        let mb = |n: u64| format!("{:.1} MB", n as f64 / (1024.0 * 1024.0));
        let big = self.biggest.as_ref().map(|(n, s)| format!(" (the biggest is {}, {})", n, mb(*s))).unwrap_or_default();
        Some(if self.warn.is_some() {
            format!("the message would be {} with the attachments encoded; Gmail takes at most {}{}", mb(self.counted), mb(self.hard), big)
        } else {
            format!("attachments total {}; the limit is {}{}", mb(self.counted), mb(self.hard), big)
        })
    }
}

/// Measure `files` (display name, size) plus a body of `body_len` bytes against `provider`.
pub fn check_attachments(provider: &str, body_len: usize, files: &[(String, u64)]) -> AttachmentCheck {
    let lim = attachment_limits(provider);
    let total_raw: u64 = files.iter().map(|f| f.1).sum();
    let counted = if lim.basis == "encoded" {
        // Headers, and a body that may be sent as both plain text and HTML.
        4096 + body_len as u64 * 2 + files.iter().map(|f| encoded_part_size(f.1)).sum::<u64>()
    } else {
        total_raw
    };
    let biggest = files.iter().max_by_key(|f| f.1).cloned();
    AttachmentCheck { total_raw, counted, hard: lim.hard, warn: lim.warn, biggest }
}

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
    /// Replace every distribution-list entry in To / Cc / Bcc with the list's members, then make
    /// sure nobody gets two copies: an address keeps its first place in To, then Cc, then Bcc
    /// (so someone in the Bcc list who is also typed in To stays visible in To). `lookup` gives a
    /// list's name and members, or None when it no longer exists. A message without list
    /// entries is left untouched. Returns what each list expanded to, for the send toast.
    pub fn expand_lists(&mut self, lookup: &dyn Fn(i64) -> Option<(String, Vec<Address>)>) -> Result<Vec<(String, usize)>, String> {
        let any = self.to.iter().chain(&self.cc).chain(&self.bcc).any(|a| a.list != 0);
        if !any { return Ok(vec![]); }
        let mut report = Vec::new();
        for field in [&mut self.to, &mut self.cc, &mut self.bcc] {
            let mut out: Vec<Address> = Vec::with_capacity(field.len());
            for a in field.drain(..) {
                if a.list == 0 { out.push(a); continue; }
                let Some((name, members)) = lookup(a.list) else {
                    return Err(format!("the list {} no longer exists", if a.name.is_empty() { "you picked".to_string() } else { format!("\"{}\"", a.name) }));
                };
                if members.is_empty() { return Err(format!("the list \"{}\" has no members", name)); }
                report.push((name, members.len()));
                out.extend(members.into_iter().map(|m| Address { list: 0, ..m }));
            }
            *field = out;
        }
        let mut seen = std::collections::HashSet::new();
        for field in [&mut self.to, &mut self.cc, &mut self.bcc] {
            field.retain(|a| seen.insert(a.email.trim().to_ascii_lowercase()));
        }
        Ok(report)
    }

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
        let mut files: Vec<(String, u64)> = Vec::new();
        for a in &self.attachments {
            let name = a.display_name();
            let meta = std::fs::metadata(&a.path).map_err(|_| format!("{} is no longer there", name))?;
            if !meta.is_file() {
                return Err(format!("{} is not a file", name));
            }
            if std::fs::File::open(&a.path).is_err() {
                return Err(format!("{} cannot be read", name));
            }
            files.push((name, meta.len()));
        }
        match check_attachments(crate::accounts::provider_of(&self.account_id), self.body.content.len(), &files).error() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Make the inline pictures agree with the body, once, when the message is queued. The editor
    /// is the source of truth: an `<img>` the body does not show is gone, so is an inline picture
    /// the body no longer refers to; an `<img>` whose source is not `cid:` (a `file:` path, a
    /// relative name, `data:`) is removed, because it would either not load for the recipient or
    /// leak a local path. Remote `http(s)` pictures are left for the recipient's client, as typed.
    /// A text body keeps no pictures at all.
    pub fn settle_inline(&mut self) {
        if self.body.format == "html" {
            // Every `cid:` stays here: a reply's quoted picture may still be fetched from the
            // original when the message goes out. `prepare_bodies` drops the ones nothing supplies.
            self.body.content = rewrite_images(&self.body.content, &|_| true);
        }
        let used = if self.body.format == "html" { referenced_cids(&self.body.content) } else { Vec::new() };
        let mut seen = std::collections::HashSet::new();
        self.attachments.retain(|a| {
            if !a.is_inline() {
                return true;
            }
            safe_cid(&a.inline_cid) && used.contains(&a.inline_cid) && seen.insert(a.inline_cid.clone())
        });
    }

    /// Content-IDs of the inline pictures this message carries.
    pub fn inline_cids(&self) -> Vec<String> {
        self.attachments.iter().filter(|a| a.is_inline()).map(|a| a.inline_cid.clone()).collect()
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

/// `(html, text)` for sending: html is None when the message is plain text. The pictures the body
/// may show are the message's own inline attachments (see `prepare_bodies_with`).
pub fn prepare_bodies(msg: &OutgoingMessage) -> (Option<String>, String) {
    prepare_bodies_with(msg, &msg.inline_cids())
}

/// Content-ID values that may be used in a `cid:` reference and a `Content-ID` header: no
/// characters that could end a header or a URL.
pub fn safe_cid(c: &str) -> bool {
    !c.is_empty() && c.len() <= 254 && c.chars().all(|ch| ch.is_ascii_alphanumeric() || "-_.@+".contains(ch))
}

fn img_tag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)<img\b[^>]*>").unwrap())
}

fn img_src_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?is)\ssrc\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))"#).unwrap())
}

/// The `src` of one `<img>` tag, unquoted.
fn img_src(tag: &str) -> Option<String> {
    let m = img_src_re().captures(tag)?;
    m.get(1).or(m.get(2)).or(m.get(3)).map(|x| x.as_str().trim().to_string())
}

/// The Content-ID a `cid:` image source names (percent-decoded, brackets removed), or None for
/// any other source.
fn cid_of_src(src: &str) -> Option<String> {
    let s = src.trim();
    if !s.get(..4).map(|p| p.eq_ignore_ascii_case("cid:")).unwrap_or(false) {
        return None;
    }
    let raw = s[4..].trim().trim_start_matches('<').trim_end_matches('>');
    Some(urlencoding::decode(raw).map(|c| c.into_owned()).unwrap_or_else(|_| raw.to_string()))
}

/// The Content-IDs the HTML's `<img src="cid:…">` refer to, in order, without repeats.
pub fn referenced_cids(html: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tag in img_tag_re().find_iter(html) {
        if let Some(cid) = img_src(tag.as_str()).and_then(|s| cid_of_src(&s)) {
            if !out.contains(&cid) {
                out.push(cid);
            }
        }
    }
    out
}

/// Keep the `<img>` tags the body may show and remove the rest. A `cid:` picture is kept when
/// `keep` says so; an `http(s)` picture is left for the recipient's client; any other source
/// (`file:`, `data:`, a relative name, no source at all) is removed, because it would not load
/// for the recipient and a local path must never leave the machine.
pub fn rewrite_images(html: &str, keep: &dyn Fn(&str) -> bool) -> String {
    img_tag_re()
        .replace_all(html, |c: &Captures| {
            let tag = &c[0];
            let ok = match img_src(tag) {
                None => false,
                Some(src) => match cid_of_src(&src) {
                    Some(cid) => keep(&cid),
                    None => {
                        let l = src.to_ascii_lowercase();
                        l.starts_with("https://") || l.starts_with("http://")
                    }
                },
            };
            if ok { tag.to_string() } else { String::new() }
        })
        .into_owned()
}

/// Like `prepare_bodies`, with the `cid:` pictures limited to the Content-IDs in `present`: a
/// picture nothing supplies is removed from the body rather than sent broken.
pub fn prepare_bodies_with(msg: &OutgoingMessage, present: &[String]) -> (Option<String>, String) {
    if msg.body.format == "text" {
        return (None, msg.body.content.clone());
    }
    let html = clean_html(&msg.body.content, msg.system_mode());
    let html = rewrite_images(&html, &|cid| present.iter().any(|p| p == cid));
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
    graph_message_json_with(msg, &msg.inline_cids())
}

/// `graph_message_json` with the pictures the draft will hold given explicitly (a reply draft
/// also holds the original's pictures that the body refers to).
pub fn graph_message_json_with(msg: &OutgoingMessage, present: &[String]) -> serde_json::Value {
    let (html, text) = prepare_bodies_with(msg, present);
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
    let mut j = serde_json::json!({
        "@odata.type": "#microsoft.graph.fileAttachment",
        "name": a.display_name(),
        "contentType": a.mime_type(),
        "contentBytes": b64(bytes),
    });
    if a.is_inline() {
        j["isInline"] = serde_json::json!(true);
        j["contentId"] = serde_json::json!(a.inline_cid);
    }
    j
}

/// Body of `POST /me/messages/{id}/attachments` for a picture copied from another message: the
/// bytes are already base64 (as Graph returned them).
pub fn graph_inline_copy_json(name: &str, content_type: &str, cid: &str, content_bytes_b64: &str) -> serde_json::Value {
    serde_json::json!({
        "@odata.type": "#microsoft.graph.fileAttachment",
        "name": name,
        "contentType": content_type,
        "contentBytes": content_bytes_b64,
        "isInline": true,
        "contentId": cid,
    })
}

/// Body of `POST /me/messages/{id}/attachments/createUploadSession` (a file of 3 MB or more).
pub fn graph_upload_session_json(a: &OutgoingAttachment, size: u64) -> serde_json::Value {
    let mut item = serde_json::json!({
        "attachmentType": "file",
        "name": a.display_name(),
        "size": size,
        "contentType": a.mime_type(),
    });
    if a.is_inline() {
        item["isInline"] = serde_json::json!(true);
        item["contentId"] = serde_json::json!(a.inline_cid);
    }
    serde_json::json!({ "AttachmentItem": item })
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

/// Header block shared by every message shape (everything up to, not including, Content-Type).
fn mime_headers(msg: &OutgoingMessage, from: &str, reply: Option<&ReplyHeaders>, date: &str) -> String {
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
    h
}

/// The text of the message as one MIME entity: its own headers, a blank line, and the content
/// (plain text, or multipart/alternative with the plain text first).
fn body_entity(msg: &OutgoingMessage, present: &[String], boundary: &str) -> String {
    let (html, text) = prepare_bodies_with(msg, present);
    let mut h = String::new();
    match html {
        None => {
            h.push_str("Content-Type: text/plain; charset=UTF-8\r\nContent-Transfer-Encoding: base64\r\n\r\n");
            h.push_str(&b64_wrapped(text.as_bytes()));
        }
        Some(html) => {
            h.push_str(&format!("Content-Type: multipart/alternative; boundary=\"{}\"\r\n\r\n", boundary));
            for (ctype, content) in [("text/plain", text.as_str()), ("text/html", html.as_str())] {
                h.push_str(&format!("--{}\r\nContent-Type: {}; charset=UTF-8\r\nContent-Transfer-Encoding: base64\r\n\r\n", boundary, ctype));
                h.push_str(&b64_wrapped(content.as_bytes()));
            }
            h.push_str(&format!("--{}--\r\n", boundary));
        }
    }
    h
}

/// Build the RFC 5322 message for a message **without attachments**. `boundary` and `date` are
/// parameters so tests are deterministic. With files use `write_mime`, which streams them.
pub fn build_mime(msg: &OutgoingMessage, from: &str, reply: Option<&ReplyHeaders>, boundary: &str, date: &str) -> String {
    let mut h = mime_headers(msg, from, reply, date);
    h.push_str(&body_entity(msg, &msg.inline_cids(), boundary));
    h
}

/// A file name as a quoted-string: printable ASCII only, quotes and backslashes made harmless.
fn ascii_filename(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_graphic() || c == ' ' { if c == '"' || c == '\\' { '_' } else { c } } else { '_' }).collect()
}

/// RFC 5987 percent-encoding of a UTF-8 parameter value (`filename*=UTF-8''...`).
fn rfc5987(name: &str) -> String {
    let mut out = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

/// `Content-Type` value that cannot break the header: a plain `type/subtype` token or octet-stream.
fn safe_content_type(t: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9!#$&^_.+-]*/[A-Za-z0-9][A-Za-z0-9!#$&^_.+-]*$").unwrap());
    if re.is_match(t) { t.to_string() } else { "application/octet-stream".to_string() }
}

/// Headers of one attachment part. When the name is not plain ASCII both forms are given, **the
/// RFC 2231 `*` form first**: measured against Gmail, whichever of the two comes first wins, so
/// the ASCII fallback only serves clients that do not understand `*`.
fn attachment_headers(a: &OutgoingAttachment) -> String {
    part_headers(a, false)
}

/// Headers of one part: `inline` makes it a picture the HTML body shows by `Content-ID`.
fn part_headers(a: &OutgoingAttachment, inline: bool) -> String {
    let name = a.display_name();
    let plain = ascii_filename(&name);
    let star = if plain != name { Some(rfc5987(&name)) } else { None };
    let params = |key: &str| match &star {
        Some(star) => format!(";\r\n\t{}*=UTF-8''{};\r\n\t{}=\"{}\"", key, star, key, plain),
        None => format!(";\r\n\t{}=\"{}\"", key, plain),
    };
    let (disposition, cid) = if inline {
        // Only the characters `safe_cid` allows reach the header, whatever the caller passed.
        let id: String = a.inline_cid.chars().filter(|c| c.is_ascii_alphanumeric() || "-_.@+".contains(*c)).collect();
        ("inline", format!("Content-ID: <{}>\r\n", id))
    } else {
        ("attachment", String::new())
    };
    format!(
        "Content-Type: {}{}\r\n{}Content-Disposition: {}{}\r\nContent-Transfer-Encoding: base64\r\n\r\n",
        safe_content_type(&a.mime_type()),
        params("name"),
        cid,
        disposition,
        params("filename")
    )
}

/// Base64 of everything `input` yields, in 76-column CRLF lines, written as it is read: memory
/// use is one buffer, whatever the file size.
fn write_b64_stream<R: std::io::Read, W: std::io::Write>(input: &mut R, w: &mut W) -> std::io::Result<()> {
    // A multiple of 57 bytes encodes to whole 76-character lines; only the last chunk is short.
    const CHUNK: usize = 57 * 2048;
    let mut buf = vec![0u8; CHUNK];
    loop {
        let mut filled = 0;
        while filled < CHUNK {
            let n = input.read(&mut buf[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            return Ok(());
        }
        let enc = b64(&buf[..filled]);
        let mut out = String::with_capacity(enc.len() + enc.len() / 38 + 2);
        for line in enc.as_bytes().chunks(76) {
            out.push_str(std::str::from_utf8(line).unwrap());
            out.push_str("\r\n");
        }
        w.write_all(out.as_bytes())?;
        if filled < CHUNK {
            return Ok(());
        }
    }
}

/// Write the whole message, attachments included, to `w`. Files are read and encoded in
/// chunks; nothing is held in memory but the text of the message. The outer boundary is derived
/// from `boundary` (`<boundary>-mixed`) so the same value drives both levels in tests.
pub fn write_mime<W: std::io::Write>(
    w: &mut W,
    msg: &OutgoingMessage,
    attachments: &[OutgoingAttachment],
    from: &str,
    reply: Option<&ReplyHeaders>,
    boundary: &str,
    date: &str,
) -> std::io::Result<()> {
    if attachments.is_empty() {
        return w.write_all(build_mime(msg, from, reply, boundary, date).as_bytes());
    }
    // Pictures of an HTML body go in a multipart/related next to the text; everything else (and
    // any picture of a plain-text body) is an ordinary file part of the mixed message.
    // Only the pictures the body still refers to (and that something supplies) are related parts; an
    // inline picture the body does not show goes as an ordinary file rather than being dropped here.
    let shown: Vec<String> = if msg.body.format != "text" {
        let supplied: Vec<String> = attachments.iter().filter(|a| a.is_inline()).map(|a| a.inline_cid.clone()).collect();
        referenced_cids(&prepare_bodies_with(msg, &supplied).0.unwrap_or_default())
    } else {
        Vec::new()
    };
    let is_related = |a: &OutgoingAttachment| a.is_inline() && shown.contains(&a.inline_cid);
    let inline: Vec<&OutgoingAttachment> = attachments.iter().filter(|a| is_related(a)).collect();
    let files: Vec<&OutgoingAttachment> = attachments.iter().filter(|a| !is_related(a)).collect();
    let present: Vec<String> = inline.iter().map(|a| a.inline_cid.clone()).collect();
    let outer = format!("{}-mixed", boundary);
    let related = format!("{}-related", boundary);
    w.write_all(mime_headers(msg, from, reply, date).as_bytes())?;
    w.write_all(format!("Content-Type: multipart/mixed; boundary=\"{}\"\r\n\r\n--{}\r\n", outer, outer).as_bytes())?;
    if inline.is_empty() {
        w.write_all(body_entity(msg, &present, boundary).as_bytes())?;
    } else {
        w.write_all(format!("Content-Type: multipart/related; boundary=\"{}\"\r\n\r\n--{}\r\n", related, related).as_bytes())?;
        w.write_all(body_entity(msg, &present, boundary).as_bytes())?;
        for a in &inline {
            w.write_all(format!("--{}\r\n", related).as_bytes())?;
            w.write_all(part_headers(a, true).as_bytes())?;
            let mut f = std::fs::File::open(&a.path).map_err(|e| std::io::Error::new(e.kind(), format!("{}: {}", a.display_name(), e)))?;
            write_b64_stream(&mut f, w)?;
        }
        w.write_all(format!("--{}--\r\n", related).as_bytes())?;
    }
    for a in files {
        w.write_all(format!("--{}\r\n", outer).as_bytes())?;
        w.write_all(attachment_headers(a).as_bytes())?;
        let mut f = std::fs::File::open(&a.path).map_err(|e| std::io::Error::new(e.kind(), format!("{}: {}", a.display_name(), e)))?;
        write_b64_stream(&mut f, w)?;
    }
    w.write_all(format!("--{}--\r\n", outer).as_bytes())
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

    #[test]
    fn lists_expand_dedupe_across_fields_and_fail_loudly() {
        let members = |l: i64| -> Option<(String, Vec<Address>)> {
            match l {
                1 => Some(("Cadets".into(), vec![addr("a@x.org", "A"), addr("B@x.org", ""), addr("me@x.org", "")])),
                2 => Some(("Leaders".into(), vec![addr("b@x.org", ""), addr("lead@x.org", "L")])),
                3 => Some(("Empty".into(), vec![])),
                _ => None,
            }
        };
        let entry = |l: i64, n: &str| Address { email: String::new(), name: n.into(), list: l };
        let mut m = OutgoingMessage { to: vec![addr("me@x.org", "")], cc: vec![entry(2, "Leaders")], bcc: vec![entry(1, "Cadets"), addr("extra@x.org", "")], ..Default::default() };
        let rep = m.expand_lists(&members).unwrap();
        assert_eq!(rep, vec![("Leaders".to_string(), 2), ("Cadets".to_string(), 3)]);
        let e = |v: &Vec<Address>| v.iter().map(|a| a.email.to_lowercase()).collect::<Vec<_>>();
        assert_eq!(e(&m.to), vec!["me@x.org"]);
        assert_eq!(e(&m.cc), vec!["b@x.org", "lead@x.org"]);
        assert_eq!(e(&m.bcc), vec!["a@x.org", "extra@x.org"], "b@ stays in Cc, me@ stays in To");
        assert!(m.to.iter().chain(&m.cc).chain(&m.bcc).all(|a| a.list == 0));
        let mut gone = OutgoingMessage { to: vec![entry(9, "Old")], ..Default::default() };
        assert!(gone.expand_lists(&members).unwrap_err().contains("no longer exists"));
        let mut empty = OutgoingMessage { to: vec![entry(3, "Empty")], ..Default::default() };
        assert!(empty.expand_lists(&members).unwrap_err().contains("no members"));
        let mut plain = OutgoingMessage { to: vec![addr("a@x.org", ""), addr("A@x.org", "")], ..Default::default() };
        assert_eq!(plain.expand_lists(&members).unwrap(), vec![]);
        assert_eq!(plain.to.len(), 2, "no list entry: left exactly as typed");
    }

    fn addr(e: &str, n: &str) -> Address { Address { email: e.into(), name: n.into(), ..Default::default() } }

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

    // ---- provider limits (phase 0 measured: Gmail upload endpoints stop at 36,700,160 bytes)

    const MIB: u64 = 1024 * 1024;

    #[test]
    fn graph_counts_raw_bytes_against_150_mb() {
        let ok = check_attachments("exchange", 100, &[("a.bin".into(), 100 * MIB), ("b.bin".into(), 50 * MIB)]);
        assert_eq!(ok.counted, 150 * MIB);
        assert!(!ok.over_hard() && !ok.over_warn() && ok.error().is_none());
        let over = check_attachments("exchange", 100, &[("a.bin".into(), 40 * MIB), ("big.iso".into(), 111 * MIB)]);
        assert_eq!(over.error().unwrap(), "attachments total 151.0 MB; the limit is 150.0 MB (the biggest is big.iso, 111.0 MB)");
    }

    #[test]
    fn gmail_counts_the_encoded_message_and_names_the_biggest_file() {
        // 24 MB of files encode to about 32.9 MB: under the hard limit, over the soft one.
        let m = check_attachments("gmail", 1000, &[("a.pdf".into(), 24_000_000)]);
        assert!(m.counted > 32_000_000 && m.counted < 33_500_000, "{}", m.counted);
        assert!(!m.over_hard() && m.over_warn());
        assert!(m.error().is_none(), "a warning is not a refusal");
        // 27 MB of files encode past 35 MiB: refused, with the numbers.
        let big = check_attachments("gmail", 1000, &[("small.txt".into(), 1000), ("huge.zip".into(), 27 * MIB)]);
        assert!(big.over_hard());
        let e = big.error().unwrap();
        assert!(e.contains("Gmail takes at most 35.0 MB") && e.contains("the biggest is huge.zip, 27.0 MB"), "{}", e);
        // about 25 MB of raw files is the edge (35 MiB / 1.37)
        assert!(!check_attachments("gmail", 0, &[("a".into(), 26_500_000)]).over_hard());
        assert!(check_attachments("gmail", 0, &[("a".into(), 26_900_000)]).over_hard());
    }

    #[test]
    fn the_encoded_size_estimate_is_never_below_the_real_one() {
        // Real encoding: base64 (4 per 3) with CRLF every 76 characters, plus part headers.
        for raw in [0u64, 1, 2, 3, 57, 58, 1_000, 1_048_576, 10_000_001] {
            let real = raw.div_ceil(3) * 4 + (raw.div_ceil(3) * 4).div_ceil(76) * 2 + 200;
            assert!(encoded_part_size(raw) >= real, "{}", raw);
        }
    }

    #[test]
    fn limits_table_per_provider() {
        let g = attachment_limits("gmail");
        assert_eq!((g.basis, g.hard, g.warn), ("encoded", 36_700_160, Some(25_000_000)));
        let x = attachment_limits("exchange");
        assert_eq!((x.basis, x.hard, x.warn), ("raw", 150 * MIB, None));
        assert_eq!(serde_json::to_value(&g).unwrap()["hard"], 36_700_160);
    }

    #[test]
    fn validate_uses_the_accounts_provider() {
        let d = tmpdir("lim");
        let mut m = msg("text", "hi", "system");
        m.account_id = "gmail-abc123".into();
        let p = d.join("big.bin");
        let f = std::fs::File::create(&p).unwrap();
        f.set_len(27 * MIB).unwrap(); // sparse: costs no disk
        m.attachments = vec![OutgoingAttachment { path: p.to_string_lossy().into(), ..Default::default() }];
        assert!(m.validate().unwrap_err().contains("Gmail takes at most"));
        m.account_id = "exchange-abc123".into();
        assert!(m.validate().is_ok(), "27 MB is fine for Graph");
        std::fs::remove_dir_all(&d).ok();
    }

    // ---- MIME with attachments (goldens by structure; no MIME parser in the tree)

    const DATE: &str = "Thu, 08 Oct 2026 12:00:00 +0000";

    fn mime_with(m: &OutgoingMessage, files: &[OutgoingAttachment], reply: Option<&ReplyHeaders>) -> String {
        let mut out = Vec::new();
        write_mime(&mut out, m, files, "me@example.com", reply, "BND", DATE).unwrap();
        String::from_utf8(out).expect("MIME is 7-bit ASCII")
    }

    /// The attachment parts of a mixed message: (headers, decoded bytes).
    fn attachment_parts_of(raw: &str) -> Vec<(String, Vec<u8>)> {
        use base64::Engine;
        let mut out = Vec::new();
        for part in raw.split("--BND-mixed\r\n").skip(1) {
            let Some((head, rest)) = part.split_once("\r\n\r\n") else { continue };
            if !head.contains("Content-Disposition: attachment") {
                continue;
            }
            let body = rest.split("\r\n--BND-mixed").next().unwrap().replace("\r\n", "");
            out.push((head.to_string(), base64::engine::general_purpose::STANDARD.decode(body).unwrap()));
        }
        out
    }

    fn disk_file(dir: &std::path::Path, name: &str, bytes: &[u8]) -> OutgoingAttachment {
        file_in(dir, name, bytes)
    }

    #[test]
    fn one_file_makes_a_mixed_message_with_the_text_first() {
        let d = tmpdir("m1");
        let a = disk_file(&d, "report.pdf", b"%PDF-1.4 hello");
        let raw = mime_with(&msg("text", "see attached", "system"), &[a], None);
        assert!(raw.contains("MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"BND-mixed\"\r\n\r\n--BND-mixed\r\nContent-Type: text/plain; charset=UTF-8\r\n"), "{}", raw);
        assert_eq!(decode_part(&raw, "text/plain"), "see attached");
        let parts = attachment_parts_of(&raw);
        assert_eq!(parts.len(), 1);
        assert!(parts[0].0.contains("Content-Type: application/pdf;\r\n\tname=\"report.pdf\""), "{}", parts[0].0);
        assert!(parts[0].0.contains("filename=\"report.pdf\"") && !parts[0].0.contains("filename*"), "plain names have no RFC 2231 form");
        assert_eq!(parts[0].1, b"%PDF-1.4 hello");
        assert!(raw.ends_with("--BND-mixed--\r\n"));
        for l in raw.split("\r\n") { assert!(l.len() <= 998, "line too long"); }
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn many_files_keep_order_and_bytes_across_encoding_chunk_edges() {
        let d = tmpdir("m5");
        // Sizes chosen around the 57-byte line and the 57*2048-byte chunk.
        let chunk = 57 * 2048;
        let sizes = [0usize, 1, 2, 56, 57, 58, chunk - 1, chunk, chunk + 1, 2 * chunk + 5];
        let mut files = Vec::new();
        let mut want: Vec<Vec<u8>> = Vec::new();
        for (i, n) in sizes.iter().enumerate() {
            let bytes: Vec<u8> = (0..*n).map(|k| ((k * 31 + i * 7) % 251) as u8).collect();
            files.push(disk_file(&d, &format!("f{}.bin", i), &bytes));
            want.push(bytes);
        }
        let raw = mime_with(&msg("text", "x", "system"), &files, None);
        let parts = attachment_parts_of(&raw);
        assert_eq!(parts.len(), sizes.len());
        for (i, (head, bytes)) in parts.iter().enumerate() {
            assert!(head.contains(&format!("filename=\"f{}.bin\"", i)), "{}", head);
            assert_eq!(bytes, &want[i], "file {} ({} bytes)", i, sizes[i]);
        }
        // every base64 line is at most 76 characters
        for l in raw.split("\r\n") {
            if !l.is_empty() && !l.contains(':') && !l.starts_with("--") && !l.starts_with('\t') { assert!(l.len() <= 76, "{}", l.len()); }
        }
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_non_ascii_name_gets_an_ascii_fallback_and_an_rfc_2231_form() {
        let d = tmpdir("mu");
        let mut a = disk_file(&d, "plain.txt", b"x");
        a.name = "Prüfung \"Q3\" – 日本語.pdf".into();
        let raw = mime_with(&msg("text", "x", "system"), &[a], None);
        let head = &attachment_parts_of(&raw)[0].0;
        assert!(head.contains("filename=\"Pr_fung _Q3_ _ ___.pdf\""), "{}", head);
        assert!(head.contains("filename*=UTF-8''Pr%C3%BCfung%20%22Q3%22%20%E2%80%93%20%E6%97%A5%E6%9C%AC%E8%AA%9E.pdf"), "{}", head);
        // Gmail uses the first of the two, so the `*` form must come first
        assert!(head.find("filename*=").unwrap() < head.find("filename=\"").unwrap(), "{}", head);
        assert!(head.find("name*=").unwrap() < head.find("name=\"").unwrap(), "{}", head);
        assert!(head.contains("name*=UTF-8''Pr%C3%BCfung"), "Content-Type carries it too: {}", head);
        assert!(raw.is_ascii());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn html_with_a_file_nests_alternative_inside_mixed() {
        let d = tmpdir("mh");
        let a = disk_file(&d, "a.txt", b"data");
        let raw = mime_with(&msg("html", QT_HTML, "system"), &[a], None);
        let mixed = raw.find("Content-Type: multipart/mixed").unwrap();
        let alt = raw.find("Content-Type: multipart/alternative; boundary=\"BND\"").unwrap();
        let att = raw.find("Content-Disposition: attachment").unwrap();
        assert!(mixed < alt && alt < att);
        assert!(raw.contains("--BND--\r\n--BND-mixed\r\n"), "the alternative is closed before the file starts");
        assert!(decode_part(&raw, "text/plain").starts_with("On Oct 7 & Pat Lee"));
        assert_eq!(attachment_parts_of(&raw)[0].1, b"data");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_threaded_reply_with_a_file_keeps_its_headers_and_one_content_type() {
        let d = tmpdir("mr");
        let a = disk_file(&d, "a.txt", b"data");
        let r = ReplyHeaders { message_id: "<m2@x>".into(), references: "<m1@x>".into() };
        let mut m = msg("text", "re", "system");
        m.kind = "reply".into();
        let raw = mime_with(&m, &[a], Some(&r));
        assert!(raw.contains("In-Reply-To: <m2@x>\r\nReferences: <m1@x> <m2@x>\r\n"));
        let head = raw.split("\r\n\r\n").next().unwrap();
        assert_eq!(head.matches("Content-Type:").count(), 1, "{}", head);
        assert!(head.find("In-Reply-To").unwrap() < head.find("Content-Type").unwrap());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_large_file_is_streamed_in_small_writes() {
        struct Probe { biggest: usize, total: u64 }
        impl std::io::Write for Probe {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> { self.biggest = self.biggest.max(b.len()); self.total += b.len() as u64; Ok(b.len()) }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        let d = tmpdir("ml");
        let p = d.join("big.bin");
        std::fs::File::create(&p).unwrap().set_len(40 * 1024 * 1024).unwrap(); // sparse
        let a = OutgoingAttachment { path: p.to_string_lossy().into(), ..Default::default() };
        let mut probe = Probe { biggest: 0, total: 0 };
        write_mime(&mut probe, &msg("text", "x", "system"), &[a], "", None, "BND", DATE).unwrap();
        assert!(probe.biggest < 200_000, "largest single write {} bytes: the file must not be buffered whole", probe.biggest);
        assert!(probe.total >= 40 * 1024 * 1024 * 4 / 3 && probe.total <= encoded_part_size(40 * 1024 * 1024) + 2000, "{}", probe.total);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_missing_file_is_an_error_naming_the_file_not_a_truncated_message() {
        let a = OutgoingAttachment { path: "/nonexistent/gone.txt".into(), ..Default::default() };
        let mut out = Vec::new();
        let e = write_mime(&mut out, &msg("text", "x", "system"), &[a], "", None, "BND", DATE).unwrap_err();
        assert!(e.to_string().starts_with("gone.txt:"), "{}", e);
    }

    #[test]
    fn content_types_and_names_cannot_inject_headers() {
        assert_eq!(safe_content_type("application/pdf"), "application/pdf");
        assert_eq!(safe_content_type("text/x-custom+json"), "text/x-custom+json");
        assert_eq!(safe_content_type("x/y\r\nBcc: evil@example.com"), "application/octet-stream");
        assert_eq!(safe_content_type("nonsense"), "application/octet-stream");
        let a = OutgoingAttachment { path: "/x".into(), name: "a\r\nBcc: evil@example.com".into(), ..Default::default() };
        assert!(!attachment_headers(&a).contains("\r\nBcc:"), "{}", attachment_headers(&a));
        assert_eq!(rfc5987("a b"), "a%20b");
    }

    #[test]
    fn no_files_means_the_old_message_byte_for_byte() {
        let m = msg("html", QT_HTML, "system");
        let mut out = Vec::new();
        write_mime(&mut out, &m, &[], "me@example.com", None, "BND", DATE).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), build_mime(&m, "me@example.com", None, "BND", DATE));
    }
}

#[cfg(test)]
mod inline_tests {
    use super::*;

    fn html_msg(content: &str) -> OutgoingMessage {
        OutgoingMessage {
            kind: "new".into(),
            account_id: "exchange-aaaaaa".into(),
            to: vec![Address { email: "pat@example.com".into(), ..Default::default() }],
            subject: "Pictures".into(),
            mode: "html".into(),
            body: Body { format: "html".into(), content: content.into() },
            ..Default::default()
        }
    }

    fn pic(dir: &std::path::Path, name: &str, cid: &str, bytes: &[u8]) -> OutgoingAttachment {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        OutgoingAttachment {
            path: p.to_string_lossy().into(),
            name: name.into(),
            size: bytes.len() as u64,
            content_type: "image/png".into(),
            inline_cid: cid.into(),
            ..Default::default()
        }
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("olook-inline-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn cid_references_are_found_decoded_and_unique() {
        let html = r#"<p>a <img src="cid:img-1%40omalook" width="40"> b <IMG SRC='cid:img-2@omalook'><img src="cid:img-1@omalook"></p>"#;
        assert_eq!(referenced_cids(html), vec!["img-1@omalook".to_string(), "img-2@omalook".to_string()]);
        assert!(referenced_cids("<p>no pictures</p>").is_empty());
    }

    #[test]
    fn rewrite_removes_local_and_relative_sources_and_keeps_cid_and_http() {
        let html = concat!(
            r#"<img src="file:///home/adam/secret.png"> "#,
            r#"<img src="cid:keep-me@omalook"> "#,
            r#"<img src="cid:gone@omalook"> "#,
            r#"<img src="https://example.com/logo.png"> "#,
            r#"<img src="data:image/png;base64,AAAA"> "#,
            r#"<img src="relative.png"> "#,
            r#"<img alt="no source">"#,
        );
        let out = rewrite_images(html, &|c| c == "keep-me@omalook");
        assert!(!out.contains("file:"), "{}", out);
        assert!(!out.contains("data:"), "{}", out);
        assert!(!out.contains("relative.png"), "{}", out);
        assert!(!out.contains("no source"), "{}", out);
        assert!(!out.contains("cid:gone"), "{}", out);
        assert!(out.contains("src=\"cid:keep-me@omalook\""), "{}", out);
        assert!(out.contains("https://example.com/logo.png"), "{}", out);
        assert_eq!(out.matches("<img").count(), 2, "{}", out);
    }

    #[test]
    fn prepare_bodies_drops_pictures_nothing_supplies() {
        let m = html_msg(r#"<p>hi <img src="cid:a@omalook"> <img src="cid:b@omalook"></p>"#);
        let (html, text) = prepare_bodies_with(&m, &["a@omalook".to_string()]);
        let html = html.unwrap();
        assert!(html.contains("cid:a@omalook") && !html.contains("cid:b@omalook"), "{}", html);
        assert!(!text.contains("img"));
    }

    #[test]
    fn settle_keeps_referenced_pictures_and_drops_the_rest() {
        let d = tmp("settle");
        let mut m = html_msg(r#"<p><img src="cid:a@omalook"><img src="file:///etc/x.png"></p>"#);
        m.attachments = vec![
            pic(&d, "a.png", "a@omalook", b"aaa"),
            pic(&d, "gone.png", "gone@omalook", b"ggg"),
            pic(&d, "dup.png", "a@omalook", b"aaa"),
            pic(&d, "bad.png", "bad cid\r\n", b"bbb"),
            OutgoingAttachment { path: "/nope/report.pdf".into(), name: "report.pdf".into(), size: 1, ..Default::default() },
        ];
        m.settle_inline();
        assert!(!m.body.content.contains("file:"), "{}", m.body.content);
        assert!(m.body.content.contains("cid:a@omalook"));
        let names: Vec<&str> = m.attachments.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["a.png", "report.pdf"], "unreferenced, duplicate and unsafe pictures go; files stay");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn settle_drops_every_picture_from_a_text_body() {
        let d = tmp("settle-text");
        let mut m = html_msg("plain");
        m.body.format = "text".into();
        m.attachments = vec![pic(&d, "a.png", "a@omalook", b"aaa")];
        m.settle_inline();
        assert!(m.attachments.is_empty());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn inline_picture_is_a_related_part_with_content_id_and_the_files_stay_mixed() {
        let d = tmp("mime");
        let p = pic(&d, "photo.png", "img-1@omalook", b"\x89PNG-bytes");
        let mut f = pic(&d, "notes.txt", "", b"notes");
        f.content_type = "text/plain".into();
        f.path = d.join("notes.txt").to_string_lossy().into();
        let m = html_msg(r#"<p>look <img src="cid:img-1@omalook"></p>"#);
        let mut out = Vec::new();
        write_mime(&mut out, &m, &[p, f], "me@example.com", None, "BND", "Thu, 08 Oct 2026 12:00:00 +0000").unwrap();
        let raw = String::from_utf8(out).unwrap();
        assert!(raw.contains("Content-Type: multipart/related; boundary=\"BND-related\""), "{}", raw);
        assert!(raw.contains("Content-ID: <img-1@omalook>\r\nContent-Disposition: inline;"), "{}", raw);
        // The related block closes before the ordinary file part starts.
        let rel_close = raw.find("--BND-related--\r\n").expect("related closed");
        let file_part = raw.find("Content-Disposition: attachment;").expect("file part");
        assert!(rel_close < file_part);
        assert!(raw.contains("--BND-mixed\r\n"));
        // The picture's bytes are base64 of the file, not the bare file.
        use base64::Engine;
        let enc = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG-bytes");
        assert!(raw.contains(&enc));
        // Text part is the alternative inside related, so the picture can be referenced from it.
        assert!(raw.contains("--BND-related\r\nContent-Type: multipart/alternative; boundary=\"BND\""));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_picture_the_body_does_not_show_is_not_sent_as_a_related_part() {
        let d = tmp("mime-unref");
        let p = pic(&d, "photo.png", "img-9@omalook", b"png");
        let m = html_msg("<p>no picture here</p>");
        let mut out = Vec::new();
        write_mime(&mut out, &m, &[p], "me@example.com", None, "BND", "Thu, 08 Oct 2026 12:00:00 +0000").unwrap();
        let raw = String::from_utf8(out).unwrap();
        assert!(!raw.contains("multipart/related"), "no cid reference, no related block");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn graph_inline_fields_are_set_only_for_pictures() {
        let d = tmp("graph");
        let p = pic(&d, "photo.png", "img-1@omalook", b"png");
        let j = graph_file_attachment_json(&p, b"png");
        assert_eq!(j["isInline"], serde_json::json!(true));
        assert_eq!(j["contentId"], serde_json::json!("img-1@omalook"));
        let mut plain = p.clone();
        plain.inline_cid.clear();
        let j2 = graph_file_attachment_json(&plain, b"png");
        assert!(j2.get("isInline").is_none() && j2.get("contentId").is_none());
        let u = graph_upload_session_json(&p, 4);
        assert_eq!(u["AttachmentItem"]["isInline"], serde_json::json!(true));
        assert_eq!(u["AttachmentItem"]["contentId"], serde_json::json!("img-1@omalook"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn graph_message_body_keeps_only_the_pictures_it_is_given() {
        let m = html_msg(r#"<p><img src="cid:a@omalook"><img src="cid:b@omalook"></p>"#);
        let j = graph_message_json_with(&m, &["b@omalook".to_string()]);
        let content = j["body"]["content"].as_str().unwrap();
        assert!(!content.contains("cid:a@") && content.contains("cid:b@omalook"), "{}", content);
    }

    #[test]
    fn safe_cid_rejects_header_breakers() {
        assert!(safe_cid("img-1a@omalook"));
        assert!(!safe_cid(""));
        assert!(!safe_cid("a b"));
        assert!(!safe_cid("a\r\nX: y"));
        assert!(!safe_cid("a>b"));
    }
}
