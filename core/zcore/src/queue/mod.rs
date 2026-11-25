//! The downloads queue: every download on the device (films, episodes and manhwa chapters), kept in `downloads.json`,
//! and the transfers that bring them down, as one model that lives as long as the app (not any screen), so it goes on
//! with the screen off. The rules of a single download are [`crate::downloads`]'s; this decides which run and when,
//! carries each one's transfer as steps ([`transfer`]), weighs what a title will take ([`weigh`]), and says what the
//! Downloads page, a title page's controls, the notification and the player show of it. The app only runs its effects
//! (the requests, the files) and keeps its job going while [`View::busy`] says so.

pub mod hls;
pub mod probe;
pub mod row;
pub mod transfer;
pub mod weigh;

use crate::downloads::{self as rules, Act, Kind, Speeds, State};
use crate::screen::{AppEvent, Asks, Context, Device, DiskOp, Effect, Empty, Glyph, Reply, Request, Screen};
use row::Row;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use transfer::{End, Job, Shared, Tag};

/// Something a transfer asks of the app.
#[derive(Clone, Debug, PartialEq)]
pub enum Io {
    Ask(Request),
    Fetch(Request, String),
    Disk(DiskOp),
    Wait(u32),
}

/// Why a round of a transfer stopped short.
#[derive(Clone, Debug, PartialEq)]
pub enum Fail {
    /// The connection (or the stream) dropped: tried again, in these words if it never comes.
    Dropped(String),
    /// It can't be downloaded: these words.
    Cant(String),
    /// Not enough room on the device.
    Full,
}

/// A video download's file, by its key and what it is ("video", "part", "vtt"...), in the downloads folder.
pub fn file(key: &str, ext: &str) -> String {
    format!("downloads/{}", rules::file_name(key, ext))
}

/// Every file a video download may have had along the way, either app's.
const VIDEO_FILES: [&str; 18] = [
    "video", "part", "vtt", "at", "sound", "sound.part", "sound.at", "level", "partial.m3u8", "alevel", "master", "v.m3u8",
    "a.m3u8", "pv.m3u8", "pa.m3u8", "hls", "got", "partial.m3u8.tmp",
];
/// The setting that lets downloads use mobile data (kept on the device, as it always was).
pub const MOBILE_DATA: &str = "mobileDownloads";
/// What the notification channels are called: the progress of what's coming down, and each one finished.
pub const CHANNEL_PROGRESS: &str = "Downloading";
pub const CHANNEL_DONE: &str = "Finished downloads";
/// The setting that says the app has asked about notifications (once, on the first download).
pub const ASKED_NOTES: &str = "askedNotifications";
/// Set in the manhwa folder once Android's chapters (its own queue and the ones kept on purpose) are in the list.
const CHAPTERS_MOVED: &str = "manhwa/.downloads-v1";
/// Where the spots offline plays reached are kept (an object of download key to seconds), as ever.
const SPOTS: &str = "offline.json";
/// How long a deleted download's transfer has to let go of its files before they go.
const LET_GO_MS: u32 = 1000;

/// The profile's settings downloads go by.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    /// The download quality setting ("standard", "high", "best").
    pub quality: String,
    /// Anime's sound ("sub", "dub").
    pub anime_audio: String,
    /// The account may have 2K and 4K.
    pub plus: bool,
}

/// A title, as much as a download needs of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WantedTitle {
    pub id: String,
    pub name: String,
    /// "cinema", "anime", "youtube", or a channel's.
    pub source: String,
    pub imdb: Option<String>,
    pub stream: Option<String>,
    /// A film's length.
    pub minutes: Option<i64>,
    /// Its own picture (a show's entry on Downloads, a film's row).
    pub art: Option<String>,
}

/// An episode, as much as a download needs of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WantedEpisode {
    pub id: String,
    pub season: Option<i64>,
    pub number: Option<i64>,
    /// An anime episode's own show.
    pub season_id: Option<String>,
    pub stream: Option<String>,
    pub still: Option<String>,
    /// Seconds.
    pub length: f64,
}

impl WantedTitle {
    /// A title page's title.
    pub fn of(t: &crate::model::Title) -> Self {
        WantedTitle {
            id: t.id.clone(),
            name: t.title.clone(),
            source: t.source.clone(),
            imdb: t.imdb.clone(),
            stream: t.stream.clone(),
            minutes: t.minutes,
            art: t.cover_big.clone().or_else(|| t.cover.clone()),
        }
    }
}

impl WantedEpisode {
    /// A title page's episode.
    pub fn of(e: &crate::model::Episode) -> Self {
        WantedEpisode {
            id: e.id.clone(),
            season: e.season,
            number: e.n,
            season_id: e.season_id.clone(),
            stream: e.stream.clone(),
            still: e.thumb.clone(),
            length: e.length,
        }
    }
}

/// A manhwa series, as its chapters' downloads keep it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WantedSeries {
    pub path: String,
    pub title: String,
    /// Its small picture and its large one.
    pub cover: String,
    pub art: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WantedChapter {
    pub path: String,
    pub label: String,
    pub thumb: Option<String>,
}

/// What happens to the queue.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The app started: what's kept is read, and whatever waits carries on once transfers may run. The profile, its
    /// quality and Plus come from the core's own tells ([`AppEvent::Account`], [`AppEvent::DownloadSettings`]); open
    /// the queue before the session so it hears them from the first.
    Start,
    /// Transfers may run (signed in; on Android, while its job runs), or must stop: what's going waits again.
    Run { on: bool },
    /// Whether downloads may use mobile data.
    MobileData { on: bool },
    /// The app asked whether it may show notifications: not asked again.
    NotesAsked,
    /// Download a film (`episodes` empty) or these episodes; one already there is this profile's too.
    Add { title: WantedTitle, episodes: Vec<WantedEpisode> },
    AddChapters { series: WantedSeries, chapters: Vec<WantedChapter> },
    /// A download's control pressed (pause, carry on, try again).
    Act { key: String, act: Act },
    /// Gone from this profile's downloads at once, for a screen that asks first itself (`show:<title id>`: a whole show).
    Remove { key: String },
    /// An offline play of the download `key` reached `at` seconds: where it carries on from.
    Spot { key: String, at: f64 },
    /// Downloads' delete: the first press asks, the second deletes (`show:<title id>`: a whole show).
    Delete { key: String },
    /// A press anywhere else: the delete that was asking lets go.
    Dismiss,
    /// What a film or these episodes will weigh, for the title page.
    Weigh { title: WantedTitle, episodes: Vec<WantedEpisode> },
    /// What these chapters of a series (their paths) will weigh, for its Download all: told under the series' path.
    WeighChapters { series: String, chapters: Vec<String> },
    /// Downloads opened: the room left asked again.
    Look,
    Search { query: String },
    /// A show opened on Downloads (its episodes), or closed (None).
    Show { title: Option<String> },
}

/// A download kept on the device to play: `file` and the others are paths as [`DiskOp`] has them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalCopy {
    pub key: String,
    pub file: String,
    /// An HLS playlist (kept as pieces), else a media file.
    pub hls: bool,
    /// The sound, a file of its own (YouTube's).
    pub sound: Option<String>,
    pub captions: Option<String>,
    pub seconds: f64,
    pub label: String,
    /// Where an offline play last got to, in seconds (0: not played).
    pub spot: f64,
}

/// A download as the app shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    pub key: String,
    pub kind: Kind,
    /// A video's title; a chapter's series path.
    pub title_id: String,
    pub episode_id: Option<String>,
    /// The title's or the series' name.
    pub title: String,
    /// What a row calls it: "S1 E2", "Episode 3", a film's title, a chapter's label.
    pub name: String,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub picture: Option<String>,
    pub art: Option<String>,
    pub imdb: Option<String>,
    /// Where a video comes from, as a title's source says it ("cinema", "anime", "youtube"; "" for a channel's).
    pub source: String,
    pub label: String,
    pub state: State,
    pub share: f32,
    pub size: u64,
    pub done: u64,
    /// What it's doing, in words.
    pub line: String,
    pub failed: bool,
    /// What its control does.
    pub act: Act,
    pub length: f64,
    pub local: Option<LocalCopy>,
}

