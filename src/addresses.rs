//! Addresses seen in mail, for recipient suggestions (docs/PLAN-distribution-lists.md).
//!
//! Every address that appears in the From, To, Cc or Bcc of a cached message (Bcc exists only on
//! sent items) is counted. Addresses on mail the user sent score higher than addresses on mail
//! received, because those are the people the user actually writes to. The user's own account
//! addresses and obvious no-reply senders are left out.

use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub email: String,
    pub name: String,
    pub score: u32,
}

/// Split a header-style recipient text ("Bob <b@x.y>, c@x.y; \"Doe, Jo\" <j@x.y>") into
/// (name, email) pairs. Commas inside quotes or angle brackets do not split; an unquoted
/// "Last, First <a@b>" (what Exchange writes) is put back together as the name.
pub fn parse_addresses(text: &str) -> Vec<(String, String)> {
    let mut parts: Vec<String> = Vec::new();
    let (mut cur, mut quoted, mut angle) = (String::new(), false, false);
    for ch in text.chars() {
        match ch {
            '"' => quoted = !quoted,
            '<' if !quoted => angle = true,
            '>' if !quoted => angle = false,
            _ => {}
        }
        if !quoted && !angle && (ch == ',' || ch == ';' || ch == '\n') {
            parts.push(std::mem::take(&mut cur));
        } else {
            cur.push(ch);
        }
    }
    parts.push(cur);

    let mut out = Vec::new();
    let mut carry = String::new();
    for p in parts {
        let p = p.trim();
        if p.is_empty() { continue; }
        if !p.contains('@') {
            // "Doe" of "Doe, John <j@x.y>": hold it for the next part.
            if !carry.is_empty() { carry.push_str(", "); }
            carry.push_str(p.trim_matches('"'));
            continue;
        }
        let (mut name, email) = match (p.rfind('<'), p.rfind('>')) {
            (Some(a), Some(b)) if a < b => (p[..a].trim().trim_matches('"').trim().to_string(), p[a + 1..b].trim().to_string()),
            _ => (String::new(), p.to_string()),
        };
        if !carry.is_empty() {
            name = if name.is_empty() { std::mem::take(&mut carry) } else { format!("{}, {}", std::mem::take(&mut carry), name) };
        }
        if crate::compose::is_valid_email(&email) {
            if name.eq_ignore_ascii_case(&email) { name.clear(); }
            out.push((name, email));
        }
    }
    out
}

fn is_noise(email: &str) -> bool {
    let local = email.split('@').next().unwrap_or("");
    let l = local.to_ascii_lowercase();
    l.contains("noreply") || l.contains("no-reply") || l.contains("donotreply") || l.contains("do-not-reply")
        || l.starts_with("bounce") || l.starts_with("mailer-daemon") || l.starts_with("notifications")
}

/// One cached message as far as suggestions care.
pub struct MessageAddrs {
    pub from_email: String,
    pub from_name: String,
    pub to: String,
    pub cc: String,
    pub bcc: String,
}

/// Count addresses over `messages` (newest first, so the first name seen for an address is the
/// most recent one), best first. `own` are the user's account addresses (lower case).
pub fn harvest<I: IntoIterator<Item = MessageAddrs>>(messages: I, own: &HashSet<String>) -> Vec<Seen> {
    let mut map: HashMap<String, (String, u32, usize)> = HashMap::new(); // key -> (name, score, first-seen order)
    let mut order = 0usize;
    let mut bump = |name: &str, email: &str, w: u32| {
        let key = email.trim().to_ascii_lowercase();
        // Exchange writes internal senders as "/o=ExchangeLabs/..." (no address): not suggestible.
        if key.is_empty() || !crate::compose::is_valid_email(&key) || own.contains(&key) || is_noise(&key) { return; }
        let e = map.entry(key).or_insert_with(|| { order += 1; (String::new(), 0, order) });
        e.1 += w;
        if e.0.is_empty() && !name.is_empty() { e.0 = name.to_string(); }
    };
    for m in messages {
        let sent_by_me = own.contains(&m.from_email.trim().to_ascii_lowercase());
        if sent_by_me {
            for text in [&m.to, &m.cc, &m.bcc] {
                for (n, e) in parse_addresses(text) { bump(&n, &e, 3); }
            }
        } else {
            bump(&m.from_name, &m.from_email, 1);
            for text in [&m.to, &m.cc, &m.bcc] {
                for (n, e) in parse_addresses(text) { bump(&n, &e, 1); }
            }
        }
    }
    let mut v: Vec<(String, String, u32, usize)> = map.into_iter().map(|(k, (n, s, o))| (k, n, s, o)).collect();
    v.sort_by(|a, b| b.2.cmp(&a.2).then(a.3.cmp(&b.3)));
    v.into_iter().map(|(email, name, score, _)| Seen { email, name, score }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_named_quoted_and_exchange_style() {
        let p = parse_addresses("Bob <bob@x.org>, cy@x.org; \"Doe, Jo\" <jo@x.org>, Smith, Ann <ann@x.org>");
        assert_eq!(p, vec![
            ("Bob".to_string(), "bob@x.org".to_string()),
            ("".to_string(), "cy@x.org".to_string()),
            ("Doe, Jo".to_string(), "jo@x.org".to_string()),
            ("Smith, Ann".to_string(), "ann@x.org".to_string()),
        ]);
    }

    #[test]
    fn garbage_and_name_equal_to_address_are_handled() {
        assert!(parse_addresses("undisclosed-recipients:;").is_empty());
        assert_eq!(parse_addresses("a@b.co <a@b.co>"), vec![("".to_string(), "a@b.co".to_string())]);
    }

    #[test]
    fn sent_mail_outranks_received_cc_and_bcc_are_counted_own_and_noreply_dropped() {
        let own: HashSet<String> = ["me@x.org".to_string()].into();
        let msgs = vec![
            MessageAddrs { from_email: "me@x.org".into(), from_name: "Me".into(), to: "Cadet <cadet@y.org>".into(), cc: "lead@y.org".into(), bcc: "Hidden <hid@y.org>, me@x.org".into() },
            MessageAddrs { from_email: "Boss@Y.org".into(), from_name: "Boss".into(), to: "me@x.org".into(), cc: "cadet@y.org".into(), bcc: "".into() },
            MessageAddrs { from_email: "/o=ExchangeLabs/ou=Exchange Administrative Group/cn=Recipients/cn=abc-boss".into(), from_name: "Legacy".into(), to: "me@x.org".into(), cc: "".into(), bcc: "".into() },
            MessageAddrs { from_email: "noreply@shop.com".into(), from_name: "Shop".into(), to: "me@x.org".into(), cc: "".into(), bcc: "".into() },
        ];
        let v = harvest(msgs, &own);
        let emails: Vec<&str> = v.iter().map(|s| s.email.as_str()).collect();
        assert_eq!(emails, vec!["cadet@y.org", "lead@y.org", "hid@y.org", "boss@y.org"]);
        assert_eq!(v[0].score, 4);
        assert_eq!(v[0].name, "Cadet");
        assert!(!emails.contains(&"me@x.org") && !emails.contains(&"noreply@shop.com"));
    }
}
