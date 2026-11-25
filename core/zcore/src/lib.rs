//! zunflix's shared logic: the rules every app follows, written once.
//!
//! The core does no I/O of its own (decision 0007): each screen asks the app for what it needs as effects
//! ([`screen::Effect`]), and the app's own HTTP client, files and player carry them out. See [`root`] for how.

pub mod abr;
pub mod api;
pub mod art;
pub mod art_answers;
pub mod browse;
pub mod cache;
pub mod cards;
pub mod cookies;
pub mod captions;
pub mod dates;
pub mod donate;
pub mod downloads;
mod error;
pub mod history;
pub mod hls;
pub mod home;
pub mod home_page;
pub mod intro;
pub mod legacy;
pub mod manhwa;
pub mod model;
pub mod nav;
pub mod on_tv;
pub mod pictures;
pub mod play;
pub mod player;
pub mod profile_settings;
pub mod profiles;
pub mod progress;
pub mod queue;
pub mod quality;
pub mod remote;
pub mod root;
pub mod screen;
pub mod search;
pub mod sections;
pub mod session;
pub mod setup;
pub mod settings;
pub mod store;
pub mod stream;
pub mod title;
pub mod title_page;
pub mod tv_link;
pub mod update;
pub mod words;
pub mod your_tv;

pub use error::Error;

/// The server every app talks to.
pub const BASE: &str = "https://host.invalid";

/// A server path as a full address; full addresses are returned as they are.
pub fn absolute(path: &str) -> String {
    api::url(BASE, path)
}
