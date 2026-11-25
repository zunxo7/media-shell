//! Your TV for Kotlin: the controlling end (`zcore::your_tv`), Sign in a TV and the TV's code (`zcore::tv_link`), and
//! the TV's end (`zcore::on_tv`), as screens on the root. The types are zcore's own, described to UniFFI here; Kotlin
//! names that would clash (the player's `Group` and `Choice`, Compose's `Key`) are prefixed.

use crate::screens::{screen, CoreRoot};
use std::sync::Arc;
use zcore::on_tv::{
    Button as TvButton, Event as OnTvEvent, Order as TvOrder, PlayerState as TvPlayerState, Volume as TvVolume,
};
use zcore::remote::{Choice as TvChoice, Group as TvGroup, Key as TvKey};
use zcore::root::{ScreenId, Turn};
use zcore::screen::{Clock as CoreClock, Screen};
use zcore::tv_link::{
    CodeEvent as TvCodeEvent, CodeView as TvCodeView, SignInEvent as SignInTvEvent, SignInView as SignInTvView,
    Stage as SignInTvStage, Step as SignInTvStep,
};
use zcore::your_tv::{Bar as YourTvBar, Event as YourTvEvent, Place as TvPlace, Playing as TvPlaying, View as YourTvView};

#[uniffi::remote(Record)]
pub struct TvChoice {
    pub label: String,
    pub on: bool,
}

#[uniffi::remote(Record)]
pub struct TvGroup {
    pub title: String,
    pub choices: Vec<TvChoice>,
}

#[uniffi::remote(Enum)]
pub enum TvKey {
    VolumeUp,
    VolumeDown,
}

// ---------- Your TV ----------

#[uniffi::remote(Enum)]
pub enum YourTvEvent {
    Opened,
    Switch,
    Play { title: String, episode: Option<String> },
    Toggle,
    Jump { forward: bool },
    Scrub { share: f32 },
    Next,
    Stop,
    Pick { group: String, choice: String },
    Key { key: TvKey },
    SignOutTv,
}

#[uniffi::remote(Record)]
pub struct TvPlaying {
    pub title: String,
    pub line: String,
    pub failed: bool,
    pub art: Option<String>,
    pub paused: bool,
    pub buffering: bool,
    pub next: Option<String>,
    pub time: f64,
    pub total: f64,
    pub at: u64,
    pub moving: bool,
}

#[uniffi::remote(Record)]
pub struct TvPlace {
    pub share: f32,
    pub at: String,
    pub left: String,
}

#[uniffi::remote(Record)]
pub struct YourTvBar {
    pub title: String,
    pub line: String,
    pub art: Option<String>,
    pub paused: Option<bool>,
}

#[uniffi::remote(Record)]
pub struct YourTvView {
    pub entry: String,
    pub has_tv: bool,
    pub plays_on_tv: bool,
    pub title: String,
    pub name: String,
    pub status: String,
    pub switch: String,
    pub on: bool,
    pub playing: Option<TvPlaying>,
    pub idle: String,
    pub waiting: bool,
    pub bar: Option<YourTvBar>,
    pub captions: Vec<TvGroup>,
    pub settings: Vec<TvGroup>,
    pub done: String,
    pub change: String,
    pub sign_out: String,
    pub sure: bool,
    pub sure_words: String,
    pub stop: String,
    pub note: Option<String>,
    pub closed: u32,
}

screen!(
    /// Your TV on a phone (`zcore::your_tv`).
    YourTvScreen(zcore::your_tv::YourTv),
    YourTvEvent,
    YourTvView
);

/// How far along what's on the TV is at `now` (seconds since 1970), or held at `scrub` while it's dragged.
#[uniffi::export]
pub fn tv_place(playing: TvPlaying, now: f64, scrub: Option<f32>) -> TvPlace {
    playing.place(now, scrub)
}

// ---------- Sign in a TV, and the TV's code ----------

#[uniffi::remote(Enum)]
pub enum SignInTvStage {
    Code,
    Steps,
    Done,
}

