//! The downloads queue as Kotlin sees it (`zcore::queue`): one screen object the app keeps for as long as it runs,
//! whose view is every download, the Downloads page, the notification and the size a title page shows. The types are
//! zcore's own, described to UniFFI here, renamed where Kotlin already has the name.

use crate::screens::CoreRoot;
use std::sync::Arc;
use zcore::root::{ScreenId, Turn};
use zcore::screen::{Clock as CoreClock, Empty as EmptyState, Screen};

pub use zcore::downloads::{Act as DownloadAct, Kind as DownloadKind, State as DownloadState};
use zcore::queue::{
    Entry as DownloadEntry, EntryKind as DownloadEntryKind, Event as QueueEvent, Item as DownloadItem, LocalCopy,
    Note as DownloadNote, Open as DownloadOpen, Page as DownloadsPage, Section as DownloadsSection,
    Told as DownloadTold, View as QueueView,
    WantedChapter, WantedEpisode, WantedSeries, WantedTitle, Weighed,
};

#[uniffi::remote(Enum)]
pub enum DownloadState {
    Queued,
    Running,
    Paused,
    Failed,
    Done,
}

#[uniffi::remote(Enum)]
pub enum DownloadKind {
    Video,
    Chapter,
}

#[uniffi::remote(Enum)]
pub enum DownloadAct {
    Add,
    Pause,
    Retry,
    Resume,
    Open,
}

#[uniffi::remote(Record)]
pub struct WantedTitle {
    pub id: String,
    pub name: String,
    pub source: String,
    pub imdb: Option<String>,
    pub stream: Option<String>,
    pub minutes: Option<i64>,
    pub art: Option<String>,
}

#[uniffi::remote(Record)]
pub struct WantedEpisode {
    pub id: String,
    pub season: Option<i64>,
    pub number: Option<i64>,
    pub season_id: Option<String>,
    pub stream: Option<String>,
    pub still: Option<String>,
    pub length: f64,
}

#[uniffi::remote(Record)]
pub struct WantedSeries {
    pub path: String,
    pub title: String,
    pub cover: String,
    pub art: String,
}

#[uniffi::remote(Record)]
pub struct WantedChapter {
    pub path: String,
    pub label: String,
    pub thumb: Option<String>,
}

#[uniffi::remote(Enum)]
pub enum QueueEvent {
    Start,
    Run { on: bool },
    MobileData { on: bool },
    NotesAsked,
    Add { title: WantedTitle, episodes: Vec<WantedEpisode> },
    AddChapters { series: WantedSeries, chapters: Vec<WantedChapter> },
    Act { key: String, act: DownloadAct },
    Remove { key: String },
    Spot { key: String, at: f64 },
    Delete { key: String },
    Dismiss,
    Weigh { title: WantedTitle, episodes: Vec<WantedEpisode> },
    WeighChapters { series: String, chapters: Vec<String> },
    Look,
    Search { query: String },
    Show { title: Option<String> },
}

#[uniffi::remote(Record)]
pub struct LocalCopy {
    pub key: String,
    pub file: String,
    pub hls: bool,
    pub sound: Option<String>,
    pub captions: Option<String>,
    pub seconds: f64,
    pub label: String,
    pub spot: f64,
}

#[uniffi::remote(Record)]
pub struct DownloadItem {
    pub key: String,
    pub kind: DownloadKind,
    pub title_id: String,
    pub episode_id: Option<String>,
    pub title: String,
    pub name: String,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub picture: Option<String>,
    pub art: Option<String>,
    pub imdb: Option<String>,
    pub source: String,
    pub label: String,
    pub state: DownloadState,
    pub share: f32,
    pub size: u64,
    pub done: u64,
    pub line: String,
    pub failed: bool,
    pub act: DownloadAct,
    pub length: f64,
    pub local: Option<LocalCopy>,
}

#[uniffi::remote(Enum)]
pub enum DownloadEntryKind {
    Film,
    Show,
    Episode,
    Chapter,
}

#[uniffi::remote(Enum)]
pub enum DownloadOpen {
    Nothing,
    Play,
    Read,
    Show,
}

#[uniffi::remote(Record)]
pub struct DownloadEntry {
    pub kind: DownloadEntryKind,
    pub key: String,
    pub item: Option<u32>,
    pub name: String,
    pub line: String,
    pub failed: bool,
    pub asking: bool,
    pub picture: Option<String>,
    pub badge: String,
    pub open: DownloadOpen,
    pub show: Option<String>,
}

#[uniffi::remote(Record)]
pub struct DownloadsSection {
    pub heading: Option<String>,
    pub entries: Vec<DownloadEntry>,
}

#[uniffi::remote(Record)]
pub struct DownloadsPage {
    pub heading: String,
    pub show: Option<String>,
    pub room: String,
    pub search_hint: String,
    pub ask: String,
    pub sections: Vec<DownloadsSection>,
    pub asking: bool,
    pub empty: Option<EmptyState>,
}

#[uniffi::remote(Record)]
pub struct DownloadNote {
    pub title: String,
    pub detail: String,
    pub percent: u8,
    pub unsure: bool,
}

#[uniffi::remote(Record)]
pub struct DownloadTold {
    pub number: u32,
    pub key: String,
    pub title: String,
    pub line: String,
    pub chapter: bool,
}

#[uniffi::remote(Record)]
pub struct Weighed {
    pub title: String,
    pub bytes: Option<u64>,
}

#[uniffi::remote(Record)]
pub struct QueueView {
    pub items: Vec<DownloadItem>,
    pub page: DownloadsPage,
    pub note: Option<DownloadNote>,
    pub told: Vec<DownloadTold>,
    pub pending: bool,
    pub busy: bool,
    pub bytes_left: u64,
    pub usable: bool,
    pub mobile_data: bool,
    pub sizes: Vec<Weighed>,
    pub running: bool,
    pub ask_notes: bool,
}

screen!(
    /// Every download and its transfers (`zcore::queue`), for as long as the app runs.
    QueueScreen(zcore::queue::Queue),
    QueueEvent,
    QueueView
);

/// What Android's two notification channels are called (`zcore::queue::CHANNEL_PROGRESS`, `CHANNEL_DONE`).
#[derive(Debug, uniffi::Record)]
pub struct NoteChannels {
    pub progress: String,
    pub finished: String,
}

#[uniffi::export]
pub fn note_channels() -> NoteChannels {
    NoteChannels { progress: zcore::queue::CHANNEL_PROGRESS.into(), finished: zcore::queue::CHANNEL_DONE.into() }
}

/// The finished copy on this device of a film or an episode, if there is one (`zcore::queue::playable`).
#[uniffi::export]
pub fn download_playable(items: Vec<DownloadItem>, title_id: String, episode: Option<String>) -> Option<LocalCopy> {
    zcore::queue::playable(&items, &title_id, episode.as_deref()).cloned()
}
