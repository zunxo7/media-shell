//! The player's decisions, for every app (decision 0008): what to play and from where, what the engine is told,
//! what's saved and reported, what to do when it stalls or fails, and every menu, word and pill on screen. The apps'
//! engines (ExoPlayer, mpv) play what they're told and report time, tracks and failures as categories; the apps draw
//! [`View`] and carry out [`Command`]s.
//!
//! One model for the whole app: it outlives each player so a title page's warm-up, a Real-Debrid link or a YouTube
//! lookup is what Play then plays, and the last save goes out after the player has gone.
//!
//! - **Sources** ([`sources`], [`youtube`], [`hls`]): the plan of `crate::stream`, each step asked
//!   through effects.
//! - **Recovery**, one rule for every app: a stream with a relay behind it moves to the relay; a Real-Debrid
//!   copy that fails gets a fresh link (twice at most), from the same spot; anything else is a fault with Try again. An
//!   engine that can't adapt by itself (mpv) steps down a level after two stalls on Auto.
//! - **Saves**: every 10 s while playing, on moving to another episode, and on leaving; in order, one at a time; not
//!   while offline (the spot is still kept here). The server's "too many requests" (the account's screen is in use
//!   elsewhere) pauses playback and says so, except for the last save and for a download's own copy.
//! - **Resume**: from the saved spot unless it's within the end credits' last 90 s (`title::unfinished`), the same rule
//!   the title page's Resume uses.

pub mod preview;
pub mod sources;
pub mod youtube;

use crate::api::{self, Kind, Outcome};
use crate::captions::{self, Cue};
use crate::abr;
use crate::hls::{self, Rung};
use crate::model::{self, Episode, Title};
use crate::player::{self, Ending, Segment, SkipsFrom};
use crate::progress::{self, Saved};
use crate::quality;
use crate::screen::{cookies_allowed, Asks, Context, Device, Effect, Header, Reply, Request, Screen};
use crate::store::{Need, PageReply};
use crate::stream::{self, Step};
use preview::Preview;
use serde_json::{json, Value};
use std::collections::HashMap;

/// How often the engine reports where it is (ms): what the controls, captions and pills move by.
pub const TICK_MS: u32 = 250;
/// The speeds offered, as Netflix's.
pub const SPEEDS: [f64; 6] = [0.5, 0.75, 1., 1.25, 1.5, 2.];
/// How many titles are read ahead at once (a title page, the next episode, a card).
const PRELOADS: usize = 3;
/// Seconds between saves while playing.
const SAVE_EVERY: f64 = 10.;
/// After this long playing, how the start went is reported (it's reported on leaving, too, if sooner).
const TIMING_AFTER_MS: u32 = 60_000;
/// How long Report's answer shows before its panel closes.
const REPORT_SHOWN_MS: u32 = 1_600;
/// Two minutes from the end of an episode (of a video over 5), the next one gets ready.
const WARM_NEXT: f64 = 120.;
const WARM_NEXT_LONGER: f64 = 300.;
/// Stalls on Auto before an engine that can't adapt steps down a level.
const STALLS_DOWN: u32 = 2;
/// A seek counts as landed once the engine says it's within this many seconds of the spot asked for.
const SEEK_NEAR: f64 = 2.;
/// How long the spot asked for is held while the engine still says it's elsewhere.
const SEEK_HOLD: f64 = 6.;
/// Seconds after Auto moves before it may move again (the engine reopens at the new level).
const SWITCH_REST: f64 = 10.;
/// Levels under this aren't offered in Quality (Auto still goes lower when the line needs it).
const LOWEST_OFFERED: i64 = 360;
/// A seek lands no nearer the end than this (s), so it doesn't finish the video by itself.
const SEEK_END: f64 = 3.;
/// Fit and Fill are offered when the picture's shape and the screen's differ by more than this share.
const FIT_DIFFERS: f64 = 0.03;

// ---------- words ----------

pub const FINDING: &str = "Finding the stream";
pub const STARTING: &str = "Starting";
pub const FAILED: &str = "This video couldn't play. Try again in a moment.";
const NOT_PLAYABLE: &str = "This one isn't playable yet.";
const NOT_AVAILABLE: &str = "This title isn't available to stream right now.";
const NO_STREAM: &str = "This episode has no stream.";
const NOT_DOWN_YET: &str = "This isn't downloaded yet. It plays from here once its download has finished.";
const SCREEN_IN_USE: &str = "This account is playing on another screen.";
/// English sound asked for, but the episode has none to be had: it plays in Japanese.
const NO_DUB: &str = "There's no English dub of this episode, so it's playing in Japanese.";
const REPORTED: &str = "Thanks, it's been reported.";
const NOT_REPORTED: &str = "Couldn't send it. Try again.";

/// The fixed words the player shows, for the apps to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct Words {
    pub try_again: String,
    pub watch_credits: String,
    pub next_episode: String,
    pub report: String,
    pub send: String,
    pub note_hint: String,
    /// The button under "it couldn't play" that tells the owner about it.
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

/// The player's fixed words.
pub fn words() -> Words {
    Words {
        try_again: "Try again".into(),
        watch_credits: "Watch credits".into(),
        next_episode: "Next episode".into(),
        report: "Report a problem".into(),
        send: "Send".into(),
        note_hint: "Details (optional)".into(),
        report_fault: "Report".into(),
        speed: "Playback speed".into(),
        subtitles: "Subtitles".into(),
        settings: "Settings".into(),
        nothing_to_change: "Nothing to change for this one.".into(),
        no_subtitles: "None for this one yet".into(),
        now_playing: "Now playing".into(),
        dismiss: "Dismiss".into(),
        play: "Play".into(),
        pause: "Pause".into(),
        back_10: "Back 10 seconds".into(),
        forward_10: "Forward 10 seconds".into(),
        full_screen: "Full screen".into(),
        leave_full_screen: "Exit full screen".into(),
    }
}

/// A speed as its stop is named: "1× (Normal)", "1.5×"; `short` drops "(Normal)".
pub fn speed_name(speed: f64, short: bool) -> String {
    let n = format!("{speed}");
    if (speed - 1.).abs() < 1e-6 {
        if short { "1×".into() } else { "1× (Normal)".into() }
    } else {
        format!("{n}×")
    }
}

/// Report's problems. "Other" stays last: a TV, with no box for details, leaves it out (it would say nothing).
const REASONS: [(&str, &str); 4] =
    [("loading", "Loading"), ("subtitles", "Subtitles"), ("audio-video", "Audio & video"), ("other", "Other")];
/// What a report from "it couldn't play" says went wrong.
const CANT_PLAY: &str = "cant-play";

// ---------- what the app sends ----------

/// The profile's settings the player follows. Its own picks change them ([`Command::Setting`]), and the app tells it
/// of any change made elsewhere ([`Event::Settings`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub profile: String,
    /// "auto", "1080p", "2160p HDR"…
    pub quality: String,
    pub captions: bool,
    /// "small", "medium", "large".
    pub caption_size: String,
    pub autoplay: bool,
    /// Dramas from abroad play dubbed.
    pub dub: bool,
    /// Anime's sound: "sub" or "dub".
    pub anime_audio: String,
    /// The account may watch 2K and 4K.
    pub plus: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            profile: String::new(),
            quality: "auto".into(),
            captions: true,
            caption_size: "medium".into(),
            autoplay: true,
            dub: false,
            anime_audio: "sub".into(),
            plus: false,
        }
    }
}

/// A copy of a title on this device, as the app's downloads have it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalCopy {
    pub url: String,
    pub hls: bool,
    /// Its sound as a file of its own (YouTube's are).
    pub sound: Option<String>,
    /// Its subtitles file's words.
    pub captions: Option<String>,
    /// Its whole length.
    pub length: f64,
    /// Where it was left, as kept with it.
    pub spot: f64,
}

/// What the engine says now: where it is in what it was given (seconds from its start), how long that is, how much
/// is loaded, and whether it's paused or waiting for data.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Status {
    pub at: f64,
    pub total: f64,
    pub buffered: f64,
    pub paused: bool,
    pub buffering: bool,
    /// Bits a second the engine downloaded at over the last moment (0: not downloading, or it can't say).
    pub throughput: f64,
}

/// Why the engine failed, as the core weighs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The source turned it away (HTTP 403 or 410): a token or link that ran out.
    Refused,
    /// The network: a dropped connection, a timeout, another HTTP error.
    Network,
    /// What came can't be played here (a format, a decoder).
    Unplayable,
    Other,
}

/// A sound track the engine has.
#[derive(Clone, Debug, PartialEq)]
pub struct Sound {
    pub id: String,
    pub label: String,
    pub selected: bool,
}

/// The player's panels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Settings,
    Subtitles,
    Speed,
    Report,
    Episodes,
}

/// What happens to the player.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Play a title: the episode asked for (None: the first, or a film); `local`: asked from Downloads, so the copy on
    /// this device plays even while it's coming down. `adaptive`: the engine picks its own level on Auto (ExoPlayer);
    /// mpv doesn't. `screen`: the screen's shape, width over height. `hide_ms`: how long the controls stay up untouched.
    /// `fill`: the picture fills the screen (the device's own setting).
    Open { title: String, episode: Option<String>, local: bool, settings: Settings, adaptive: bool, screen: f64, hide_ms: u32, fill: bool },
    /// A title page (or a card) wants a title ready before Play: what Play would play first is found, and read ahead.
    Warm { title: String, episode: Option<String>, settings: Settings },
    /// The profile's settings changed elsewhere.
    Settings { settings: Settings },
    /// The answer to [`Command::FindLocal`]: the copy on this device, and (asked offline) what's known of the title
    /// there, as a title page's JSON.
    Local { title: String, episode: Option<String>, copy: Option<LocalCopy>, offline: Option<String> },
    /// The engine, as it plays (every [`TICK_MS`]).
    Status { status: Status },
    /// The engine's sound tracks.
    Sounds { sounds: Vec<Sound> },
    /// The picture's shape, width over height.
    Picture { aspect: f64 },
    /// The screen's shape changed (a window resized).
    Screen { aspect: f64 },
    /// What it was given played to its end.
    Ended,
    /// The engine failed: why, and its own words (for the report).
    Failed { failure: Failure, message: String },
    /// Play or pause.
    Toggle,
    /// Paused or playing, as a media key says.
    Pause { paused: bool },
    Seek { to: f64 },
    Jump { seconds: f64 },
    /// The Skip / Next episode pill pressed.
    Pill,
    WatchCredits,
    Next,
    /// Another episode of this show, from its panel.
    Episode { id: String },
    Retry,
    /// A panel opened, or closed (None).
    Panel { panel: Option<Panel> },
    /// A choice in the settings or subtitles panel: its group and its place in it.
    Pick { panel: Panel, group: u32, choice: u32 },
    Speed { index: u32 },
    Captions { on: bool },
    /// Report's problem chosen, and then sent with a note.
    Reason { index: u32 },
    Send { note: String },
    /// Report pressed under "it couldn't play".
    ReportFault,
    /// Something was touched: the controls come up and stay a while.
    Poke,
    /// The controls put away.
    Hide,
    /// The notice dismissed.
    Dismiss,
    /// The player has gone.
    Close,
}

// ---------- what the core asks of the app ----------

/// Where a video's Referer goes: its host, and its playlist's folder (an anime source's pieces come from other hosts
/// under the playlist's own folder).
#[derive(Clone, Debug, PartialEq)]
pub struct Referer {
    pub value: String,
    pub host: String,
    pub folder: Option<String>,
}

/// What the engine is given to play.
#[derive(Clone, Debug, PartialEq)]
pub struct Media {
    pub url: String,
    pub hls: bool,
    /// Every request for it goes with this User-Agent.
    pub agent: String,
    pub referer: Option<Referer>,
    /// It's the zunflix server's: its requests carry the sign-in.
    pub cookies: bool,
    /// Its sound as a file of its own.
    pub sound: Option<String>,
    /// The sound wanted, best first (ISO 639-1), and every name each goes by (mpv's alang).
    pub languages: Vec<String>,
    pub track_names: String,
    /// What a read-ahead of it is known by.
    pub key: String,
}

