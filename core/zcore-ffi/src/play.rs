//! The player as Kotlin sees it (`zcore::play`): one screen object for the whole app, its events, view and commands,
//! and the source lookups downloads share (YouTube, HLS playlists, the subtitle sync's framing). Named for
//! Kotlin where the app's own names would clash.

use crate::screens::CoreRoot;
use std::sync::Arc;
use zcore::hls::Rung as LevelRung;
use zcore::play::preview::{Frame as SpriteFrame, Preview as SeekPreview, Thumb as SeekThumb};
use zcore::play::youtube::Board as StoryBoard;
use zcore::play::{
    Choice as PlayChoice, Command as PlayCommand, EpisodeRow as PlayEpisode, Event as PlayEvent, Failure as EngineFailure,
    Group as PlayGroup, LocalCopy as CoreLocalCopy, Media as PlayMedia, Panel as PlayerPanel, Pill as PlayPill, Play,
    Referer as VideoReferer, Remote as PlayRemote, ReportView as PlayReport, Settings as PlaySettings, Sound as EngineSound,
    Status as EngineStatus, View as PlayView, Words as PlayerWords,
};
use zcore::progress::Saved as CoreSaved;
use zcore::root::{ScreenId, Turn};
use zcore::screen::{Clock as CoreClock, Screen};

#[uniffi::remote(Record)]
pub struct PlaySettings {
    pub profile: String,
    pub quality: String,
    pub captions: bool,
    pub caption_size: String,
    pub autoplay: bool,
    pub dub: bool,
    pub anime_audio: String,
    pub plus: bool,
}

#[uniffi::remote(Record)]
pub struct CoreLocalCopy {
    pub url: String,
    pub hls: bool,
    pub sound: Option<String>,
    pub captions: Option<String>,
    pub length: f64,
    pub spot: f64,
}

#[uniffi::remote(Record)]
pub struct EngineStatus {
    pub at: f64,
    pub total: f64,
    pub buffered: f64,
    pub paused: bool,
    pub buffering: bool,
    pub throughput: f64,
}

#[uniffi::remote(Enum)]
pub enum EngineFailure {
    Refused,
    Network,
    Unplayable,
    Other,
}

#[uniffi::remote(Record)]
pub struct EngineSound {
    pub id: String,
    pub label: String,
    pub selected: bool,
}

#[uniffi::remote(Enum)]
pub enum PlayerPanel {
    Settings,
    Subtitles,
    Speed,
    Report,
    Episodes,
}

#[uniffi::remote(Enum)]
pub enum PlayEvent {
    Open { title: String, episode: Option<String>, local: bool, settings: PlaySettings, adaptive: bool, screen: f64, hide_ms: u32, fill: bool },
    Warm { title: String, episode: Option<String>, settings: PlaySettings },
    Settings { settings: PlaySettings },
    Local { title: String, episode: Option<String>, copy: Option<CoreLocalCopy>, offline: Option<String> },
    Status { status: EngineStatus },
    Sounds { sounds: Vec<EngineSound> },
    Picture { aspect: f64 },
    Screen { aspect: f64 },
    Ended,
    Failed { failure: EngineFailure, message: String },
    Toggle,
    Pause { paused: bool },
    Seek { to: f64 },
    Jump { seconds: f64 },
    Pill,
    WatchCredits,
    Next,
    Episode { id: String },
    Retry,
    Panel { panel: Option<PlayerPanel> },
    Pick { panel: PlayerPanel, group: u32, choice: u32 },
    Speed { index: u32 },
    Captions { on: bool },
    Reason { index: u32 },
    Send { note: String },
    ReportFault,
    Poke,
    Hide,
    Dismiss,
    Close,
}

#[uniffi::remote(Record)]
pub struct VideoReferer {
    pub value: String,
    pub host: String,
    pub folder: Option<String>,
}

#[uniffi::remote(Record)]
pub struct PlayMedia {
    pub url: String,
    pub hls: bool,
    pub agent: String,
    pub referer: Option<VideoReferer>,
    pub cookies: bool,
    pub sound: Option<String>,
    pub languages: Vec<String>,
    pub track_names: String,
    pub key: String,
}

#[uniffi::remote(Record)]
pub struct LevelRung {
    pub height: i64,
    pub bandwidth: u64,
}

#[uniffi::remote(Enum)]
pub enum PlayCommand {
    FindLocal { title: String, episode: Option<String>, offline: bool },
    Load { media: PlayMedia, start: f64, paused: bool, speed: f64, level: Option<LevelRung>, widest: Option<i64>, first: Option<LevelRung> },
    Pause { paused: bool },
    Seek { to: f64 },
    Speed { speed: f64 },
    Level { level: Option<LevelRung>, widest: Option<i64> },
    Adapt,
    Track { id: String },
    Fit { fill: bool },
    Preload { media: PlayMedia, start: f64, playlists: bool },
    Drop { key: String },
    Setting { group: String, key: String, value: String },
    Spot { key: String, at: f64 },
    Saved { spot: CoreSaved },
    Watched,
    Close,
}

