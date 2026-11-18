//! Downloads, kept on this PC: films and episodes (a video's files) and manhwa chapters (their pages' images). The
//! shared core's queue (`zcore::queue`) decides everything about them: which come down and when, each one's transfer,
//! what they weigh, and every word shown. This keeps its latest view for the pages, tells it what's pressed, and hands
//! the player a download's files.
use crate::core_screen::Lasting;
use crate::data::{Episode, Title};
use crate::manhwa::{Chapter, Comic};
use crate::net::{self, Json, Res};
use crate::playback::{error, Playback};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
pub use zcore::downloads::{Kind, State};
use zcore::downloads as rules;
use zcore::queue::{Event, Item, Queue, Settings, View, WantedChapter, WantedEpisode, WantedSeries, WantedTitle};

/// What was last told to the queue, so each is told only when it changes.
#[derive(Default)]
struct Told {
    profile: Option<String>,
    settings: Option<Settings>,
    show: Option<Option<String>>,
    query: Option<String>,
    /// The series last asked to be weighed, and how many chapters it had.
    weighed: Option<(String, usize)>,
}

static VIEW: Mutex<Option<Arc<View>>> = Mutex::new(None);
static TOLD: Mutex<Option<Told>> = Mutex::new(None);
/// Title pages waiting to be told what they'll weigh, by title id.
static WEIGHING: Mutex<Vec<(String, tokio::sync::oneshot::Sender<Option<u64>>)>> = Mutex::new(Vec::new());

fn queue() -> &'static Lasting<Queue> {
    static QUEUE: OnceLock<Lasting<Queue>> = OnceLock::new();
    QUEUE.get_or_init(|| {
        let queue = Lasting::new(shown);
        queue.send(Event::Start { profile: crate::profile_settings::current_id(), settings: settings() });
        queue
    })
}

/// A new view from the queue (on the core's thread): kept, the title pages waiting on a size told, and a frame asked
/// for.
fn shown(view: View) {
    {
        let mut waiting = WEIGHING.lock().unwrap();
        let mut i = 0;
        while i < waiting.len() {
            match view.sizes.iter().find(|s| s.title == waiting[i].0) {
                Some(weighed) => {
                    let bytes = weighed.bytes;
                    let _ = waiting.swap_remove(i).1.send(bytes);
                }
                None => i += 1,
            }
        }
    }
    *VIEW.lock().unwrap() = Some(Arc::new(view));
    changed();
}

fn settings() -> Settings {
    Settings {
        quality: crate::profile_settings::download_tier(),
        anime_audio: crate::profile_settings::anime_audio(),
        plus: crate::data::plus(),
    }
}

fn send(event: Event) {
    queue().send(event);
}

/// The profile watching and its settings, told to the queue when they've changed (each profile has its own downloads).
fn keep_up() {
    let me = crate::profile_settings::current_id();
    let now = settings();
    let mut told = TOLD.lock().unwrap();
    let told = told.get_or_insert_with(Told::default);
    if told.profile.as_ref() != Some(&me) {
        told.profile = Some(me.clone());
        send(Event::Profile { id: me });
    }
    if told.settings.as_ref() != Some(&now) {
        told.settings = Some(now.clone());
        send(Event::Settings { settings: now });
    }
}

/// A download's state changed: a frame for it, but one for however many come within 250 ms (a transfer's pieces can
/// land many times a second); the first at once, the last always drawn.
fn changed() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static PENDING: AtomicBool = AtomicBool::new(false);
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    if PENDING.swap(true, Ordering::AcqRel) {
        return;
    }
    let wait = LAST.lock().unwrap().map_or(Duration::ZERO, |at| Duration::from_millis(250).saturating_sub(at.elapsed()));
    crate::spawn(async move {
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        *LAST.lock().unwrap() = Some(std::time::Instant::now());
        PENDING.store(false, Ordering::Release);
        crate::ui(|_| {});
    });
}

/// Everything the queue shows: this profile's downloads, the Downloads page, the sizes title pages asked for.
pub fn view() -> Arc<View> {
    keep_up();
    VIEW.lock().unwrap().clone().unwrap_or_default()
}

/// This profile's downloads, in the order they were added.
pub fn items() -> Vec<Item> {
    view().items.clone()
}

/// This profile's download of `key`, if it has it: where it stands and how far along (0 to 1).
pub fn progress_of(key: &str) -> Option<(State, f32)> {
    view().items.iter().find(|i| i.key == key).map(|i| (i.state, i.share))
}

/// How many of a chapter's pages are down, of how many (0: not known yet), while it comes down.
pub fn pages_of(path: &str) -> (u64, u64) {
    view().items.iter().find(|i| i.key == path).map_or((0, 0), |i| (i.done, i.size))
}

/// Transfers may run (signed in), or stop.
pub fn enabled(value: bool) {
    send(Event::Run { on: value });
}

/// Paused, or carried on (paused, failed): as every download's control does it.
pub fn act(key: &str, act: rules::Act) {
    send(Event::Act { key: key.into(), act });
}

/// Gone from this profile's downloads (asked once more first, by the page).
pub fn remove(key: &str) {
    send(Event::Remove { key: key.into() });
}

/// What Downloads is searching for.
pub fn search(query: &str) {
    let mut told = TOLD.lock().unwrap();
    let told = told.get_or_insert_with(Told::default);
    if told.query.as_deref() != Some(query) {
        told.query = Some(query.into());
        send(Event::Search { query: query.into() });
    }
}

