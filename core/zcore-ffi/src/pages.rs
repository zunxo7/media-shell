//! The app's pages as Kotlin sees them: where the app is (`zcore::nav`), Home (`zcore::home_page`), a title's page
//! (`zcore::title_page`), the sections and search (`zcore::browse`). zcore's own types, described to UniFFI here
//! (`remote`), renamed where Kotlin already has the name.

use crate::screens::CoreRoot;
use std::sync::Arc;
use zcore::browse::{Chip as SectionChip, SearchEvent, SearchView, SectionEvent, SectionView};
use zcore::cards::Card as CoreCard;
use zcore::home_page::{Event as HomeEvent, RowView as HomeRowView, View as HomeView};
use zcore::model::{Episode as CoreEpisode, Item as CoreItem, Season as CoreSeason, Title as CoreTitle};
use zcore::nav::{
    Entry as NavEntry, Event as NavEvent, Move as NavMove, Playing as NavPlaying, Sheet as NavSheet, Tab as NavTab,
    TroubleWords, View as NavView,
};
use zcore::root::{ScreenId, Turn};
use zcore::screen::{
    Clock as CoreClock, Empty as EmptyState, Glyph as Lucide, Route as CoreRoute, Screen, Trouble as TroubleState,
};
use zcore::title_page::{EpisodeRow, Event as TitleEvent, SeasonChip, View as TitleView, Watching as TitleWatching};

#[uniffi::remote(Record)]
pub struct CoreItem {
    pub id: String,
    pub title: String,
    pub poster: Option<String>,
    pub cover: Option<String>,
    pub cover_big: Option<String>,
    pub fallback: Option<String>,
    pub poster_hd: Option<String>,
    pub logo: Option<String>,
    pub source: Option<String>,
    pub kind: Option<String>,
    pub episodes: Option<i64>,
    pub year: Option<String>,
    pub genres: Vec<String>,
    pub description: Option<String>,
    pub age: Option<String>,
    pub channel: Option<String>,
    pub minutes: Option<i64>,
    pub rating: Option<String>,
    pub resume: Option<String>,
    pub sub: Option<String>,
    pub poster_clean: bool,
    pub textless: bool,
    pub origin: Option<String>,
    pub new_at: Option<i64>,
    pub new_kind: Option<String>,
    pub available_at: Option<i64>,
}

#[uniffi::remote(Record)]
pub struct CoreEpisode {
    pub id: String,
    pub n: Option<i64>,
    pub season: Option<i64>,
    pub season_id: Option<String>,
    pub season_label: Option<String>,
    pub thumb: Option<String>,
    pub length: f64,
    pub date: Option<String>,
    pub stream: Option<String>,
    pub sprite: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
}

#[uniffi::remote(Record)]
pub struct CoreSeason {
    pub id: String,
    pub label: String,
    pub extra: bool,
}

#[uniffi::remote(Record)]
pub struct CoreTitle {
    pub id: String,
    pub title: String,
    pub source: String,
    pub kind: String,
    pub media_type: Option<String>,
    pub imdb: Option<String>,
    pub description: Option<String>,
    pub synopsis: Option<String>,
    pub cover: Option<String>,
    pub cover_big: Option<String>,
    pub poster: Option<String>,
    pub logo: Option<String>,
    pub rating: Option<String>,
    pub year: Option<String>,
    pub genres: Vec<String>,
    pub age: Option<String>,
    pub duration: Option<String>,
    pub textless: bool,
    pub minutes: Option<i64>,
    pub stream: Option<String>,
    pub episodes: Vec<CoreEpisode>,
    pub seasons: Vec<CoreSeason>,
    pub cast: Vec<String>,
    pub origin: Option<String>,
    pub poster_clean: bool,
    pub poster_hd: Option<String>,
    pub available_at: Option<i64>,
    pub film: bool,
}

#[uniffi::remote(Record)]
pub struct CoreCard {
    pub item: CoreItem,
    pub tag: Option<String>,
    pub top10: bool,
    pub progress: Option<f32>,
}

// ---------- where the app is ----------

#[uniffi::remote(Record)]
pub struct NavTab {
    pub key: String,
    pub label: String,
    pub glyph: Lucide,
    pub on: bool,
}

#[uniffi::remote(Record)]
pub struct NavEntry {
    pub route: CoreRoute,
    pub label: String,
    pub on: bool,
}

#[uniffi::remote(Record)]
pub struct NavSheet {
    pub name: String,
    pub arg: Option<String>,
}

#[uniffi::remote(Record)]
pub struct NavPlaying {
    pub show: String,
    pub episode: Option<String>,
    pub local: bool,
}

#[uniffi::remote(Enum)]
pub enum NavMove {
    Still,
    Push { under: Option<CoreRoute> },
    Pop { left: CoreRoute },
    Swap { old: Option<CoreRoute> },
    Tab { from: u32 },
}

#[uniffi::remote(Record)]
pub struct NavView {
    pub tabs: Vec<NavTab>,
    pub tab: u32,
    pub stack: Vec<CoreRoute>,
    pub top: Option<CoreRoute>,
    pub moving: NavMove,
    pub moves: u32,
    pub playing: Option<NavPlaying>,
    pub menu: bool,
    pub sheet: Option<NavSheet>,
    pub can_back: bool,
    pub asking: Option<String>,
    pub search: NavEntry,
    pub pages: Vec<NavEntry>,
    pub foot: Vec<NavEntry>,
    pub home_top: u32,
    pub profiles: String,
    pub sign_out: String,
}