/// The title as far as downloads know it with no connection (what Next episode has to go on): a film, or a show with
/// every episode of it that's here, in order. None if nothing of it is here to tell.
pub fn offline_title(items: &[Item], title_id: &str) -> Option<crate::model::Title> {
    let mine: Vec<&Item> = items.iter().filter(|i| i.kind == Kind::Video && i.title_id == title_id).collect();
    let first = *mine.first()?;
    let film = first.episode_id.is_none();
    let episodes = mine
        .iter()
        .filter(|i| i.local.is_some())
        .filter_map(|i| {
            Some(crate::model::Episode {
                id: i.episode_id.clone()?,
                n: i.episode,
                season: i.season,
                thumb: i.picture.clone(),
                length: i.length,
                ..Default::default()
            })
        })
        .collect();
    Some(crate::model::Title {
        id: first.title_id.clone(),
        title: first.title.clone(),
        source: first.source.clone(),
        kind: if film { "movie" } else { "series" }.into(),
        imdb: first.imdb.clone(),
        cover: first.picture.clone(),
        cover_big: first.picture.clone(),
        minutes: Some((first.length / 60.) as i64).filter(|m| film && *m > 0),
        episodes,
        film,
        ..Default::default()
    })
}

/// The finished copy on this device of a film (`episode` None, or the title's own id) or an episode: what it plays from.
/// One that's still coming down (or failed) has none.
pub fn playable<'a>(items: &'a [Item], title_id: &str, episode: Option<&str>) -> Option<&'a LocalCopy> {
    let episode = episode.filter(|e| *e != title_id);
    items
        .iter()
        .filter(|i| i.kind == Kind::Video && i.title_id == title_id && i.episode_id.as_deref() == episode)
        .find_map(|i| i.local.as_ref())
}

/// What an entry on Downloads is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EntryKind {
    #[default]
    Film,
    /// A show's stacked entry: opens on its episodes.
    Show,
    Episode,
    Chapter,
}

/// What pressing an entry does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Open {
    /// Nothing (it isn't down yet).
    #[default]
    Nothing,
    /// Play it (a film or an episode that's down).
    Play,
    /// Read it (a chapter that's down).
    Read,
    /// Open the show on its episodes ([`Entry::show`]).
    Show,
}

/// One entry on Downloads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub kind: EntryKind,
    /// What its delete and its control name (a show's "show:<title id>").
    pub key: String,
    /// The download it is, in [`View::items`].
    pub item: Option<u32>,
    pub name: String,
    pub line: String,
    pub failed: bool,
    /// Its delete is asking.
    pub asking: bool,
    pub picture: Option<String>,
    /// A show's "3 episodes".
    pub badge: String,
    /// What pressing it does.
    pub open: Open,
    /// A show's title id: what [`Event::Show`] opens it by.
    pub show: Option<String>,
}

/// A group of entries on Downloads: the films and shows (no heading), or one manhwa's chapters under its name; or, with a
/// show open, its episodes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Section {
    pub heading: Option<String>,
    pub entries: Vec<Entry>,
}

/// The Downloads page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub heading: String,
    /// The show open, if one is.
    pub show: Option<String>,
    pub room: String,
    pub search_hint: String,
    /// What a delete that's asking says.
    pub ask: String,
    pub sections: Vec<Section>,
    /// A delete is asking (a press anywhere else lets it go: [`Event::Dismiss`]).
    pub asking: bool,
    pub empty: Option<Empty>,
}

/// The notification while downloads come down.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Note {
    pub title: String,
    pub detail: String,
    pub percent: u8,
    /// How far along isn't known.
    pub unsure: bool,
}

/// A download that's finished, told once.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Told {
    /// Counts up: one the app has shown isn't shown again.
    pub number: u32,
    pub key: String,
    pub title: String,
    pub line: String,
    pub chapter: bool,
}

/// What a title page (or a series' page) was told it will weigh.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Weighed {
    /// The title's id, or the series' path.
    pub title: String,
    /// None: it can't tell.
    pub bytes: Option<u64>,
}

/// A download as the screens that show its control (a title page, a manhwa series) have it: where it stands, how far
/// along, and what it says of itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Held {
    pub key: String,
    pub kind: Kind,
    /// A video's title; a chapter's series path.
    pub title_id: String,
    pub episode_id: Option<String>,
    pub state: State,
    pub share: f32,
    /// What it's doing, in words ([`Item::line`]).
    pub line: String,
}

/// This profile's downloads and what was weighed, as the other screens are told them ([`Screen::downloads`]) whenever
/// either changes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub items: Vec<Held>,
    pub sizes: Vec<Weighed>,
    /// The profile's anime sound, which what's on offer depends on.
    pub anime_audio: String,
}

impl Snapshot {
    /// The download kept under `key`.
    pub fn get(&self, key: &str) -> Option<&Held> {
        self.items.iter().find(|h| h.key == key)
    }

    /// The film (`episode` None) or episode of `title` that's downloaded.
    pub fn video(&self, title: &str, episode: Option<&str>) -> Option<&Held> {
        self.items.iter().find(|h| h.kind == Kind::Video && h.title_id == title && h.episode_id.as_deref() == episode)
    }

    /// About what `title` (a title's id, or a series' path) was weighed at; None if it can't tell, or hasn't been.
    pub fn weighed(&self, title: &str) -> Option<u64> {
        self.sizes.iter().find(|s| s.title == title).and_then(|s| s.bytes)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// This profile's downloads: films and shows as they were first added (a show's episodes in order), then each
    /// manhwa's chapters.
    pub items: Vec<Item>,
    pub page: Page,
    pub note: Option<Note>,
    pub told: Vec<Told>,
    /// Something is waiting or coming down (any profile's): Android keeps a job for it.
    pub pending: bool,
    /// The job should go on running now: something is coming down, or could start.
    pub busy: bool,
    /// About what's still to come, for Android's job.
    pub bytes_left: u64,
    /// Downloads may use the connection there is.
    pub usable: bool,
    pub mobile_data: bool,
    pub sizes: Vec<Weighed>,
    /// [`Event::Run`] has been taken: `busy` now says whether there is work (before it, it can't).
    pub running: bool,
    /// The first download is about to be added and the app hasn't asked about notifications yet: ask now ([`Event::NotesAsked`]).
    pub ask_notes: bool,
}

/// What's known of a download's files.
#[derive(Clone, Debug, Default)]
struct Live {
    video: bool,
    sound: bool,
    vtt: bool,
    complete: bool,
    /// Kept as pieces by an older version: `video` is their playlist.
    pieces: bool,
}

#[derive(Debug)]
struct Running {
    job: Job,
    round: u32,
    tries: u32,
    before: u64,
    open: HashSet<u32>,
}

/// What an effect of the queue's was for.
#[derive(Clone, Debug)]
enum Ask {
    List,
    Sizes,
    Spots,
    Pref,
    Asked,
    Manhwa,
    /// Android's chapters, being brought into the list.
    Move(MoveStep),
    Files(Vec<String>),
    Save,
    Quiet,
    Free,
    Retry(String, u32),
    LetGo(Vec<String>),
    Job(String, u32, Tag),
    Weigh(String, usize, WeighTag),
    WeighChapter(String, usize, weigh::ChapterStep),
}

#[derive(Clone, Debug)]
enum MoveStep {
    Queue,
    Catalog,
    Series(String),
    Kept(Vec<(String, String, WantedChapter)>),
}

#[derive(Clone, Copy, Debug)]
enum WeighTag {
    Probe(probe::Step),
    Piece(usize),
}

/// A title page's size being weighed.
#[derive(Debug)]
struct Estimate {
    title: String,
    count: usize,
    picks: Vec<(String, String, probe::Copy, i64)>,
    next: usize,
    out: HashMap<usize, Weighing>,
    weighed: Vec<u64>,
}

/// A series' chapters being weighed.
#[derive(Debug)]
struct ChapterEstimate {
    series: String,
    count: usize,
    picked: usize,
    /// The chapters still to weigh (their paths).
    picks: Vec<String>,
    next: usize,
    out: HashMap<usize, weigh::ChapterWeight>,
    weighed: Vec<u64>,
}

#[derive(Debug)]
enum Weighing {
    Probe(Box<probe::Probe>),
    Weight(weigh::Weight),
}

/// The queue. See the module's docs.
#[derive(Debug, Default)]
pub struct Queue {
    loaded: bool,
    /// Reading what's kept: 0 the list and the rest, 1 their files.
    stage: u8,
    loading: usize,
    rows: Vec<Row>,
    live: HashMap<String, Live>,
    jobs: HashMap<String, Running>,
    asks: Asks<Ask>,
    me: String,
    settings: Settings,
    run: bool,
    mobile_data: bool,
    /// Whether the app has asked about notifications (None: not read yet).
    noted: Option<bool>,
    device: Option<Device>,
    usable: bool,
    speeds: Speeds,
    shared: Shared,
    sizes: serde_json::Map<String, Value>,
    /// Where offline plays got to, by download key.
    spots: HashMap<String, f64>,
    estimates: Vec<Estimate>,
    chapter_estimates: Vec<ChapterEstimate>,
    weighed: Vec<Weighed>,
    held: Vec<Event>,
    told: Vec<Told>,
    numbered: u32,
    query: String,
    show: Option<String>,
    asking: String,
    saving: bool,
    save_again: bool,
    next_round: u32,
    moving: usize,
    moved: Vec<Row>,
    /// The series lists kept in the manhwa folder, and the series found in its catalog: path, name, pictures.
    move_lists: HashSet<String>,
    move_series: Vec<(String, String, String, String)>,
    move_files: Vec<String>,
    now: f64,
}

impl Queue {
    fn ask(&mut self, what: Ask, io: Io) -> Effect {
        let id = self.asks.ask(what);
        match io {
            Io::Ask(request) => Effect::Request { id, request },
            Io::Fetch(request, file) => Effect::Fetch { id, request, file },
            Io::Disk(op) => Effect::Disk { id, op },
            Io::Wait(ms) => Effect::Timer { id, ms },
        }
    }

