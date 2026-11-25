//! A manhwa chapter's own still and what its pages are asked with (`zcore::manhwa`), and the sizes an older app kept (`zcore::queue::weigh`), as Kotlin sees them.

use crate::manhwa::OldSetting;
use std::sync::{Mutex, PoisonError};
use zcore::manhwa::still::{self, Still, Window as StillWindow};

#[uniffi::remote(Record)]
pub struct StillWindow {
    pub top: u32,
    pub height: u32,
}

/// The size a page is decoded at for its still.
#[derive(Debug, uniffi::Record)]
pub struct StillSize {
    pub width: u32,
    pub height: u32,
}

/// A chapter's own still being picked (`zcore::manhwa::still::Still`).
#[derive(Debug, uniffi::Object)]
pub struct ChapterStill(Mutex<Still>);

#[uniffi::export]
impl ChapterStill {
    /// A chapter of `pages` pages; `saved`: they're on the device.
    #[uniffi::constructor]
    pub fn new(pages: u32, saved: bool) -> Self {
        Self(Mutex::new(Still::new(pages as usize, saved)))
    }

    /// The page to look at next; None once the still is picked.
    pub fn next(&self) -> Option<u32> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).next().map(|i| i as u32)
    }

    /// The page given, decoded at [`still_size`] as straight RGBA (None: it wouldn't come or decode). Some: its window
    /// to crop and keep, in place of any before.
    pub fn looked(&self, rgba: Option<Vec<u8>>, width: u32, height: u32) -> Option<StillWindow> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).looked(rgba.as_deref(), width, height)
    }
}

/// The size a page `width` by `height` is decoded at for its still (`zcore::manhwa::still::fit`).
#[uniffi::export]
pub fn still_size(width: u32, height: u32) -> StillSize {
    let (width, height) = still::fit(width, height);
    StillSize { width, height }
}

/// Where a chapter's own still is kept (`zcore::manhwa::still::file`).
#[uniffi::export]
pub fn chapter_still_file(chapter: String) -> String {
    still::file(&chapter)
}

/// The JPEG quality a chapter's own still is kept at (`zcore::manhwa::still::QUALITY`).
#[uniffi::export]
pub fn chapter_still_quality() -> u8 {
    still::QUALITY
}

/// How many stills are made at once (`zcore::manhwa::still::AT_ONCE`).
#[uniffi::export]
pub fn chapter_still_at_once() -> u32 {
    still::AT_ONCE as u32
}

/// The two files a chapter's folder keeps: `complete` (written once every page is there, with how many) and `kept`
/// (written when it was downloaded on purpose) (`zcore::downloads::COMPLETE`, `KEPT`).
#[derive(Debug, uniffi::Record)]
pub struct ChapterMarkers {
    pub complete: String,
    pub kept: String,
}

#[uniffi::export]
pub fn chapter_markers() -> ChapterMarkers {
    ChapterMarkers { complete: zcore::downloads::COMPLETE.into(), kept: zcore::downloads::KEPT.into() }
}

/// How many pages a chapter's `complete` file says it has; 0 when it isn't a count (`zcore::downloads::saved_pages`).
#[uniffi::export]
pub fn chapter_saved_pages(complete: String) -> u32 {
    zcore::downloads::saved_pages(&complete).map_or(0, |n| n as u32)
}

/// A saved chapter's page's file in its folder (`zcore::downloads::page_file`).
#[uniffi::export]
pub fn chapter_page_file(index: u32) -> String {
    zcore::downloads::page_file(index as usize)
}

/// The browser a page's request on Asura's CDN says it's from (`zcore::manhwa::PAGE_AGENT`).
#[uniffi::export]
pub fn manhwa_page_agent() -> String {
    zcore::manhwa::PAGE_AGENT.into()
}

/// The file the queue keeps weighed sizes in (`zcore::queue::weigh::SIZES`).
#[uniffi::export]
pub fn download_sizes_file() -> String {
    zcore::queue::weigh::SIZES.into()
}

/// The sizes file's text with an older app's kept sizes ("bytes:when") added (`zcore::queue::weigh::moved_sizes`).
#[uniffi::export]
pub fn download_moved_sizes(kept: Option<String>, old: Vec<OldSetting>) -> String {
    let old: Vec<(String, String)> = old.into_iter().map(|s| (s.key, s.value)).collect();
    zcore::queue::weigh::moved_sizes(kept.as_deref(), &old)
}
