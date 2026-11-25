//! A Manhwa series: its cover, name and facts, Read (or Continue from the chapter you're up to), what it's about, and
//! its chapters as rows.

use super::{chapter_line, chapters_key, chapters_path, parse_chapters, read_back, read_label, reading, shared, start};
use super::{Chapter, Comic, Shared, READING_PREF};
use crate::api::{self, Kind};
use crate::downloads::{self, Act, State};
use crate::queue::{self, Snapshot, WantedChapter, WantedSeries};
use crate::screen::{AppEvent, Asks, Context, Effect, Need, Reply, Route, Screen, Trouble};

const NO_CHAPTERS: &str = "No chapters are out yet.";
const HEADING: &str = "Chapters";
const VERTICAL: &str = "Vertical scroll";
/// Between a series' facts.
const FACTS_BETWEEN: &str = "   ·   ";

/// What happens on a series' page.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The page opened on `comic`.
    Opened { comic: Comic },
    /// Try again, after its chapters couldn't load.
    Retry,
    /// What it's about pressed: all of it, or back to a few lines.
    About,
    /// Download all pressed: it does what [`DownloadAll`] says (the chapters are added, paused, carried on or tried
    /// again, or Downloads opens).
    DownloadAll,
    /// A chapter's download control pressed (its row's [`Row::act`]; a chapter that's down is read by the app).
    DownloadChapter { path: String },
}

/// A series' Download all: what it says and does for the chapters as they are.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DownloadAll {
    pub act: Act,
    pub label: String,
    /// About what it'll take, while it adds.
    pub chip: Option<String>,
}

/// Exactly what a series' page draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub title: String,
    /// Its cover, big, whole address.
    pub cover: String,
    /// The picture its page glows from: Asura's own copy of the cover (the server reads its colour, not of its own
    /// small one); None when there's none.
    pub glow: Option<String>,
    /// "120 chapters   ·   Vertical scroll   ·   3 saved".
    pub facts: String,
    /// The first button; None until the chapters are here (its skeleton).
    pub read: Option<Read>,
    pub about: Option<String>,
    /// What it's about, all of it (else a few lines).
    pub about_open: bool,
    pub heading: String,
    pub rows: Vec<Row>,
    /// Download all; None until there are chapters.
    pub download_all: Option<DownloadAll>,
    /// No chapters yet, and they're coming: the rows' skeleton.
    pub loading: bool,
    pub trouble: Option<Trouble>,
}

/// Read, or Continue: what it says and the chapter it opens.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Read {
    pub label: String,
    pub chapter: Chapter,
}

/// A chapter's row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    pub chapter: Chapter,
    /// "Reading · Start · 2 days ago".
    pub line: String,
    /// The one you're up to (its line brighter).
    pub reading: bool,
    /// Its still, whole address: a scene from it made on the server, else the series' cover. Empty when there's
    /// neither.
    pub still: String,
    /// It has no still from the server (the app may make one from its pages).
    pub own_still: bool,
    /// Where its download stands; None: not downloaded.
    pub download: Option<State>,
    /// What its control does ([`downloads::tap`]): Open means the app reads the chapter.
    pub act: Act,
    /// How far along its download is, 0 to 1.
    pub share: f32,
}

#[derive(Debug)]
enum Ask {
    Reading,
    Chapters,
}

/// A series page's model.
#[derive(Debug)]
pub struct Series {
    kept: Shared,
    base: String,
    comic: Option<Comic>,
    chapters: Vec<Chapter>,
    about: Option<String>,
    about_open: bool,
    loading: bool,
    failed: Option<(String, bool)>,
    /// This profile's downloads, as the queue last told them.
    held: Snapshot,
    /// How many chapters the series was last asked to be weighed at.
    weigh_asked: Option<usize>,
    /// What the queue is to be told.
    outbox: Vec<queue::Event>,
    asks: Asks<Ask>,
}

impl Default for Series {
    fn default() -> Self {
        Self::sharing(shared())
    }
}

impl Series {
    /// A series page reading the spots in `kept`.
    pub fn sharing(kept: Shared) -> Self {
        Self {
            kept,
            base: String::new(),
            comic: None,
            chapters: vec![],
            about: None,
            about_open: false,
            loading: false,
            failed: None,
            held: Snapshot::default(),
            weigh_asked: None,
            outbox: vec![],
            asks: Asks::default(),
        }
    }