    fn device(&self) -> Device {
        self.device.unwrap_or(Device::Phone)
    }

    fn find(&self, key: &str) -> Option<usize> {
        self.rows.iter().position(|r| r.key == key)
    }

    fn save(&mut self) -> Vec<Effect> {
        if !self.loaded {
            return vec![];
        }
        if self.saving {
            self.save_again = true;
            return vec![];
        }
        self.saving = true;
        let text = row::write_list(&self.rows);
        vec![self.ask(Ask::Save, Io::Disk(DiskOp::Write { path: row::LIST.into(), text }))]
    }

    // ---------- reading what's kept ----------

    fn start(&mut self, cx: &Context) -> Vec<Effect> {
        if self.device.is_some() {
            return vec![];
        }
        self.device = Some(cx.platform.device);
        self.usable = self.usable_now(cx);
        let mut out = vec![
            self.ask(Ask::List, Io::Disk(DiskOp::Read { path: row::LIST.into() })),
            self.ask(Ask::Sizes, Io::Disk(DiskOp::Read { path: weigh::SIZES.into() })),
            self.ask(Ask::Spots, Io::Disk(DiskOp::Read { path: SPOTS.into() })),
            Effect::ReadPref { id: self.asks.ask(Ask::Pref), key: MOBILE_DATA.into() },
            Effect::ReadPref { id: self.asks.ask(Ask::Asked), key: ASKED_NOTES.into() },
        ];
        if cx.platform.device != Device::Desktop {
            out.push(self.ask(Ask::Manhwa, Io::Disk(DiskOp::Listing { path: "manhwa".into() })));
        }
        self.loading = out.len();
        out
    }

    /// Every file the list's downloads may have, asked about once they're read.
    fn look_at_files(&mut self) -> Vec<Effect> {
        if self.rows.is_empty() {
            return vec![];
        }
        let paths: Vec<String> = self.rows.iter().flat_map(row_files).collect();
        vec![self.ask(Ask::Files(paths.clone()), Io::Disk(DiskOp::Sizes { paths }))]
    }

    /// Once every answer of a stage of reading what's kept is in, the next stage.
    fn settle(&mut self, cx: &Context) -> Vec<Effect> {
        if self.loaded || self.loading > 0 || self.moving > 0 {
            return vec![];
        }
        if self.stage == 0 {
            self.stage = 1;
            let mut out = self.moved_in();
            let look = self.look_at_files();
            if !look.is_empty() {
                self.loading = look.len();
                out.extend(look);
                return out;
            }
            out.extend(self.finish_loading(cx));
            return out;
        }
        self.finish_loading(cx)
    }

    fn finish_loading(&mut self, cx: &Context) -> Vec<Effect> {
        let mut out = vec![];
        let mut gone = vec![];
        let live = &self.live;
        self.rows.retain(|r| {
            let l = live.get(&r.key).cloned().unwrap_or_default();
            let keep = !r.deleting
                && match (r.kind, r.state) {
                    (Kind::Video, State::Done) => l.video,
                    (Kind::Chapter, State::Done) => l.complete,
                    _ => true,
                };
            if !keep {
                gone.push(r.clone());
            }
            keep
        });
        for r in &gone {
            if r.deleting {
                out.push(self.let_go(r, 0));
            }
        }
        for r in &mut self.rows {
            r.state = r.state.restored();
            if r.kind == Kind::Video && r.state == State::Done {
                r.done = r.size;
            }
        }
        self.loaded = true;
        out.extend(self.save());
        let held = std::mem::take(&mut self.held);
        for e in held {
            out.extend(self.update(e, cx));
        }
        out.extend(self.kick(cx));
        out
    }

    /// An answer about what's kept, taken in.
    fn apply(&mut self, ask: Ask, reply: Reply, cx: &Context) -> Vec<Effect> {
        let disk = match &reply {
            Reply::Disk { disk } => Some(disk.clone()),
            _ => None,
        };
        let text = disk.as_ref().and_then(|d| d.text.clone());
        match ask {
            Ask::List => self.rows = text.map(|t| row::read_list(&t)).unwrap_or_default(),
            Ask::Sizes => {
                self.sizes = text.and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default();
            }
            Ask::Spots => {
                let kept = text.and_then(|t| serde_json::from_str::<Value>(&t).ok());
                self.spots = kept.and_then(|v| v.as_object().cloned()).map(|o| o.into_iter().filter_map(|(k, v)| Some((k, v.as_f64()?))).collect()).unwrap_or_default();
            }
            Ask::Pref => {
                if let Reply::Pref { value } = reply {
                    self.mobile_data = value.as_deref() == Some("true");
                    self.usable = self.usable_now(cx);
                }
            }
            Ask::Asked => {
                if let Reply::Pref { value } = reply {
                    self.noted = Some(value.as_deref() == Some("true"));
                }
            }
            Ask::Manhwa => {
                let names: Vec<String> = disk.map(|d| d.entries.into_iter().map(|e| e.name).collect()).unwrap_or_default();
                if !names.iter().any(|n| format!("manhwa/{n}") == CHAPTERS_MOVED) {
                    return self.move_chapters(&names);
                }
            }
            Ask::Files(paths) => {
                let sizes: HashMap<&str, u64> =
                    disk.as_ref().map(|d| paths.iter().map(String::as_str).zip(d.entries.iter().map(|e| e.bytes)).collect()).unwrap_or_default();
                for r in &mut self.rows {
                    let mine = row_files(r);
                    if !mine.iter().all(|p| sizes.contains_key(p.as_str())) {
                        continue;
                    }
                    let has = |ext: &str| sizes.get(file(&r.key, ext).as_str()).copied().unwrap_or(0);
                    let live = self.live.entry(r.key.clone()).or_default();
                    match r.kind {
                        Kind::Video => {
                            live.video = has("video") > 0;
                            live.sound = has("sound") > 0;
                            live.vtt = has("vtt") > 0;
                            live.pieces = has("level") > 0;
                            if r.state != State::Done {
                                r.done = has("part") + has("sound.part");
                            }
                        }
                        Kind::Chapter => live.complete = mine.iter().any(|p| sizes.get(p.as_str()).copied().unwrap_or(0) > 0),
                    }
                }
            }
            _ => {}
        }
        vec![]
    }

    // ---------- Android's chapters, from before they were in the list ----------

    /// Android kept its chapter downloads apart: a queue of its own (`queue-….json`), and the finished ones only as
    /// folders marked "kept". Once, they join the list: the queue's as they stood, the kept ones as down.
    fn move_chapters(&mut self, names: &[String]) -> Vec<Effect> {
        let mut out = vec![];
        for n in names.iter().filter(|n| n.starts_with("queue-") && n.ends_with(".json")) {
            let path = format!("manhwa/{n}");
            self.move_files.push(path.clone());
            out.push(self.ask(Ask::Move(MoveStep::Queue), Io::Disk(DiskOp::Read { path })));
        }
        if names.iter().any(|n| n == "catalog.json") {
            out.push(self.ask(Ask::Move(MoveStep::Catalog), Io::Disk(DiskOp::Read { path: "manhwa/catalog.json".into() })));
        }
        self.move_lists = names.iter().filter(|n| n.ends_with(".json")).cloned().collect();
        // (marked even when there was nothing to bring, so it's never looked for again)
        self.move_files.push(CHAPTERS_MOVED.into());
        self.moving += out.len();
        out
    }