/// What the app is asked to do for the player.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Say what's on this device of a title ([`Event::Local`]); `offline`: the title page couldn't be had, so what
    /// downloads know of it too.
    FindLocal { title: String, episode: Option<String>, offline: bool },
    /// Play this from `start`, paused or not, at `speed`; `level`: a fixed level (None: the engine's Auto, never wider
    /// than `widest`); `first`: for an engine that adapts by itself on Auto, the level to hold until [`Command::Adapt`],
    /// where [`abr`] starts.
    Load { media: Media, start: f64, paused: bool, speed: f64, level: Option<Rung>, widest: Option<i64>, first: Option<Rung> },
    Pause { paused: bool },
    Seek { to: f64 },
    Speed { speed: f64 },
    /// A level picked (None: Auto, under `widest`).
    Level { level: Option<Rung>, widest: Option<i64> },
    /// Auto may leave its starting level now: the engine believes its own measuring from here.
    Adapt,
    /// One of the engine's sound tracks.
    Track { id: String },
    Fit { fill: bool },
    /// Read the first seconds of this at `start`, ahead of Play; `playlists`: only its playlists (a fixed quality is
    /// chosen once its levels are known).
    Preload { media: Media, start: f64, playlists: bool },
    /// What was read ahead under `key` won't be played: let it go.
    Drop { key: String },
    /// A profile setting changed: the app's settings store keeps it (`value` as JSON).
    Setting { group: String, key: String, value: String },
    /// A download's own spot.
    Spot { key: String, at: f64 },
    /// A spot just saved here, shown before the server's page has it.
    Saved { spot: Saved },
    /// The last spot is saved: what shows saved spots asks for them again.
    Watched,
    /// Nothing more to play: the player closes.
    Close,
}

// ---------- what the app draws ----------

/// A choice in a panel: its words, whether it's the one in use, and a small mark after it.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub label: String,
    pub on: bool,
    pub badge: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub title: String,
    pub choices: Vec<Choice>,
}

/// The pill at the bottom right: Skip intro / recap / credits, or Next episode (with Watch credits beside it), how far
/// its countdown is (0 to 1) and the seconds left of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Pill {
    pub label: String,
    pub next: bool,
    pub count: f64,
    pub left: f64,
}

/// Report a problem.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReportView {
    pub reasons: Vec<Choice>,
    /// A box for details goes under the problems (not on a TV).
    pub details: bool,
    pub ready: bool,
    pub sending: bool,
    /// What the server said, shown in the panel's place before it closes.
    pub answer: Option<String>,
}

/// An episode in the player's episodes panel.
#[derive(Clone, Debug, PartialEq)]
pub struct EpisodeRow {
    pub id: String,
    pub number: String,
    pub name: String,
    pub playing: bool,
    pub picture: Option<String>,
    pub about: Option<String>,
}

/// What a TV tells the phones steering it (`crate::on_tv` words it): the episode's season name and number, and the
/// next one's number.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Remote {
    pub season: Option<String>,
    pub number: Option<i64>,
    pub next_number: Option<i64>,
}

/// Exactly what the player draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// A player is open.
    pub open: bool,
    pub title_id: String,
    pub episode_id: Option<String>,
    pub name: String,
    /// "S1 E3"; "" for a film.
    pub place: String,
    /// While it waits: "Name  ·  S1 E3".
    pub heading: String,
    /// What it's doing while it looks ("Finding the stream").
    pub status: String,
    /// It couldn't play: why (Try again goes with it).
    pub fault: Option<String>,
    /// Report shows beside Try again (once: gone for good when it's pressed).
    pub fault_report: bool,
    /// The picture is up.
    pub started: bool,
    pub paused: bool,
    pub buffering: bool,
    pub time: f64,
    pub total: f64,
    pub buffered: f64,
    /// What's left, as the clock beside the bar shows it.
    pub left: String,
    pub caption: String,
    pub captions_on: bool,
    pub has_captions: bool,
    pub caption_size: String,
    pub pill: Option<Pill>,
    /// There's a next episode.
    pub has_next: bool,
    pub controls: bool,
    pub panel: Option<Panel>,
    /// Quality, sound and picture.
    pub settings: Vec<Group>,
    /// Subtitles on or off, and their size.
    pub subtitles: Vec<Group>,
    pub speeds: Vec<Choice>,
    pub speed: f64,
    /// "Speed (1×)".
    pub speed_label: String,
    pub report: ReportView,
    pub notice: Option<String>,
    /// The season's episodes, while their panel is open; its name.
    pub episodes: Vec<EpisodeRow>,
    pub season: Option<String>,
    pub preview: Option<Preview>,
    pub fill: bool,
    /// The screen stays on.
    pub awake: bool,
    pub remote: Remote,
}

// ---------- the model ----------

/// Where a stream came from: what's retried, and what the start report calls it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum From {
    Local,
    Debrid,
    Anime,
    YouTube,
    Stream,
}

/// Where its subtitles come from.
#[derive(Clone, Debug, PartialEq)]
enum Words_ {
    None,
    Text(String),
    Url(String),
    YouTube(String),
    Ary(String),
    Cinema(Value),
}

#[derive(Clone, Debug, PartialEq)]
struct Source {
    from: From,
    url: String,
    hls: bool,
    referer: Option<String>,
    fallback: Option<String>,
    sound: Option<String>,
    audio: Option<String>,
    audios: Vec<String>,
    skip: Vec<Segment>,
    captions: Words_,
    youtube: Option<String>,
    /// The Real-Debrid copy it came from.
    server: Option<String>,
}

impl Source {
    fn new(from: From, url: String, hls: bool) -> Self {
        Self {
            from,
            url,
            hls,
            referer: None,
            fallback: None,
            sound: None,
            audio: None,
            audios: Vec::new(),
            skip: Vec::new(),
            captions: Words_::None,
            youtube: None,
            server: None,
        }
    }
}

/// How far a lookup shared by every play has got.
#[derive(Clone, Debug, PartialEq)]
enum Known<T> {
    Asking,
    Got(T),
}

/// How a step of finding the stream stands.
enum Found {
    Wait,
    Yes(Box<Source>),
    /// This step has nothing: on to the next.
    No,
    /// Nothing plays: why.
    Fail(String),
}

/// The stage a player is at.
#[derive(Debug, PartialEq)]
enum Stage {
    Title,
    Local,
    Finding { steps: Vec<Step>, index: usize },
    Playing,
    Fault,
}

struct Player {
    /// Bumped for every load (an episode, a restart): answers for an older one are dropped.
    run: u32,
    /// Bumped for every source played: captions for another are dropped.
    src: u32,
    title_id: String,
    wanted: Option<String>,
    local_only: bool,
    adaptive: bool,
    screen: f64,
    hide_ms: u32,
    title: Option<Title>,
    ep: Option<Episode>,
    local: Option<Option<LocalCopy>>,
    spots: Option<Vec<Saved>>,
    stage: Stage,
    restart: Option<f64>,
    status: String,
    fault: Option<String>,
    source: Option<Source>,
    // (scratch for the step being tried)
    anime: Option<Result<sources::Anime, String>>,
    yt_server: Option<Result<String, String>>,
    yt_waited: bool,
    yt_timer: bool,
    // playing
    offset: f64,
    begun_at: f64,
    engine: Status,
    /// The engine has said where it is since it was last given something to play.
    loaded: bool,
    paused: bool,
    started: bool,
    cues: Vec<Cue>,
    /// The source's own subtitles were asked for.
    words_asked: bool,
    segments: Vec<Segment>,
    skips_asked: bool,
    skipped: Option<Segment>,
    ending: Ending,
    levels: Vec<Rung>,
    height: i64,
    auto_cap: i64,
    recent_stalls: u32,
    /// The bandwidth as the engine measured it, when it was last told, and when Auto last moved.
    estimate: abr::Estimate,
    measured_at: Option<f64>,
    switched_at: f64,
    /// An engine that adapts by itself has been let go of its starting level.
    adapting: bool,
    /// A seek asked for (the engine's time, and when) that the engine hasn't reached yet.
    seeking: Option<(f64, f64)>,
    /// Times the Real-Debrid copy failed this play: each of the first two gets a fresh link.
    debrid_fails: u32,
    sounds: Vec<Sound>,
    aspect: f64,
    fill: bool,
    preview: Option<Preview>,
    // how the start went
    opened: f64,
    ready_ms: Option<f64>,
    first_ms: Option<f64>,
    stalls: u32,
    stalled_ms: f64,
    stuck_at: Option<f64>,
    moving: bool,
    reported: bool,
    warmed_next: bool,
    last_save: f64,
    // controls
    controls: bool,
    poked: f64,
    hiding: bool,
    panel: Option<Panel>,
    reason: Option<usize>,
    sending: bool,
    answer: Option<String>,
    /// "It couldn't play" reported from its own Report: once for the title opened, whatever Try again does.
    fault_told: bool,
    notice: Option<String>,
}

impl Player {
    fn new(title: String, episode: Option<String>, local: bool, adaptive: bool, screen: f64, hide_ms: u32, now: f64) -> Self {
        Self {
            run: 0,
            src: 0,
            title_id: title,
            wanted: episode,
            local_only: local,
            adaptive,
            screen,
            hide_ms,
            title: None,
            ep: None,
            local: None,
            spots: None,
            stage: Stage::Title,
            restart: None,
            status: String::new(),
            fault: None,
            source: None,
            anime: None,
            yt_server: None,
            yt_waited: false,
            yt_timer: false,
            offset: 0.,
            begun_at: 0.,
            engine: Status::default(),
            loaded: false,
            paused: false,
            started: false,
            cues: Vec::new(),
            words_asked: false,
            segments: Vec::new(),
            skips_asked: false,
            skipped: None,
            ending: Ending::default(),
            levels: Vec::new(),
            height: 0,
            auto_cap: abr::START,
            recent_stalls: 0,
            estimate: abr::Estimate::default(),
            measured_at: None,
            switched_at: 0.,
            adapting: false,
            seeking: None,
            debrid_fails: 0,
            sounds: Vec::new(),
            aspect: 0.,
            fill: false,
            preview: None,
            opened: now,
            ready_ms: None,
            first_ms: None,
            stalls: 0,
            stalled_ms: 0.,
            stuck_at: None,
            moving: false,
            reported: false,
            warmed_next: false,
            last_save: now,
            controls: true,
            poked: now,
            hiding: true,
            panel: None,
            reason: None,
            sending: false,
            answer: None,
            fault_told: false,
            notice: None,
        }
    }

    /// Where it is in the film.
    fn time(&self) -> f64 {
        if self.loaded {
            self.offset + self.engine.at
        } else {
            self.begun_at
        }
    }

    fn total(&self) -> f64 {
        self.offset + self.engine.total
    }

    fn key(&self) -> String {
        self.ep.as_ref().map(|e| e.id.clone()).unwrap_or_else(|| self.title_id.clone())
    }

    fn next(&self) -> Option<&Episode> {
        let title = self.title.as_ref()?;
        let at = title.episodes.iter().position(|e| Some(&e.id) == self.ep.as_ref().map(|e| &e.id))?;
        title.episodes.get(at + 1)
    }

    fn on(&self, from: From) -> bool {
        self.source.as_ref().is_some_and(|s| s.from == from)
    }

    fn credits(&self) -> Option<f64> {
        player::end_credits(&self.segments)
    }

    /// The intro, recap or credits playing now that a Skip pill offers to jump past, unless it was skipped already.
    fn skip_now(&self) -> Option<&Segment> {
        let at = self.time();
        self.segments.iter().find(|s| {
            s.end.is_some_and(|end| at >= s.start && at < end - 1.) && self.skipped.as_ref() != Some(*s)
        })
    }
}

/// What a warm-up is waiting for.
#[derive(Debug)]
struct Warm {
    title_id: String,
    wanted: Option<String>,
    title: Option<Title>,
    local: Option<bool>,
    spots: Option<Vec<Saved>>,
    done: bool,
}

/// What an effect was asked for.
#[derive(Clone, Debug, PartialEq)]
enum Ask {
    Title { run: u32 },
    /// A show asked for with no episode: the saved spots, to pick up where it was left.
    Pick { run: u32 },
    Progress { run: u32 },
    WarmTitle { title: String },
    WarmProgress { title: String },
    Copies { title: String },
    Link { copy: String },
    YouTube { id: String, visitor: Option<String> },
    YouTubeWait { run: u32 },
    YouTubeServer { run: u32 },
    Anime { run: u32 },
    Master { src: u32 },
    Captions { src: u32, kind: CaptionAsk },
    Intro { run: u32 },
    Skips { run: u32, left: u32 },
    SkipsWait { run: u32, left: u32 },
    Preview { run: u32, board: bool },
    Save { ended: bool, local: bool, title: String },
    Report,
    Timing,
    Hide { opened: u64 },
    ReportShown,
    Minute { opened: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaptionAsk {
    File,
    YouTube,
    Server,
    Quick,
    Full,
}

/// The player for the whole app. See the module's docs.
pub struct Play {
    settings: Settings,
    speed: f64,
    /// The spots saved this run, newer than the account's list until it's asked for again.
    spots: HashMap<String, (f64, f64)>,
    /// Real-Debrid copies, by film or episode.
    copies: HashMap<String, (f64, Known<Vec<sources::DebridCopy>>)>,
    /// A copy's play link, by copy id.
    links: HashMap<String, (f64, Known<Option<String>>)>,
    lookups: HashMap<String, (f64, Known<Result<youtube::Info, String>>)>,
    warms: Vec<Warm>,
    /// What's been read ahead and not played yet, by key, with when.
    preloads: Vec<(String, f64)>,
    asks: Asks<Ask>,
    p: Option<Player>,
    opens: u64,
    saving: bool,
    queued: Option<(String, bool, bool, String)>,
    tv: bool,
}

impl Default for Play {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
            speed: 1.,
            spots: HashMap::new(),
            copies: HashMap::new(),
            links: HashMap::new(),
            lookups: HashMap::new(),
            warms: Vec::new(),
            preloads: Vec::new(),
            asks: Asks::default(),
            p: None,
            opens: 0,
            saving: false,
            queued: None,
            tv: false,
        }
    }
}