    fn ask(&mut self, need: Need) -> Vec<Effect> {
        let Some(comic) = &self.comic else { return vec![] };
        let (path, key) = (chapters_path(&comic.path), chapters_key(&comic.path));
        self.loading = true;
        vec![Effect::Page { id: self.asks.ask(Ask::Chapters), path, key, need }]
    }
}

impl Series {
    /// Download all, by the one rule for several at once ([`downloads::batch`]): what it does, says, which of the
    /// chapters it acts on (their places), and how many there are.
    fn plan(&self) -> (Act, String, Vec<usize>, usize) {
        let items: Vec<(Option<State>, f32)> =
            self.chapters.iter().map(|c| self.held.get(&c.path).map_or((None, 0.), |h| (Some(h.state), h.share))).collect();
        let (act, label) = downloads::batch(&items, false);
        let states: Vec<Option<State>> = items.iter().map(|i| i.0).collect();
        (act, label, downloads::batch_targets(&states, act), items.len())
    }

    fn download_all(&self) -> DownloadAll {
        let (act, label, targets, of) = self.plan();
        let chip = downloads::size_chip(act, self.held.weighed(self.comic.as_ref().map_or("", |c| c.path.as_str())), targets.len(), of, false);
        DownloadAll { act, label, chip }
    }

    /// The queue told to add `chapters` of this series.
    fn add(&mut self, chapters: &[&Chapter]) {
        let Some(comic) = &self.comic else { return };
        let series = WantedSeries { path: comic.path.clone(), title: comic.title.clone(), cover: comic.small().to_string(), art: comic.cover.clone() };
        let chapters = chapters
            .iter()
            .map(|c| WantedChapter { path: c.path.clone(), label: c.label.clone(), thumb: Some(c.thumb.clone()).filter(|t| !t.is_empty()) })
            .collect();
        self.outbox.push(queue::Event::AddChapters { series, chapters });
    }

    fn press_all(&mut self) -> Vec<Effect> {
        let (act, _, targets, _) = self.plan();
        let chapters: Vec<Chapter> = targets.iter().filter_map(|i| self.chapters.get(*i).cloned()).collect();
        match act {
            Act::Open => return vec![Effect::Open { route: Route::Downloads }],
            Act::Add => self.add(&chapters.iter().collect::<Vec<_>>()),
            _ => self.outbox.extend(chapters.iter().map(|c| queue::Event::Act { key: c.path.clone(), act })),
        }
        vec![]
    }

    fn press_chapter(&mut self, path: &str) {
        let Some(chapter) = self.chapters.iter().find(|c| c.path == path).cloned() else { return };
        match downloads::tap(self.held.get(path).map(|h| h.state)) {
            Act::Add => self.add(&[&chapter]),
            Act::Open => {}
            act => self.outbox.push(queue::Event::Act { key: path.into(), act }),
        }
    }
}

impl Screen for Series {
    type Event = Event;
    type View = View;

    fn update(&mut self, event: Event, cx: &Context) -> Vec<Effect> {
        match event {
            Event::Opened { comic } => {
                let same = self.comic.as_ref().is_some_and(|c| c.path == comic.path);
                let kept = self.kept.clone();
                if !same {
                    let held = std::mem::take(&mut self.held);
                    *self = Self::sharing(kept);
                    self.held = held;
                }
                self.base = cx.base.clone();
                self.comic = Some(comic);
                let mut effects = vec![];
                if reading(&self.kept, |_| ()).is_none() {
                    effects.push(Effect::ReadPref { id: self.asks.ask(Ask::Reading), key: READING_PREF.into() });
                }
                effects.extend(self.ask(Need::Any));
                effects
            }
            Event::Retry => {
                self.failed = None;
                self.ask(Need::Server)
            }
            Event::About => {
                self.about_open = !self.about_open;
                vec![]
            }
            Event::DownloadAll => self.press_all(),
            Event::DownloadChapter { path } => {
                self.press_chapter(&path);
                vec![]
            }
        }
    }