    fn move_step(&mut self, step: MoveStep, disk: Option<crate::screen::DiskReply>) -> Vec<Effect> {
        let text = disk.as_ref().and_then(|d| d.text.clone()).unwrap_or_default();
        let wrap = |text: &str, key: &str| if text.trim_start().starts_with('[') { format!("{{\"{key}\": {text}}}") } else { text.to_string() };
        let mut out = vec![];
        match step {
            MoveStep::Queue => self.moved.extend(row::read_chapter_queue(&text)),
            MoveStep::Catalog => {
                let comics = crate::manhwa::parse_catalog(&wrap(&text, "items"), 1, "", "").map(|s| s.items).unwrap_or_default();
                for c in comics {
                    let name = format!("{}.json", rules::chapter_folder(&c.path));
                    if self.move_lists.contains(&name) {
                        let ask = Ask::Move(MoveStep::Series(c.path.clone()));
                        self.move_series.push((c.path.clone(), c.title.clone(), c.small().to_string(), c.cover.clone()));
                        out.push(self.ask(ask, Io::Disk(DiskOp::Read { path: format!("manhwa/{name}") })));
                    }
                }
            }
            MoveStep::Series(path) => {
                let chapters = crate::manhwa::parse_chapters(&wrap(&text, "chapters")).map(|s| s.chapters).unwrap_or_default();
                let found: Vec<(String, String, WantedChapter)> =
                    chapters.into_iter().map(|c| (path.clone(), c.path.clone(), WantedChapter { path: c.path, label: c.label, thumb: Some(c.thumb).filter(|t| !t.is_empty()) })).collect();
                let mut paths = vec![];
                for (_, chapter, _) in &found {
                    let dir = transfer::chapter_dir(chapter);
                    paths.push(format!("{dir}/{}", rules::KEPT));
                    paths.push(format!("{dir}/{}", rules::COMPLETE));
                }
                if !found.is_empty() {
                    out.push(self.ask(Ask::Move(MoveStep::Kept(found)), Io::Disk(DiskOp::Sizes { paths })));
                }
            }
            MoveStep::Kept(found) => {
                let sizes: Vec<u64> = disk.map(|d| d.entries.iter().map(|e| e.bytes).collect()).unwrap_or_default();
                for (i, (series, _, chapter)) in found.into_iter().enumerate() {
                    let kept = sizes.get(i * 2).copied().unwrap_or(0) > 0 && sizes.get(i * 2 + 1).copied().unwrap_or(0) > 0;
                    let Some(s) = self.move_series.iter().find(|s| s.0 == series).cloned() else { continue };
                    if kept {
                        self.moved.push(Row {
                            key: chapter.path,
                            kind: Kind::Chapter,
                            title_id: s.0,
                            title: s.1,
                            cover: Some(s.2).filter(|c| !c.is_empty()),
                            art: Some(s.3).filter(|c| !c.is_empty()),
                            label: chapter.label,
                            thumb: chapter.thumb,
                            state: State::Done,
                            ..Default::default()
                        });
                    }
                }
            }
        }
        self.moving += out.len();
        out
    }

    /// Android's chapters added to the list (those already in it left as they are), its old queues gone, and the mark
    /// that says it's done.
    fn moved_in(&mut self) -> Vec<Effect> {
        for r in std::mem::take(&mut self.moved) {
            if self.find(&r.key).is_none() {
                self.rows.push(r);
            }
        }
        let files = std::mem::take(&mut self.move_files);
        let mut out = vec![];
        let (marks, queues): (Vec<String>, Vec<String>) = files.into_iter().partition(|f| f == CHAPTERS_MOVED);
        if !queues.is_empty() {
            out.push(self.ask(Ask::Quiet, Io::Disk(DiskOp::Remove { paths: queues })));
        }
        for m in marks {
            out.push(self.ask(Ask::Quiet, Io::Disk(DiskOp::Write { path: m, text: "1".into() })));
        }
        out
    }

    // ---------- what's done to downloads ----------

    fn usable_now(&self, cx: &Context) -> bool {
        cx.online && (self.mobile_data || !cx.metered)
    }

    /// Whatever may start now does, and whatever can't go on (no connection it may use, transfers stopped) waits.
    fn kick(&mut self, cx: &Context) -> Vec<Effect> {
        if !self.loaded {
            return vec![];
        }
        self.usable = self.usable_now(cx);
        let mut out = vec![];
        if !(self.run && self.usable) {
            let keys: Vec<String> = self.jobs.keys().cloned().collect();
            for key in keys {
                out.extend(self.stop(&key, State::Queued));
            }
            out.extend(self.save());
            return out;
        }
        let slots: Vec<rules::Slot> =
            self.rows.iter().map(|r| rules::Slot { kind: r.kind, state: r.state, busy: self.jobs.contains_key(&r.key) || r.deleting }).collect();
        let starting = rules::to_start(&slots);
        for i in &starting {
            let key = self.rows[*i].key.clone();
            self.rows[*i].state = State::Running;
            self.rows[*i].note.clear();
            let job = Job::new(&self.rows[*i], cx);
            self.next_round += 1;
            let before = self.rows[*i].done;
            self.jobs.insert(key.clone(), Running { job, round: self.next_round, tries: 0, before, open: HashSet::new() });
            out.extend(self.round(&key, cx));
        }
        if !starting.is_empty() {
            out.extend(self.save());
        }
        out
    }

    /// A round of a transfer starts.
    fn round(&mut self, key: &str, cx: &Context) -> Vec<Effect> {
        let Some(i) = self.find(key) else { return vec![] };
        let Some(running) = self.jobs.get_mut(key) else { return vec![] };
        running.before = self.rows[i].done;
        let step = running.job.start(&self.rows[i], self.settings.plus, cx, &self.shared);
        self.step(key, step, cx)
    }

    /// What a step of a transfer asks for, under the job's round, and how it ended.
    fn step(&mut self, key: &str, step: transfer::Step, cx: &Context) -> Vec<Effect> {
        let mut out = self.issue(key, step.io);
        if let Some(end) = step.end {
            out.extend(self.ended(key, end, cx));
        }
        out
    }

    /// A transfer's asks, under its round.
    fn issue(&mut self, key: &str, io: Vec<(Tag, Io)>) -> Vec<Effect> {
        let Some(round) = self.jobs.get(key).map(|r| r.round) else { return vec![] };
        let mut out = vec![];
        for (tag, io) in io {
            let effect = self.ask(Ask::Job(key.into(), round, tag), io);
            if let Some(id) = effect_id(&effect) {
                if let Some(r) = self.jobs.get_mut(key) {
                    r.open.insert(id);
                }
            }
            out.push(effect);
        }
        out
    }

    /// A transfer's round ended: down, failed, or (a dropped connection) tried again in a while.
    fn ended(&mut self, key: &str, end: End, cx: &Context) -> Vec<Effect> {
        let Some(i) = self.find(key) else { return vec![] };
        let mut out = self.cancel_open(key);
        match end {
            End::Done { total } => {
                self.jobs.remove(key);
                let r = &mut self.rows[i];
                r.state = State::Done;
                r.note.clear();
                if r.kind == Kind::Video {
                    r.size = total;
                    r.done = total;
                    let tallest = transfer::target(&r.quality, &r.copy, self.settings.plus);
                    // (now it's known for certain: the title page says the real size from here on, not the estimate)
                    let sized = format!("{}:{tallest}", r.copy);
                    out.extend(self.keep_size(&sized, total, cx));
                }
                let r = &self.rows[i];
                self.numbered += 1;
                let told = match r.kind {
                    Kind::Video => Told { number: self.numbered, key: r.key.clone(), title: r.named(), line: "Ready to watch".into(), chapter: false },
                    Kind::Chapter => {
                        Told { number: self.numbered, key: r.key.clone(), title: r.title.clone(), line: format!("{} · Ready to read", r.label), chapter: true }
                    }
                };
                self.told.push(told);
                if self.told.len() > 8 {
                    self.told.remove(0);
                }
                let r = r.clone();
                out.extend(self.refresh(&r));
                out.extend(self.save());
                out.extend(self.kick(cx));
            }
            End::Failed(fail) => {
                let (note, again) = match fail {
                    Fail::Cant(words) => (words, false),
                    Fail::Full => (rules::FULL.into(), false),
                    Fail::Dropped(words) => (words, true),
                };
                let stopped = !(self.run && self.usable_now(cx));
                if again && stopped {
                    out.extend(self.stop(key, State::Queued));
                    out.extend(self.save());
                    return out;
                }
                let wait = if again {
                    let done = self.rows[i].done;
                    self.jobs.get_mut(key).and_then(|r| {
                        if done > r.before {
                            r.tries = 0;
                        }
                        r.tries += 1;
                        rules::retry_wait(r.tries)
                    })
                } else {
                    None
                };
                match wait {
                    Some(ms) => {
                        self.rows[i].note = note;
                        self.next_round += 1;
                        let round = self.next_round;
                        if let Some(r) = self.jobs.get_mut(key) {
                            r.round = round;
                        }
                        let ms = u32::try_from(ms).unwrap_or(u32::MAX);
                        out.push(self.ask(Ask::Retry(key.into(), round), Io::Wait(ms)));
                    }
                    None => {
                        self.jobs.remove(key);
                        let r = &mut self.rows[i];
                        r.state = State::Failed;
                        r.note = note;
                        out.extend(self.save());
                        out.extend(self.kick(cx));
                    }
                }
            }
        }
        out
    }

