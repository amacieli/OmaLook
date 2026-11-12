//! Account identity.
//!
//! Every account id has the form `<provider>-<6 hex chars>` (e.g.
//! `exchange-3f9a1c`, `gmail-07be42`), including the first account a user
//! adds. The only exception is `exchange-primary` (`token_store::DEFAULT_ACCOUNT`):
//! the id given to the account that existed before multi-account support, so
//! its keyring entry and data keep working. It is never generated for new accounts.
//!
//! Ids are opaque and stable: they never encode the email address (addresses
//! can change and would leak into keyring labels). Uniqueness of the mailbox
//! itself is enforced by `UNIQUE(provider, email)` in the `accounts` table.
//!
//! Provider-supplied row ids (message / folder / event ids) share one table
//! namespace across accounts. Graph ids are globally unique; providers whose
//! ids are only unique per mailbox (IMAP UIDs, folder names like "INBOX")
//! must prefix them with the account id.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Provider slug usable in an id: lowercase ASCII alphanumerics only.
pub fn provider_slug(provider: &str) -> String {
    let s: String = provider
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if s.is_empty() { "account".to_string() } else { s }
}

/// A fresh `<provider>-<6 hex>` id. Callers must still check for a collision
/// (see `Database::create_account`, which retries).
pub fn new_account_id(provider: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = RandomState::new().build_hasher(); // OS-seeded per instance
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    h.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    format!("{}-{:06x}", provider_slug(provider), h.finish() & 0xff_ffff)
}

/// Provider part of an account id (`exchange-3f9a1c` → `exchange`).
pub fn provider_of(account_id: &str) -> &str {
    account_id.split('-').next().unwrap_or(account_id)
}

/// Personal Microsoft accounts (Outlook.com, Hotmail, Live) are the `outlook` provider; work and school
/// accounts are `exchange`.
pub fn is_personal_microsoft(account_id: &str) -> bool {
    provider_of(account_id) == "outlook"
}

/// What a personal Microsoft account cannot do with calendar entries. Create, edit and plain delete work;
/// Microsoft documents the scheduling and invitation calls as not supported for personal accounts
/// (docs/PLAN-calendar-events.md, "Personal Microsoft accounts").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonalLimit {
    /// Attendees on a create or an edit.
    Invite,
    /// Cancel that tells attendees (an organiser's cancel with a message).
    CancelWithMessage,
    /// The free-time finder (`getSchedule`, `findMeetingTimes`).
    Finder,
}

/// `Some(polite refusal)` when `account_id` is personal and the action is one of its limits; `None` otherwise.
pub fn personal_refusal(account_id: &str, limit: PersonalLimit) -> Option<&'static str> {
    if !is_personal_microsoft(account_id) {
        return None;
    }
    Some(match limit {
        PersonalLimit::Invite => "Personal Microsoft accounts can't invite attendees: Microsoft does not support that for them. Create the entry without attendees, or use a work or school account.",
        PersonalLimit::CancelWithMessage => "Personal Microsoft accounts can't cancel with a message to attendees: Microsoft does not support that for them. Delete the entry instead.",
        PersonalLimit::Finder => "Personal Microsoft accounts have no free-time finder: Microsoft does not support the scheduling lookups for them. Use a work or school account.",
    })
}

#[cfg(test)]
mod personal_tests {
    use super::*;

    #[test]
    fn only_outlook_accounts_are_personal() {
        assert!(is_personal_microsoft("outlook-bbbbbb"));
        assert!(!is_personal_microsoft("exchange-1"));
        assert!(!is_personal_microsoft("gmail-2"));
    }

    #[test]
    fn personal_limits_are_refused_politely_and_work_accounts_pass() {
        assert!(personal_refusal("exchange-1", PersonalLimit::Invite).is_none());
        for l in [PersonalLimit::Invite, PersonalLimit::CancelWithMessage, PersonalLimit::Finder] {
            let m = personal_refusal("outlook-1", l).expect("refused");
            assert!(m.contains("Microsoft does not support") && !m.contains("error"));
        }
    }
}

/// Provider-supplied ids that are only unique within one mailbox (Gmail message / label ids,
/// Google event and contact ids) are stored as `<account id>:<provider id>` so two accounts can
/// never collide in the shared tables. Account ids contain no ':'.
pub fn scoped(account_id: &str, raw: &str) -> String {
    format!("{}:{}", account_id, raw)
}

/// Inverse of `scoped`; ids without this account's prefix are returned unchanged.
pub fn unscoped<'a>(account_id: &str, id: &'a str) -> &'a str {
    id.strip_prefix(account_id).and_then(|r| r.strip_prefix(':')).unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token_store::DEFAULT_ACCOUNT;

    #[test]
    fn ids_follow_provider_dash_hex_pattern() {
        for p in ["exchange", "Gmail", "IMAP account", "proton-mail", ""] {
            let id = new_account_id(p);
            let (prov, suffix) = id.rsplit_once('-').unwrap();
            assert_eq!(prov, provider_slug(p));
            assert_eq!(suffix.len(), 6, "{}", id);
            assert!(suffix.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{}", id);
        }
    }

    #[test]
    fn first_account_is_not_special() {
        // New accounts never get the legacy id, even the first Exchange one.
        for _ in 0..1000 {
            assert_ne!(new_account_id("exchange"), DEFAULT_ACCOUNT);
        }
    }

    #[test]
    fn ids_are_well_distributed() {
        let ids: std::collections::HashSet<_> = (0..500).map(|_| new_account_id("exchange")).collect();
        assert!(ids.len() >= 495, "too many collisions: {}", ids.len());
    }

    #[test]
    fn scoped_ids_round_trip_and_never_collide_across_accounts() {
        assert_eq!(scoped("gmail-aaaaaa", "INBOX"), "gmail-aaaaaa:INBOX");
        assert_ne!(scoped("gmail-aaaaaa", "18f3"), scoped("gmail-bbbbbb", "18f3"));
        assert_eq!(unscoped("gmail-aaaaaa", "gmail-aaaaaa:18f3"), "18f3");
        assert_eq!(unscoped("gmail-aaaaaa", "18f3"), "18f3");
        assert_eq!(unscoped("gmail-aaaaaa", "gmail-bbbbbb:18f3"), "gmail-bbbbbb:18f3");
    }

    #[test]
    fn provider_round_trips() {
        assert_eq!(provider_of(&new_account_id("gmail")), "gmail");
        assert_eq!(provider_of("exchange-primary"), "exchange");
    }
}
