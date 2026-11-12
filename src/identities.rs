//! Send-as identities (OL-051): the addresses an account may put in From. Gmail lists them as `sendAs`
//! (settings API, `gmail.settings.basic` is already requested); Exchange keeps them as `proxyAddresses`
//! on `/me` (`User.Read`). Whether Exchange accepts one as the `from` of a Graph message is up to the
//! tenant: Graph answers the send with its own refusal, which the outbox then shows.

use crate::compose::is_valid_email;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Identity {
    pub email: String,
    pub name: String,
    /// The account's own sign-in address.
    pub primary: bool,
    /// The provider's own default (Gmail `isDefault`); the primary for Exchange.
    pub is_default: bool,
}

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Gmail `settings.sendAs.list` -> identities. Only the primary address and verified ones (`verificationStatus`
/// "accepted") are usable: Gmail silently replaces the From of a message sent as anything else.
pub fn parse_gmail_send_as(json: &Value) -> Vec<Identity> {
    let mut out: Vec<Identity> = Vec::new();
    for s in json["sendAs"].as_array().into_iter().flatten() {
        let Some(email) = s["sendAsEmail"].as_str().map(str::trim).filter(|e| is_valid_email(e)) else { continue };
        let primary = s["isPrimary"].as_bool().unwrap_or(false);
        let status = s["verificationStatus"].as_str().unwrap_or("accepted");
        if !primary && status != "accepted" {
            continue;
        }
        if out.iter().any(|i| same(&i.email, email)) {
            continue;
        }
        out.push(Identity {
            email: email.to_string(),
            name: s["displayName"].as_str().unwrap_or("").trim().to_string(),
            primary,
            is_default: s["isDefault"].as_bool().unwrap_or(false),
        });
    }
    out
}

/// Graph `/me?$select=displayName,mail,userPrincipalName,proxyAddresses` -> identities. `SMTP:` (capitals) is the primary
/// SMTP address, `smtp:` an alias; other address types (`x500:`, `SIP:`, `SPO:`) are not mail. The
/// `*.onmicrosoft.com` routing addresses are left out: nobody writes from them. `otherMails` is a contact
/// address of the user, not a mailbox alias, and is never used.
pub fn parse_graph_proxies(me: &Value, account_email: &str) -> Vec<Identity> {
    let name = me["displayName"].as_str().unwrap_or("").trim().to_string();
    let mut out: Vec<Identity> = Vec::new();
    for p in me["proxyAddresses"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let Some((kind, addr)) = p.split_once(':') else { continue };
        if !kind.eq_ignore_ascii_case("smtp") {
            continue;
        }
        let addr = addr.trim();
        if !is_valid_email(addr) || addr.to_ascii_lowercase().ends_with(".onmicrosoft.com") {
            continue;
        }
        if out.iter().any(|i| same(&i.email, addr)) {
            continue;
        }
        let upper = kind == "SMTP";
        out.push(Identity { email: addr.to_string(), name: name.clone(), primary: same(addr, account_email) || (upper && account_email.is_empty()), is_default: upper });
    }
    ensure_primary(out, account_email)
}

/// The account's own address is always an identity, and the first one.
pub fn ensure_primary(mut list: Vec<Identity>, account_email: &str) -> Vec<Identity> {
    let acct = account_email.trim();
    if acct.is_empty() {
        return list;
    }
    match list.iter().position(|i| same(&i.email, acct)) {
        Some(p) => {
            list[p].primary = true;
            let id = list.remove(p);
            list.insert(0, id);
        }
        None => list.insert(0, Identity { email: acct.to_string(), name: String::new(), primary: true, is_default: list.iter().all(|i| !i.is_default) }),
    }
    list
}

/// The address a new message starts with: the user's own choice (`pref`) while it still exists, else the provider's
/// default, else the account address.
pub fn default_identity(list: &[Identity], pref: Option<&str>, account_email: &str) -> String {
    if let Some(p) = pref.filter(|p| !p.trim().is_empty()) {
        if let Some(i) = list.iter().find(|i| same(&i.email, p)) {
            return i.email.clone();
        }
    }
    list.iter().find(|i| i.is_default).map(|i| i.email.clone()).unwrap_or_else(|| account_email.to_string())
}

/// A reply starts as the identity the original was addressed to (any recipient that matches one), so the answer comes
/// from the address the sender used; None when no identity was addressed.
pub fn reply_identity(list: &[Identity], original_recipients: &[String]) -> Option<String> {
    original_recipients.iter().find_map(|r| list.iter().find(|i| same(&i.email, r)).map(|i| i.email.clone()))
}

/// What the user asked to send as, checked against the account's identities. Ok("") = the account's own address
/// (no override is sent); Ok(addr) = the canonical spelling of an identity; Err = refused before anything is queued.
pub fn choose_from(list: &[Identity], account_email: &str, requested: &str) -> Result<String, String> {
    let r = requested.trim();
    if r.is_empty() || same(r, account_email) {
        return Ok(String::new());
    }
    match list.iter().find(|i| same(&i.email, r)) {
        Some(i) => Ok(i.email.clone()),
        None => Err(format!("{} is not one of this account's send-as addresses", r)),
    }
}

/// Graph `from` property for a message sent as `from` (empty = the mailbox itself: nothing is added).
pub fn graph_from_json(from: &str) -> Option<Value> {
    let f = from.trim();
    if f.is_empty() {
        None
    } else {
        Some(serde_json::json!({ "emailAddress": { "address": f } }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id(e: &str, primary: bool, default: bool) -> Identity { Identity { email: e.into(), name: String::new(), primary, is_default: default } }

    #[test]
    fn gmail_send_as_keeps_primary_and_verified_only() {
        let j = json!({ "sendAs": [
            { "sendAsEmail": "me@gmail.com", "displayName": "Me", "isPrimary": true, "isDefault": false },
            { "sendAsEmail": "work@corp.com", "displayName": "Work", "verificationStatus": "accepted", "isDefault": true },
            { "sendAsEmail": "pending@x.org", "verificationStatus": "pending" },
            { "sendAsEmail": "not an address", "verificationStatus": "accepted" },
            { "sendAsEmail": "WORK@corp.com", "verificationStatus": "accepted" },
        ]});
        let l = parse_gmail_send_as(&j);
        assert_eq!(l.iter().map(|i| i.email.as_str()).collect::<Vec<_>>(), ["me@gmail.com", "work@corp.com"]);
        assert!(l[0].primary && l[1].is_default && l[1].name == "Work");
        assert!(parse_gmail_send_as(&json!({})).is_empty());
    }

    #[test]
    fn graph_proxy_addresses() {
        let me = json!({ "displayName": "Adam", "otherMails": ["private@gmail.com"], "proxyAddresses": [
            "SMTP:adam@corp.com", "smtp:sales@corp.com", "smtp:adam@corp.onmicrosoft.com", "smtp:adam@corp.mail.onmicrosoft.com",
            "x500:/o=x", "SIP:adam@corp.com", "smtp:SALES@corp.com", "smtp:junk" ]});
        let l = parse_graph_proxies(&me, "adam@corp.com");
        assert_eq!(l.iter().map(|i| i.email.as_str()).collect::<Vec<_>>(), ["adam@corp.com", "sales@corp.com"]);
        assert!(l[0].primary && l[0].is_default && !l[1].primary && l[1].name == "Adam");
        // No proxyAddresses at all (as for the work account probed 2026-11-12 it lists only the primary): the account address stands alone.
        let one = parse_graph_proxies(&json!({ "proxyAddresses": ["SMTP:adam@corp.com"] }), "adam@corp.com");
        assert_eq!(one.len(), 1);
        let none = parse_graph_proxies(&json!({}), "adam@corp.com");
        assert_eq!(none, vec![Identity { email: "adam@corp.com".into(), name: "".into(), primary: true, is_default: true }]);
        // The sign-in address is first even when the primary SMTP address differs from it.
        let l = parse_graph_proxies(&json!({ "proxyAddresses": ["SMTP:main@corp.com", "smtp:login@corp.com"] }), "login@corp.com");
        assert_eq!(l[0].email, "login@corp.com");
        assert!(l.iter().find(|i| i.email == "main@corp.com").unwrap().is_default);
    }

    #[test]
    fn default_and_reply_identity() {
        let l = vec![id("me@x.org", true, false), id("alias@x.org", false, true), id("other@x.org", false, false)];
        assert_eq!(default_identity(&l, None, "me@x.org"), "alias@x.org");
        assert_eq!(default_identity(&l, Some("OTHER@x.org"), "me@x.org"), "other@x.org");
        assert_eq!(default_identity(&l, Some("gone@x.org"), "me@x.org"), "alias@x.org", "a removed alias falls back");
        assert_eq!(default_identity(&[], None, "me@x.org"), "me@x.org");
        let to = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(reply_identity(&l, &to(&["list@y.org", "Other@X.org"])), Some("other@x.org".into()));
        assert_eq!(reply_identity(&l, &to(&["list@y.org"])), None);
        assert_eq!(reply_identity(&l, &[]), None);
    }

    #[test]
    fn choosing_the_from_address() {
        let l = vec![id("me@x.org", true, true), id("Alias@x.org", false, false)];
        assert_eq!(choose_from(&l, "me@x.org", ""), Ok(String::new()));
        assert_eq!(choose_from(&l, "me@x.org", " ME@x.org "), Ok(String::new()), "the account itself needs no override");
        assert_eq!(choose_from(&l, "me@x.org", "alias@X.org"), Ok("Alias@x.org".into()));
        assert!(choose_from(&l, "me@x.org", "boss@x.org").unwrap_err().contains("boss@x.org"));
    }

    #[test]
    fn graph_from_is_added_only_for_an_alias() {
        assert_eq!(graph_from_json(""), None);
        assert_eq!(graph_from_json(" a@x.org "), Some(json!({ "emailAddress": { "address": "a@x.org" } })));
    }
}