fn order(command: Command) -> Effect {
    Effect::Player { order: command }
}

/// The last spot is in: the app's own reader of spots, and every screen showing how far things are watched, ask again.
fn watched() -> [Effect; 2] {
    [order(Command::Watched), Effect::Tell { event: crate::screen::AppEvent::Watched }]
}

fn now_ms(cx: &Context) -> f64 {
    cx.clock.now * 1000.
}

fn body(reply: Reply, cx: &Context) -> Result<String, api::Failure> {
    match api::outcome(reply, cx.online) {
        Outcome::Body { body, .. } => Ok(body),
        Outcome::Unchanged => Ok(String::new()),
        Outcome::Failed { failure } => Err(failure),
    }
}

fn page(reply: Reply) -> Option<PageReply> {
    match reply {
        Reply::Page { page } => Some(page),
        _ => None,
    }
}

/// What a title (or episode) is known by among the plays.
fn play_key(title: &Title, ep: Option<&Episode>) -> String {
    format!("{}:{}", title.id, ep.map_or("movie", |e| e.id.as_str()))
}

impl Play {
    fn post(&mut self, cx: &Context, ask: Ask, path: &str, json: String) -> Effect {
        Effect::Request { id: self.asks.ask(ask), request: api::post(&cx.base, &cx.who(), path, json) }
    }

    fn get(&mut self, cx: &Context, ask: Ask, path: &str) -> Effect {
        Effect::Request { id: self.asks.ask(ask), request: api::get(&cx.base, &cx.who(), path, None) }
    }

    fn send(&mut self, ask: Ask, request: Request) -> Effect {
        Effect::Request { id: self.asks.ask(ask), request }
    }

    fn timer(&mut self, ask: Ask, ms: u32) -> Effect {
        Effect::Timer { id: self.asks.ask(ask), ms }
    }

    fn progress_page(&mut self, ask: Ask) -> Effect {
        let key = crate::session::mine("progress", &self.settings.profile);
        Effect::Page { id: self.asks.ask(ask), path: progress::PATH.into(), key, need: Need::Current }
    }

    fn title_page(&mut self, ask: Ask, id: &str) -> Effect {
        Effect::Page { id: self.asks.ask(ask), path: format!("/api/title/{id}"), key: format!("title-{id}"), need: Need::Any }
    }

    // ---------- shared lookups ----------

    /// A film's or an episode's Real-Debrid copies, kept [`stream::DEBRID_KEPT`]; None while they're asked for.
    fn debrid(&mut self, title: &Title, ep: Option<&Episode>, cx: &Context, out: &mut Vec<Effect>) -> Option<Vec<sources::DebridCopy>> {
        let now = cx.clock.now;
        self.copies.retain(|_, (at, known)| *known == Known::Asking || now - *at < stream::DEBRID_KEPT);
        let key = play_key(title, ep);
        match self.copies.get(&key) {
            Some((_, Known::Got(copies))) => Some(copies.clone()),
            Some((_, Known::Asking)) => None,
            None => {
                if ep.is_some_and(|e| e.season.is_none() || e.n.is_none()) {
                    return Some(Vec::new());
                }
                self.copies.insert(key.clone(), (now, Known::Asking));
                let body = sources::debrid_body(title, ep, &cx.platform.codecs);
                out.push(self.post(cx, Ask::Copies { title: key }, sources::DEBRID_PATH, body));
                None
            }
        }
    }

    /// A copy's play link; None while it's asked for, Some(None): it can't be had.
    fn debrid_link(&mut self, copy: &str, cx: &Context, out: &mut Vec<Effect>) -> Option<Option<String>> {
        let now = cx.clock.now;
        self.links.retain(|_, (at, known)| *known == Known::Asking || now - *at < stream::DEBRID_KEPT);
        match self.links.get(copy) {
            Some((_, Known::Got(url))) => Some(url.clone()),
            Some((_, Known::Asking)) => None,
            None => {
                self.links.insert(copy.into(), (now, Known::Asking));
                let body = sources::debrid_link_body(copy);
                out.push(self.post(cx, Ask::Link { copy: copy.into() }, sources::DEBRID_LINK_PATH, body));
                None
            }
        }
    }

    /// A film's or an episode's Real-Debrid copy for the profile's quality, ready to play; None while asking, Some(None): none.
    fn debrid_source(&mut self, title: &Title, ep: Option<&Episode>, cx: &Context, out: &mut Vec<Effect>) -> Option<Option<Source>> {
        let copies = self.debrid(title, ep, cx, out)?;
        let Some(copy) = sources::choose_copy(&copies, &self.settings.quality).cloned() else { return Some(None) };
        let url = self.debrid_link(&copy.id, cx, out)?;
        Some(url.map(|url| {
            let hls = stream::is_hls(&url);
            let mut s = Source::new(From::Debrid, url, hls);
            s.server = Some(copy.id);
            s.captions = Words_::Cinema(sources::ids(title, ep));
            s
        }))
    }

    /// A drama episode's streams from YouTube, kept [`youtube::KEEP`]; None while they're asked for.
    fn lookup(&mut self, id: &str, cx: &Context, out: &mut Vec<Effect>) -> Option<Result<youtube::Info, String>> {
        let now = cx.clock.now;
        self.lookups.retain(|_, (at, known)| *known == Known::Asking || now - *at < youtube::KEEP);
        match self.lookups.get(id) {
            Some((_, Known::Got(found))) => Some(found.clone()),
            Some((_, Known::Asking)) => None,
            None => {
                self.lookups.insert(id.into(), (now, Known::Asking));
                out.push(self.send(Ask::YouTube { id: id.into(), visitor: None }, youtube::visitor_request()));
                None
            }
        }
    }

    fn media(&self, source: &Source, title: &Title, base: &str) -> Media {
        // (anime's sound as picked, Japanese or English: some of its files carry both, and with no origin it went English)
        let languages = if source.from == From::Anime {
            stream::languages(Some("japanese"), source.audio.as_deref() == Some("dub"))
        } else {
            stream::languages(title.origin.as_deref(), self.settings.dub)
        };
        let (url, sound) = (source.url.clone(), source.sound.clone());
        let referer = source.referer.as_ref().and_then(|value| referer(&url, value));
        Media {
            cookies: cookies_allowed(base, &url),
            key: url.clone(),
            url,
            hls: source.hls,
            agent: youtube::AGENT.into(),
            referer,
            sound,
            track_names: stream::track_names(&languages),
            languages: languages.into_iter().map(String::from).collect(),
        }
    }

    // ---------- opening ----------

    fn open(&mut self, p: Player, cx: &Context) -> Vec<Effect> {
        let mut out = Vec::new();
        if self.p.is_some() {
            out.extend(self.leave(cx));
        }
        self.opens += 1;
        let (id, hide) = (p.title_id.clone(), p.hide_ms);
        self.p = Some(p);
        out.push(self.timer(Ask::Hide { opened: self.opens }, hide));
        out.push(self.title_page(Ask::Title { run: 0 }, &id));
        out
    }

    /// The episode playing and what's on this device of it: asked once the title is known.
    fn titled(&mut self, title: Title, cx: &Context) -> Vec<Effect> {
        let Some(p) = self.p.as_mut() else { return vec![] };
        if !title.episodes.is_empty() && p.wanted.is_none() {
            let run = p.run;
            p.title = Some(title);
            return vec![self.progress_page(Ask::Pick { run })];
        }
        let ep = if title.episodes.is_empty() {
            None
        } else {
            title.episodes.iter().find(|e| Some(&e.id) == p.wanted.as_ref()).or(title.episodes.first()).cloned()
        };
        p.ep = ep;
        p.title = Some(title);
        p.wanted = p.ep.as_ref().map(|e| e.id.clone());
        self.load(None, cx)
    }

    /// Finds the stream and plays it: from `at` (a restart), or from the saved spot (None: a new episode).
    fn load(&mut self, at: Option<f64>, cx: &Context) -> Vec<Effect> {
        let mut out = Vec::new();
        let Some(p) = self.p.as_mut() else { return out };
        p.run += 1;
        p.fault = None;
        p.status.clear();
        p.anime = None;
        p.yt_server = None;
        p.yt_waited = false;
        p.yt_timer = false;
        p.restart = at;
        p.engine.buffering = true;
        if at.is_none() {
            p.started = false;
            p.engine = Status { buffering: true, ..Status::default() };
            p.cues.clear();
            p.source = None;
            p.levels.clear();
            p.debrid_fails = 0;
            p.segments.clear();
            p.skips_asked = false;
            p.skipped = None;
            p.ending = Ending::default();
            p.preview = None;
            p.local = None;
            p.spots = None;
            p.height = quality::height(&self.settings.quality);
            p.auto_cap = abr::START;
            p.recent_stalls = 0;
            p.estimate = abr::Estimate::default();
            p.measured_at = None;
        }
        let run = p.run;
        let (title, ep) = (p.title.as_ref().map(|t| t.id.clone()).unwrap_or_default(), p.ep.as_ref().map(|e| e.id.clone()));
        if p.local.is_none() {
            p.stage = Stage::Local;
            out.push(order(Command::FindLocal { title, episode: ep, offline: false }));
            out.push(self.progress_page(Ask::Progress { run }));
            return out;
        }
        out.extend(self.find(cx));
        out
    }

    /// With the title, what's on this device and the saved spots known: where to play from, and from what.
    fn find(&mut self, cx: &Context) -> Vec<Effect> {
        let mut out = Vec::new();
        let run_spots = self.spots.clone();
        let Some(p) = self.p.as_mut() else { return out };
        if p.local.is_none() || p.spots.is_none() && p.restart.is_none() {
            return out;
        }
        let Some(title) = p.title.clone() else { return out };
        let key = p.key();
        let copy = p.local.clone().flatten();
        if p.local_only && copy.is_none() {
            p.stage = Stage::Fault;
            p.fault = Some(NOT_DOWN_YET.into());
            return out;
        }
        let from = p.restart.unwrap_or_else(|| {
            let saved = run_spots
                .get(&key)
                .copied()
                .or_else(|| p.spots.as_ref()?.iter().find(|s| s.key == key).map(|s| (s.at, s.length)))
                .or_else(|| copy.as_ref().map(|c| (c.spot, if c.length > 0. { c.length } else { f64::MAX })));
            saved.filter(|(at, length)| crate::title::unfinished(*at, *length)).map_or(0., |(at, _)| at)
        });
        p.begun_at = from;
        if let Some(c) = copy {
            let mut source = Source::new(From::Local, c.url.clone(), c.hls);
            source.sound = c.sound.clone();
            source.captions = c.captions.clone().map_or(Words_::None, Words_::Text);
            out.extend(self.begin(source, from, true, cx));
            return out;
        }
        let ep = p.ep.clone();
        let stream_of = ep.as_ref().and_then(|e| e.stream.clone()).or_else(|| title.stream.clone());
        let steps = stream::plan(&stream::Ask {
            source: &title.source,
            kind: &title.kind,
            episode: ep.is_some(),
            imdb: title.imdb.is_some(),
            stream: stream_of.as_deref(),
        });
        if steps.is_empty() {
            p.stage = Stage::Fault;
            p.fault = Some(NOT_PLAYABLE.into());
            return out;
        }
        if title.source == "cinema" {
            p.status = FINDING.into();
        }
        p.stage = Stage::Finding { steps, index: 0 };
        out.extend(self.drive(cx));
        out
    }

    /// The step being tried, again: on to the next when it has nothing, played when it found something.
    fn drive(&mut self, cx: &Context) -> Vec<Effect> {
        let mut out = Vec::new();
        loop {
            let Some(p) = self.p.as_ref() else { return out };
            let Stage::Finding { steps, index } = &p.stage else { return out };
            let (step, index, count) = (steps[*index], *index, steps.len());
            match self.step(step, cx, &mut out) {
                Found::Wait => return out,
                Found::Yes(source) => {
                    let at = self.p.as_ref().map_or(0., |p| p.begun_at);
                    out.extend(self.begin(*source, at, true, cx));
                    return out;
                }
                Found::No => {
                    let Some(p) = self.p.as_mut() else { return out };
                    if index + 1 < count {
                        if let Stage::Finding { index, .. } = &mut p.stage {
                            *index += 1;
                        }
                    } else {
                        p.stage = Stage::Fault;
                        p.fault = Some(NOT_AVAILABLE.into());
                        return out;
                    }
                }
                Found::Fail(words) => {
                    let Some(p) = self.p.as_mut() else { return out };
                    p.stage = Stage::Fault;
                    p.fault = Some(words);
                    return out;
                }
            }
        }
    }