    fn cancel_open(&mut self, key: &str) -> Vec<Effect> {
        let Some(r) = self.jobs.get_mut(key) else { return vec![] };
        let open: Vec<u32> = r.open.drain().collect();
        open.into_iter()
            .map(|id| {
                self.asks.take(id);
                Effect::Cancel { id }
            })
            .collect()
    }

    /// Its transfer stopped, and it stands as `state` (paused; waiting again).
    fn stop(&mut self, key: &str, state: State) -> Vec<Effect> {
        let out = self.cancel_open(key);
        self.jobs.remove(key);
        if let Some(i) = self.find(key) {
            let r = &mut self.rows[i];
            if r.state == State::Running || state == State::Paused {
                r.state = if state == State::Paused { r.state.paused() } else { state };
            }
        }
        out
    }

    /// What's known of a download's files, asked again (it's just finished).
    fn refresh(&mut self, r: &Row) -> Vec<Effect> {
        let paths = row_files(r);
        vec![self.ask(Ask::Files(paths.clone()), Io::Disk(DiskOp::Sizes { paths }))]
    }

    fn keep_size(&mut self, key: &str, bytes: u64, cx: &Context) -> Vec<Effect> {
        self.sizes.insert(key.into(), json!([bytes, (cx.clock.now * 1000.) as i64]));
        let text = Value::Object(self.sizes.clone()).to_string();
        vec![self.ask(Ask::Quiet, Io::Disk(DiskOp::Write { path: weigh::SIZES.into(), text }))]
    }

    /// What `key` weighed when it was weighed within [`weigh::SIZE_KEPT_MS`] (ever, `for_good`).
    fn kept_size(&self, key: &str, for_good: bool, cx: &Context) -> Option<u64> {
        let entry = self.sizes.get(key)?.as_array()?;
        let (bytes, at) = (entry.first()?.as_u64()?, entry.get(1)?.as_f64()?);
        (for_good || cx.clock.now * 1000. - at < weigh::SIZE_KEPT_MS).then_some(bytes)
    }

    /// A deleted download's files gone, once its transfer has let go of them.
    fn let_go(&mut self, r: &Row, wait: u32) -> Effect {
        let paths: Vec<String> = match r.kind {
            Kind::Video => VIDEO_FILES.iter().map(|e| file(&r.key, e)).collect(),
            Kind::Chapter => vec![transfer::chapter_dir(&r.key)],
        };
        if wait == 0 {
            self.ask(Ask::Quiet, Io::Disk(DiskOp::Remove { paths }))
        } else {
            self.ask(Ask::LetGo(paths), Io::Wait(wait))
        }
    }

    /// An entry of Downloads gone: a download (its key), or every episode of a show (`show:<title id>`).
    fn delete_entry(&mut self, key: String, cx: &Context) -> Vec<Effect> {
        let keys: Vec<String> = match key.strip_prefix("show:") {
            Some(id) => self.rows.iter().filter(|r| r.title_id == id && r.kind == Kind::Video && rules::owned(r.profiles.as_deref(), &self.me)).map(|r| r.key.clone()).collect(),
            None => vec![key],
        };
        let mut out = vec![];
        for k in keys {
            out.extend(self.delete(&k, cx));
        }
        if self.show.as_ref().is_some_and(|id| !self.rows.iter().any(|r| &r.title_id == id)) {
            self.show = None;
        }
        out
    }

    /// Gone from this profile's downloads; gone for good, files and all, once no other profile has it.
    fn delete(&mut self, key: &str, cx: &Context) -> Vec<Effect> {
        let Some(i) = self.find(key) else { return vec![] };
        if let Some(left) = rules::after_delete(self.rows[i].profiles.as_deref(), &self.me) {
            self.rows[i].profiles = Some(left);
            return self.save();
        }
        let mut out = self.stop(key, State::Paused);
        let r = self.rows.remove(i);
        self.live.remove(key);
        out.push(self.let_go(&r, LET_GO_MS));
        out.extend(self.save());
        out.extend(self.kick(cx));
        out
    }

    fn add(&mut self, title: WantedTitle, episodes: Vec<WantedEpisode>, cx: &Context) -> Vec<Effect> {
        if cx.platform.device == Device::Tv {
            return vec![];
        }
        let (quality, label) = crate::quality::download_saved(&self.settings.quality);
        let wanted: Vec<Option<WantedEpisode>> = if episodes.is_empty() { vec![None] } else { episodes.into_iter().map(Some).collect() };
        for ep in wanted {
            let key = ep.as_ref().map_or(title.id.clone(), |e| e.id.clone());
            if let Some(i) = self.find(&key) {
                let r = &mut self.rows[i];
                // (deleted a moment ago, its transfer still letting go: wanted again, it carries on from what's there)
                if r.deleting {
                    r.deleting = false;
                    r.state = State::Queued;
                    r.note.clear();
                }
                // (saved already, by another profile: now this one's too)
                if let Some(list) = rules::shared_with(r.profiles.as_deref(), &self.me) {
                    r.profiles = Some(list);
                }
                continue;
            }
            let Some(copy) = copy_of(&title, ep.as_ref(), &self.settings.anime_audio) else { continue };
            let length = ep.as_ref().map_or(0., |e| e.length);
            self.rows.push(Row {
                key,
                kind: Kind::Video,
                title_id: title.id.clone(),
                episode_id: ep.as_ref().map(|e| e.id.clone()),
                title: title.name.clone(),
                season: ep.as_ref().and_then(|e| e.season),
                episode: ep.as_ref().and_then(|e| e.number),
                cover: ep.as_ref().and_then(|e| e.still.clone()).or_else(|| title.art.clone()),
                art: title.art.clone(),
                imdb: title.imdb.clone(),
                copy,
                label: label.into(),
                quality: quality.into(),
                length: if length > 0. { length } else { title.minutes.unwrap_or(0) as f64 * 60. },
                profiles: (!self.me.is_empty()).then(|| vec![self.me.clone()]),
                ..Default::default()
            });
        }
        let mut out = self.save();
        out.extend(self.kick(cx));
        out
    }

    fn add_chapters(&mut self, series: WantedSeries, chapters: Vec<WantedChapter>, cx: &Context) -> Vec<Effect> {
        for c in chapters {
            match self.find(&c.path) {
                Some(i) => {
                    let r = &mut self.rows[i];
                    if let Some(list) = rules::shared_with(r.profiles.as_deref(), &self.me) {
                        r.profiles = Some(list);
                    }
                    r.state = if r.deleting { State::Queued } else { r.state.resumed() };
                    r.deleting = false;
                    if r.state == State::Queued {
                        r.note.clear();
                    }
                }
                None => self.rows.push(Row {
                    key: c.path,
                    kind: Kind::Chapter,
                    title_id: series.path.clone(),
                    title: series.title.clone(),
                    cover: Some(series.cover.clone()).filter(|s| !s.is_empty()),
                    art: Some(series.art.clone()).filter(|s| !s.is_empty()),
                    label: c.label,
                    thumb: c.thumb,
                    profiles: (!self.me.is_empty()).then(|| vec![self.me.clone()]),
                    ..Default::default()
                }),
            }
        }
        let mut out = self.save();
        out.extend(self.kick(cx));
        out
    }

    fn act(&mut self, key: &str, act: Act, cx: &Context) -> Vec<Effect> {
        let Some(i) = self.find(key) else { return vec![] };
        let mut out = vec![];
        match act {
            Act::Pause => out.extend(self.stop(key, State::Paused)),
            Act::Resume | Act::Retry => {
                let r = &mut self.rows[i];
                r.state = r.state.resumed();
                if r.state == State::Queued {
                    r.note.clear();
                }
            }
            Act::Add | Act::Open => return out,
        }
        out.extend(self.save());
        out.extend(self.kick(cx));
        out
    }

    // ---------- pieces under way ----------

    /// More pieces asked for, as many as each transfer allows.
    fn pump(&mut self) -> Vec<Effect> {
        let keys: Vec<String> = self.rows.iter().filter(|r| self.jobs.contains_key(&r.key)).map(|r| r.key.clone()).collect();
        let mut out = vec![];
        for key in keys {
            let Some(running) = self.jobs.get_mut(&key) else { continue };
            let io = match &mut running.job {
                Job::Video(v) => v.pump(usize::MAX),
                Job::Chapter(c) => {
                    let Some(i) = self.rows.iter().position(|r| r.key == key) else { continue };
                    c.pump(&self.rows[i])
                }
            };
            out.extend(self.issue(&key, io));
        }
        out
    }