    fn answer(&mut self, id: u32, reply: Reply, _cx: &Context) -> Vec<Effect> {
        match (self.asks.take(id), reply) {
            (Some(Ask::Reading), Reply::Pref { value }) => read_back(&self.kept, value.as_deref()),
            (Some(Ask::Chapters), Reply::Page { page }) => {
                self.loading = false;
                match page.body.as_deref().map(parse_chapters) {
                    Some(Ok(series)) => {
                        self.chapters = series.chapters;
                        self.about = series.about;
                        self.failed = None;
                        // (what they'll all take, asked of the queue once per count)
                        if let Some(comic) = self.comic.as_ref().filter(|_| !self.chapters.is_empty() && self.weigh_asked != Some(self.chapters.len())) {
                            self.weigh_asked = Some(self.chapters.len());
                            let (series, chapters) = (comic.path.clone(), self.chapters.iter().map(|c| c.path.clone()).collect());
                            self.outbox.push(queue::Event::WeighChapters { series, chapters });
                        }
                    }
                    Some(Err(_)) if self.chapters.is_empty() => self.failed = Some((NO_CHAPTERS.into(), false)),
                    Some(Err(_)) => {}
                    None => self.failed = page.failure.map(|f| (f.words, f.kind == Kind::Offline)),
                }
            }
            _ => {}
        }
        vec![]
    }

    fn downloads(&mut self, now: &Snapshot, _cx: &Context) -> Vec<Effect> {
        self.held = now.clone();
        vec![]
    }

    fn to_queue(&mut self) -> Vec<queue::Event> {
        std::mem::take(&mut self.outbox)
    }

    fn app(&mut self, event: &AppEvent, _cx: &Context) -> Vec<Effect> {
        match (event, &self.comic) {
            (AppEvent::PageChanged { key }, Some(comic)) if *key == chapters_key(&comic.path) => self.ask(Need::Any),
            // (back online with nothing to show: asked again)
            (AppEvent::Connectivity { online: true, .. }, Some(_)) if self.failed.is_some() && self.chapters.is_empty() => self.ask(Need::Any),
            _ => vec![],
        }
    }