    fn step(&mut self, step: Step, cx: &Context, out: &mut Vec<Effect>) -> Found {
        let Some(p) = self.p.as_ref() else { return Found::Wait };
        let (Some(title), ep, run) = (p.title.clone(), p.ep.clone(), p.run) else { return Found::Fail(FAILED.into()) };
        match step {
            Step::Anime => {
                let Some(ep) = ep else { return Found::No };
                match self.p.as_ref().and_then(|p| p.anime.clone()) {
                    None => {
                        let path = sources::anime_path(&title, &ep, &self.settings.anime_audio);
                        out.push(self.get(cx, Ask::Anime { run }, &path));
                        Found::Wait
                    }
                    Some(Err(words)) => Found::Fail(words),
                    Some(Ok(a)) => {
                        let mut s = Source::new(From::Anime, a.url, true);
                        s.referer = a.referer;
                        s.fallback = a.fallback;
                        s.captions = a.captions.map_or(Words_::None, Words_::Url);
                        let no_dub = self.settings.anime_audio == "dub" && a.audio.as_deref() == Some("sub");
                        s.audio = a.audio;
                        s.audios = a.audios;
                        s.skip = a.skip;
                        if let Some(p) = self.p.as_mut().filter(|_| no_dub) {
                            p.notice = Some(NO_DUB.into());
                        }
                        Found::Yes(Box::new(s))
                    }
                }
            }
            Step::YouTube => {
                let Some(ep) = ep else { return Found::No };
                let id = sources::bare(&ep.id, "yt-").to_string();
                let server = |s: &mut Self, out: &mut Vec<Effect>| -> Found {
                    match s.p.as_ref().and_then(|p| p.yt_server.clone()) {
                        None => {
                            out.push(s.get(cx, Ask::YouTubeServer { run }, &sources::youtube_play_path(&id)));
                            Found::Wait
                        }
                        Some(Err(words)) => Found::Fail(words),
                        Some(Ok(url)) => {
                            let mut src = Source::new(From::YouTube, api::url(&cx.base, &url), true);
                            src.captions = Words_::YouTube(id.clone());
                            Found::Yes(Box::new(src))
                        }
                    }
                };
                if self.p.as_ref().is_some_and(|p| p.yt_waited || p.yt_server.is_some()) {
                    return server(self, out);
                }
                match self.lookup(&id, cx, out) {
                    Some(Ok(info)) if info.hls.is_some() => {
                        let mut src = Source::new(From::YouTube, info.hls.unwrap_or_default(), true);
                        src.fallback = Some(api::url(&cx.base, &sources::youtube_relay_path(&id)));
                        src.youtube = Some(id.clone());
                        src.captions = Words_::YouTube(id);
                        Found::Yes(Box::new(src))
                    }
                    Some(_) => server(self, out),
                    None => {
                        // (straight from YouTube only if it answers quickly: else through the server)
                        if let Some(p) = self.p.as_mut().filter(|p| !p.yt_timer) {
                            p.yt_timer = true;
                            out.push(self.timer(Ask::YouTubeWait { run }, youtube::DIRECT_WAIT_MS));
                        }
                        Found::Wait
                    }
                }
            }
            Step::Page { hls } => {
                let named = ep.as_ref().and_then(|e| e.stream.clone()).or_else(|| title.stream.clone()).unwrap_or_default();
                let mut s = Source::new(From::Stream, api::url(&cx.base, &named), hls);
                if hls {
                    s.captions = Words_::Ary(named);
                }
                Found::Yes(Box::new(s))
            }
            Step::Debrid => {
                match self.debrid_source(&title, ep.as_ref(), cx, out) {
                    None => Found::Wait,
                    Some(None) => Found::No,
                    Some(Some(s)) => Found::Yes(Box::new(s)),
                }
            }
        }
    }

    /// Plays `source` from `at` (seconds in the film); `fresh`: another source (not the same one again from elsewhere),
    /// so its levels, captions and previews are asked for.
    fn begin(&mut self, source: Source, at: f64, fresh: bool, cx: &Context) -> Vec<Effect> {
        let mut out = Vec::new();
        let speed = self.speed;
        let most_width = self.widest();
        let Some(p) = self.p.as_mut() else { return out };
        let Some(title) = p.title.clone() else { return out };
        if fresh {
            p.src += 1;
            p.words_asked = false;
        }
        p.stage = Stage::Playing;
        p.status = STARTING.into();
        p.offset = 0.;
        p.begun_at = at;
        p.engine.at = 0.;
        p.loaded = false;
        p.moving = true;
        p.last_save = cx.clock.now;
        p.ready_ms.get_or_insert(now_ms(cx) - p.opened * 1000.);
        if fresh {
            p.levels.clear();
        }
        p.switched_at = cx.clock.now;
        p.adapting = false;
        let level = self.level_now();
        let first = self.first_level();
        let Some(p) = self.p.as_mut() else { return out };
        let media_source = source.clone();
        p.source = Some(source);
        let media = self.media(&media_source, &title, &cx.base);
        let paused = self.p.as_ref().is_some_and(|p| p.paused);
        // (taken from what was read ahead: the engine has it now)
        self.preloads.retain(|(key, _)| *key != media.key);
        out.push(order(Command::Load { media, start: at, paused, speed, level, widest: most_width, first }));
        if fresh {
            out.extend(self.after_begin(cx));
        }
        out
    }

    /// What starts with a new source: its levels, its captions, what can be skipped, the previews.
    fn after_begin(&mut self, cx: &Context) -> Vec<Effect> {
        let mut out = Vec::new();
        let Some(p) = self.p.as_ref() else { return out };
        let (Some(source), Some(title)) = (p.source.clone(), p.title.clone()) else { return out };
        let (run, src) = (p.run, p.src);
        if source.hls && source.from != From::Local && p.levels.is_empty() {
            let mut request = sources::direct(&source.url, youtube::AGENT);
            if let Some(r) = &source.referer {
                request.headers.push(Header::new("Referer", r));
            }
            request.cookies = cookies_allowed(&cx.base, &source.url);
            out.push(self.send(Ask::Master { src }, request));
        }
        out.extend(self.captions(source.captions.clone(), CaptionAsk::File, cx));
        let desktop = cx.platform.device == Device::Desktop;
        let wants_preview = desktop && self.p.as_ref().is_some_and(|p| p.preview.is_none()) && source.from != From::Local;
        if wants_preview {
            let ep = self.p.as_ref().and_then(|p| p.ep.clone());
            if let Some(sprite) = ep.as_ref().and_then(|e| e.sprite.clone()) {
                out.push(self.get(cx, Ask::Preview { run, board: false }, &preview::sprites_path(&sprite)));
            } else if title.source == "youtube" {
                if let Some(ep) = ep {
                    let id = sources::bare(&ep.id, "yt-").to_string();
                    if let Some(Ok(info)) = self.lookup(&id, cx, &mut out) {
                        if let (Some(board), Some(p)) = (youtube::board(&info), self.p.as_mut()) {
                            p.preview = Some(Preview::Board { board });
                        }
                    }
                }
            }
        }
        out
    }

    /// The level to play now: a fixed one as the profile's quality says, or on an engine that can't adapt, Auto's
    /// under where stalls have brought it.
    fn level_now(&self) -> Option<Rung> {
        let p = self.p.as_ref()?;
        let heights: Vec<i64> = p.levels.iter().map(|r| r.height).collect();
        let most = if p.height > 0 {
            p.height
        } else if !p.adaptive {
            p.auto_cap
        } else {
            return None;
        };
        let top = quality::ceiling(&heights, self.top());
        quality::pick_level(&heights, most.min(top)).map(|i| p.levels[i])
    }

    /// Auto's next move ([`abr`]): an engine that adapts by itself is let go of its starting level once it has
    /// [`abr::UP_AFTER`] seconds buffered (or stalls); one that can't is moved here and reopened at the new level.
    fn auto(&mut self, status: Status, now: f64, cx: &Context) -> Vec<Effect> {
        let top = self.top();
        let Some(p) = self.p.as_mut() else { return vec![] };
        if p.height != 0 || p.levels.is_empty() || !p.started {
            return vec![];
        }
        let ahead = (status.buffered - status.at).max(0.);
        if p.adaptive {
            if p.adapting || (ahead < abr::UP_AFTER && !status.buffering) {
                return vec![];
            }
            p.adapting = true;
            return vec![order(Command::Adapt)];
        }
        if let Some(at) = p.measured_at.replace(now) {
            p.estimate.sample(status.throughput, now - at);
        }
        let (Some(bits), Some(level_now)) = (p.estimate.bits(), self.level_now()) else { return vec![] };
        let Some(p) = self.p.as_mut() else { return vec![] };
        if now - p.switched_at < SWITCH_REST || status.paused {
            return vec![];
        }
        let heights: Vec<i64> = p.levels.iter().map(|r| r.height).collect();
        let ceiling = quality::ceiling(&heights, top);
        let Some(to) = abr::ideal(&p.levels, bits, ceiling).and_then(|ideal| abr::switch(level_now, ideal, ahead)) else {
            return vec![];
        };
        p.auto_cap = to.height;
        p.recent_stalls = 0;
        self.reopen(cx)
    }

    /// For an engine that adapts by itself, on Auto: the level it holds until it's let go ([`abr::START`]).
    fn first_level(&self) -> Option<Rung> {
        let p = self.p.as_ref().filter(|p| p.adaptive && p.height == 0)?;
        let heights: Vec<i64> = p.levels.iter().map(|r| r.height).collect();
        let top = quality::ceiling(&heights, self.top());
        quality::pick_level(&heights, abr::START.min(top)).map(|i| p.levels[i])
    }

    fn top(&self) -> i64 {
        let source = self.p.as_ref().and_then(|p| p.title.as_ref()).map_or("", |t| t.source.as_str());
        quality::most(source, self.settings.plus)
    }

    fn widest(&self) -> Option<i64> {
        quality::widest(self.top())
    }

    // ---------- captions ----------

    fn captions(&mut self, words: Words_, kind: CaptionAsk, cx: &Context) -> Vec<Effect> {
        let mut out = Vec::new();
        let Some(p) = self.p.as_mut() else { return out };
        let src = p.src;
        match words {
            Words_::None => {}
            Words_::Text(text) => p.cues = captions::parse(&text),
            Words_::Url(url) => out.push(self.get(cx, Ask::Captions { src, kind: CaptionAsk::File }, &url)),
            Words_::Ary(stream) => {
                out.push(self.get(cx, Ask::Captions { src, kind: CaptionAsk::Server }, &sources::ary_captions_path(&stream)));
            }
            Words_::YouTube(id) => {
                if kind == CaptionAsk::Server {
                    let path = sources::youtube_captions_path(&id);
                    out.push(self.get(cx, Ask::Captions { src, kind: CaptionAsk::Server }, &path));
                    return out;
                }
                match self.lookup(&id, cx, &mut out) {
                    None => {}
                    Some(Ok(info)) => {
                        // (no English track: none, never the automatic ones)
                        if let Some(url) = youtube::captions(&info).filter(|_| self.p.as_ref().is_some_and(|p| !p.words_asked)) {
                            if let Some(p) = self.p.as_mut() {
                                p.words_asked = true;
                            }
                            out.push(self.send(Ask::Captions { src, kind: CaptionAsk::YouTube }, youtube::captions_request(&url)));
                        }
                    }
                    Some(Err(_)) if self.p.as_ref().is_some_and(|p| !p.words_asked) => {
                        if let Some(p) = self.p.as_mut() {
                            p.words_asked = true;
                        }
                        let path = sources::youtube_captions_path(&id);
                        out.push(self.get(cx, Ask::Captions { src, kind: CaptionAsk::Server }, &path));
                    }
                    Some(Err(_)) => {}
                }
            }
            Words_::Cinema(ids) => {
                let quick = kind != CaptionAsk::Full;
                let body = sources::subtitles_body(&ids, quick);
                let kind = if quick { CaptionAsk::Quick } else { CaptionAsk::Full };
                out.push(self.post(cx, Ask::Captions { src, kind }, sources::SUBTITLES_PATH, body));
            }
        }
        out
    }

    /// Captions arrived for source `src`.
    fn caption_answer(&mut self, src: u32, kind: CaptionAsk, reply: Reply, cx: &Context) -> Vec<Effect> {
        let mut out = Vec::new();
        let Some(p) = self.p.as_mut() else { return out };
        if p.src != src {
            return out;
        }
        let words = p.source.as_ref().map(|s| s.captions.clone()).unwrap_or(Words_::None);
        let text = body(reply, cx);
        match kind {
            CaptionAsk::File => {
                if let Ok(text) = text {
                    p.cues = captions::parse(&text);
                }
            }
            CaptionAsk::Server => {
                if let Some(vtt) = text.ok().as_deref().and_then(sources::first_vtt) {
                    p.cues = captions::parse(&vtt);
                }
            }
            CaptionAsk::YouTube => match text {
                Ok(vtt) => p.cues = captions::parse(&vtt),
                Err(_) => out.extend(self.captions(words, CaptionAsk::Server, cx)),
            },
            CaptionAsk::Quick | CaptionAsk::Full => {
                let Ok(text) = text else { return out };
                let found = sources::subtitles(&text);
                if let Some(vtt) = &found.vtt {
                    p.cues = captions::parse(vtt);
                }
                if kind == CaptionAsk::Quick && found.syncing {
                    out.extend(self.captions(words, CaptionAsk::Full, cx));
                }
            }
        }
        out
    }

