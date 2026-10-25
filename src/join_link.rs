//! Join-link detection for calendar events (docs/PLAN-calendar-events.md 7.4).
//!
//! Order: the provider's structured link, then a known-host match in location and body, then the
//! first `https://` URL in the location. Only `https` links are ever returned.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinLink {
    /// teams | zoom | meet | webex | jitsi | link
    pub provider: &'static str,
    pub url: String,
}

/// Provider name for a URL host/path, None when it is not a known meeting service.
pub fn provider_of(url: &str) -> Option<&'static str> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.split(':').next().unwrap_or("").to_ascii_lowercase();
    let path = path.to_ascii_lowercase();
    let on = |d: &str| host == d || host.ends_with(&format!(".{}", d));
    if host == "teams.microsoft.com" && path.starts_with("l/meetup-join") {
        Some("teams")
    } else if host == "teams.live.com" && path.starts_with("meet") {
        Some("teams")
    } else if on("zoom.us") && (path.starts_with("j/") || path.starts_with("my/") || path.starts_with("wc/")) {
        Some("zoom")
    } else if host == "meet.google.com" {
        Some("meet")
    } else if on("webex.com") {
        Some("webex")
    } else if host == "meet.jit.si" {
        Some("jitsi")
    } else {
        None
    }
}

/// Every `https://...` token in `text`, with trailing punctuation and markup stripped.
fn https_urls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find("https://") {
        let start = from + i;
        let end = text[start..]
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | ')' | ']' | '}'))
            .map(|n| start + n)
            .unwrap_or(text.len());
        let url = text[start..end].trim_end_matches(|c: char| matches!(c, '.' | ',' | ';' | ':' | '!' | '?'));
        if url.len() > "https://".len() {
            out.push(url.to_string());
        }
        from = end.max(start + 1);
    }
    out
}

pub fn detect(structured: &str, location: &str, body: &str) -> Option<JoinLink> {
    if structured.starts_with("https://") {
        return Some(JoinLink { provider: provider_of(structured).unwrap_or("link"), url: structured.to_string() });
    }
    let loc_urls = https_urls(location);
    for url in loc_urls.iter().chain(https_urls(body).iter()) {
        if let Some(provider) = provider_of(url) {
            return Some(JoinLink { provider, url: url.clone() });
        }
    }
    loc_urls.into_iter().next().map(|url| JoinLink { provider: "link", url })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_link_wins_even_over_a_known_host_in_the_body() {
        let j = detect("https://teams.microsoft.com/l/meetup-join/abc", "", "https://zoom.us/j/1").unwrap();
        assert_eq!((j.provider, j.url.as_str()), ("teams", "https://teams.microsoft.com/l/meetup-join/abc"));
    }

    #[test]
    fn known_hosts_are_recognised_in_location_and_body() {
        let cases = [
            ("https://zoom.us/j/123456?pwd=x", "zoom"),
            ("https://acme.zoom.us/my/room", "zoom"),
            ("https://us02web.zoom.us/j/9", "zoom"),
            ("https://teams.live.com/meet/9342", "teams"),
            ("https://meet.google.com/abc-defg-hij", "meet"),
            ("https://acme.webex.com/meet/bob", "webex"),
            ("https://meet.jit.si/Standup", "jitsi"),
        ];
        for (url, provider) in cases {
            let j = detect("", "", &format!("Join: {}.", url)).unwrap_or_else(|| panic!("{}", url));
            assert_eq!((j.provider, j.url.as_str()), (provider, url), "{}", url);
        }
    }

    #[test]
    fn trailing_punctuation_and_html_do_not_end_up_in_the_url() {
        let j = detect("", "", "<a href=\"https://zoom.us/j/55\">https://zoom.us/j/55</a>, thanks").unwrap();
        assert_eq!(j.url, "https://zoom.us/j/55");
    }

    #[test]
    fn plain_location_url_is_the_fallback_but_body_urls_are_not() {
        assert_eq!(detect("", "https://example.com/room-4", "").unwrap().provider, "link");
        assert_eq!(detect("", "Room 4", "see https://example.com/agenda"), None);
    }

    #[test]
    fn non_https_and_lookalike_hosts_are_ignored() {
        assert_eq!(detect("http://zoom.us/j/1", "", ""), None);
        assert_eq!(detect("", "", "https://evilzoom.us/j/1 https://zoom.us.evil.com/j/1"), None);
        assert_eq!(detect("", "", "https://teams.microsoft.com/other"), None);
    }
}
