//! Bounce / redirect of a received message (OL-052, mutt's `b`): the original is sent on to new recipients.
//!
//! Neither provider can do a true bounce (resend with the original From and an envelope of the user's choosing):
//! Gmail's `messages.send` and Exchange's `sendMail` both take the recipients from the To / Cc / Bcc headers and
//! always send as the signed-in mailbox. So the honest equivalent is built here: the original message's bytes, body
//! and attachments untouched, with the transport headers replaced:
//!   * From: the user (the only address the provider lets the user send as); Date / Message-ID are fresh;
//!   * To / Cc: the new recipients; the original To / Cc / Bcc are dropped (nobody else gets it);
//!   * Reply-To: the original's Reply-To, else the original From, so a reply goes to the author, not the user;
//!   * X-Original-From, Resent-From / Resent-To / Resent-Date: who it came from and who passed it on;
//!   * the original's DKIM / ARC / Received / Return-Path and receipt requests are dropped (a DKIM signature
//!     would fail after the changes and a receipt would go to the author).
//! The recipient therefore sees a message from the user with the original's subject, body and files.

use crate::compose::{is_valid_email, Address};

/// Headers that belong to the original transport, or that the bounce replaces (case-insensitive).
const DROP: &[&str] = &[
    "from", "sender", "to", "cc", "bcc", "date", "message-id", "reply-to", "return-path", "delivered-to", "received", "x-received",
    "dkim-signature", "domainkey-signature", "authentication-results", "received-spf", "disposition-notification-to",
    "return-receipt-to", "x-original-from", "x-original-to", "x-forwarded-to", "x-forwarded-for", "envelope-to",
];
const DROP_PREFIX: &[&str] = &["arc-", "resent-", "x-google-", "x-gm-", "x-ms-", "x-microsoft-"];

fn dropped(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    DROP.contains(&n.as_str()) || DROP_PREFIX.iter().any(|p| n.starts_with(p))
}

/// Header section / body split at the first empty line. Returns (headers, separator, body).
fn split(raw: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    for i in 0..raw.len() {
        if raw[i..].starts_with(b"\r\n\r\n") {
            return Some((&raw[..i], &raw[i..i + 4], &raw[i + 4..]));
        }
        if raw[i..].starts_with(b"\n\n") {
            return Some((&raw[..i], &raw[i..i + 2], &raw[i + 2..]));
        }
    }
    None
}

/// Logical headers: (name, whole text with its folds). Garbage lines without a colon are dropped.
fn logical_headers(head: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    for line in head.split(|b| *b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.first().map(|b| *b == b' ' || *b == b'\t').unwrap_or(false) {
            if let Some((_, text)) = out.last_mut() {
                text.extend_from_slice(b"\r\n");
                text.extend_from_slice(line);
            }
            continue;
        }
        let Some(c) = line.iter().position(|b| *b == b':') else { continue };
        let name = String::from_utf8_lossy(&line[..c]).trim().to_string();
        if name.is_empty() || name.contains(' ') {
            continue;
        }
        out.push((name, line.to_vec()));
    }
    out
}

/// Value of a header, unfolded, as text.
fn value(h: &(String, Vec<u8>)) -> String {
    let t = String::from_utf8_lossy(&h.1).to_string();
    let v = t.splitn(2, ':').nth(1).unwrap_or("").to_string();
    v.replace("\r\n", "").trim().to_string()
}