    // ---------- playing ----------

    fn status(&mut self, mut status: Status, cx: &Context) -> Vec<Effect> {
        let mut out = Vec::new();
        let now = cx.clock.now;
        let autoplay = self.settings.autoplay;
        let Some(p) = self.p.as_mut() else { return out };
        if p.stage != Stage::Playing {
            return out;
        }
        // (a seek the engine hasn't finished: it still says where it was, and the bar and the words jumped back
        // there before landing; the spot asked for holds until the engine is near it, or SEEK_HOLD has passed)
        if let Some((to, since)) = p.seeking {
            if (status.at - to).abs() > SEEK_NEAR && now - since < SEEK_HOLD {
                status.at = to;
            } else {
                p.seeking = None;
            }
        }
        let was = p.engine;
        p.loaded = true;
        p.engine = status;
        if !status.buffering {
            if !p.started || p.first_ms.is_none() {
                p.started = true;
                p.status.clear();
                if p.first_ms.is_none() {
                    p.first_ms = Some(now * 1000. - p.opened * 1000.);
                    out.push(self.timer(Ask::Minute { opened: self.opens }, TIMING_AFTER_MS));
                }
            }
            let Some(p) = self.p.as_mut() else { return out };
            p.moving = false;
            if let Some(at) = p.stuck_at.take() {
                p.stalled_ms += (now - at) * 1000.;
            }
        } else if !was.buffering && p.started && !p.moving && p.stuck_at.is_none() {
            p.stalls += 1;
            p.recent_stalls += 1;
            p.stuck_at = Some(now);
        }
        let Some(p) = self.p.as_mut() else { return out };
        if !p.skips_asked && p.total() > 0. {
            p.skips_asked = true;
            out.extend(self.ask_skips(cx));
        }
        let Some(p) = self.p.as_mut() else { return out };
        let (time, total, credits, has_next) = (p.time(), p.total(), p.credits(), p.next().is_some());
        p.ending.track(time, total, credits);
        if p.ending.now(time, total, credits, has_next, autoplay).is_some_and(|up| up.count >= 1.) {
            out.extend(self.go_next(cx));
            return out;
        }
        out.extend(self.auto(status, now, cx));
        let Some(p) = self.p.as_mut() else { return out };
        // (an engine that can't adapt steps down a level after two stalls on Auto)
        if !p.adaptive && p.height == 0 && p.recent_stalls >= STALLS_DOWN && status.buffering {
            if let Some(now_level) = self.level_now() {
                let Some(p) = self.p.as_mut() else { return out };
                if let Some(lower) = p.levels.iter().rev().find(|r| r.height < now_level.height) {
                    p.auto_cap = lower.height;
                    p.recent_stalls = 0;
                    out.extend(self.reopen(cx));
                }
            }
        }
        let Some(p) = self.p.as_mut() else { return out };
        if !p.warmed_next && total > WARM_NEXT_LONGER && total - time < WARM_NEXT {
            p.warmed_next = true;
            if let (Some(next), Some(title)) = (p.next().cloned(), p.title.clone()) {
                let settings = self.settings.clone();
                out.extend(self.warm(title.id, Some(next.id), settings));
            }
        }
        let Some(p) = self.p.as_mut() else { return out };
        if !p.paused && now - p.last_save >= SAVE_EVERY {
            out.extend(self.save(false, cx));
        }
        out
    }

    fn reopen(&mut self, cx: &Context) -> Vec<Effect> {
        let Some(p) = self.p.as_mut() else { return vec![] };
        let Some(source) = p.source.clone() else { return vec![] };
        let at = p.time();
        p.moving = true;
        // (the same source: its captions and levels stay)
        self.begin(source, at, false, cx)
    }

    fn ended(&mut self, cx: &Context) -> Vec<Effect> {
        let mut out = Vec::new();
        let autoplay = self.settings.autoplay;
        let Some(p) = self.p.as_mut() else { return out };
        if p.stage != Stage::Playing {
            return out;
        }
        p.engine.at = (p.total() - p.offset).max(0.);
        out.extend(self.save(false, cx));
        let Some(p) = self.p.as_mut() else { return out };
        match p.next().cloned() {
            None => out.push(order(Command::Close)),
            Some(next) if autoplay => out.extend(self.go(next.id, cx)),
            // (with autoplay off, the player stays on the last picture with Next episode up, as Netflix's)
            Some(_) => {
                p.paused = true;
                p.engine.paused = true;
            }
        }
        out
    }

    /// The engine failed: the one recovery rule (see the module's docs).
    fn failed(&mut self, failure: Failure, message: &str, cx: &Context) -> Vec<Effect> {
        let mut out = Vec::new();
        let Some(p) = self.p.as_mut() else { return out };
        if p.stage != Stage::Playing {
            return out;
        }
        let Some(mut source) = p.source.clone() else { return out };
        let at = p.time();
        // (the source turned it away: through the server instead, from the same spot)
        if let Some(relay) = source.fallback.take() {
            if let Some(id) = source.youtube.take() {
                self.lookups.remove(&id);
            }
            source.url = relay;
            source.referer = None;
            let Some(p) = self.p.as_mut() else { return out };
            p.levels.clear();
            out.extend(self.begin(source, at, true, cx));
            return out;
        }
        // (a Real-Debrid copy that failed: a fresh link, twice at most, from the same spot)
        if source.from == From::Debrid && p.debrid_fails < 2 {
            p.debrid_fails += 1;
            if let Some(copy) = source.server.as_ref() {
                self.links.remove(copy);
            }
            out.push(Effect::Report { place: "player.debrid".into(), message: format!("{failure:?}: {message}") });
            out.extend(self.load(Some(at), cx));
            return out;
        }
        let Some(p) = self.p.as_mut() else { return out };
        p.stage = Stage::Fault;
        p.fault = Some(FAILED.into());
        let from = format!("{:?}", source.from);
        out.push(Effect::Report { place: "player.play".into(), message: format!("{from} {failure:?}: {message}") });
        out
    }

    // ---------- the controls ----------

    fn toggle(&mut self, paused: bool) -> Vec<Effect> {
        let Some(p) = self.p.as_mut() else { return vec![] };
        p.paused = paused;
        p.engine.paused = paused;
        if paused && p.ending.now(p.time(), p.total(), p.credits(), true, true).is_some() {
            p.ending.hold();
        }
        vec![order(Command::Pause { paused })]
    }

    fn seek(&mut self, target: f64, cx: &Context) -> Vec<Effect> {
        let Some(p) = self.p.as_mut() else { return vec![] };
        if p.stage != Stage::Playing {
            return vec![];
        }
        let end = p.total();
        let mut to = target.max(0.);
        if end > 0. {
            to = to.min((end - SEEK_END).max(0.));
        }
        if to < p.time() && p.ending.now(p.time(), p.total(), p.credits(), true, true).is_some() {
            p.ending.hold();
        }
        p.moving = true;
        p.engine.at = to - p.offset;
        p.seeking = Some((to - p.offset, cx.clock.now));
        vec![order(Command::Seek { to: to - p.offset })]
    }

    fn skip(&mut self, cx: &Context) -> Vec<Effect> {
        let Some(p) = self.p.as_mut() else { return vec![] };
        let Some(segment) = p.skip_now().cloned() else { return vec![] };
        let Some(end) = segment.end else { return vec![] };
        p.skipped = Some(segment);
        self.seek(end, cx)
    }

    fn go_next(&mut self, cx: &Context) -> Vec<Effect> {
        let Some(next) = self.p.as_ref().and_then(|p| p.next().cloned()) else { return vec![] };
        self.go(next.id, cx)
    }

    /// Another episode, in this player.
    fn go(&mut self, id: String, cx: &Context) -> Vec<Effect> {
        let mut out = self.save(false, cx);
        let Some(p) = self.p.as_mut() else { return out };
        let Some(ep) = p.title.as_ref().and_then(|t| t.episodes.iter().find(|e| e.id == id).cloned()) else { return out };
        p.ep = Some(ep);
        p.wanted = Some(id);
        p.warmed_next = false;
        p.panel = None;
        p.offset = 0.;
        out.extend(self.load(None, cx));
        out
    }

    fn retry(&mut self, cx: &Context) -> Vec<Effect> {
        let Some(p) = self.p.as_mut() else { return vec![] };
        if p.title.is_none() {
            let id = p.title_id.clone();
            p.stage = Stage::Title;
            p.fault = None;
            return vec![self.title_page(Ask::Title { run: 0 }, &id)];
        }
        let at = (p.started).then(|| p.time());
        self.load(at, cx)
    }

    fn setting(&mut self, group: &str, key: &str, value: Value) -> Effect {
        match (group, key) {
            ("playback", "quality") => self.settings.quality = value.as_str().unwrap_or("auto").into(),
            ("subtitles", "on") => self.settings.captions = value.as_bool().unwrap_or(true),
            ("subtitles", "size") => self.settings.caption_size = value.as_str().unwrap_or("medium").into(),
            ("languages", "anime") => self.settings.anime_audio = value.as_str().unwrap_or("sub").into(),
            _ => {}
        }
        order(Command::Setting { group: group.into(), key: key.into(), value: value.to_string() })
    }

    fn pick(&mut self, panel: Panel, group: usize, choice: usize, cx: &Context) -> Vec<Effect> {
        let menus = match panel {
            Panel::Subtitles => self.subtitle_menus(),
            _ => self.menus(),
        };
        let Some(action) = menus.get(group).and_then(|(_, picks)| picks.get(choice)).map(|(_, a)| a.clone()) else {
            return vec![];
        };
        self.act(action, cx)
    }

    fn act(&mut self, action: Action, cx: &Context) -> Vec<Effect> {
        let mut out = Vec::new();
        match action {
            Action::None => {}
            Action::Level(h) => {
                let value = if h == 0 { "auto".to_string() } else { format!("{h}p") };
                out.push(self.setting("playback", "quality", json!(value)));
                let Some(p) = self.p.as_mut() else { return out };
                // (a Real-Debrid copy is one file: another copy, from the same spot)
                if p.source.as_ref().is_some_and(|s| s.from == From::Debrid && !s.hls) {
                    let at = p.time();
                    p.height = h;
                    out.extend(self.load(Some(at), cx));
                    return out;
                }
                p.height = h;
                p.auto_cap = abr::START;
                p.recent_stalls = 0;
                if p.adaptive {
                    let level = self.level_now();
                    out.push(order(Command::Level { level, widest: self.widest() }));
                } else {
                    out.extend(self.reopen(cx));
                }
            }
            Action::Anime(a) => {
                out.push(self.setting("languages", "anime", json!(a)));
                let at = self.p.as_ref().map(Player::time);
                out.extend(self.load(at, cx));
            }
            Action::Track(id) => {
                if let Some(p) = self.p.as_mut() {
                    for s in &mut p.sounds {
                        s.selected = s.id == id;
                    }
                }
                out.push(order(Command::Track { id }));
            }
            Action::Fit(fill) => {
                if let Some(p) = self.p.as_mut() {
                    p.fill = fill;
                }
                out.push(order(Command::Fit { fill }));
            }
            Action::Captions(on) => out.push(self.setting("subtitles", "on", json!(on))),
            Action::Size(size) => out.push(self.setting("subtitles", "size", json!(size))),
        }
        out
    }

