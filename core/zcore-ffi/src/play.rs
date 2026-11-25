//! The player as Kotlin sees it (`zcore::play`): one screen object for the whole app, its events, view and commands,
//! and the source lookups downloads share (YouTube, HLS playlists, the subtitle sync's framing). Named for
//! Kotlin where the app's own names would clash.

use crate::screens::CoreRoot;
use std::sync::Arc;
use zcore::hls::Rung as LevelRung;
use zcore::play::codecs::{Claimed as DeviceCodecs, Decoders as DeviceDecoders};
use zcore::play::preview::{Frame as SpriteFrame, Preview as SeekPreview, Thumb as SeekThumb};
use zcore::play::youtube::Board as StoryBoard;
use zcore::play::{
    Choice as PlayChoice, Command as PlayCommand, EpisodeRow as PlayEpisode, Event as PlayEvent, Failure as EngineFailure,
    Burst as PlayBurst, Group as PlayGroup, LocalCopy as CoreLocalCopy, Media as PlayMedia, Panel as PlayerPanel, Pill as PlayPill, Play,
    Referer as VideoReferer, ReportView as PlayReport, Settings as PlaySettings, Sound as EngineSound,
    Status as EngineStatus, View as PlayView, Words as PlayerWords,
};
use zcore::on_tv::PlayerState as TvPlayerState;
use zcore::profile_settings::Setting as SettingValue;
use zcore::queue::Item as DownloadItem;
use zcore::root::{ScreenId, Turn};
use zcore::screen::{Clock as CoreClock, Screen};

#[uniffi::remote(Record)]
pub struct PlaySettings {
    pub profile: String,
    pub quality: String,
    pub captions: bool,
    pub caption_size: String,
    pub autoplay: bool,
    pub dramas_audio: String,
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
    pub lang: Option<String>,
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
    Open { title: String, episode: Option<String>, local: bool, settings: PlaySettings, adaptive: bool, screen: f64 },
    Warm { title: String, episode: Option<String>, settings: PlaySettings },
    Settings { settings: PlaySettings },
    Local { title: String, episode: Option<String>, copy: Option<CoreLocalCopy>, offline: Option<Vec<DownloadItem>> },
    Status { status: EngineStatus },
    Sounds { sounds: Vec<EngineSound> },
    Picture { aspect: f64 },
    Screen { aspect: f64 },
    Ended,
    Failed { failure: EngineFailure, message: String },
    Toggle,
    Pause { paused: bool },
    Seek { to: f64 },
    Jump { ahead: bool, held: bool },
    Pill,
    WatchCredits,
    Next,
    Episode { id: String },
    Retry,
    Panel { panel: Option<PlayerPanel> },
    Pick { panel: PlayerPanel, group: u32, choice: u32 },
    PickNamed { group: String, choice: String },
    Speed { index: u32 },
    Captions { on: bool },
    Reason { index: u32 },
    Send { note: String },
    ReportFault,
    Poke,
    Light { level: f64 },
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
    pub relay: bool,
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
    Setting { group: String, key: String, value: SettingValue },
    Spot { key: String, at: f64 },
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
pub struct PlayBurst {
    pub ahead: bool,
    pub seconds: u32,
    pub count: u32,
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
    pub has_episodes: bool,
    pub preview: Option<SeekPreview>,
    pub fill: bool,
    pub awake: bool,
    pub light: Option<f64>,
    pub tv: TvPlayerState,
    pub burst: Option<PlayBurst>,
    pub burst_label: String,
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
    pub episodes: String,
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
/// Whether a source's HTTP status means it turned the stream away (`zcore::play::refused`).
#[uniffi::export]
pub fn source_refused(status: u16) -> bool {
    zcore::play::refused(status)
}

/// ExoPlayer's error as the core weighs it (`zcore::play::failure_of_exo`).
#[uniffi::export]
pub fn exo_failure(code: i32, refused: bool) -> EngineFailure {
    zcore::play::failure_of_exo(code, refused)
}

/// A sound track's place in ExoPlayer's track groups (`zcore::play::parse_sound_id`).
#[derive(Debug, uniffi::Record)]
pub struct SoundPlace {
    pub group: u32,
    pub track: u32,
}

/// How a sound track is named for the core (`zcore::play::sound_id`).
#[uniffi::export]
pub fn sound_id(group: u32, track: u32) -> String {
    zcore::play::sound_id(group, track)
}

/// The place a sound track's name stands for (`zcore::play::parse_sound_id`).
#[uniffi::export]
pub fn parse_sound_id(id: String) -> Option<SoundPlace> {
    zcore::play::parse_sound_id(&id).map(|(group, track)| SoundPlace { group, track })
}

/// What a sound track is called (`zcore::play::sound_label`): the file's name for it, its language in words, or "Track 2"
/// (`n` counts from 1).
#[uniffi::export]
pub fn sound_label(title: Option<String>, language: Option<String>, n: u32) -> String {
    zcore::play::sound_label(title.as_deref(), language.as_deref(), n as usize)
}

#[uniffi::remote(Record)]
pub struct DeviceDecoders {
    pub hevc: bool,
    pub hevc_main10: bool,
    pub av1: bool,
    pub hdr_screen: bool,
}

#[uniffi::remote(Record)]
pub struct DeviceCodecs {
    pub codecs: Vec<String>,
    pub hdr: bool,
}

/// The codec words a device claims for what its hardware decoders and screen do (`zcore::play::codecs::claim`).
#[uniffi::export]
pub fn device_codecs(decoders: DeviceDecoders) -> DeviceCodecs {
    zcore::play::codecs::claim(decoders)
}

/// The line's speed to start on before one is measured, in bits a second (`zcore::play::LINE_SPEED`).
#[uniffi::export]
pub fn default_line_speed() -> u64 {
    zcore::play::LINE_SPEED
}

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
