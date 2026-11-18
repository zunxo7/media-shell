//! A Manhwa series: its cover, name and facts, Read (or Continue from the chapter you're up to), what it's about, and
//! its chapters as rows.

use super::{chapter_line, chapters_key, chapters_path, parse_chapters, read_back, read_label, reading, shared, start};
use super::{Chapter, Comic, Shared, READING_PREF};
use crate::api::{self, Kind};
use crate::screen::{AppEvent, Asks, Context, Effect, Need, Reply, Screen, Trouble};
use std::collections::HashSet;

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
    /// The series' chapters on the device, downloaded (their paths); told whenever that may have changed.
    Downloaded { paths: Vec<String> },
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
    downloaded: HashSet<String>,
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
            downloaded: HashSet::new(),
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

impl Screen for Series {
    type Event = Event;
    type View = View;

    fn update(&mut self, event: Event, cx: &Context) -> Vec<Effect> {
        match event {
            Event::Opened { comic } => {
                let same = self.comic.as_ref().is_some_and(|c| c.path == comic.path);
                let kept = self.kept.clone();
                if !same {
                    *self = Self::sharing(kept);
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
            Event::Downloaded { paths } => {
                self.downloaded = paths.into_iter().collect();
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
        let saved = chapters.iter().filter(|c| self.downloaded.contains(&c.path)).count();
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
                    Row {
                        chapter: c.clone(),
                        line: chapter_line(c, current, self.downloaded.contains(&c.path)),
                        reading: current,
                        still: if still.is_empty() { String::new() } else { api::url(&self.base, still) },
                        own_still: c.thumb.is_empty(),
                    }
                })
                .collect(),
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
    use crate::screen::{test_context, PageReply};

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
        series.update(Event::Downloaded { paths: vec!["/comics/a/chapter/1".into()] }, &cx);
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
