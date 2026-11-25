//! Manhwa's screens as Kotlin sees them: the shelf, a series and the reader (`zcore::manhwa`), described to UniFFI
//! (`remote`). Names that would clash in Kotlin are given their own (the app has its own `Comic`, `Chapter`, `Page`).

use crate::queue::{DownloadAct, DownloadState};
use crate::screens::CoreRoot;
use std::sync::Arc;
use zcore::manhwa::reader::{
    End as ChapterEnd, Event as ReaderEvent, GoTo as ReaderGoTo, Load as PageLoad, Page as ReaderPage,
    View as ReaderView, Zoom as ReaderZoom,
};
use zcore::manhwa::series::{
    DownloadAll as SeriesDownloadAll, Event as ManhwaSeriesEvent, Read as ManhwaRead, Row as ChapterRow,
    View as ManhwaSeriesView,
};
use zcore::manhwa::shelf::{Card as ManhwaCard, Event as ManhwaShelfEvent, Search as ManhwaSearch, View as ManhwaShelfView};
use zcore::manhwa::{Chapter as CoreChapter, Comic as CoreComic};
use zcore::root::{ScreenId, Turn};
use zcore::screen::{Clock as CoreClock, Empty as EmptyState, Screen, Trouble as TroubleState};

#[uniffi::remote(Record)]
pub struct CoreComic {
    pub path: String,
    pub title: String,
    pub cover: String,
    pub thumb: String,
    pub rating: String,
}

#[uniffi::remote(Record)]
pub struct CoreChapter {
    pub path: String,
    pub label: String,
    pub name: String,
    pub date: String,
    pub thumb: String,
}

// ---------- the shelf ----------

#[uniffi::remote(Enum)]
pub enum ManhwaShelfEvent {
    Opened,
    Chip { index: u32 },
    Scrolled { to_end: f32, screen: f32 },
    Search { open: bool },
    Typed { text: String },
    Retry,
}

#[uniffi::remote(Record)]
pub struct ManhwaShelfView {
    pub title: String,
    pub chips: Vec<String>,
    pub chip: u32,
    pub cards: Vec<ManhwaCard>,
    pub loading: bool,
    pub more: bool,
    pub trouble: Option<TroubleState>,
    pub search: ManhwaSearch,
}

#[uniffi::remote(Record)]
pub struct ManhwaCard {
    pub comic: CoreComic,
    pub title: String,
    pub picture: String,
    pub line: Option<String>,
}

#[uniffi::remote(Record)]
pub struct ManhwaSearch {
    pub open: bool,
    pub hint: String,
    pub shown: bool,
    pub busy: bool,
    pub note: String,
    pub cards: Vec<ManhwaCard>,
    pub empty: Option<EmptyState>,
}

screen!(
    /// Manhwa's shelf (`zcore::manhwa::shelf`).
    ManhwaShelfScreen(zcore::manhwa::shelf::Shelf),
    ManhwaShelfEvent,
    ManhwaShelfView
);

// ---------- a series ----------

#[uniffi::remote(Enum)]
pub enum ManhwaSeriesEvent {
    Opened { comic: CoreComic },
    Retry,
    About,
    DownloadAll,
    DownloadChapter { path: String },
}

#[uniffi::remote(Record)]
pub struct SeriesDownloadAll {
    pub act: DownloadAct,
    pub label: String,
    pub chip: Option<String>,
}

#[uniffi::remote(Record)]
pub struct ManhwaSeriesView {
    pub title: String,
    pub cover: String,
    pub glow: Option<String>,
    pub facts: String,
    pub read: Option<ManhwaRead>,
    pub about: Option<String>,
    pub about_open: bool,
    pub heading: String,
    pub rows: Vec<ChapterRow>,
    pub download_all: Option<SeriesDownloadAll>,
    pub loading: bool,
    pub trouble: Option<TroubleState>,
}

#[uniffi::remote(Record)]
pub struct ManhwaRead {
    pub label: String,
    pub chapter: CoreChapter,
}