    /// Quality, sound and picture: each group's choices and what picking one does.
    fn menus(&self) -> Vec<(String, Vec<(Choice, Action)>)> {
        let mut out = Vec::new();
        let Some(p) = self.p.as_ref() else { return out };
        let Some(source) = p.source.as_ref() else { return out };
        let choice = |label: String, on: bool, badge: Option<&str>| Choice { label, on, badge: badge.map(String::from) };
        let heights: Vec<i64> = p.levels.iter().map(|r| r.height).collect();
        let cap = quality::ceiling(&heights, self.top());
        let levels: Vec<i64> = heights.iter().copied().filter(|h| *h <= cap).collect();
        let mut picks = Vec::new();
        // (a Real-Debrid copy: the film's other copies, by height; no Auto)
        let file = source.from == From::Debrid && !source.hls;
        if file {
            let copies = match p.title.as_ref().and_then(|t| self.copies.get(&play_key(t, p.ep.as_ref()))) {
                Some((_, Known::Got(copies))) => copies.clone(),
                _ => Vec::new(),
            };
            let now = copies.iter().find(|c| source.server.as_deref() == Some(c.id.as_str())).map(|c| quality::height(&c.quality));
            let mut tall: Vec<i64> = copies.iter().map(|c| quality::height(&c.quality)).collect();
            tall.dedup();
            for h in tall {
                let on = Some(h) == now;
                picks.push((choice(quality::level_name(h), on, None), if on { Action::None } else { Action::Level(h) }));
            }
        }
        match if file { 0 } else { levels.len() } {
            0 => {}
            // (one level: just its name; playing, nothing to switch to, so no Auto)
            1 => picks.push((choice(quality::level_name(levels[0]), true, None), Action::None)),
            _ => {
                let low: Vec<i64> = levels.iter().copied().filter(|h| *h >= LOWEST_OFFERED).collect();
                let shown = if low.is_empty() { levels.clone() } else { low };
                for h in std::iter::once(0).chain(shown.into_iter().rev()) {
                    let on = h == p.height.min(cap);
                    let label = if h == 0 { "Auto".into() } else { quality::level_name(h) };
                    picks.push((choice(label, on, None), if on { Action::None } else { Action::Level(h) }));
                }
            }
        }
        if !picks.is_empty() {
            out.push(("Quality".into(), picks));
        }
    
        if source.audios.len() > 1 {
            let picks = ["sub", "dub"]
                .into_iter()
                .filter(|a| source.audios.iter().any(|x| x == a))
                .map(|a| {
                    let on = source.audio.as_deref() == Some(a);
                    let label = if a == "sub" { "Japanese" } else { "English" };
                    (choice(label.into(), on, None), if on { Action::None } else { Action::Anime(a.into()) })
                })
                .collect();
            out.push(("Audio".into(), picks));
        } else if p.sounds.len() > 1 {
            let picks = p
                .sounds
                .iter()
                .map(|s| (choice(s.label.clone(), s.selected, None), if s.selected { Action::None } else { Action::Track(s.id.clone()) }))
                .collect();
            out.push(("Audio".into(), picks));
        }
        // (Fit and Fill only differ when the picture's shape isn't the screen's: a wide film, an old 4:3 show)
        if p.aspect > 0. && p.screen > 0. && (p.aspect - p.screen).abs() / p.screen > FIT_DIFFERS {
            let picks = [(false, "Fit"), (true, "Fill")]
                .into_iter()
                .map(|(fill, label)| (choice(label.into(), p.fill == fill, None), Action::Fit(fill)))
                .collect();
            out.push(("Picture".into(), picks));
        }
        out
    }

    /// Subtitles on or off, and (while on) their size; nothing when there are none.
    fn subtitle_menus(&self) -> Vec<(String, Vec<(Choice, Action)>)> {
        let mut out = Vec::new();
        let Some(p) = self.p.as_ref() else { return out };
        if p.cues.is_empty() {
            return out;
        }
        let on = self.settings.captions;
        let choice = |label: &str, on: bool| Choice { label: label.into(), on, badge: None };
        out.push(("Subtitles".into(), vec![(choice("Off", !on), Action::Captions(false)), (choice("English", on), Action::Captions(true))]));
        if on {
            let picks = captions::SIZES
                .iter()
                .map(|(value, label)| (choice(label, self.settings.caption_size == *value), Action::Size((*value).into())))
                .collect();
            out.push(("Subtitle size".into(), picks));
        }
        out
    }

    // ---------- skips ----------

    fn ask_skips(&mut self, cx: &Context) -> Vec<Effect> {
        let mut out = Vec::new();
        let Some(p) = self.p.as_ref() else { return out };
        let (Some(title), ep, run) = (p.title.clone(), p.ep.clone(), p.run) else { return out };
        let has_next = p.next().is_some();
        let total = p.total();
        match player::skips_from(&title.source, ep.is_some()) {
            Some(SkipsFrom::IntroDb) => {
                let season = ep.as_ref().and_then(|e| e.season.zip(e.n));
                let url = title.imdb.as_deref().and_then(|i| crate::intro::url(i, season.map(|s| s.0), season.map(|s| s.1), total));
                if let Some(url) = url {
                    let mut request = sources::direct(&url, youtube::AGENT);
                    request.timeout_ms = INTRO_MS;
                    out.push(self.send(Ask::Intro { run }, request));
                }
            }
            Some(SkipsFrom::Stream) => {
                let found = p.source.as_ref().map(|s| s.skip.clone()).unwrap_or_default();
                if let Some(p) = self.p.as_mut() {
                    p.segments = player::fit_skip(found, total, has_next);
                }
            }
            Some(SkipsFrom::Server) => {
                if let Some(ep) = ep {
                    let path = sources::youtube_skip_path(sources::bare(&ep.id, "yt-"));
                    out.push(self.get(cx, Ask::Skips { run, left: player::SKIP_ASKS - 1 }, &path));
                }
            }
            None => {}
        }
        out
    }

    fn skips_found(&mut self, found: Vec<Segment>) {
        let Some(p) = self.p.as_mut() else { return };
        let total = p.total();
        let has_next = p.next().is_some();
        p.segments = player::fit_skip(found, total, has_next);
    }

    // ---------- saving and reporting ----------

    /// The spot, saved to the account (and kept here). See the module's docs.
    fn save(&mut self, ended: bool, cx: &Context) -> Vec<Effect> {
        let mut out = Vec::new();
        let Some(p) = self.p.as_mut() else { return out };
        let (time, total) = (p.time(), p.total());
        if !p.started || !total.is_finite() || total <= 0. || !time.is_finite() {
            if ended {
                out.extend(watched());
            }
            return out;
        }
        p.last_save = cx.clock.now;
        let Some(title) = p.title.clone() else { return out };
        let ep = p.ep.clone();
        let key = p.key();
        let local = p.on(From::Local);
        let has_copy = matches!(p.local, Some(Some(_)));
        self.spots.insert(key.clone(), (time, total));
        if has_copy {
            out.push(order(Command::Spot { key: key.clone(), at: time }));
        }
        let cover = ep.as_ref().and_then(|e| e.thumb.clone()).or_else(|| title.cover_big.clone()).or_else(|| title.cover.clone());
        let spot = Saved {
            key: key.clone(),
            show: title.id.clone(),
            title: title.title.clone(),
            ep: ep.as_ref().and_then(|e| e.n),
            cover: cover.clone(),
            at: time,
            length: total,
            updated: cx.clock.now,
        };
        out.push(order(Command::Saved { spot }));
        let mut body = json!({"key": key, "show": title.id, "title": title.title, "ep": ep.as_ref().and_then(|e| e.n),
                              "cover": cover, "t": time as i64, "d": total as i64});
        // (sent only when it's true: leaving the player, which frees the account's screen)
        if ended {
            body["ended"] = json!(true);
        }
        if !cx.online {
            if ended {
                out.extend(watched());
            }
            return out;
        }
        let save = (body.to_string(), ended, local, title.id);
        if self.saving {
            // (in order: the newest waits for the one under way; leaving stays leaving)
            let ended = ended || self.queued.as_ref().is_some_and(|q| q.1);
            self.queued = Some((save.0, ended, local, save.3));
            return out;
        }
        out.push(self.send_save(save, cx));
        out
    }

    fn send_save(&mut self, (json, ended, local, title): (String, bool, bool, String), cx: &Context) -> Effect {
        self.saving = true;
        self.post(cx, Ask::Save { ended, local, title }, progress::PATH, json)
    }

    fn saved(&mut self, ended: bool, local: bool, title: &str, reply: Reply, cx: &Context) -> Vec<Effect> {
        let mut out = Vec::new();
        self.saving = false;
        if let Err(failure) = body(reply, cx) {
            // (a download isn't one of the account's screens: nothing elsewhere can stop it)
            if failure.kind == (Kind::Answered { status: 429 }) && !ended && !local {
                if let Some(p) = self.p.as_mut().filter(|p| p.title_id == title || p.title.as_ref().is_some_and(|t| t.id == title)) {
                    p.paused = true;
                    p.notice = None;
                    p.fault = Some(if failure.words.is_empty() { SCREEN_IN_USE.into() } else { failure.words });
                    p.stage = Stage::Fault;
                    out.push(order(Command::Pause { paused: true }));
                }
            }
        }
        if ended {
            out.extend(watched());
        }
        if let Some(next) = self.queued.take() {
            out.push(self.send_save(next, cx));
        }
        out
    }

    /// How this Play went, as seen here: one small record a minute in, or when it's left (timings.py on the server).
    fn timing(&mut self, cx: &Context) -> Vec<Effect> {
        let Some(p) = self.p.as_mut() else { return vec![] };
        if p.reported || !cx.online {
            return vec![];
        }
        p.reported = true;
        let source = match p.source.as_ref().map(|s| s.from) {
            Some(From::Local) => "download",
            Some(From::Debrid) => "debrid",
            Some(From::Anime) => "anime",
            _ => "stream",
        };
        let now = cx.clock.now;
        let stalled = p.stalled_ms + p.stuck_at.map_or(0., |at| (now - at) * 1000.);
        let round = |n: Option<f64>| n.map(|n| n.round() as i64);
        let body = json!({"title": p.title.as_ref().map_or(p.title_id.clone(), |t| t.title.clone()),
            "ep": p.ep.as_ref().and_then(|e| e.n), "source": source, "ready": round(p.ready_ms), "first": round(p.first_ms),
            "stalls": p.stalls, "stalled": stalled.round() as i64, "gave_up": p.first_ms.is_none(),
            "waited": ((now - p.opened) * 1000.).round() as i64});
        vec![self.post(cx, Ask::Timing, "/api/timing", body.to_string())]
    }

    fn report(&mut self, note: &str, cx: &Context) -> Vec<Effect> {
        let Some(reason) = self.p.as_ref().and_then(|p| p.reason) else { return vec![] };
        self.send_report(REASONS[reason].0, note, cx)
    }

    /// "It couldn't play" reported as it stands: its words as the note.
    fn report_fault(&mut self, cx: &Context) -> Vec<Effect> {
        let Some(p) = self.p.as_mut() else { return vec![] };
        let Some(fault) = p.fault.clone().filter(|_| p.stage == Stage::Fault && !p.fault_told) else { return vec![] };
        let out = self.send_report(CANT_PLAY, &fault, cx);
        if let Some(p) = self.p.as_mut().filter(|_| !out.is_empty()) {
            p.fault_told = true;
        }
        out
    }

    fn send_report(&mut self, reason: &str, note: &str, cx: &Context) -> Vec<Effect> {
        let Some(p) = self.p.as_mut() else { return vec![] };
        let Some(title) = p.title.clone() else { return vec![] };
        if p.sending {
            return vec![];
        }
        p.sending = true;
        let ep = p.ep.clone();
        let mut body = sources::ids(&title, ep.as_ref());
        body["reason"] = json!(reason);
        body["note"] = json!(note.trim());
        let episode = ep
            .as_ref()
            .and_then(|e| e.n.map(|n| format!(" · {}Episode {n}", e.season_label.as_ref().map(|s| format!("{s} · ")).unwrap_or_default())))
            .unwrap_or_default();
        body["title"] = json!(format!("{}{episode} ({})", title.title, title.id));
        body["position"] = json!(p.time());
        vec![self.post(cx, Ask::Report, "/api/report", body.to_string())]
    }

    // ---------- warming up ----------

    fn warm(&mut self, title: String, episode: Option<String>, settings: Settings) -> Vec<Effect> {
        if self.p.is_none() {
            self.settings = settings;
        }
        if self.warms.iter().any(|w| w.title_id == title && w.wanted == episode && !w.done) {
            return vec![];
        }
        self.warms.retain(|w| !w.done);
        self.warms.push(Warm { title_id: title.clone(), wanted: episode, title: None, local: None, spots: None, done: false });
        vec![self.title_page(Ask::WarmTitle { title: title.clone() }, &title), self.progress_page(Ask::WarmProgress { title })]
    }

    /// Every warm-up, again: each takes what's come and asks for what's next.
    fn warm_drive(&mut self, cx: &Context) -> Vec<Effect> {
        let mut out = Vec::new();
        for i in 0..self.warms.len() {
            let w = &self.warms[i];
            if w.done {
                continue;
            }
            let (Some(title), Some(local), Some(spots)) = (w.title.clone(), w.local, w.spots.clone()) else { continue };
            // (a finished copy on the device plays from there: nothing to get ready)
            if local {
                self.warms[i].done = true;
                continue;
            }
            let ep = if title.episodes.is_empty() {
                None
            } else {
                title.episodes.iter().find(|e| Some(&e.id) == w.wanted.as_ref()).or(title.episodes.first()).cloned()
            };
            let key = ep.as_ref().map_or(title.id.clone(), |e| e.id.clone());
            let start = self
                .spots
                .get(&key)
                .copied()
                .or_else(|| spots.iter().find(|s| s.key == key).map(|s| (s.at, s.length)))
                .filter(|(at, length)| crate::title::unfinished(*at, *length))
                .map_or(0., |(at, _)| at);
            match title.source.as_str() {
                "youtube" => {
                    if let Some(ep) = ep {
                        let _ = self.lookup(sources::bare(&ep.id, "yt-"), cx, &mut out);
                    }
                    self.warms[i].done = true;
                }
                "cinema" => {
                    // (the Real-Debrid copy and its link got ready, and its start read ahead)
                    match self.debrid_source(&title, ep.as_ref(), cx, &mut out) {
                        None => continue,
                        Some(Some(source)) => {
                            let media = self.media(&source, &title, &cx.base);
                            self.preload(media, start, cx.clock.now, &mut out);
                        }
                        Some(None) => {}
                    }
                    self.warms[i].done = true;
                }
                _ => self.warms[i].done = true,
            }
        }
        self.warms.retain(|w| !w.done);
        out
    }