#[uniffi::remote(Enum)]
pub enum NavEvent {
    Started { offline: bool },
    Tab { index: u32 },
    Open { route: CoreRoute },
    Rail { route: CoreRoute },
    Back,
    Home,
    Menu { open: bool },
    Sheet { name: String, arg: Option<String> },
    CloseSheet,
    Play { show: String, episode: Option<String>, local: bool },
    PlayerEpisode { episode: String },
    PlayerClosed,
    Link { url: String },
    OpenDownloads,
    YourTv { linked: bool, on: bool },
    Ask { key: String },
    Unask,
}

screen!(
    /// Where the app is (`zcore::nav`).
    NavScreen(zcore::nav::Nav),
    NavEvent,
    NavView
);

#[uniffi::remote(Record)]
pub struct TroubleWords {
    pub title: String,
    pub line: String,
    pub retry: String,
    pub downloads: Option<String>,
}

/// What a page that couldn't load says (`zcore::nav::trouble_words`).
#[uniffi::export]
pub fn trouble_words(trouble: TroubleState, downloads: bool) -> TroubleWords {
    zcore::nav::trouble_words(&trouble, downloads)
}

/// The sheet that signs a TV in (`zcore::nav::SIGN_IN_TV`).
#[uniffi::export]
pub fn sign_in_tv_sheet() -> String {
    zcore::nav::SIGN_IN_TV.into()
}

// ---------- Home ----------

#[uniffi::remote(Enum)]
pub enum HomeEvent {
    Opened { profile: String },
    Retry,
    Showing { on: bool },
    Hold { on: bool },
    Hero { index: u32 },
    Next,
    Previous,
    HeroPlay,
    HeroInfo,
    Tapped { row: u32, card: u32 },
    Info { row: u32, card: u32 },
}

#[uniffi::remote(Record)]
pub struct HomeRowView {
    pub title: String,
    pub cards: Vec<CoreCard>,
    pub wide: bool,
    pub ranked: bool,
    pub continuing: bool,
}

#[uniffi::remote(Record)]
pub struct HomeView {
    pub hero: Vec<CoreItem>,
    pub hero_at: u32,
    pub stay_ms: u32,
    pub warm: Option<String>,
    pub warm_after_ms: u32,
    pub play: String,
    pub info: String,
    pub rows: Vec<HomeRowView>,
    pub loading: bool,
    pub trouble: Option<TroubleState>,
}

screen!(
    /// Home (`zcore::home_page`).
    HomeScreen(zcore::home_page::HomePage),
    HomeEvent,
    HomeView
);

// ---------- a title's page ----------

#[uniffi::remote(Enum)]
pub enum TitleEvent {
    Opened { id: String, profile: String },
    Retry,
    Season { id: String },
    Order,
    Play,
    FromStart,
    Episode { id: String },
    Related { index: u32 },
    Story,
    Revealed,
    Tick,
}

#[uniffi::remote(Record)]
pub struct TitleWatching {
    pub line: String,
    pub share: f32,
}

#[uniffi::remote(Record)]
pub struct SeasonChip {
    pub id: String,
    pub label: String,
    pub on: bool,
}

#[uniffi::remote(Record)]
pub struct EpisodeRow {
    pub id: String,
    pub number: String,
    pub name: String,
    pub length: String,
    pub date: Option<String>,
    pub line: String,
    pub description: Option<String>,
    pub picture: Option<String>,
    pub share: f32,
}

#[uniffi::remote(Record)]
pub struct TitleView {
    pub loading: bool,
    pub trouble: Option<TroubleState>,
    pub title: Option<CoreTitle>,
    pub facts: String,
    pub age: Option<String>,
    pub story: String,
    pub story_open: bool,
    pub cast: Option<String>,
    pub genres: Option<String>,
    pub cast_label: String,
    pub genres_label: String,
    pub play: String,
    pub locked: bool,
    pub from_start: Option<String>,
    pub watching: Option<TitleWatching>,
    pub episodes_title: String,
    pub newest: bool,
    pub order: String,
    pub seasons: Vec<SeasonChip>,
    pub episodes: Vec<EpisodeRow>,
    pub related_title: String,
    pub related: Vec<CoreCard>,
    pub reveal: Option<String>,
}

screen!(
    /// A title's page (`zcore::title_page`).
    TitleScreen(zcore::title_page::TitlePage),
    TitleEvent,
    TitleView
);

// ---------- the sections and search ----------

#[uniffi::remote(Record)]
pub struct SearchView {
    pub query: String,
    pub hint: String,
    pub note: String,
    pub cards: Vec<CoreCard>,
    pub busy: bool,
    pub typed: bool,
    pub empty: Option<EmptyState>,
    pub fresh: u32,
}

#[uniffi::remote(Enum)]
pub enum SearchEvent {
    Opened,
    Query { text: String },
    Tapped { index: u32 },
}

screen!(
    /// Search everywhere (`zcore::browse::Search`).
    SearchScreen(zcore::browse::Search),
    SearchEvent,
    SearchView
);

#[uniffi::remote(Enum)]
pub enum SectionEvent {
    Opened { section: String },
    Chip { index: u32 },
    More,
    Retry,
    Tapped { index: u32 },
    Find { open: bool },
    Query { text: String },
    Found { index: u32 },
}

#[uniffi::remote(Record)]
pub struct SectionChip {
    pub label: String,
    pub on: bool,
}

#[uniffi::remote(Record)]
pub struct SectionView {
    pub title: String,
    pub chips: Vec<SectionChip>,
    pub cards: Vec<CoreCard>,
    pub loading: bool,
    pub more: bool,
    pub building: Option<String>,
    pub trouble: Option<TroubleState>,
    pub finding: bool,
    pub search: SearchView,
    pub fresh: u32,
}

screen!(
    /// A section's page (`zcore::browse::Section`).
    SectionScreen(zcore::browse::Section),
    SectionEvent,
    SectionView
);