    fn view(&self) -> View {
        let Some(comic) = &self.comic else { return View::default() };
        let last = reading(&self.kept, |r| r.last(&comic.path).map(String::from)).flatten();
        let chapters = &self.chapters;
        let kept = |path: &str| self.held.get(path).is_some_and(|d| d.state == State::Done);
        let saved = chapters.iter().filter(|c| kept(&c.path)).count();
        let facts = [
            (!chapters.is_empty()).then(|| format!("{} chapters", chapters.len())),
            Some(VERTICAL.to_string()),
            (saved > 0).then(|| format!("{saved} saved")),
        ];
        let small = comic.small();
        View {
            title: comic.title.clone(),
            cover: api::url(&self.base, &comic.large()),
            glow: (!comic.cover.is_empty()).then(|| comic.cover.clone()),
            facts: facts.into_iter().flatten().collect::<Vec<_>>().join(FACTS_BETWEEN),
            read: start(chapters, last.as_deref())
                .map(|i| Read { label: read_label(chapters, last.as_deref()), chapter: chapters[i].clone() }),
            about: self.about.clone(),
            about_open: self.about_open,
            heading: HEADING.into(),
            rows: chapters
                .iter()
                .map(|c| {
                    let current = last.as_deref() == Some(c.path.as_str());
                    let still = if c.thumb.is_empty() { small } else { c.thumb.as_str() };
                    let download = self.held.get(&c.path);
                    // (while it comes down, how the queue says it goes, as Downloads does; else the chapter's own line)
                    let line = match download {
                        Some(d) if d.state != State::Done => d.line.clone(),
                        _ => chapter_line(c, current, kept(&c.path)),
                    };
                    Row {
                        chapter: c.clone(),
                        line,
                        reading: current,
                        still: if still.is_empty() { String::new() } else { api::url(&self.base, still) },
                        own_still: c.thumb.is_empty(),
                        download: download.map(|d| d.state),
                        act: downloads::tap(download.map(|d| d.state)),
                        share: download.map_or(0., |d| d.share),
                    }
                })
                .collect(),
            download_all: (!chapters.is_empty()).then(|| self.download_all()),
            loading: chapters.is_empty() && self.loading,
            trouble: match (&self.failed, chapters.is_empty()) {
                (Some((message, offline)), true) => Some(Trouble { message: message.clone(), offline: *offline, busy: self.loading }),
                _ => None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::{Held, Weighed};
    use crate::screen::{test_context, PageReply};

    /// The queue's word of a chapter's download.
    fn held(path: &str, state: State, share: f32, line: &str) -> Held {
        Held { key: path.into(), kind: downloads::Kind::Chapter, title_id: "/comics/a".into(), state, share, line: line.into(), ..Default::default() }
    }

    fn told(series: &mut Series, items: Vec<Held>, sizes: Vec<Weighed>) {
        series.downloads(&Snapshot { items, sizes, ..Default::default() }, &test_context(1000.));
    }

    const CHAPTERS: &str = r#"{"chapters": [
        {"path": "/comics/a/chapter/1", "label": "Chapter 1", "name": "Start", "date": "2 days ago", "thumb": "/mimg/still?c=1"},
        {"path": "/comics/a/chapter/2", "label": "Chapter 2"}],
        "description": "Tom &amp; Jerry"}"#;

    fn comic() -> Comic {
        Comic { path: "/comics/a".into(), title: "A".into(), cover: "https://cdn/a.webp".into(), thumb: "/mimg/a".into(), rating: "9".into() }
    }

    fn page(body: Option<&str>, failure: Option<api::Failure>) -> Reply {
        Reply::Page { page: PageReply { body: body.map(String::from), at: 1., failure, stale: false } }
    }

    #[test]
    fn a_series_with_its_chapters_and_where_you_are() {
        let cx = test_context(1000.);
        let kept = Shared::default();
        let mut series = Series::sharing(kept.clone());
        let effects = series.update(Event::Opened { comic: comic() }, &cx);
        let [Effect::ReadPref { id: pref, .. }, Effect::Page { id, path, key, need: Need::Any }] = effects.as_slice() else { panic!("{effects:?}") };
        assert_eq!(path, "/api/manhwa/chapters?path=%2Fcomics%2Fa");
        assert_eq!(key, "manhwa-chapters-/comics/a");
        assert!(series.view().loading && series.view().read.is_none());
        series.answer(*pref, Reply::Pref { value: Some(r#"{"last": {"/comics/a": "/comics/a/chapter/2"}}"#.into()) }, &cx);
        series.answer(*id, page(Some(CHAPTERS), None), &cx);
        told(&mut series, vec![held("/comics/a/chapter/1", State::Done, 1., "Ready to read")], vec![]);
        let view = series.view();
        assert_eq!(view.cover, "https://host.invalid/mimg/a&w=720");
        assert_eq!(view.glow.as_deref(), Some("https://cdn/a.webp"));
        assert_eq!(view.facts, "2 chapters   ·   Vertical scroll   ·   1 saved");
        assert_eq!(view.read.as_ref().map(|r| (r.label.as_str(), r.chapter.label.as_str())), Some(("Continue · Ch 2", "Chapter 2")));
        assert_eq!(view.about.as_deref(), Some("Tom & Jerry"));
        assert_eq!(view.rows[0].line, "Start · 2 days ago");
        assert_eq!(view.rows[0].still, "https://host.invalid/mimg/still?c=1");
        assert_eq!((view.rows[1].line.as_str(), view.rows[1].reading, view.rows[1].own_still), ("Reading", true, true));
        assert_eq!(view.rows[1].still, "https://host.invalid/mimg/a");
        series.update(Event::About, &cx);
        assert!(series.view().about_open);
    }

    #[test]
    fn downloads_word_each_chapter_and_the_whole_series() {
        let cx = test_context(1000.);
        let mut series = Series::sharing(Shared::default());
        read_back(&series.kept, None);
        let effects = series.update(Event::Opened { comic: comic() }, &cx);
        let [Effect::Page { id, .. }] = effects.as_slice() else { panic!("{effects:?}") };
        series.answer(*id, page(Some(CHAPTERS), None), &cx);
        // (the queue is asked what they'll all weigh, once)
        assert_eq!(series.to_queue(), [queue::Event::WeighChapters { series: "/comics/a".into(), chapters: vec!["/comics/a/chapter/1".into(), "/comics/a/chapter/2".into()] }]);
        let all = series.view().download_all.expect("chapters are here");
        assert_eq!((all.act, all.label.as_str(), all.chip), (Act::Add, "Download all", None));
        told(&mut series, vec![], vec![Weighed { title: "/comics/a".into(), bytes: Some(10_000_000) }]);
        assert_eq!(series.view().download_all.and_then(|a| a.chip).as_deref(), Some("~10 MB"));
        // (pressed: both chapters are added)
        assert!(series.update(Event::DownloadAll, &cx).is_empty());
        let asked = series.to_queue();
        let [queue::Event::AddChapters { series: wanted, chapters }] = asked.as_slice() else { panic!() };
        assert_eq!((wanted.path.as_str(), wanted.cover.as_str(), chapters.len()), ("/comics/a", "/mimg/a", 2));
        assert_eq!(chapters[0].thumb.as_deref(), Some("/mimg/still?c=1"));
        let going = held("/comics/a/chapter/1", State::Running, 0.25, "3 of 12 pages");
        let done = held("/comics/a/chapter/2", State::Done, 1., "Downloaded");
        told(&mut series, vec![going, done], vec![]);
        let view = series.view();
        assert_eq!(view.rows[0].line, "3 of 12 pages");
        assert_eq!((view.rows[0].download, view.rows[0].share, view.rows[0].act), (Some(State::Running), 0.25, Act::Pause));
        assert_eq!((view.rows[1].line.as_str(), view.rows[1].download, view.rows[1].act), ("Downloaded", Some(State::Done), Act::Open));
        let all = view.download_all.expect("chapters are here");
        assert_eq!((all.act, all.label.as_str(), all.chip), (Act::Pause, "Pause · 1 of 2", None));
        assert!(view.facts.ends_with("1 saved"));
        // (Download all pauses the one that's going; a chapter's control acts as its row says; one that's down: the app reads it)
        series.update(Event::DownloadAll, &cx);
        series.update(Event::DownloadChapter { path: "/comics/a/chapter/2".into() }, &cx);
        series.update(Event::DownloadChapter { path: "/comics/a/chapter/1".into() }, &cx);
        assert_eq!(
            series.to_queue(),
            [
                queue::Event::Act { key: "/comics/a/chapter/1".into(), act: Act::Pause },
                queue::Event::Act { key: "/comics/a/chapter/1".into(), act: Act::Pause }
            ]
        );
        // (all down: it opens Downloads)
        told(&mut series, vec![held("/comics/a/chapter/1", State::Done, 1., ""), held("/comics/a/chapter/2", State::Done, 1., "")], vec![]);
        assert_eq!(series.update(Event::DownloadAll, &cx), [Effect::Open { route: Route::Downloads }]);
    }

    #[test]
    fn a_chapter_waiting_says_what_downloads_says_of_it() {
        // (on mobile data with Wi-Fi only, the queue says "Waiting for Wi-Fi": so does the series, as it is the queue's line)
        let cx = test_context(1000.);
        let mut series = Series::sharing(Shared::default());
        read_back(&series.kept, None);
        let effects = series.update(Event::Opened { comic: comic() }, &cx);
        let [Effect::Page { id, .. }] = effects.as_slice() else { panic!("{effects:?}") };
        series.answer(*id, page(Some(CHAPTERS), None), &cx);
        told(&mut series, vec![held("/comics/a/chapter/1", State::Queued, 0., "Waiting for Wi-Fi")], vec![]);
        assert_eq!(series.view().rows[0].line, "Waiting for Wi-Fi");
    }

    #[test]
    fn no_chapters_is_trouble_and_a_newer_list_is_taken() {
        let cx = test_context(1000.);
        let mut series = Series::sharing(Shared::default());
        read_back(&series.kept, None);
        let effects = series.update(Event::Opened { comic: comic() }, &cx);
        let [Effect::Page { id, .. }] = effects.as_slice() else { panic!("{effects:?}") };
        let away = api::Failure { kind: Kind::Offline, words: api::OFFLINE.into() };
        series.answer(*id, page(None, Some(away)), &cx);
        assert_eq!(series.view().trouble.map(|t| t.offline), Some(true));
        let effects = series.update(Event::Retry, &cx);
        let [Effect::Page { id, need: Need::Server, .. }] = effects.as_slice() else { panic!("{effects:?}") };
        series.answer(*id, page(Some(r#"{"chapters": []}"#), None), &cx);
        assert_eq!(series.view().trouble.map(|t| t.message), Some(NO_CHAPTERS.into()));
        let changed = series.app(&AppEvent::PageChanged { key: "manhwa-chapters-/comics/a".into() }, &cx);
        let [Effect::Page { id, .. }] = changed.as_slice() else { panic!("{changed:?}") };
        series.answer(*id, page(Some(CHAPTERS), None), &cx);
        assert_eq!(series.view().rows.len(), 2);
        assert!(series.app(&AppEvent::PageChanged { key: "home".into() }, &cx).is_empty());
    }
}
