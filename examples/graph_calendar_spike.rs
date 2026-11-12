//! LIVE spike for a Teams meeting on the Microsoft (Graph) calendar (OL-021 track B). Self-only, on one work account.
//!
//! Creates one `[omalook spike]` event with a Teams meeting (isOnlineMeeting + onlineMeetingProvider=teamsForBusiness) (tomorrow 10:00 local, 30 min, nobody invited),
//! reads it back through the provider's `fetch_window`, deletes it by id and checks it is gone.
//! Cleanup runs even when a check fails. Needs the account to be signed in with the calendar scope.
//!
//!   OMALOOK_GRAPH_SPIKE_ACCOUNT=exchange-primary cargo run --example graph_calendar_spike
//!
//! Not run by `cargo test` (unit-test builds use an in-memory token store and no network by design).

use chrono::{Duration, Local, Utc};
use omalook::calendar_write::EventDraft;
use omalook::providers::calendar::local_time_zone;
use omalook::auth::AuthManager;
use omalook::providers::{CalendarProvider, GraphCalendarProvider};

fn main() {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime").block_on(run());
}

async fn run() {
    let account = std::env::var("OMALOOK_GRAPH_SPIKE_ACCOUNT").expect("set OMALOOK_GRAPH_SPIKE_ACCOUNT");
    let provider = GraphCalendarProvider::new(AuthManager::for_account(&account));
    let tomorrow = (Local::now().date_naive() + Duration::days(1)).and_hms_opt(10, 0, 0).unwrap();
    let title = "[omalook spike] track B teams create/read/delete";
    let draft = EventDraft {
        subject: title.into(),
        body_text: String::new(),
        start: tomorrow,
        end: tomorrow + Duration::minutes(30),
        time_zone: local_time_zone(),
        is_all_day: false,
        location: None,
        attendees: vec![],
        reminder_minutes: None,
        show_as: "busy".into(),
        recurrence: None,
        transaction_id: format!("spike{}", Utc::now().timestamp_millis()),
        online_provider: Some("teamsForBusiness".into()),
    };
    let made = match provider.create_event(&draft).await {
        Ok(m) => m,
        Err(e) => {
            println!("create failed: {e}");
            std::process::exit(2);
        }
    };
    println!("created id={} join={:?} link={}", made.id, made.join_url, made.web_link);

    let day = tomorrow.date();
    let checks: Result<(), String> = async {
        let read = provider.fetch_window(day, day + Duration::days(1)).await.map_err(|e| format!("read back: {e}"))?;
        let seen = read.iter().find(|e| e.id == made.id).ok_or("created event not in the window")?;
        println!("read back: subject={:?} start={} end={} online_url={:?}", seen.subject, seen.start, seen.end, seen.online_url);
        if seen.subject != title {
            return Err("subject differs".into());
        }
        if made.join_url.is_empty() {
            return Err("no Teams link came back (onlineMeeting.joinUrl empty)".into());
        }
        Ok(())
    }
    .await;

    let deleted = provider.delete_event(&made.id).await;
    let left = match provider.fetch_window(day, day + Duration::days(1)).await {
        Ok(read) => read.iter().filter(|e| e.id == made.id).count(),
        Err(e) => {
            println!("re-read after delete failed: {e}");
            usize::MAX
        }
    };
    println!("deleted: {:?}; spike events left with this id: {left}", deleted.is_ok());
    if let Err(e) = &deleted {
        println!("DELETE FAILED, clean up {} by hand: {e}", made.id);
    }
    match checks {
        Ok(()) if deleted.is_ok() && left == 0 => println!("SPIKE OK"),
        Ok(()) => {
            println!("SPIKE: read-back ok but cleanup not confirmed");
            std::process::exit(1);
        }
        Err(e) => {
            println!("SPIKE CHECK FAILED: {e}");
            std::process::exit(1);
        }
    }
}