fn addr_line(list: &[Address]) -> String {
    list.iter()
        .map(|a| {
            let n: String = a.name.chars().filter(|c| !matches!(c, '"' | '\r' | '\n' | '<' | '>' | '\\')).collect();
            // A non-ASCII name would need RFC 2047 encoding; the bare address is always right.
            if n.trim().is_empty() || !n.is_ascii() {
                a.email.trim().to_string()
            } else {
                format!("\"{}\" <{}>", n.trim(), a.email.trim())
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Who wrote the original, for the confirmation text.
pub struct Origin {
    pub from: String,
    pub reply_to: String,
    pub subject: String,
}

pub fn origin(raw: &[u8]) -> Option<Origin> {
    let (head, _, _) = split(raw)?;
    let hs = logical_headers(head);
    let get = |n: &str| hs.iter().find(|h| h.0.eq_ignore_ascii_case(n)).map(value).unwrap_or_default();
    Some(Origin { from: get("From"), reply_to: get("Reply-To"), subject: get("Subject") })
}

/// The message to send: the original with the headers above replaced. Errors (as text for the user) when `raw` is not
/// a message, an address is not valid, or there is no recipient.
pub fn rewrite(raw: &[u8], from: &str, to: &[Address], cc: &[Address], date: &str, message_id: &str) -> Result<Vec<u8>, String> {
    if to.is_empty() {
        return Err("a bounce needs at least one recipient".into());
    }
    if let Some(bad) = to.iter().chain(cc).find(|a| !is_valid_email(&a.email)) {
        return Err(format!("not a valid address: {}", bad.email));
    }
    if !is_valid_email(from) || message_id.contains(['\r', '\n']) || date.contains(['\r', '\n']) {
        return Err("the sending address is not usable".into());
    }
    let (head, _, body) = split(raw).ok_or_else(|| "the original message has no body separator; it cannot be bounced".to_string())?;
    let hs = logical_headers(head);
    let get = |n: &str| hs.iter().find(|h| h.0.eq_ignore_ascii_case(n)).map(value).unwrap_or_default();
    let orig_from = get("From");
    let reply_to = {
        let r = get("Reply-To");
        if r.is_empty() { orig_from.clone() } else { r }
    };
    let one_line = |s: String| s.replace(['\r', '\n'], " ");

    let mut out: Vec<u8> = Vec::with_capacity(raw.len() + 512);
    let mut put = |s: String| {
        out.extend_from_slice(s.as_bytes());
        out.extend_from_slice(b"\r\n");
    };
    put(format!("From: {}", from.trim()));
    put(format!("To: {}", addr_line(to)));
    if !cc.is_empty() {
        put(format!("Cc: {}", addr_line(cc)));
    }
    put(format!("Date: {}", date));
    put(format!("Message-ID: {}", message_id));
    if !reply_to.is_empty() {
        put(format!("Reply-To: {}", one_line(reply_to)));
    }
    if !orig_from.is_empty() {
        put(format!("X-Original-From: {}", one_line(orig_from)));
    }
    put(format!("Resent-From: {}", from.trim()));
    put(format!("Resent-To: {}", addr_line(&to.iter().chain(cc).cloned().collect::<Vec<_>>())));
    put(format!("Resent-Date: {}", date));
    for h in hs.iter().filter(|h| !dropped(&h.0)) {
        out.extend_from_slice(&h.1);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"\r\n");
    // The body keeps its own line endings, as the original had them.
    out.extend_from_slice(body);
    Ok(out)
}

/// The `Message-ID` of a bounce: unique, in the sender's own domain.
pub fn new_message_id(from: &str, unique: u128) -> String {
    let domain = from.rsplit('@').next().unwrap_or("localhost").trim();
    format!("<bounce.{:x}@{}>", unique, domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(e: &str) -> Address { Address { email: e.into(), name: String::new(), list: 0 } }
    const RAW: &str = "Received: from mx by y\r\nReturn-Path: <bob@o.org>\r\nDKIM-Signature: v=1; a=rsa;\r\n b=abc\r\nARC-Seal: i=1\r\nFrom: \"Bob\" <bob@o.org>\r\nTo: me@x.org,\r\n carol@z.org\r\nCc: dan@z.org\r\nBcc: hidden@z.org\r\nSubject: Re: =?UTF-8?B?w6RiYw==?=\r\nMessage-ID: <orig@o.org>\r\nDate: Mon, 1 Jan 2026 10:00:00 +0000\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"B\"\r\nDisposition-Notification-To: bob@o.org\r\nX-Gm-Message-State: zzz\r\nIn-Reply-To: <p@o.org>\r\n\r\n--B\r\nContent-Type: text/plain\r\n\r\nhello\r\n--B--\r\n";

    fn run(to: &[&str], cc: &[&str]) -> String {
        let to: Vec<Address> = to.iter().map(|e| a(e)).collect();
        let cc: Vec<Address> = cc.iter().map(|e| a(e)).collect();
        String::from_utf8(rewrite(RAW.as_bytes(), "me@x.org", &to, &cc, "Thu, 12 Nov 2026 10:00:00 +0000", "<bounce.1@x.org>").unwrap()).unwrap()
    }

    #[test]
    fn the_original_recipients_and_transport_headers_are_replaced() {
        let m = run(&["new@y.org"], &[]);
        let (head, body) = m.split_once("\r\n\r\n").unwrap();
        assert_eq!(body, "--B\r\nContent-Type: text/plain\r\n\r\nhello\r\n--B--\r\n", "the body is untouched");
        for gone in ["carol@z.org", "dan@z.org", "hidden@z.org", "orig@o.org", "DKIM", "ARC-", "Received:", "Return-Path", "Disposition", "X-Gm-", "Mon, 1 Jan"] {
            assert!(!head.contains(gone), "{} must be dropped: {}", gone, head);
        }
        assert!(head.starts_with("From: me@x.org\r\nTo: new@y.org\r\nDate: Thu, 12 Nov 2026"));
        for kept in ["Message-ID: <bounce.1@x.org>", "Reply-To: \"Bob\" <bob@o.org>", "X-Original-From: \"Bob\" <bob@o.org>", "Resent-From: me@x.org",
                     "Resent-To: new@y.org", "Subject: Re: =?UTF-8?B?w6RiYw==?=", "MIME-Version: 1.0", "Content-Type: multipart/mixed; boundary=\"B\"", "In-Reply-To: <p@o.org>"] {
            assert!(head.contains(kept), "{} must stay or be added: {}", kept, head);
        }
        assert_eq!(head.matches("From:").count(), 3, "From, X-Original-From, Resent-From only");
    }

    #[test]
    fn reply_to_of_the_original_wins_and_cc_is_carried() {
        let raw = RAW.replace("Subject:", "Reply-To: list@o.org\r\nSubject:");
        let m = String::from_utf8(rewrite(raw.as_bytes(), "me@x.org", &[a("n@y.org")], &[a("c@y.org")], "d", "<i@x.org>").unwrap()).unwrap();
        assert!(m.contains("Reply-To: list@o.org\r\n") && !m.contains("Reply-To: \"Bob\""));
        assert!(m.contains("Cc: c@y.org\r\n") && m.contains("Resent-To: n@y.org, c@y.org\r\n"));
    }

    #[test]
    fn refusals_and_injection() {
        let raw = RAW.as_bytes();
        assert!(rewrite(raw, "me@x.org", &[], &[], "d", "<i>").is_err());
        assert!(rewrite(raw, "me@x.org", &[a("nope")], &[], "d", "<i>").unwrap_err().contains("nope"));
        assert!(rewrite(b"no separator", "me@x.org", &[a("a@b.org")], &[], "d", "<i>").is_err());
        assert!(rewrite(raw, "me@x.org", &[a("a@b.org")], &[], "d\r\nBcc: x@y.org", "<i>").is_err());
        let mut n = a("a@b.org");
        n.name = "Eve\r\nBcc: evil@y.org".into();
        let m = String::from_utf8(rewrite(raw, "me@x.org", &[n], &[], "d", "<i>").unwrap()).unwrap();
        assert!(!m.contains("\r\nBcc: evil"));
    }

    #[test]
    fn lf_only_messages_and_origin_facts() {
        let lf = "From: Bob <b@o.org>\nSubject: hi\nTo: me@x.org\n\nbody\n";
        let m = String::from_utf8(rewrite(lf.as_bytes(), "me@x.org", &[a("n@y.org")], &[], "d", "<i>").unwrap()).unwrap();
        assert!(m.contains("Subject: hi\r\n\r\nbody\n"));
        let o = origin(RAW.as_bytes()).unwrap();
        assert_eq!((o.from.as_str(), o.reply_to.as_str()), ("\"Bob\" <bob@o.org>", ""));
        assert!(o.subject.starts_with("Re: "));
        assert_eq!(new_message_id("me@x.org", 255), "<bounce.ff@x.org>");
    }

    /// `BOUNCE_SAMPLE=/path/to/message.eml cargo test --lib bounce_sample -- --ignored --nocapture`: a real message, header part only.
    #[test]
    #[ignore]
    fn bounce_sample() {
        let Ok(p) = std::env::var("BOUNCE_SAMPLE") else { return };
        let raw = std::fs::read(p).unwrap();
        let out = rewrite(&raw, "me@x.org", &[a("new@y.org")], &[], "Thu, 12 Nov 2026 10:00:00 +0000", "<bounce.1@x.org>").unwrap();
        let (h, _, b) = split(&out).unwrap();
        let (oh, _, ob) = split(&raw).unwrap();
        println!("{}\n-- body kept byte for byte: {} ({} bytes); original header names: {:?}", String::from_utf8_lossy(h), b == ob, b.len(),
            logical_headers(oh).iter().map(|x| x.0.clone()).collect::<Vec<_>>());
    }
}