    fn job_answer(&mut self, key: String, round: u32, tag: Tag, id: u32, reply: Reply, cx: &Context) -> Vec<Effect> {
        let Some(running) = self.jobs.get_mut(&key) else { return vec![] };
        running.open.remove(&id);
        if running.round != round {
            return vec![];
        }
        let Some(i) = self.rows.iter().position(|r| r.key == key) else { return vec![] };
        let (before, label) = (self.rows[i].done, self.rows[i].label.clone());
        let step = running.job.answer(tag, reply, &mut self.rows[i], cx, &mut self.shared);
        let r = &self.rows[i];
        if r.done != before {
            self.speeds.seen(&key, r.done, cx.clock.now * 1000.);
        }
        let named = r.label != label;
        let mut out = self.step(&key, step, cx);
        if named {
            out.extend(self.save());
        }
        out.extend(self.pump());
        out
    }

    // ---------- weighing for a title page ----------

    fn weigh(&mut self, title: WantedTitle, episodes: Vec<WantedEpisode>, cx: &Context) -> Vec<Effect> {
        self.estimates.retain(|e| e.title != title.id);
        self.weighed.retain(|w| w.title != title.id);
        if cx.platform.device == Device::Tv || title.source == "youtube" {
            self.weighed.push(Weighed { title: title.id, bytes: None });
            return vec![];
        }
        let count = episodes.len().max(1);
        let chosen: Vec<Option<&WantedEpisode>> = if episodes.is_empty() {
            vec![None]
        } else {
            weigh::picks(episodes.len(), title.source == "anime").into_iter().map(|i| episodes.get(i)).collect()
        };
        let mut estimate = Estimate { title: title.id.clone(), count, picks: vec![], next: 0, out: HashMap::new(), weighed: vec![] };
        for ep in chosen {
            let Some(copy) = copy_of(&title, ep, &self.settings.anime_audio) else { continue };
            let quality = crate::quality::download_saved(&self.settings.quality).0;
            let height = transfer::target(quality, &copy, self.settings.plus);
            let key = format!("{copy}:{height}");
            if let Some(bytes) = self.kept_size(&key, false, cx) {
                estimate.weighed.push(bytes);
                continue;
            }
            let Some(read) = probe::Copy::read(&copy, title.imdb.as_deref(), ep.and_then(|e| e.season), ep.and_then(|e| e.number)) else { continue };
            estimate.picks.push((copy, key, read, height));
        }
        self.estimates.push(estimate);
        self.weigh_more(&title.id, cx)
    }

    fn weigh_more(&mut self, title: &str, cx: &Context) -> Vec<Effect> {
        let Some(e) = self.estimates.iter_mut().find(|e| e.title == title) else { return vec![] };
        let mut asks = vec![];
        while e.out.len() < weigh::EPISODES_AT_ONCE && e.next < e.picks.len() {
            let n = e.next;
            e.next += 1;
            let (_, _, copy, height) = &e.picks[n];
            let mut p = probe::Probe::new(copy.clone(), *height, false);
            for (step, io) in p.start(cx, &self.shared.known) {
                asks.push((n, WeighTag::Probe(step), io));
            }
            e.out.insert(n, Weighing::Probe(Box::new(p)));
        }
        if e.out.is_empty() && e.next >= e.picks.len() {
            let bytes = weigh::whole(&e.weighed, e.count);
            let title = e.title.clone();
            self.estimates.retain(|x| x.title != title);
            self.weighed.push(Weighed { title, bytes });
            return vec![];
        }
        asks.into_iter().map(|(n, tag, io)| self.ask(Ask::Weigh(title.into(), n, tag), io)).collect()
    }

    fn weigh_answer(&mut self, title: String, n: usize, tag: WeighTag, reply: Reply, cx: &Context) -> Vec<Effect> {
        let Some(e) = self.estimates.iter_mut().find(|e| e.title == title) else { return vec![] };
        let Some(w) = e.out.remove(&n) else { return vec![] };
        let mut asks: Vec<(WeighTag, Io)> = vec![];
        let mut bytes = None;
        let mut weighing = None;
        match (w, tag) {
            (Weighing::Probe(mut p), WeighTag::Probe(step)) => {
                let (io, end) = p.answer(step, reply, cx, &mut self.shared.known);
                match end {
                    None => {
                        asks = io.into_iter().map(|(s, io)| (WeighTag::Probe(s), io)).collect();
                        weighing = Some(Weighing::Probe(p));
                    }
                    Some(Ok(Some(found))) => match weigh::Weight::start(&title, &found, &self.shared.rates) {
                        (_, weigh::Next::Bytes(b)) => bytes = Some(b),
                        (w, weigh::Next::Ask(io)) => {
                            asks = io.into_iter().map(|(i, io)| (WeighTag::Piece(i), io)).collect();
                            weighing = Some(Weighing::Weight(w));
                        }
                    },
                    Some(_) => bytes = Some(0),
                }
            }
            (Weighing::Weight(mut w), WeighTag::Piece(i)) => match w.answer(i, reply, &mut self.shared.rates) {
                weigh::Next::Bytes(b) => bytes = Some(b),
                weigh::Next::Ask(io) => {
                    asks = io.into_iter().map(|(i, io)| (WeighTag::Piece(i), io)).collect();
                    weighing = Some(Weighing::Weight(w));
                }
            },
            (w, _) => weighing = Some(w),
        }
        let Some(e) = self.estimates.iter_mut().find(|e| e.title == title) else { return vec![] };
        let mut out = vec![];
        if let Some(w) = weighing {
            e.out.insert(n, w);
        }
        if let Some(b) = bytes.filter(|b| *b > 0) {
            e.weighed.push(b);
            let key = e.picks[n].1.clone();
            out.extend(self.keep_size(&key, b, cx));
        }
        for (tag, io) in asks {
            out.push(self.ask(Ask::Weigh(title.clone(), n, tag), io));
        }
        out.extend(self.weigh_more(&title, cx));
        out
    }

    // ---------- weighing a series' chapters ----------

    fn weigh_chapters(&mut self, series: String, chapters: Vec<String>, cx: &Context) -> Vec<Effect> {
        self.chapter_estimates.retain(|e| e.series != series);
        self.weighed.retain(|w| w.title != series);
        let picks: Vec<String> = weigh::chapter_picks(chapters.len()).into_iter().map(|i| chapters[i].clone()).collect();
        let mut e = ChapterEstimate { series: series.clone(), count: chapters.len(), picked: picks.len(), picks: vec![], next: 0, out: HashMap::new(), weighed: vec![] };
        for path in picks {
            match self.kept_size(&path, true, cx) {
                Some(bytes) => e.weighed.push(bytes),
                None => e.picks.push(path),
            }
        }
        self.chapter_estimates.push(e);
        self.weigh_chapters_more(&series)
    }

    fn weigh_chapters_more(&mut self, series: &str) -> Vec<Effect> {
        let Some(e) = self.chapter_estimates.iter_mut().find(|e| e.series == series) else { return vec![] };
        let mut asks = vec![];
        while e.out.len() < weigh::CHAPTERS_AT_ONCE && e.next < e.picks.len() {
            let n = e.next;
            e.next += 1;
            let (w, io) = weigh::ChapterWeight::start(&e.picks[n]);
            asks.extend(io.into_iter().map(|(step, io)| (n, step, io)));
            e.out.insert(n, w);
        }
        if e.out.is_empty() && e.next >= e.picks.len() {
            let bytes = weigh::chapters_whole(&e.weighed, e.picked, e.count);
            self.chapter_estimates.retain(|x| x.series != series);
            self.weighed.push(Weighed { title: series.into(), bytes });
            return vec![];
        }
        asks.into_iter().map(|(n, step, io)| self.ask(Ask::WeighChapter(series.into(), n, step), io)).collect()
    }

    fn weigh_chapter_answer(&mut self, series: String, n: usize, step: weigh::ChapterStep, reply: Reply, cx: &Context) -> Vec<Effect> {
        let Some(e) = self.chapter_estimates.iter_mut().find(|e| e.series == series) else { return vec![] };
        let Some(w) = e.out.get_mut(&n) else { return vec![] };
        let mut out = vec![];
        match w.answer(step, reply, cx) {
            weigh::ChapterNext::Wait => {}
            weigh::ChapterNext::Ask(io) => {
                for (step, io) in io {
                    out.push(self.ask(Ask::WeighChapter(series.clone(), n, step), io));
                }
            }
            weigh::ChapterNext::Bytes { bytes, keep } => {
                e.out.remove(&n);
                let path = e.picks[n].clone();
                if let Some(b) = bytes {
                    e.weighed.push(b);
                    if keep {
                        out.extend(self.keep_size(&path, b, cx));
                    }
                }
                out.extend(self.weigh_chapters_more(&series));
            }
        }
        out
    }

    // ---------- the view ----------

