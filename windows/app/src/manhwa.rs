//! Manhwa's I/O on this PC: a chapter's pages' list for its download, and the reading spots moved into the setting the
//! core keeps them in. What the answers mean, and every screen, is the shared core's (`zcore::manhwa`).
use crate::net::{self};
pub use zcore::manhwa::{Chapter, Comic};

/// Once: the reading spots from the file this app kept them in, into the setting the core keeps them in (the file
/// stays, so a write cut short by the app's end loses nothing).
pub fn move_reading() {
    if net::pref_get(zcore::manhwa::READING_PREF).is_some() {
        return;
    }
    if let Ok(text) = std::fs::read_to_string(net::root().join("manhwa-reading.json")) {
        net::pref_put(zcore::manhwa::READING_PREF, Some(text));
    }
}
