//! Google Meet links for calendar events (docs/PLAN-calendar-events.md section 7.1: a *native* meeting kind,
//! made by the calendar service itself, so it needs no separate meeting account).
//!
//! Pure: builds the `conferenceData` part of a Google Calendar insert and reads the link back. No network.
//! Used by `providers/google_calendar_write.rs`. For the online-meetings picker (track B): a Google account
//! offers "Google Meet" when `PROVIDER` is what the editor sends; the link comes back in `CreatedEvent.join_url`.

use serde_json::{json, Value};

/// The `online_provider` value that asks Google for a Meet room (also what a stored event reports).
pub const PROVIDER: &str = "googleMeet";

/// Query parameter Google requires on any insert/patch that carries `conferenceData`.
pub const CONFERENCE_QUERY: &str = "conferenceDataVersion=1";

/// True when a draft's online provider asks for a Meet room.
pub fn wants_meet(online_provider: Option<&str>) -> bool {
    matches!(online_provider, Some(p) if p == PROVIDER || p.eq_ignore_ascii_case("meet"))
}

/// The `conferenceData` object for a create. `request_id` is the idempotency key: the same id on a retry
/// does not make a second room (Google ignores a repeat), so it is the draft's transaction id.
pub fn create_request(request_id: &str) -> Value {
    json!({
        "createRequest": {
            "requestId": request_id,
            "conferenceSolutionKey": { "type": "hangoutsMeet" },
        }
    })
}

/// The join link of an event resource as Google returns it: `hangoutLink` for Meet, else the video entry point.
pub fn join_link(event: &Value) -> Option<String> {
    event["hangoutLink"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or_else(|| {
            event["conferenceData"]["entryPoints"]
                .as_array()?
                .iter()
                .find(|e| e["entryPointType"].as_str() == Some("video"))
                .and_then(|e| e["uri"].as_str())
                .filter(|s| !s.is_empty())
                .map(String::from)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_meet_provider_asks_for_a_room() {
        assert!(wants_meet(Some("googleMeet")));
        assert!(wants_meet(Some("Meet")));
        assert!(!wants_meet(Some("teamsForBusiness")));
        assert!(!wants_meet(Some("")));
        assert!(!wants_meet(None));
    }

    #[test]
    fn create_request_golden() {
        assert_eq!(
            create_request("ol-1f-002"),
            json!({"createRequest": {"requestId": "ol-1f-002", "conferenceSolutionKey": {"type": "hangoutsMeet"}}})
        );
    }

    #[test]
    fn join_link_is_read_back_from_hangout_link_or_entry_points() {
        assert_eq!(join_link(&json!({"hangoutLink": "https://meet.google.com/abc-defg-hij"})).as_deref(), Some("https://meet.google.com/abc-defg-hij"));
        let entry = json!({"conferenceData": {"entryPoints": [
            {"entryPointType": "phone", "uri": "tel:+1"},
            {"entryPointType": "video", "uri": "https://meet.google.com/xyz-abcd-efg"}]}});
        assert_eq!(join_link(&entry).as_deref(), Some("https://meet.google.com/xyz-abcd-efg"));
        assert_eq!(join_link(&json!({"hangoutLink": ""})), None);
        assert_eq!(join_link(&json!({"summary": "no meet"})), None);
    }
}
