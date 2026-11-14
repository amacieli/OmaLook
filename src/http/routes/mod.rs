//! Route groups for the UI backend (R3.1). Each group module exposes `routes()`; `all()` joins
//! them into the table `router::table()` serves. Routes not listed here still go through the old
//! chain in `main.rs` until their group moves.

pub mod contacts;
pub mod core;
pub mod sms_files;

use super::router::Route;

pub fn all() -> Vec<Route> {
    let mut v = Vec::new();
    v.extend(contacts::routes());
    v.extend(core::routes());
    v.extend(sms_files::routes());
    v
}
