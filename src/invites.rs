//! Meeting invitations (Microsoft Graph only): what kind of calendar message a mail is, and
//! answering it. Answering accepts or tentatively accepts (or declines) the invitation on the
//! server, which puts the event on the mailbox calendar, and Graph sends the organizer a reply.

/// The response words the UI uses, mapped to the Graph action and the `meetingMessageType` the
/// message is left with. `None` for anything else.
pub fn response_op(choice: &str) -> Option<(&'static str, &'static str)> {
    match choice {
        "accept" => Some(("accept", "meetingAccepted")),
        "tentative" => Some(("tentativelyAccept", "meetingTentativelyAccepted")),
        "decline" => Some(("decline", "meetingDeclined")),
        _ => None,
    }
}

/// Types that can be answered. A cancelled meeting or ordinary mail cannot.
pub fn is_actionable(meeting_type: &str) -> bool {
    matches!(
        meeting_type,
        "meetingRequest" | "meetingTentativelyAccepted" | "meetingAccepted" | "meetingDeclined"
    )
}

/// Whether a response to an event on the calendar is needed (accept/tentative add it).
pub fn adds_to_calendar(choice: &str) -> bool {
    matches!(choice, "accept" | "tentative")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_each_response_to_graph_action_and_state() {
        assert_eq!(response_op("accept"), Some(("accept", "meetingAccepted")));
        assert_eq!(response_op("tentative"), Some(("tentativelyAccept", "meetingTentativelyAccepted")));
        assert_eq!(response_op("decline"), Some(("decline", "meetingDeclined")));
        assert_eq!(response_op("maybe"), None);
    }

    #[test]
    fn only_requests_and_responses_are_actionable() {
        assert!(is_actionable("meetingRequest"));
        assert!(is_actionable("meetingAccepted"));
        assert!(!is_actionable("meetingCancelled"));
        assert!(!is_actionable("none"));
        assert!(!is_actionable(""));
    }

    #[test]
    fn accept_and_tentative_touch_the_calendar_decline_does_not() {
        assert!(adds_to_calendar("accept"));
        assert!(adds_to_calendar("tentative"));
        assert!(!adds_to_calendar("decline"));
    }
}