#[uniffi::remote(Record)]
pub struct ChapterRow {
    pub chapter: CoreChapter,
    pub line: String,
    pub reading: bool,
    pub still: String,
    pub own_still: bool,
    pub download: Option<DownloadState>,
    pub act: DownloadAct,
    pub share: f32,
}

screen!(
    /// A Manhwa series (`zcore::manhwa::series`).
    ManhwaSeriesScreen(zcore::manhwa::series::Series),
    ManhwaSeriesEvent,
    ManhwaSeriesView
);

// ---------- the reader ----------

#[uniffi::remote(Enum)]
pub enum ReaderEvent {
    Opened { series: CoreComic, chapter: CoreChapter, saved: u32 },
    Layout { column: f32, room: f32, screen: f32, scale: f32 },
    Measured { round: u32, index: u32, width: u32, height: u32 },
    Missed { round: u32, index: u32, status: u16, offline: bool },
    Scrolled { pos: f32 },
    Went { seq: u32 },
    Zoom { inward: Option<bool>, at: f32 },
    Retry,
    Left,
}

#[uniffi::remote(Record)]
pub struct ReaderView {
    pub series: String,
    pub chapter: String,
    pub open: bool,
    pub progress: Option<String>,
    pub trouble: Option<TroubleState>,
    pub column: f32,
    pub width: f32,
    pub height: f32,
    pub pages: Vec<ReaderPage>,
    pub total: u32,
    pub load: Vec<PageLoad>,
    pub round: u32,
    pub folder: String,
    pub counter: Option<String>,
    pub coming: bool,
    pub stopped: Option<TroubleState>,
    pub end: Option<ChapterEnd>,
    pub before: Option<CoreChapter>,
    pub after: Option<CoreChapter>,
    pub previous_label: String,
    pub next_label: String,
    pub zoom: ReaderZoom,
    pub go_to: Option<ReaderGoTo>,
}

#[uniffi::remote(Record)]
pub struct ReaderPage {
    pub file: String,
    pub url: Option<String>,
    pub width: u32,
    pub height: u32,
    pub top: f32,
    pub tall: f32,
}

#[uniffi::remote(Record)]
pub struct PageLoad {
    pub index: u32,
    pub file: String,
    pub url: Option<String>,
}

#[uniffi::remote(Record)]
pub struct ChapterEnd {
    pub title: String,
    pub chapters: String,
}

#[uniffi::remote(Record)]
pub struct ReaderZoom {
    pub shown: String,
    pub can_in: bool,
    pub can_out: bool,
}

#[uniffi::remote(Record)]
pub struct ReaderGoTo {
    pub pos: f32,
    pub seq: u32,
}

screen!(
    /// The Manhwa reader (`zcore::manhwa::reader`).
    ManhwaReaderScreen(zcore::manhwa::reader::Reader),
    ReaderEvent,
    ReaderView
);

// ---------- the reading spots, as the app keeps them ----------

/// The app setting the reading spots are kept under (`zcore::manhwa::READING_PREF`).
#[uniffi::export]
pub fn manhwa_reading_pref() -> String {
    zcore::manhwa::READING_PREF.into()
}

/// One of the first app's "manhwa-reader" settings, its value as text.
#[derive(Debug, uniffi::Record)]
pub struct OldSetting {
    pub key: String,
    pub value: String,
}

/// The first app's reading spots in today's form (`zcore::manhwa::moved_reading`).
#[uniffi::export]
pub fn manhwa_moved_reading(settings: Vec<OldSetting>) -> String {
    let entries: Vec<(String, String)> = settings.into_iter().map(|s| (s.key, s.value)).collect();
    zcore::manhwa::moved_reading(&entries)
}

/// The status a missed page carries when it was never answered (`zcore::manhwa::reader::NOT_ANSWERED`).
#[uniffi::export]
pub fn manhwa_not_answered() -> u16 {
    zcore::manhwa::reader::NOT_ANSWERED
}