    fn wifi_only(&self) -> bool {
        self.device() != Device::Desktop && !self.mobile_data
    }

    fn stacked(&self) -> bool {
        self.device() != Device::Desktop
    }

    fn line_of(&self, r: &Row) -> String {
        let speed = self.speeds.now(&r.key, self.now * 1000.).filter(|_| r.state == State::Running);
        rules::line(&rules::Line {
            kind: r.kind,
            state: Some(r.state),
            size: r.size,
            done: r.done,
            bytes: r.bytes,
            label: &r.label,
            note: &r.note,
            speed,
            online: self.usable,
            wifi_only: self.wifi_only(),
            stacked: self.stacked(),
        })
    }

    /// This profile's downloads, as the other screens are told them.
    pub fn snapshot(&self) -> Snapshot {
        let items = self
            .rows
            .iter()
            .filter(|r| !r.deleting && rules::owned(r.profiles.as_deref(), &self.me))
            .map(|r| Held {
                key: r.key.clone(),
                kind: r.kind,
                title_id: r.title_id.clone(),
                // (a film's own id isn't an episode)
                episode_id: r.episode_id.clone().filter(|e| *e != r.title_id),
                state: r.state,
                share: rules::share(r.done, r.size),
                line: self.line_of(r),
            })
            .collect();
        Snapshot { items, sizes: self.weighed.clone(), anime_audio: self.settings.anime_audio.clone() }
    }

    fn item(&self, r: &Row) -> Item {
        let live = self.live.get(&r.key);
        let line = self.line_of(r);
        let name = match (r.kind, r.episode) {
            (Kind::Chapter, _) => r.label.clone(),
            (Kind::Video, Some(n)) => r.season.map_or(format!("Episode {n}"), |s| format!("S{s} E{n}")),
            (Kind::Video, None) => r.title.clone(),
        };
        let picture = match r.kind {
            Kind::Chapter => r.thumb.clone().or_else(|| r.cover.clone()),
            Kind::Video => r.cover.clone(),
        };
        Item {
            key: r.key.clone(),
            kind: r.kind,
            title_id: r.title_id.clone(),
            episode_id: r.episode_id.clone(),
            title: r.title.clone(),
            name,
            season: r.season,
            episode: r.episode,
            picture,
            art: r.art.clone(),
            imdb: r.imdb.clone(),
            source: match r.copy.split('|').next() {
                Some("dulo") => "cinema",
                Some("anime") => "anime",
                Some("yt") => "youtube",
                _ => "",
            }
            .into(),
            label: r.label.clone(),
            state: r.state,
            share: rules::share(r.done, r.size),
            size: r.size,
            done: r.done,
            line,
            failed: r.state == State::Failed,
            act: rules::tap(Some(r.state)),
            length: r.length,
            local: live.and_then(|l| self.local(r, l)),
        }
    }

    /// What a finished download plays from.
    fn local(&self, r: &Row, live: &Live) -> Option<LocalCopy> {
        if r.kind != Kind::Video {
            return None;
        }
        let label = if crate::quality::name_rank(&r.label) > 0 || r.quality == "best" { r.label.clone() } else { format!("{} · {}", r.label, r.quality) };
        let captions = live.vtt.then(|| file(&r.key, "vtt"));
        if r.state != State::Done || !live.video {
            return None;
        }
        Some(LocalCopy {
            key: r.key.clone(),
            file: file(&r.key, "video"),
            hls: live.pieces,
            sound: live.sound.then(|| file(&r.key, "sound")),
            captions,
            seconds: r.length,
            label,
            spot: self.spots.get(&r.key).copied().unwrap_or(0.),
        })
    }

    fn page(&self, items: &[Item]) -> Page {
        let desktop = self.device() == Device::Desktop;
        let listed: Vec<rules::Listed> = items
            .iter()
            .map(|i| rules::Listed { kind: i.kind, group: &i.title_id, name: &i.title, season: i.season, episode: i.episode, chapter: &i.key })
            .collect();
        let query = if self.show.is_some() { "" } else { self.query.as_str() };
        let groups = rules::grouped(&listed, query);
        let entry = |kind: EntryKind, n: usize| {
            let it = &items[n];
            Entry {
                kind,
                key: it.key.clone(),
                item: u32::try_from(n).ok(),
                name: it.name.clone(),
                line: it.line.clone(),
                failed: it.failed,
                asking: self.asking == it.key,
                picture: it.picture.clone(),
                badge: String::new(),
                open: match kind {
                    EntryKind::Chapter if it.state == State::Done => Open::Read,
                    EntryKind::Film | EntryKind::Episode if it.local.is_some() => Open::Play,
                    _ => Open::Nothing,
                },
                show: None,
            }
        };
        let mut videos = vec![];
        let mut sections = vec![];
        let open = self.show.as_ref().and_then(|id| groups.iter().find(|g| items[g[0]].kind == Kind::Video && &items[g[0]].title_id == id));
        match open {
            Some(g) => videos.extend(g.iter().map(|n| entry(EntryKind::Episode, *n))),
            None => {
                for g in &groups {
                    let first = &items[g[0]];
                    match (first.kind, first.episode) {
                        (Kind::Video, None) => videos.push(entry(EntryKind::Film, g[0])),
                        (Kind::Video, Some(_)) => {
                            let rows: Vec<(State, u64, u64)> = g.iter().map(|n| (items[*n].state, items[*n].size, items[*n].done)).collect();
                            let levels: Vec<String> = g.iter().map(|n| items[*n].label.clone()).collect();
                            let speed: f64 = g.iter().filter_map(|n| self.speeds.now(&items[*n].key, self.now * 1000.).filter(|_| items[*n].state == State::Running)).sum();
                            let key = format!("show:{}", first.title_id);
                            let n = g.len();
                            videos.push(Entry {
                                kind: EntryKind::Show,
                                asking: self.asking == key,
                                key,
                                item: None,
                                name: first.title.clone(),
                                line: rules::show_line(&rows, &levels, Some(speed), self.usable, self.wifi_only(), self.stacked()),
                                failed: g.iter().any(|n| items[*n].failed),
                                picture: first.art.clone().or_else(|| first.picture.clone()),
                                badge: if n == 1 { "1 episode".into() } else { format!("{n} episodes") },
                                open: Open::Show,
                                show: Some(first.title_id.clone()),
                            });
                        }
                        (Kind::Chapter, _) => {
                            sections.push(Section { heading: Some(first.title.clone()), entries: g.iter().map(|n| entry(EntryKind::Chapter, *n)).collect() });
                        }
                    }
                }
            }
        }
        // (the videos first, side by side across the page; then each manhwa under its name)
        if !videos.is_empty() {
            sections.insert(0, Section { heading: None, entries: videos });
        }
        let asking = sections.iter().flat_map(|s| &s.entries).any(|e| e.asking);
        let empty = (sections.is_empty() && open.is_none()).then(|| {
            if self.query.trim().is_empty() {
                Empty {
                    glyph: Glyph::Download,
                    title: "No downloads yet".into(),
                    line: "Download a film, an episode or a manhwa chapter to watch or read it here, even offline.".into(),
                }
            } else {
                Empty { glyph: Glyph::Search, title: "Nothing found".into(), line: "Nothing here goes by that name.".into() }
            }
        });
        Page {
            heading: open.map_or("Downloads".into(), |g| items[g[0]].title.clone()),
            show: open.map(|g| items[g[0]].title_id.clone()),
            room: self.shared.free.map(rules::room_words).unwrap_or_default(),
            search_hint: "Search downloads".into(),
            ask: if desktop { "Click again to delete" } else { "Press again to delete" }.into(),
            sections,
            asking,
            empty,
        }
    }

