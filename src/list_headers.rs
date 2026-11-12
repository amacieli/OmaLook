//! Mailing-list detection for reply-to-list (OL-053). The cache keeps no `List-*` headers, so they are
//! fetched lazily per message (Graph `internetMessageHeaders`, Gmail `format=metadata`) and the answer is
//! cached in `list_info`. Parsing is pure and lives here.

use crate::compose::is_valid_email;

/// What a `List-Post` (or `Mailing-List`) header says about posting.
#[derive(Debug, Clone, PartialEq)]
pub enum ListPost {
    /// No usable header.
    Absent,
    /// `List-Post: NO`: the list does not accept posts (announce-only).
    NoPosting,
    /// The address to post to.
    Address(String),
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The address of one `mailto:` URI (`mailto:list@x.org?subject=help` -> `list@x.org`); None for anything
/// else or an invalid address.
fn mailto_address(uri: &str) -> Option<String> {
    let u = uri.trim();
    if u.len() < 7 || !u[..7].eq_ignore_ascii_case("mailto:") {
        return None;
    }
    let rest = &u[7..];
    let addr = percent_decode(rest.split('?').next().unwrap_or("")).trim().to_string();
    // A mailto may name several recipients; a list posts to one.
    let addr = addr.split(',').next().unwrap_or("").trim().to_string();
    is_valid_email(&addr).then_some(addr)
}

/// Parse a `List-Post` value: `<mailto:list@x.org>`, `<mailto:a@b?subject=x>, <https://...>`,
/// `NO`, `NO (posting not allowed)`. The first valid `mailto:` wins; URLs are ignored.
pub fn parse_list_post(value: &str) -> ListPost {
    let v = value.trim();
    if v.is_empty() {
        return ListPost::Absent;
    }
    let mut rest = v;
    let mut found_angle = false;
    while let Some(start) = rest.find('<') {
        let Some(len) = rest[start + 1..].find('>') else { break };
        found_angle = true;
        let item = &rest[start + 1..start + 1 + len];
        if let Some(a) = mailto_address(item) {
            return ListPost::Address(a);
        }
        rest = &rest[start + 1 + len + 1..];
    }
    if found_angle {
        return ListPost::Absent;
    }
    // Not in angle brackets: `NO`, or a bare mailto.
    let head: String = v.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    if head.eq_ignore_ascii_case("no") && v[head.len()..].chars().next().map(|c| !c.is_ascii_alphanumeric()).unwrap_or(true) {
        return ListPost::NoPosting;
    }
    mailto_address(v.split_whitespace().next().unwrap_or("")).map(ListPost::Address).unwrap_or(ListPost::Absent)
}

/// `Mailing-List: list foo@yahoogroups.com; contact foo-owner@yahoogroups.com` (ezmlm, Yahoo groups).
fn parse_mailing_list(value: &str) -> ListPost {
    for part in value.split(';') {
        let p = part.trim();
        if p.len() > 5 && p[..5].eq_ignore_ascii_case("list ") {
            let a = p[5..].trim();
            if is_valid_email(a) {
                return ListPost::Address(a.to_string());
            }
        }
    }
    ListPost::Absent
}

/// The list identity of one message.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ListInfo {
    /// Any of `List-Id`, `List-Post`, `Mailing-List` was present.
    pub is_list: bool,
    pub list_id: String,
    /// Where to post; None when unknown.
    pub post: Option<String>,
    /// `List-Post: NO` and no other header gave an address.
    pub no_post: bool,
}

impl ListInfo {
    /// From header (name, value) pairs; names compare case-insensitively.
    pub fn from_headers(headers: &[(String, String)]) -> ListInfo {
        let get = |n: &str| headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.as_str());
        let list_id = get("List-Id").map(|v| v.trim().to_string()).unwrap_or_default();
        let mut post = get("List-Post").map(parse_list_post).unwrap_or(ListPost::Absent);
        if !matches!(post, ListPost::Address(_)) {
            if let Some(ListPost::Address(a)) = get("Mailing-List").map(parse_mailing_list) {
                post = ListPost::Address(a);
            }
        }
        let is_list = !list_id.is_empty() || get("List-Post").is_some() || get("Mailing-List").is_some();
        match post {
            ListPost::Address(a) => ListInfo { is_list, list_id, post: Some(a), no_post: false },
            ListPost::NoPosting => ListInfo { is_list, list_id, post: None, no_post: true },
            ListPost::Absent => ListInfo { is_list, list_id, post: None, no_post: false },
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "is_list": self.is_list, "list_id": self.list_id, "post": self.post, "no_post": self.no_post })
    }

    pub fn from_json(v: &serde_json::Value) -> ListInfo {
        ListInfo {
            is_list: v["is_list"].as_bool().unwrap_or(false),
            list_id: v["list_id"].as_str().unwrap_or("").to_string(),
            post: v["post"].as_str().map(str::to_string),
            no_post: v["no_post"].as_bool().unwrap_or(false),
        }
    }
}