    /// Reads `media` ahead at `start`: the newest few, while fresh, the oldest let go.
    fn preload(&mut self, media: Media, start: f64, now: f64, out: &mut Vec<Effect>) {
        let (stale, kept): (Vec<_>, Vec<_>) = self.preloads.drain(..).partition(|(_, at)| now - at >= stream::STREAM_FRESH);
        self.preloads = kept;
        out.extend(stale.into_iter().map(|(key, _)| order(Command::Drop { key })));
        if self.preloads.iter().any(|(key, _)| *key == media.key) {
            return;
        }
        self.preloads.push((media.key.clone(), now));
        while self.preloads.len() > PRELOADS {
            let (key, _) = self.preloads.remove(0);
            out.push(order(Command::Drop { key }));
        }
        let playlists = quality::height(&self.settings.quality) > 0;
        out.push(order(Command::Preload { media, start, playlists }));
    }

    /// A shared lookup answered: whoever waits on it carries on.
    fn wake(&mut self, cx: &Context) -> Vec<Effect> {
        let mut out = self.warm_drive(cx);
        let finding = self.p.as_ref().is_some_and(|p| matches!(p.stage, Stage::Finding { .. }));
        if finding {
            out.extend(self.drive(cx));
        }
        if let Some(p) = self.p.as_ref() {
            if let Some(Words_::YouTube(_)) = p.source.as_ref().map(|s| &s.captions) {
                if p.cues.is_empty() {
                    let words = p.source.as_ref().map(|s| s.captions.clone()).unwrap_or(Words_::None);
                    out.extend(self.captions(words, CaptionAsk::File, cx));
                }
            }
        }
        out
    }

    /// The player has gone: its spot saved as left (freeing the account's screen), how it started reported, its
    /// own waits ended. The shared lookups and the last save carry on.
    fn leave(&mut self, cx: &Context) -> Vec<Effect> {
        let mut out = self.save(true, cx);
        out.extend(self.timing(cx));
        self.p = None;
        out
    }

    fn answer_to(&mut self, ask: Ask, reply: Reply, cx: &Context) -> Vec<Effect> {
        let run_now = self.p.as_ref().map(|p| p.run);
        let mut out = Vec::new();
        match ask {
            Ask::Title { run } => {
                if self.p.as_ref().map(|p| p.run) != Some(run) || self.p.as_ref().is_some_and(|p| p.title.is_some()) {
                    return out;
                }
                let found = page(reply).and_then(|pg| pg.body.map(Ok).or(pg.failure.map(Err)));
                match found {
                    Some(Ok(text)) => match serde_json::from_str::<Value>(&text) {
                        Ok(v) => {
                            let id = self.p.as_ref().map(|p| p.title_id.clone()).unwrap_or_default();
                            out.extend(self.titled(model::title(&id, &v), cx));
                        }
                        Err(_) => out.extend(self.fault(FAILED.into())),
                    },
                    // (with no connection, what downloads know of it: Next episode still works)
                    other => {
                        let words = match other {
                            Some(Err(f)) => f.words,
                            _ => api::OFFLINE.into(),
                        };
                        let Some(p) = self.p.as_mut() else { return out };
                        p.fault = Some(words);
                        p.stage = Stage::Local;
                        out.push(order(Command::FindLocal { title: p.title_id.clone(), episode: p.wanted.clone(), offline: true }));
                    }
                }
            }
            Ask::Pick { run } => {
                if run_now != Some(run) {
                    return out;
                }
                let spots = page(reply).and_then(|pg| pg.body).and_then(|b| progress::saved(&b).ok()).unwrap_or_default();
                let Some(title) = self.p.as_mut().and_then(|p| p.title.take()) else { return out };
                // (the latest unfinished episode, by the title page's own rule; none watched: the first)
                let keys: Vec<&str> = title.episodes.iter().map(|e| e.id.as_str()).chain([title.id.as_str()]).collect();
                let shown: Vec<crate::home::Spot> = spots.iter().map(Saved::spot).collect();
                let picked = crate::title::resume(&shown, &keys).map(|i| spots[i].key.clone());
                if let Some(p) = self.p.as_mut() {
                    p.wanted = picked.or_else(|| title.episodes.first().map(|e| e.id.clone()));
                }
                out.extend(self.titled(title, cx));
            }
            Ask::Progress { run } => {
                if run_now != Some(run) {
                    return out;
                }
                let spots = page(reply).and_then(|pg| pg.body).and_then(|b| progress::saved(&b).ok()).unwrap_or_default();
                if let Some(p) = self.p.as_mut() {
                    p.spots = Some(spots);
                }
                out.extend(self.find(cx));
            }
            Ask::WarmTitle { title } => {
                let body = page(reply).and_then(|pg| pg.body).and_then(|b| serde_json::from_str::<Value>(&b).ok());
                match body {
                    Some(v) => {
                        let parsed = model::title(&title, &v);
                        let mut asks = Vec::new();
                        for w in self.warms.iter_mut().filter(|w| w.title_id == title && w.title.is_none()) {
                            w.title = Some(parsed.clone());
                            let ep = if parsed.episodes.is_empty() {
                                None
                            } else {
                                parsed.episodes.iter().find(|e| Some(&e.id) == w.wanted.as_ref()).or(parsed.episodes.first()).map(|e| e.id.clone())
                            };
                            asks.push(Command::FindLocal { title: title.clone(), episode: ep, offline: false });
                        }
                        out.extend(asks.into_iter().map(order));
                    }
                    None => self.warms.retain(|w| w.title_id != title),
                }
            }
            Ask::WarmProgress { title } => {
                let spots = page(reply).and_then(|pg| pg.body).and_then(|b| progress::saved(&b).ok()).unwrap_or_default();
                for w in self.warms.iter_mut().filter(|w| w.title_id == title) {
                    w.spots = Some(spots.clone());
                }
                out.extend(self.warm_drive(cx));
            }
            Ask::Copies { title } => {
                let copies = body(reply, cx).ok().map(|t| sources::debrid_copies(&t)).unwrap_or_default();
                self.copies.insert(title, (cx.clock.now, Known::Got(copies)));
                out.extend(self.wake(cx));
            }
            Ask::Link { copy } => {
                let url = body(reply, cx).ok().as_deref().and_then(sources::link);
                self.links.insert(copy, (cx.clock.now, Known::Got(url)));
                out.extend(self.wake(cx));
            }
            Ask::YouTube { id, visitor: None } => match body(reply, cx).ok().as_deref().and_then(youtube::visitor) {
                Some(v) => {
                    let request = youtube::player_request(&id, &v, now_ms(cx));
                    out.push(self.send(Ask::YouTube { id, visitor: Some(v) }, request));
                }
                None => {
                    self.lookups.insert(id, (cx.clock.now, Known::Got(Err("no visitor id".into()))));
                    out.extend(self.wake(cx));
                }
            },
            Ask::YouTube { id, visitor: Some(_) } => {
                let found = body(reply, cx).map_err(|f| f.words).and_then(|text| youtube::info(&text));
                self.lookups.insert(id, (cx.clock.now, Known::Got(found)));
                out.extend(self.wake(cx));
            }
            Ask::YouTubeWait { run } => {
                if run_now == Some(run) {
                    if let Some(p) = self.p.as_mut() {
                        p.yt_waited = true;
                    }
                    out.extend(self.drive(cx));
                }
            }
            Ask::YouTubeServer { run } => {
                if run_now != Some(run) {
                    return out;
                }
                let found = body(reply, cx).map_err(|f| f.words).and_then(|t| sources::stream(&t).ok_or_else(|| NO_STREAM.to_string()));
                if let Some(p) = self.p.as_mut() {
                    p.yt_server = Some(found);
                }
                out.extend(self.drive(cx));
            }
            Ask::Anime { run } => {
                if run_now != Some(run) {
                    return out;
                }
                let found = body(reply, cx).map_err(|f| f.words).and_then(|t| sources::anime(&t, &cx.base).ok_or_else(|| NO_STREAM.into()));
                if let Some(p) = self.p.as_mut() {
                    p.anime = Some(found);
                }
                out.extend(self.drive(cx));
            }
            Ask::Master { src } => {
                let plus = self.settings.plus;
                let Some(p) = self.p.as_mut().filter(|p| p.src == src) else { return out };
                let base = p.source.as_ref().map(|s| s.url.clone()).unwrap_or_default();
                if let Ok(text) = body(reply, cx) {
                    let levels = hls::levels(&text, &base);
                    let rungs = hls::ladder(&levels);
                    let cinema = p.title.as_ref().is_some_and(|t| t.source == "cinema");
                    p.levels = if cinema { for_account(rungs, plus) } else { rungs };
                    if !p.adaptive {
                        if let Some(level) = self.level_now() {
                            out.push(order(Command::Level { level: Some(level), widest: self.widest() }));
                        }
                    }
                }
            }
            Ask::Captions { src, kind } => out.extend(self.caption_answer(src, kind, reply, cx)),
            Ask::Intro { run } => {
                if run_now != Some(run) {
                    return out;
                }
                let total = self.p.as_ref().map_or(0., |p| p.total());
                let found = body(reply, cx).map(|t| crate::intro::segments(&t, total)).unwrap_or_default();
                self.skips_found(found);
            }
            Ask::Skips { run, left } => {
                if run_now != Some(run) {
                    return out;
                }
                let Ok(text) = body(reply, cx) else { return out };
                let (found, pending) = youtube::skips(&text);
                if !found.is_empty() {
                    self.skips_found(found);
                } else if pending && left > 0 {
                    out.push(self.timer(Ask::SkipsWait { run, left }, (player::SKIP_ASK_EVERY * 1000.) as u32));
                }
            }
            Ask::SkipsWait { run, left } => {
                if run_now != Some(run) {
                    return out;
                }
                if let Some(ep) = self.p.as_ref().and_then(|p| p.ep.clone()) {
                    let path = sources::youtube_skip_path(sources::bare(&ep.id, "yt-"));
                    out.push(self.get(cx, Ask::Skips { run, left: left - 1 }, &path));
                }
            }
            Ask::Preview { run, .. } => {
                if run_now != Some(run) {
                    return out;
                }
                if let Ok(text) = body(reply, cx) {
                    let frames = preview::frames(&text, &cx.base);
                    if let Some(p) = self.p.as_mut() {
                        p.preview = (!frames.is_empty()).then_some(Preview::Frames { frames });
                    }
                }
            }
            Ask::Save { ended, local, title } => out.extend(self.saved(ended, local, &title, reply, cx)),
            Ask::Report => {
                let Some(p) = self.p.as_mut() else { return out };
                p.sending = false;
                p.answer = Some(match body(reply, cx) {
                    Ok(_) => REPORTED.into(),
                    Err(f) if f.words.is_empty() => NOT_REPORTED.into(),
                    Err(f) => f.words,
                });
                out.push(self.timer(Ask::ReportShown, REPORT_SHOWN_MS));
            }
            Ask::Timing => {}
            Ask::Hide { opened } => {
                if opened != self.opens {
                    return out;
                }
                // (never while paused, a panel's open, or before the picture's up)
                let now = cx.clock.now;
                let Some(p) = self.p.as_mut() else { return out };
                let held = p.paused || p.panel.is_some() || !p.started || p.fault.is_some();
                let left = f64::from(p.hide_ms) / 1000. - (now - p.poked);
                if held || left > 0. {
                    let wait = if left > 0. { (left * 1000.).ceil() as u32 } else { p.hide_ms };
                    out.push(self.timer(Ask::Hide { opened }, wait.max(1)));
                } else {
                    p.controls = false;
                    p.hiding = false;
                }
            }
            Ask::ReportShown => {
                if let Some(p) = self.p.as_mut() {
                    if p.panel == Some(Panel::Report) && p.answer.is_some() {
                        p.panel = None;
                    }
                    p.answer = None;
                    p.reason = None;
                }
            }
            Ask::Minute { opened } => {
                if opened == self.opens {
                    out.extend(self.timing(cx));
                }
            }
        }
        out
    }

    fn fault(&mut self, words: String) -> Vec<Effect> {
        if let Some(p) = self.p.as_mut() {
            p.stage = Stage::Fault;
            p.fault = Some(words);
        }
        vec![]
    }

