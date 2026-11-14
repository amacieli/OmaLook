//! Shared retry rules for the HTTP providers: `Retry-After` parsing and the one-shot 401 retry.
//!
//! Pure functions and a small state machine, so both are unit-tested without a network. The
//! providers call them from their request paths (`google_api::GoogleApi::send`, Graph's
//! `fetch_delta` and friends).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Parse a `Retry-After` value: delay-seconds (`"120"`) or an HTTP-date (IMF-fixdate, RFC 7231
/// §7.1.3: `"Sun, 06 Nov 1994 08:49:37 GMT"`), measured from `now`. A date already past means
/// "now" (zero). Anything else (negative, fractional, empty, other date formats) is `None`, so
/// the caller falls back to its own backoff.
pub fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    if v.bytes().all(|b| b.is_ascii_digit()) {
        return v.parse::<u64>().ok().map(Duration::from_secs);
    }
    let at = parse_http_date(v)?;
    let now_secs = now.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    Some(Duration::from_secs((at - now_secs).max(0) as u64))
}

/// `Retry-After` from response headers, clamped to `cap`. `None` when the header is absent or
/// unparseable (the caller then uses its normal backoff).
pub fn retry_after_capped(headers: &reqwest::header::HeaderMap, now: SystemTime, cap: Duration) -> Option<Duration> {
    let raw = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    parse_retry_after(raw, now).map(|d| d.min(cap))
}

/// Unix seconds for an IMF-fixdate, or `None`.
fn parse_http_date(s: &str) -> Option<i64> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() != 6 || parts[5] != "GMT" || !parts[0].ends_with(',') {
        return None;
    }
    let day: i64 = parts[1].parse().ok()?;
    let month = match parts[2] {
        "Jan" => 1, "Feb" => 2, "Mar" => 3, "Apr" => 4, "May" => 5, "Jun" => 6,
        "Jul" => 7, "Aug" => 8, "Sep" => 9, "Oct" => 10, "Nov" => 11, "Dec" => 12,
        _ => return None,
    };
    let year: i64 = parts[3].parse().ok()?;
    let hms: Vec<&str> = parts[4].split(':').collect();
    if hms.len() != 3 {
        return None;
    }
    let (h, m, sec): (i64, i64, i64) = (hms[0].parse().ok()?, hms[1].parse().ok()?, hms[2].parse().ok()?);
    if !(1..=31).contains(&day) || !(0..24).contains(&h) || !(0..60).contains(&m) || !(0..61).contains(&sec) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + h * 3600 + m * 60 + sec)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// What to do after a response to a request that carried an access token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStep {
    /// Not a 401: the response is handled by the caller as usual.
    Done,
    /// First 401: drop the cached access token and send the request again, once.
    InvalidateAndResend,
    /// A 401 after the resend: the grant is really rejected; return the auth error.
    Fail,
}

/// One-shot 401 handling for a single logical request. Create one per request; feed it every
/// response status. Only the first 401 asks for a resend, so a request is never sent more than
/// twice for auth reasons. A 401 means the server did not accept the request, so resending a
/// request body is safe for every method.
#[derive(Debug, Default)]
pub struct AuthRetry {
    resent: bool,
}

impl AuthRetry {
    pub fn on_status(&mut self, status: u16) -> AuthStep {
        if status != 401 {
            AuthStep::Done
        } else if !self.resent {
            self.resent = true;
            AuthStep::InvalidateAndResend
        } else {
            AuthStep::Fail
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn retry_after_seconds() {
        let now = at(1_000_000);
        assert_eq!(parse_retry_after("120", now), Some(Duration::from_secs(120)));
        assert_eq!(parse_retry_after("  7 ", now), Some(Duration::from_secs(7)));
        assert_eq!(parse_retry_after("0", now), Some(Duration::ZERO));
    }

    #[test]
    fn retry_after_http_date() {
        // 1994-11-06 08:49:37 UTC is 784111777 s after the epoch.
        let date = "Sun, 06 Nov 1994 08:49:37 GMT";
        assert_eq!(parse_retry_after(date, at(784_111_777 - 90)), Some(Duration::from_secs(90)));
        // A date already past means retry now.
        assert_eq!(parse_retry_after(date, at(784_111_777 + 5)), Some(Duration::ZERO));
        // Leap-year day: 2024-02-29 00:00:00 GMT = 1709164800.
        assert_eq!(parse_retry_after("Thu, 29 Feb 2024 00:00:00 GMT", at(1_709_164_800 - 60)), Some(Duration::from_secs(60)));
    }

    #[test]
    fn retry_after_garbage_is_none() {
        let now = at(1_000_000);
        for bad in ["", "   ", "-5", "1.5", "soon", "120s", "Sun, 06 Nov 1994 08:49:37 PST", "Sun, 32 Nov 1994 08:49:37 GMT", "Sun, 06 Foo 1994 08:49:37 GMT", "Sun, 06 Nov 1994 25:00:00 GMT", "Sun, 06 Nov 1994"] {
            assert_eq!(parse_retry_after(bad, now), None, "{:?} should be ignored", bad);
        }
    }

    #[test]
    fn retry_after_header_is_capped() {
        let now = at(1_000_000);
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(reqwest::header::RETRY_AFTER, "3600".parse().unwrap());
        assert_eq!(retry_after_capped(&h, now, Duration::from_secs(120)), Some(Duration::from_secs(120)));
        let mut h2 = reqwest::header::HeaderMap::new();
        h2.insert(reqwest::header::RETRY_AFTER, "5".parse().unwrap());
        assert_eq!(retry_after_capped(&h2, now, Duration::from_secs(120)), Some(Duration::from_secs(5)));
        assert_eq!(retry_after_capped(&reqwest::header::HeaderMap::new(), now, Duration::from_secs(120)), None);
    }

    #[test]
    fn first_401_resends_second_fails_200_is_ok() {
        let mut r = AuthRetry::default();
        assert_eq!(r.on_status(401), AuthStep::InvalidateAndResend);
        assert_eq!(r.on_status(401), AuthStep::Fail);
        assert_eq!(r.on_status(200), AuthStep::Done);

        let mut ok = AuthRetry::default();
        assert_eq!(ok.on_status(200), AuthStep::Done);
        assert_eq!(ok.on_status(429), AuthStep::Done);

        let mut recovered = AuthRetry::default();
        assert_eq!(recovered.on_status(401), AuthStep::InvalidateAndResend);
        assert_eq!(recovered.on_status(200), AuthStep::Done);
    }
}