#[uniffi::remote(Record)]
pub struct PlayChoice {
    pub label: String,
    pub on: bool,
    pub badge: Option<String>,
}

#[uniffi::remote(Record)]
pub struct PlayGroup {
    pub title: String,
    pub choices: Vec<PlayChoice>,
}

#[uniffi::remote(Record)]
pub struct PlayPill {
    pub label: String,
    pub next: bool,
    pub count: f64,
    pub left: f64,
}

#[uniffi::remote(Record)]
pub struct PlayReport {
    pub reasons: Vec<PlayChoice>,
    pub details: bool,
    pub ready: bool,
    pub sending: bool,
    pub answer: Option<String>,
}

#[uniffi::remote(Record)]
pub struct PlayEpisode {
    pub id: String,
    pub number: String,
    pub name: String,
    pub playing: bool,
    pub picture: Option<String>,
    pub about: Option<String>,
}

#[uniffi::remote(Record)]
pub struct PlayRemote {
    pub season: Option<String>,
    pub number: Option<i64>,
    pub next_number: Option<i64>,
}

#[uniffi::remote(Record)]
pub struct StoryBoard {
    pub template: String,
    pub w: i64,
    pub h: i64,
    pub count: i64,
    pub cols: i64,
    pub rows: i64,
    pub interval: f64,
    pub name: String,
}

#[uniffi::remote(Record)]
pub struct SeekThumb {
    pub url: String,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[uniffi::remote(Record)]
pub struct SpriteFrame {
    pub start: f64,
    pub end: f64,
    pub thumb: SeekThumb,
}

#[uniffi::remote(Enum)]
pub enum SeekPreview {
    Board { board: StoryBoard },
    Frames { frames: Vec<SpriteFrame> },
}

#[uniffi::remote(Record)]
pub struct PlayView {
    pub open: bool,
    pub title_id: String,
    pub episode_id: Option<String>,
    pub name: String,
    pub place: String,
    pub heading: String,
    pub status: String,
    pub fault: Option<String>,
    pub fault_report: bool,
    pub started: bool,
    pub paused: bool,
    pub buffering: bool,
    pub time: f64,
    pub total: f64,
    pub buffered: f64,
    pub left: String,
    pub caption: String,
    pub captions_on: bool,
    pub has_captions: bool,
    pub caption_size: String,
    pub pill: Option<PlayPill>,
    pub has_next: bool,
    pub controls: bool,
    pub panel: Option<PlayerPanel>,
    pub settings: Vec<PlayGroup>,
    pub subtitles: Vec<PlayGroup>,
    pub speeds: Vec<PlayChoice>,
    pub speed: f64,
    pub speed_label: String,
    pub report: PlayReport,
    pub notice: Option<String>,
    pub episodes: Vec<PlayEpisode>,
    pub season: Option<String>,
    pub preview: Option<SeekPreview>,
    pub fill: bool,
    pub awake: bool,
    pub remote: PlayRemote,
}

#[uniffi::remote(Record)]
pub struct PlayerWords {
    pub try_again: String,
    pub watch_credits: String,
    pub next_episode: String,
    pub report: String,
    pub send: String,
    pub note_hint: String,
    pub report_fault: String,
    pub speed: String,
    pub subtitles: String,
    pub settings: String,
    pub nothing_to_change: String,
    pub no_subtitles: String,
    pub now_playing: String,
    pub dismiss: String,
    pub play: String,
    pub pause: String,
    pub back_10: String,
    pub forward_10: String,
    pub full_screen: String,
    pub leave_full_screen: String,
}

/// The app's one player (`zcore::play::Play`), live on the root for the app's whole run.
#[derive(uniffi::Object)]
pub struct PlayScreen {
    root: Arc<CoreRoot>,
    id: ScreenId,
}

#[uniffi::export]
impl PlayScreen {
    #[uniffi::constructor]
    pub fn new(root: Arc<CoreRoot>) -> Self {
        let id = root.lock().open(Play::default());
        Self { root, id }
    }

    /// Its id on the root, as `Turn`s name it.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Something happened to the player. Draw `view()` again for each changed screen.
    pub fn update(&self, event: PlayEvent, clock: CoreClock) -> Turn {
        self.root.lock().update::<Play>(self.id, event, clock)
    }

    /// Exactly what to draw.
    pub fn view(&self) -> PlayView {
        self.root.lock().view::<Play>(self.id).unwrap_or_else(|| Play::default().view())
    }
}

/// The player's fixed words (`zcore::play::words`).
#[uniffi::export]
pub fn player_words() -> PlayerWords {
    zcore::play::words()
}

/// How often the engine says where it is, in ms (`zcore::play::TICK_MS`).
#[uniffi::export]
pub fn player_tick_ms() -> u32 {
    zcore::play::TICK_MS
}

/// The Referer a video request to `url` goes with (`zcore::play::referer_for`).
#[uniffi::export]
pub fn video_referer(referer: VideoReferer, url: String) -> Option<String> {
    zcore::play::referer_for(&referer, &url)
}

/// What YouTube's files are fetched with.
#[uniffi::export]
pub fn youtube_agent() -> String {
    zcore::play::youtube::AGENT.into()
}