/// The show open on Downloads (its title id), or none.
pub fn show(title: Option<String>) {
    let mut told = TOLD.lock().unwrap();
    let told = told.get_or_insert_with(Told::default);
    if told.show.as_ref() != Some(&title) {
        told.show = Some(title.clone());
        send(Event::Show { title });
    }
}

/// What the player plays of a finished download: its files on this PC.
pub fn playback(title: &str, episode: Option<&str>) -> Option<Playback> {
    let view = view();
    let item = view.items.iter().find(|i| i.kind == Kind::Video && i.title_id == title && i.episode_id.as_deref() == episode && i.state == State::Done)?;
    let local = item.local.as_ref()?;
    let file = |p: &str| crate::disk::path(p).to_string_lossy().into_owned();
    Some(Playback {
        uri: file(&local.file),
        sound: local.sound.as_deref().map(file),
        captions: local.captions.as_deref().map(file),
    })
}

fn spots() -> &'static Mutex<Value> {
    static SPOTS: OnceLock<Mutex<Value>> = OnceLock::new();
    SPOTS.get_or_init(|| {
        Mutex::new(
            std::fs::read_to_string(net::root().join("offline.json"))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(json!({})),
        )
    })
}

pub fn spot(key: &str) -> f64 {
    spots().lock().unwrap().d(key)
}

pub fn remember(key: &str, time: f64) {
    let mut spots = spots().lock().unwrap();
    if spots.get(key).and_then(|v| v.as_f64()) == Some(time) {
        return;
    }
    spots[key] = json!(time);
    net::write_soon(net::root().join("offline.json"), spots.to_string());
}

/// Whether a film (`ep` None) or an episode can be downloaded (`zcore::queue::offered`).
pub fn offered(title: &Title, ep: Option<&Episode>) -> bool {
    let audio = crate::profile_settings::anime_audio();
    zcore::queue::offered(zcore::screen::Device::Desktop, &WantedTitle::of(title), ep.map(WantedEpisode::of).as_ref(), &audio)
}

pub async fn add(title: &Title, ep: Option<&Episode>) -> Res<()> {
    if !offered(title, ep) {
        return Err(error("No download is available for this title."));
    }
    keep_up();
    send(Event::Add { title: WantedTitle::of(title), episodes: ep.map(WantedEpisode::of).into_iter().collect() });
    Ok(())
}

/// About what a film (`episodes` empty) or these episodes will weigh, in the profile's download quality, as the
/// queue weighs it. None if it can't tell.
pub async fn estimate(title: &Title, episodes: &[Episode]) -> Option<u64> {
    keep_up();
    let (answer, waiting) = tokio::sync::oneshot::channel();
    WEIGHING.lock().unwrap().push((title.id.clone(), answer));
    send(Event::Weigh { title: WantedTitle::of(title), episodes: episodes.iter().map(WantedEpisode::of).collect() });
    waiting.await.ok().flatten()
}

// ---------- manhwa chapters ----------
// A chapter is its pages' images in a folder of its own (`chapter_dir`); `complete` says how many once they're all
// there, `kept` that it was downloaded on purpose.

/// About what all of a series' chapters take, as the queue weighs them (asked once per series and count). None while
/// it's weighed, or if it can't tell.
pub fn chapters_size(series: &str, chapters: &[Chapter]) -> Option<u64> {
    let asked = (series.to_string(), chapters.len());
    let ask = {
        let mut told = TOLD.lock().unwrap();
        let told = told.get_or_insert_with(Told::default);
        let ask = told.weighed.as_ref() != Some(&asked);
        told.weighed = Some(asked);
        ask
    };
    if ask {
        send(Event::WeighChapters { series: series.into(), chapters: chapters.iter().map(|c| c.path.clone()).collect() });
        return None;
    }
    view().sizes.iter().find(|s| s.title == series).and_then(|s| s.bytes)
}

/// Downloads these chapters of a series (in this profile's list too, if another saved them; one paused or failed
/// carries on).
pub fn add_chapters(comic: &Comic, chapters: &[Chapter]) {
    keep_up();
    let series = WantedSeries { path: comic.path.clone(), title: comic.title.clone(), cover: comic.small().to_string(), art: comic.cover.clone() };
    let chapters = chapters
        .iter()
        .map(|c| WantedChapter { path: c.path.clone(), label: c.label.clone(), thumb: Some(c.thumb.clone()).filter(|t| !t.is_empty()) })
        .collect();
    send(Event::AddChapters { series, chapters });
}

/// A chapter's folder: its pages, and the marks that say it's whole.
pub fn chapter_dir(path: &str) -> PathBuf {
    crate::disk::path(&zcore::queue::transfer::chapter_dir(path))
}

/// A downloaded chapter's pages on this PC, in order; None unless every one is there.
pub fn chapter_pages(path: &str) -> Option<Vec<PathBuf>> {
    let dir = chapter_dir(path);
    let count = rules::saved_pages(&std::fs::read_to_string(dir.join(rules::COMPLETE)).ok()?)?;
    let pages: Vec<PathBuf> = (0..count).map(|i| dir.join(rules::page_file(i))).collect();
    pages.iter().all(|p| p.is_file()).then_some(pages)
}