    /// What's on this device of a title, as the app said.
    fn local(&mut self, title: String, episode: Option<String>, copy: Option<LocalCopy>, offline: Option<String>, cx: &Context) -> Vec<Effect> {
        let mut out = Vec::new();
        let mut warmed = false;
        for w in self.warms.iter_mut().filter(|w| w.title_id == title && w.local.is_none()) {
            w.local = Some(copy.is_some());
            warmed = true;
        }
        if warmed {
            out.extend(self.warm_drive(cx));
        }
        let Some(p) = self.p.as_mut() else { return out };
        if p.title_id != title {
            return out;
        }
        // (asked offline: the title as downloads know it)
        if p.title.is_none() {
            match offline.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()) {
                Some(v) => {
                    p.fault = None;
                    let parsed = model::title(&title, &v);
                    out.extend(self.titled(parsed, cx));
                }
                None => p.stage = Stage::Fault,
            }
            return out;
        }
        if p.ep.as_ref().map(|e| &e.id) != episode.as_ref() {
            return out;
        }
        if p.stage == Stage::Local {
            p.local = Some(copy);
            out.extend(self.find(cx));
            return out;
        }
        out
    }
}

/// How long TheIntroDB may take.
const INTRO_MS: u32 = 8_000;

/// What picking a choice does.
#[derive(Clone, Debug, PartialEq)]
enum Action {
    None,
    /// A stream's level (0: Auto).
    Level(i64),
    Anime(String),
    Track(String),
    Fit(bool),
    Captions(bool),
    Size(String),
}

/// A stream's levels this account may play (`quality::ceiling`): nothing above 1080p without 2K and 4K, unless a copy
/// has nothing else (then its smallest).
fn for_account(rungs: Vec<Rung>, plus: bool) -> Vec<Rung> {
    let heights: Vec<i64> = rungs.iter().map(|r| r.height).collect();
    let ceiling = quality::ceiling(&heights, quality::most("cinema", plus));
    rungs.into_iter().filter(|r| r.height <= ceiling).collect()
}

/// Where a video's Referer goes, for the video at `url`: its host, and its playlist's folder when that's three levels
/// deep at least ("/anime/<id>/<id>"), so it can't match what isn't this stream's.
fn referer(url: &str, value: &str) -> Option<Referer> {
    let rest = url.split_once("://")?.1;
    let (host, path) = rest.find('/').map_or((rest, ""), |at| rest.split_at(at));
    let path = path.split(['?', '#']).next().unwrap_or("");
    let folder = path.rsplit_once('/').map(|(dir, _)| dir.to_string()).filter(|dir| dir.matches('/').count() >= 3);
    Some(Referer { value: value.into(), host: host.into(), folder })
}

/// The Referer a request to `url` goes with, by where a video's scope says it goes (its host or under its folder).
pub fn referer_for(referer: &Referer, url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let (host, path) = rest.find('/').map_or((rest, ""), |at| rest.split_at(at));
    let under = referer.folder.as_ref().is_some_and(|f| path.starts_with(&format!("{f}/")));
    (host == referer.host || under).then(|| referer.value.clone())
}

impl Screen for Play {
    type Event = Event;
    type View = View;

    fn update(&mut self, event: Event, cx: &Context) -> Vec<Effect> {
        let now = cx.clock.now;
        self.tv = cx.platform.device == Device::Tv;
        let touched = matches!(
            event,
            Event::Toggle
                | Event::Pause { .. }
                | Event::Seek { .. }
                | Event::Jump { .. }
                | Event::Panel { .. }
                | Event::Pick { .. }
                | Event::Speed { .. }
                | Event::Captions { .. }
                | Event::Reason { .. }
                | Event::Poke
        );
        let mut out = match event {
            Event::Open { title, episode, local, settings, adaptive, screen, hide_ms, fill } => {
                self.settings = settings;
                let mut p = Player::new(title, episode, local, adaptive, screen, hide_ms.max(1), now);
                p.fill = fill;
                self.open(p, cx)
            }
            Event::Warm { title, episode, settings } => self.warm(title, episode, settings),
            Event::Settings { settings } => {
                self.settings = settings;
                vec![]
            }
            Event::Local { title, episode, copy, offline } => self.local(title, episode, copy, offline, cx),
            Event::Status { status } => self.status(status, cx),
            Event::Sounds { sounds } => {
                if let Some(p) = self.p.as_mut() {
                    p.sounds = sounds;
                }
                vec![]
            }
            Event::Picture { aspect } => {
                if let Some(p) = self.p.as_mut() {
                    p.aspect = aspect;
                }
                vec![]
            }
            Event::Screen { aspect } => {
                if let Some(p) = self.p.as_mut() {
                    p.screen = aspect;
                }
                vec![]
            }
            Event::Ended => self.ended(cx),
            Event::Failed { failure, message } => self.failed(failure, &message, cx),
            Event::Toggle => {
                let paused = !self.p.as_ref().is_some_and(|p| p.paused);
                self.toggle(paused)
            }
            Event::Pause { paused } => self.toggle(paused),
            Event::Seek { to } => self.seek(to, cx),
            Event::Jump { seconds } => {
                let at = self.p.as_ref().map_or(0., Player::time);
                self.seek(at + seconds, cx)
            }
            Event::Pill => {
                let next = self.p.as_ref().is_some_and(|p| {
                    p.ending.now(p.time(), p.total(), p.credits(), p.next().is_some(), self.settings.autoplay).is_some()
                });
                if next {
                    self.go_next(cx)
                } else {
                    self.skip(cx)
                }
            }
            Event::WatchCredits => {
                if let Some(p) = self.p.as_mut() {
                    p.ending.watch_credits();
                }
                vec![]
            }
            Event::Next => self.go_next(cx),
            Event::Episode { id } => self.go(id, cx),
            Event::Retry => self.retry(cx),
            Event::Panel { panel } => {
                if let Some(p) = self.p.as_mut() {
                    p.panel = panel;
                    if panel == Some(Panel::Report) || panel.is_none() {
                        p.reason = None;
                        p.answer = None;
                    }
                }
                vec![]
            }
            Event::Pick { panel, group, choice } => self.pick(panel, group as usize, choice as usize, cx),
            Event::Speed { index } => {
                let Some(speed) = SPEEDS.get(index as usize).copied() else { return vec![] };
                self.speed = speed;
                vec![order(Command::Speed { speed })]
            }
            Event::Captions { on } => vec![self.setting("subtitles", "on", json!(on))],
            Event::Reason { index } => {
                if let Some(p) = self.p.as_mut() {
                    p.reason = Some((index as usize).min(REASONS.len() - 1));
                }
                vec![]
            }
            Event::Send { note } => self.report(&note, cx),
            Event::ReportFault => self.report_fault(cx),
            Event::Poke => vec![],
            Event::Hide => {
                if let Some(p) = self.p.as_mut() {
                    p.controls = false;
                }
                vec![]
            }
            Event::Dismiss => {
                if let Some(p) = self.p.as_mut() {
                    p.notice = None;
                }
                vec![]
            }
            Event::Close => {
                if self.p.is_none() {
                    return vec![];
                }
                self.leave(cx)
            }
        };
        if touched {
            out.extend(self.touch(now));
        }
        out
    }

    fn answer(&mut self, id: u32, reply: Reply, cx: &Context) -> Vec<Effect> {
        let Some(ask) = self.asks.take(id) else { return vec![] };
        self.answer_to(ask, reply, cx)
    }

    fn view(&self) -> View {
        let Some(p) = self.p.as_ref() else { return View::default() };
        let title = p.title.as_ref();
        let name = title.map(|t| t.title.clone()).unwrap_or_default();
        let place = p.ep.as_ref().map(|e| player::episode_label(e.season, e.n, e.season_label.as_deref())).unwrap_or_default();
        let heading = if place.is_empty() || name.is_empty() { name.clone() } else { format!("{name}  ·  {place}") };
        let (time, total) = (p.time(), p.total());
        let next = p.next();
        let up = p.ending.now(time, total, p.credits(), next.is_some(), self.settings.autoplay);
        let pill = match (up, p.skip_now()) {
            (Some(up), _) => {
                let n = next.and_then(|e| e.n).map(|n| format!(" · E{n}")).unwrap_or_default();
                Some(Pill { label: format!("Next episode{n}"), next: true, count: up.count, left: up.left })
            }
            (None, Some(s)) => {
                let label = match s.kind.as_str() {
                    "intro" => "Skip intro",
                    "recap" => "Skip recap",
                    _ => "Skip credits",
                };
                Some(Pill { label: label.into(), next: false, count: 0., left: 0. })
            }
            _ => None,
        };
        let captions_on = self.settings.captions;
        let caption = if captions_on && p.started { captions::at(&p.cues, time) } else { String::new() };
        let shown = |menus: Vec<(String, Vec<(Choice, Action)>)>| {
            menus.into_iter().map(|(title, picks)| Group { title, choices: picks.into_iter().map(|(c, _)| c).collect() }).collect()
        };
        let episodes = if p.panel == Some(Panel::Episodes) {
            let season = p.ep.as_ref().and_then(|e| e.season_id.clone());
            title
                .map(|t| {
                    t.episodes
                        .iter()
                        .filter(|e| e.season_id == season)
                        .map(|e| EpisodeRow {
                            id: e.id.clone(),
                            number: e.n.map(|n| n.to_string()).unwrap_or_default(),
                            name: e.name.clone().unwrap_or_else(|| format!("Episode {}", e.n.unwrap_or_default())),
                            playing: Some(&e.id) == p.ep.as_ref().map(|c| &c.id),
                            picture: e.thumb.clone(),
                            about: e.description.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let season = p.ep.as_ref().and_then(|e| e.season_label.clone().or_else(|| e.season.map(|n| format!("Season {n}"))));
        let remote = Remote {
            season: p.ep.as_ref().and_then(|e| e.season_label.clone()),
            number: p.ep.as_ref().and_then(|e| e.n),
            next_number: next.and_then(|e| e.n),
        };
        View {
            open: true,
            title_id: p.title_id.clone(),
            episode_id: p.ep.as_ref().map(|e| e.id.clone()),
            name,
            place,
            heading,
            status: if p.status.is_empty() && !p.started { FINDING.into() } else { p.status.clone() },
            fault: p.fault.clone().filter(|_| p.stage == Stage::Fault),
            fault_report: p.stage == Stage::Fault && p.title.is_some() && !p.fault_told,
            started: p.started,
            paused: p.paused,
            // (the spinner from a seek until the engine is there, as Netflix does: mpv itself only says it waits once
            // its buffer runs dry)
            buffering: p.engine.buffering || p.seeking.is_some(),
            time,
            total,
            buffered: p.offset + p.engine.buffered,
            left: player::clock((total - time).max(0.)),
            caption,
            captions_on,
            has_captions: !p.cues.is_empty(),
            caption_size: self.settings.caption_size.clone(),
            pill,
            has_next: next.is_some(),
            controls: p.controls,
            panel: p.panel,
            settings: shown(self.menus()),
            subtitles: shown(self.subtitle_menus()),
            speeds: SPEEDS
                .iter()
                .map(|s| Choice { label: speed_name(*s, false), on: (s - self.speed).abs() < 1e-6, badge: None })
                .collect(),
            speed: self.speed,
            speed_label: format!("Speed ({})", speed_name(self.speed, true)),
            report: ReportView {
                reasons: REASONS
                    .iter()
                    .enumerate()
                    .filter(|(_, (key, _))| !(self.tv && *key == "other"))
                    .map(|(i, (_, label))| Choice { label: (*label).into(), on: p.reason == Some(i), badge: None })
                    .collect(),
                details: !self.tv,
                ready: p.reason.is_some() && !p.sending,
                sending: p.sending,
                answer: p.answer.clone(),
            },
            notice: p.notice.clone(),
            episodes,
            season,
            preview: p.preview.clone(),
            fill: p.fill,
            awake: !p.paused && p.fault.is_none(),
            remote,
        }
    }

    fn app(&mut self, event: &crate::screen::AppEvent, cx: &Context) -> Vec<Effect> {
        // (a phone or a TV pauses behind another app; a desktop window minimised plays on, as a desktop player does)
        match event {
            crate::screen::AppEvent::Lifecycle { foreground: false }
                if cx.platform.device != Device::Desktop && self.p.as_ref().is_some_and(|p| !p.paused) =>
            {
                self.toggle(true)
            }
            _ => vec![],
        }
    }
}

impl Play {
    /// Something touched: the controls up, and their wait to hide counted from now.
    fn touch(&mut self, now: f64) -> Vec<Effect> {
        let Some(p) = self.p.as_mut() else { return vec![] };
        p.poked = now;
        p.controls = true;
        if p.hiding {
            return vec![];
        }
        p.hiding = true;
        let ms = p.hide_ms;
        vec![self.timer(Ask::Hide { opened: self.opens }, ms)]
    }
}

#[cfg(test)]
mod tests;