    fn note(&self) -> Option<Note> {
        let going: Vec<&Row> = self.rows.iter().filter(|r| r.state.going()).collect();
        if going.is_empty() {
            return None;
        }
        let (videos, chapters): (Vec<&Row>, Vec<&Row>) = going.into_iter().partition(|r| r.kind == Kind::Video);
        let size = videos.iter().map(|r| r.size).sum::<u64>().max(1);
        let done: u64 = videos.iter().map(|r| r.done).sum();
        let pages: u64 = chapters.iter().map(|r| r.size).sum();
        let pages_done: u64 = chapters.iter().map(|r| r.done).sum();
        let percent = if videos.is_empty() && pages > 0 { pages_done * 100 / pages } else { done * 100 / size }.min(100) as u8;
        let title = match (videos.len(), chapters.len()) {
            (0, c) => format!("Downloading {c} chapters"),
            (1, 0) => videos[0].named(),
            (v, 0) => format!("Downloading {v} videos"),
            (v, c) => format!("Downloading {} items", v + c),
        };
        let detail = match (videos.len(), chapters.len()) {
            (_, 0) => format!("{} of {}", rules::size_words(done), rules::size_words(size)),
            (0, _) => {
                let queued = chapters.iter().filter(|r| r.state == State::Queued).count();
                format!("{pages_done} / {pages} pages · {queued} queued")
            }
            (v, c) => format!("{v} videos · {c} chapters"),
        };
        let unsure = chapters.iter().any(|r| r.size == 0) || (!videos.is_empty() && !chapters.is_empty());
        Some(Note { title, detail, percent, unsure })
    }
}

/// The files that say what's down of a download: a video's own, a chapter's mark that it's whole.
fn row_files(r: &Row) -> Vec<String> {
    match r.kind {
        Kind::Video => ["video", "sound", "vtt", "part", "sound.part", "level"].map(|e| file(&r.key, e)).to_vec(),
        Kind::Chapter => vec![format!("{}/{}", transfer::chapter_dir(&r.key), rules::COMPLETE)],
    }
}

/// The id an effect waits under, if it waits.
fn effect_id(e: &Effect) -> Option<u32> {
    match e {
        Effect::Request { id, .. } | Effect::Fetch { id, .. } | Effect::Disk { id, .. } | Effect::Timer { id, .. } => Some(*id),
        _ => None,
    }
}

/// Where a film's (`ep` None) or an episode's download comes from ([`rules::copy_of`]); None when it can't be.
pub fn copy_of(title: &WantedTitle, ep: Option<&WantedEpisode>, anime_audio: &str) -> Option<String> {
    rules::copy_of(&rules::Source {
        source: &title.source,
        title_id: &title.id,
        has_imdb: title.imdb.is_some(),
        episode_id: ep.map(|e| e.id.as_str()),
        season_id: ep.and_then(|e| e.season_id.as_deref()),
        stream: ep.and_then(|e| e.stream.as_deref()).or(title.stream.as_deref()),
        anime_audio,
    })
}

/// Whether a film (`ep` None) or an episode can be downloaded here: not on a TV, and from a source downloads take.
pub fn offered(device: Device, title: &WantedTitle, ep: Option<&WantedEpisode>, anime_audio: &str) -> bool {
    device != Device::Tv && copy_of(title, ep, anime_audio).is_some()
}

impl Screen for Queue {
    type Event = Event;
    type View = View;

    fn update(&mut self, event: Event, cx: &Context) -> Vec<Effect> {
        self.now = cx.clock.now;
        if let Event::Start = event {
            return self.start(cx);
        }
        if !self.loaded {
            if matches!(event, Event::Search { .. } | Event::Show { .. } | Event::Dismiss | Event::Run { .. }) {
                // (these need nothing kept: done now)
            } else {
                self.held.push(event);
                return vec![];
            }
        }
        match event {
            Event::Start => vec![],
            Event::Run { on } => {
                self.run = on;
                self.kick(cx)
            }
            Event::MobileData { on } => {
                self.mobile_data = on;
                let mut out = vec![Effect::WritePref { key: MOBILE_DATA.into(), value: Some(on.to_string()) }];
                out.extend(self.kick(cx));
                out
            }
            Event::NotesAsked => {
                self.noted = Some(true);
                vec![Effect::WritePref { key: ASKED_NOTES.into(), value: Some("true".into()) }]
            }
            Event::Add { title, episodes } => self.add(title, episodes, cx),
            Event::AddChapters { series, chapters } => self.add_chapters(series, chapters, cx),
            Event::Act { key, act } => self.act(&key, act, cx),
            Event::Remove { key } => self.delete_entry(key, cx),
            Event::Delete { key } => {
                if self.asking != key {
                    self.asking = key;
                    return vec![];
                }
                self.asking.clear();
                self.delete_entry(key, cx)
            }
            Event::Dismiss => {
                self.asking.clear();
                vec![]
            }
            Event::Weigh { title, episodes } => self.weigh(title, episodes, cx),
            Event::WeighChapters { series, chapters } => self.weigh_chapters(series, chapters, cx),
            Event::Look => vec![self.ask(Ask::Free, Io::Disk(DiskOp::Free))],
            Event::Spot { key, at } => {
                if self.spots.get(&key) == Some(&at) {
                    return vec![];
                }
                self.spots.insert(key, at);
                let kept: serde_json::Map<String, Value> = self.spots.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
                vec![self.ask(Ask::Quiet, Io::Disk(DiskOp::Write { path: SPOTS.into(), text: Value::Object(kept).to_string() }))]
            }
            Event::Search { query } => {
                self.query = query.trim().into();
                vec![]
            }
            Event::Show { title } => {
                self.show = title;
                self.asking.clear();
                vec![]
            }
        }
    }

    fn answer(&mut self, id: u32, reply: Reply, cx: &Context) -> Vec<Effect> {
        self.now = cx.clock.now;
        let Some(ask) = self.asks.take(id) else { return vec![] };
        match ask {
            Ask::Save => {
                self.saving = false;
                if std::mem::take(&mut self.save_again) {
                    return self.save();
                }
                vec![]
            }
            Ask::Quiet => vec![],
            Ask::Free => {
                if let Reply::Disk { disk } = reply {
                    self.shared.free = Some(disk.bytes);
                }
                vec![]
            }
            Ask::LetGo(paths) => vec![self.ask(Ask::Quiet, Io::Disk(DiskOp::Remove { paths }))],
            Ask::Retry(key, round) => {
                let still = self.jobs.get(&key).is_some_and(|r| r.round == round) && self.find(&key).is_some_and(|i| self.rows[i].state == State::Running);
                if !still {
                    return vec![];
                }
                let mut out = self.round(&key, cx);
                out.extend(self.pump());
                out
            }
            Ask::Job(key, round, tag) => self.job_answer(key, round, tag, id, reply, cx),
            Ask::Weigh(title, n, tag) => self.weigh_answer(title, n, tag, reply, cx),
            Ask::WeighChapter(series, n, step) => self.weigh_chapter_answer(series, n, step, reply, cx),
            Ask::Move(step) => {
                let disk = match reply {
                    Reply::Disk { disk } => Some(disk),
                    _ => None,
                };
                self.moving = self.moving.saturating_sub(1);
                let mut out = self.move_step(step, disk);
                out.extend(self.settle(cx));
                out
            }
            other => {
                let mut out = self.apply(other, reply, cx);
                if !self.loaded {
                    self.loading = self.loading.saturating_sub(1);
                    out.extend(self.settle(cx));
                }
                out
            }
        }
    }

    fn view(&self) -> View {
        let mine: Vec<&Row> = self.rows.iter().filter(|r| !r.deleting && rules::owned(r.profiles.as_deref(), &self.me)).collect();
        // (films and shows as they were first added, a show's episodes in order, then each manhwa's chapters)
        let listed: Vec<rules::Listed> = mine
            .iter()
            .map(|r| rules::Listed { kind: r.kind, group: &r.title_id, name: &r.title, season: r.season, episode: r.episode, chapter: &r.key })
            .collect();
        let items: Vec<Item> = rules::grouped(&listed, "").into_iter().flatten().map(|i| self.item(mine[i])).collect();
        let page = self.page(&items);
        let pending = self.rows.iter().any(|r| r.state.going());
        let queued = self.rows.iter().any(|r| r.state == State::Queued);
        let bytes_left = self
            .rows
            .iter()
            .filter(|r| r.state == State::Queued)
            .map(|r| match r.kind {
                Kind::Video => r.size.saturating_sub(r.done),
                Kind::Chapter => rules::chapter_bytes_left(r.done, r.size),
            })
            .sum();
        View {
            items,
            page,
            note: self.note(),
            told: self.told.clone(),
            pending,
            busy: self.run && (!self.loaded || !self.jobs.is_empty() || self.usable && queued),
            bytes_left,
            usable: self.usable,
            mobile_data: self.mobile_data,
            sizes: self.weighed.clone(),
            running: self.run,
            ask_notes: self.noted == Some(false),
        }
    }

    fn app(&mut self, event: &AppEvent, cx: &Context) -> Vec<Effect> {
        match event {
            AppEvent::Connectivity { .. } => self.kick(cx),
            // (what the next add goes by: told by the core, never pushed by a shell)
            AppEvent::DownloadSettings { quality, anime_audio } => {
                self.settings.quality = quality.clone();
                self.settings.anime_audio = anime_audio.clone();
                vec![]
            }
            AppEvent::Account { account } => {
                self.settings.plus = account.as_ref().is_some_and(|a| a.plus);
                // (each profile has its own downloads: another one watching shows its own)
                if let Some(id) = account.as_ref().and_then(|a| a.current.clone()).filter(|id| *id != self.me) {
                    self.me = id;
                    self.show = None;
                    self.asking.clear();
                }
                vec![]
            }
            _ => vec![],
        }
    }

    fn behind(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests;
