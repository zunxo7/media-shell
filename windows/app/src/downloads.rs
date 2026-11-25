//! Downloads, kept on this PC: films and episodes (a video's files) and manhwa chapters (their pages' images). The
//! shared core's queue (`zcore::queue`) decides everything about them: which come down and when, each one's transfer,
//! what they weigh, and every word shown. This keeps its latest view for the pages, tells it what's pressed, and hands
//! the player a download's files.
use crate::core_screen::Lasting;
use crate::playback::Playback;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
pub use zcore::downloads::{Kind, State};
use zcore::downloads as rules;
use zcore::queue::{Event, Item, Queue, View};

static VIEW: Mutex<Option<Arc<View>>> = Mutex::new(None);

fn queue() -> &'static Lasting<Queue> {
    static QUEUE: OnceLock<Lasting<Queue>> = OnceLock::new();
    QUEUE.get_or_init(|| {
        let queue = Lasting::new(shown);
        queue.send(Event::Start);
        queue
    })
}

/// A new view from the queue (on the core's thread): kept, and a frame asked for.
fn shown(view: View) {
    *VIEW.lock().unwrap() = Some(Arc::new(view));
    changed();
}

fn send(event: Event) {
    queue().send(event);
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
    VIEW.lock().unwrap().clone().unwrap_or_default()
}

/// This profile's downloads, in the order they were added.
pub fn items() -> Vec<Item> {
    view().items.clone()
}

/// Transfers may run (signed in), or stop.
pub fn enabled(value: bool) {
    send(Event::Run { on: value });
}

/// Paused, or carried on (paused, failed): as every download's control does it.
pub fn act(key: &str, act: rules::Act) {
    send(Event::Act { key: key.into(), act });
}

/// Gone from this profile's downloads at once (a download's key, or `show:<title id>` for a whole show), once the page has
/// asked.
pub fn remove(key: &str) {
    send(Event::Remove { key: key.into() });
}

/// What Downloads is searching for.
pub fn search(query: &str) {
    send(Event::Search { query: query.into() });
}

/// Opened on the core as the app starts, before the session: the queue hears of the account (and so the profile) from the first.
pub fn start() {
    let _ = queue();
}

/// Opens (its title id) or closes (none) a show on Downloads.
pub fn show(title: Option<String>) {
    send(Event::Show { title });
}

/// What the player plays of a finished download: its files on this PC.
pub fn playback(title: &str, episode: Option<&str>) -> Option<Playback> {
    let view = view();
    let local = zcore::queue::playable(&view.items, title, episode)?;
    let file = |p: &str| crate::disk::path(p).to_string_lossy().into_owned();
    Some(Playback {
        uri: file(&local.file),
        sound: local.sound.as_deref().map(file),
        captions: local.captions.as_deref().map(file),
    })
}

/// Where an offline play of `key` got to, in seconds (0: not played): the core keeps it with the download.
pub fn spot(key: &str) -> f64 {
    view().items.iter().find(|i| i.key == key).and_then(|i| i.local.as_ref()).map_or(0., |l| l.spot)
}

pub fn remember(key: &str, time: f64) {
    send(Event::Spot { key: key.into(), at: time });
}

// ---------- manhwa chapters ----------
// A chapter is its pages' images in a folder of its own (`chapter_dir`); `complete` says how many once they're all
// there, `kept` that it was downloaded on purpose.

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