/// Header pairs from Graph's `internetMessageHeaders` (`[{name, value}]`).
pub fn headers_from_graph(v: &serde_json::Value) -> Vec<(String, String)> {
    v.as_array().into_iter().flatten()
        .filter_map(|h| Some((h["name"].as_str()?.to_string(), h["value"].as_str()?.to_string())))
        .collect()
}

/// Header pairs from a Gmail message resource (`payload.headers`).
pub fn headers_from_gmail(msg: &serde_json::Value) -> Vec<(String, String)> {
    headers_from_graph(&msg["payload"]["headers"])
}

/// The words shown when reply-to-list cannot go ahead; None when it can.
pub fn refusal(info: &ListInfo) -> Option<&'static str> {
    if !info.is_list {
        Some("This message did not come through a mailing list.")
    } else if info.no_post {
        Some("This list does not accept posts (List-Post: NO).")
    } else if info.post.is_none() {
        Some("This is list mail, but it does not say where to post. Use Reply all.")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> { pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect() }

    #[test]
    fn list_post_forms() {
        use ListPost::*;
        assert_eq!(parse_list_post("<mailto:dev@lists.example.org>"), Address("dev@lists.example.org".into()));
        assert_eq!(parse_list_post("  <MAILTO:Dev@Lists.Example.org?subject=hi>  "), Address("Dev@Lists.Example.org".into()));
        assert_eq!(parse_list_post("<https://x.org/post>, <mailto:a@b.org>"), Address("a@b.org".into()));
        assert_eq!(parse_list_post("<mailto:a%2Bb@b.org>"), Address("a+b@b.org".into()));
        assert_eq!(parse_list_post("<mailto:a@b.org> (Post)"), Address("a@b.org".into()));
        assert_eq!(parse_list_post("mailto:a@b.org"), Address("a@b.org".into()));
        assert_eq!(parse_list_post("NO"), NoPosting);
        assert_eq!(parse_list_post("no (posting not allowed)"), NoPosting);
        assert_eq!(parse_list_post("<https://x.org/post>"), Absent);
        assert_eq!(parse_list_post("<mailto:not an address>"), Absent);
        assert_eq!(parse_list_post("<mailto:>"), Absent);
        assert_eq!(parse_list_post("nobody@x.org"), Absent);
        assert_eq!(parse_list_post("<mailto:a@b.org"), Absent);
        assert_eq!(parse_list_post(""), Absent);
        assert_eq!(parse_list_post("%"), Absent);
    }

    #[test]
    fn info_from_headers() {
        let i = ListInfo::from_headers(&h(&[("list-id", "Dev <dev.lists.example.org>"), ("LIST-POST", "<mailto:dev@lists.example.org>")]));
        assert!(i.is_list);
        assert_eq!(i.post.as_deref(), Some("dev@lists.example.org"));
        assert_eq!(i.list_id, "Dev <dev.lists.example.org>");
        assert_eq!(refusal(&i), None);

        let announce = ListInfo::from_headers(&h(&[("List-Id", "<news.x.org>"), ("List-Post", "NO")]));
        assert!(announce.is_list && announce.no_post && announce.post.is_none());
        assert!(refusal(&announce).unwrap().contains("does not accept"));

        let id_only = ListInfo::from_headers(&h(&[("List-Id", "<x.y>")]));
        assert!(id_only.is_list && id_only.post.is_none() && !id_only.no_post);
        assert!(refusal(&id_only).unwrap().contains("Reply all"));

        let yahoo = ListInfo::from_headers(&h(&[("Mailing-List", "list foo@yahoogroups.com; contact foo-owner@yahoogroups.com")]));
        assert_eq!(yahoo.post.as_deref(), Some("foo@yahoogroups.com"));

        // Newsletters carry only List-Unsubscribe: not a list you can reply to.
        let news = ListInfo::from_headers(&h(&[("List-Unsubscribe", "<mailto:u@x.org>"), ("From", "a@b.c")]));
        assert!(!news.is_list);
        assert!(refusal(&news).unwrap().contains("did not come"));
    }

    #[test]
    fn json_round_trip_and_provider_shapes() {
        let i = ListInfo { is_list: true, list_id: "<a.b>".into(), post: Some("a@b.org".into()), no_post: false };
        assert_eq!(ListInfo::from_json(&i.to_json()), i);
        let graph = serde_json::json!([{ "name": "List-Post", "value": "<mailto:a@b.org>" }, { "name": "X", "value": "y" }]);
        assert_eq!(ListInfo::from_headers(&headers_from_graph(&graph)).post.as_deref(), Some("a@b.org"));
        let gmail = serde_json::json!({ "payload": { "headers": [{ "name": "List-Id", "value": "<a.b>" }] } });
        assert!(ListInfo::from_headers(&headers_from_gmail(&gmail)).is_list);
    }
}