#[uniffi::remote(Enum)]
pub enum SignInTvEvent {
    Opened,
    Steps,
    Back,
    Typed,
    Submit { name: String, code: String },
}

#[uniffi::remote(Record)]
pub struct SignInTvStep {
    pub number: String,
    pub before: String,
    pub bold: String,
    pub after: String,
}

#[uniffi::remote(Record)]
pub struct SignInTvView {
    pub stage: SignInTvStage,
    pub title: String,
    pub line: String,
    pub name_hint: String,
    pub code_hint: String,
    pub button: String,
    pub busy: bool,
    pub error: Option<String>,
    pub no_app: String,
    pub how: String,
    pub steps: Vec<SignInTvStep>,
    pub back: String,
    pub done: String,
}

screen!(
    /// Sign in a TV, on a phone (`zcore::tv_link::SignInTv`).
    SignInTvScreen(zcore::tv_link::SignInTv),
    SignInTvEvent,
    SignInTvView
);

/// A code as it's typed: upper case, letters and numbers only, no longer than a code.
#[uniffi::export]
pub fn tv_code_clean(text: String) -> String {
    zcore::tv_link::clean_code(&text)
}

#[uniffi::remote(Enum)]
pub enum TvCodeEvent {
    Opened,
}

#[uniffi::remote(Record)]
pub struct TvCodeView {
    pub hint: String,
    pub qr: Option<String>,
    pub code: String,
    pub spinner: bool,
    pub signed_in: bool,
}

screen!(
    /// The TV's sign-in code (`zcore::tv_link::TvCode`).
    TvCodeScreen(zcore::tv_link::TvCode),
    TvCodeEvent,
    TvCodeView
);

// ---------- the TV's end ----------

#[uniffi::remote(Enum)]
pub enum TvVolume {
    Up,
    Down,
    Mute,
}

#[uniffi::remote(Enum)]
pub enum TvButton {
    Up,
    Down,
    Left,
    Right,
    Ok,
    Back,
    PlayPause,
    Rewind,
    Forward,
}

#[uniffi::remote(Enum)]
pub enum TvOrder {
    Stop,
    Home,
    Search { typed: Option<String> },
    Toggle,
    Seek { to: f64 },
    Next,
    Pick { group: String, choice: String },
    Volume { change: TvVolume },
    Press { button: TvButton },
    Sample,
}

#[uniffi::remote(Record)]
pub struct TvPlayerState {
    pub title_id: String,
    pub title: String,
    pub episode_id: Option<String>,
    pub season: Option<String>,
    pub number: Option<i64>,
    pub status: String,
    pub time: f64,
    pub total: f64,
    pub paused: bool,
    pub buffering: bool,
    pub started: bool,
    pub fault: String,
    pub has_next: bool,
    pub next_number: Option<i64>,
    pub groups: Vec<TvGroup>,
}

#[uniffi::remote(Enum)]
pub enum OnTvEvent {
    Opened,
    Playing { state: Option<TvPlayerState> },
}

/// The TV's end (`zcore::on_tv`): what the phones ask of it comes out as `Effect.Tv` and `Effect.Play`. It draws
/// nothing, so it has no view.
#[derive(uniffi::Object)]
pub struct OnTvScreen {
    root: Arc<CoreRoot>,
    id: ScreenId,
}

#[uniffi::export]
impl OnTvScreen {
    #[uniffi::constructor]
    pub fn new(root: Arc<CoreRoot>) -> Self {
        let id = root.lock().open(zcore::on_tv::OnTv::default());
        Self { root, id }
    }

    /// Its id on the root.
    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn update(&self, event: OnTvEvent, clock: CoreClock) -> Turn {
        self.root.lock().update::<zcore::on_tv::OnTv>(self.id, event, clock)
    }
}

impl Drop for OnTvScreen {
    fn drop(&mut self) {
        self.root.lock().close(self.id, CoreClock::default());
    }
}
