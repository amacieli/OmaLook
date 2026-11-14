//! Route groups for the UI backend (R3.1). Each group module exposes `routes()`; `all()` joins
//! them into the table `router::table()` serves. Anything not listed here gets the
//! old chain's 404 (the chain itself is gone since R3.1).

pub mod calendar;
pub mod compose;
pub mod contacts;
pub mod core;
pub mod messages;
pub mod rules;
pub mod settings;
pub mod sms_files;

use super::router::Route;

pub fn all() -> Vec<Route> {
    let mut v = Vec::new();
    v.extend(calendar::routes());
    v.extend(compose::routes());
    v.extend(contacts::routes());
    v.extend(core::routes());
    v.extend(messages::routes());
    v.extend(rules::routes());
    v.extend(settings::routes());
    v.extend(sms_files::routes());
    v
}
