//! The Manhwa reader: a chapter's pages as one strip, opening where it was left as soon as there's more than a screen
//! and a half of them from there, the rest landing under you; a page that doesn't come asked for again; where you are
//! written down; zoomed as a browser zooms; the chapter's end with the next one.
//!
//! The app loads each page the view lists in `load` (from its file on the device, else from Asura's CDN with
//! [`super::REFERER`], kept as its file) and tells its size, or that it didn't come.

use super::{chapters_key, neighbours, pages_key, pages_path, parse_chapters, parse_pages, read_back, reading, shared};
use super::{zoom_shown, zoom_step, zoomed_width, Chapter, Comic, Shared, Spot, Strip, READING_PREF, ZOOMS, ZOOM_RESET};
use crate::api::{self, Kind};
use crate::downloads::{chapter_folder, page_file, CHAPTER_NOT_OUT, PAGE_MISSING};
use crate::screen::{AppEvent, Asks, Context, Effect, Need, Reply, Screen, Trouble};

/// The app setting the reader's zoom is kept under (a step of [`ZOOMS`]), for every chapter.
pub const ZOOM_PREF: &str = "manhwaZoom";
/// Pages asked for at once, past those in.
const AT_ONCE: usize = 4;
/// Tries at a chapter before its failure is shown.
const TRIES: u32 = 3;
/// The wait before the next try, times the tries so far.
const TRY_WAIT_MS: u32 = 1_500;
/// How long where you are may go unwritten while you read.
const WRITE_MS: u32 = 1_000;

const SIGN_IN: &str = "This chapter needs a sign-in.";
const CHAPTERS: &str = "Chapters";
const NEXT: &str = "Next chapter";
const PREVIOUS: &str = "Previous";

/// The status a missed page carries when the CDN never answered it (not reached, or a file that isn't there).
pub const NOT_ANSWERED: u16 = 0;

/// What happens in the reader.
// (each event is made once and handed over the bridge, which takes no boxes)
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A chapter opened (in place of the one being read, if any). `saved`: how many pages it has whole on the device
    /// (downloaded, by its folder's `complete`), 0 when it isn't.
    Opened { series: Comic, chapter: Chapter, saved: u32 },
    /// The room for the strip: its readable `column` (100%), the `room` a zoom can widen it to, the `screen`'s
    /// height (all in points), and pixels to a point.
    Layout { column: f32, room: f32, screen: f32, scale: f32 },
    /// A page the view asked for in `round` is in: its size in pixels (0 when it isn't a picture).
    Measured { round: u32, index: u32, width: u32, height: u32 },
    /// A page the view asked for in `round` didn't come: the CDN's status (0 when it wasn't reached), and whether the
    /// device is offline.
    Missed { round: u32, index: u32, status: u16, offline: bool },
    /// The strip scrolled: `pos` points down it at the screen's top.
    Scrolled { pos: f32 },
    /// The scroll the view asked for (its `seq`) is done.
    Went { seq: u32 },
    /// A zoom step in or out (None: back to 100%), keeping the point `at` points down the screen where it is.
    Zoom { inward: Option<bool>, at: f32 },
    /// Try again, after it couldn't load.
    Retry,
    /// The reader is going: where you are is written down now.
    Left,
}

/// Exactly what the reader draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// The series' name and the chapter's, for the bar.
    pub series: String,
    pub chapter: String,
    /// Enough is in to read: the strip shows.
    pub open: bool,
    /// Opening, how far: "3 / 42 pages", once it's known.
    pub progress: Option<String>,
    /// Why it couldn't open, with Try again.
    pub trouble: Option<Trouble>,
    /// The strip's width as asked (its zoom's), and as laid out on whole pixels, in points; and its height.
    pub column: f32,
    pub width: f32,
    pub height: f32,
    /// The pages in so far, in order, and how many the chapter has (0 until that's known).
    pub pages: Vec<Page>,
    pub total: u32,
    /// The pages to load now, for the round they're asked in.
    pub load: Vec<Load>,
    pub round: u32,
    /// The chapter's folder on the device (its pages' files are in it).
    pub folder: String,
    /// The page you're on: "3 / 42".
    pub counter: Option<String>,
    /// The rest is coming: a spinner under the pages.
    pub coming: bool,
    /// The rest didn't come: why, with Try again, under the pages.
    pub stopped: Option<Trouble>,
    /// Every page is in: the chapter's end.
    pub end: Option<End>,
    /// The chapters before and after this one.
    pub before: Option<Chapter>,
    pub after: Option<Chapter>,
    pub previous_label: String,
    pub next_label: String,
    pub zoom: Zoom,
    /// Scroll the strip to here once it's laid out that far (or every page is in), then send [`Event::Went`].
    pub go_to: Option<GoTo>,
}

/// A page in the strip.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    /// Its file in the chapter's folder; and where it is on Asura's CDN, None when it's only on the device.
    pub file: String,
    pub url: Option<String>,
    /// Its size in pixels.
    pub width: u32,
    pub height: u32,
    /// Where it starts down the strip, and how tall it is there, in points (on whole pixels).
    pub top: f32,
    pub tall: f32,
}

/// A page for the app to load.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Load {
    pub index: u32,
    pub file: String,
    pub url: Option<String>,
}

/// The chapter's end.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct End {
    /// "End of chapter 12".
    pub title: String,
    /// Back to the chapters.
    pub chapters: String,
}

/// The zoom in the bar, as a browser's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Zoom {
    /// "125%".
    pub shown: String,
    pub can_in: bool,
    pub can_out: bool,
}

/// A scroll the reader asks for.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GoTo {
    pub pos: f32,
    pub seq: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    column: f32,
    room: f32,
    screen: f32,
    scale: f32,
}

#[derive(Debug)]
enum Ask {
    Reading,
    Zoom,
    Chapters,
    Pages,
    Again,
    Write,
}

/// The reader's model.
#[derive(Debug)]
pub struct Reader {
    kept: Shared,
    series: Comic,
    chapter: Chapter,
    chapters: Vec<Chapter>,
    /// Each page's address (None: only on the device) and size, once known.
    urls: Vec<Option<String>>,
    sizes: Vec<Option<(u32, u32)>>,
    /// The chapter's first round: what's measured from it on is this chapter's.
    first_round: u32,
    round: u32,
    tries: u32,
    /// Waiting to try again.
    waiting: bool,
    failed: Option<(String, bool)>,
    start: Option<Spot>,
    open: bool,
    layout: Option<Layout>,
    zoom: Option<u32>,
    strip: Strip,
    pos: f32,
    go_to: Option<GoTo>,
    seq: u32,
    /// Where you are is to be written down, and the timer for it.
    unwritten: bool,
    writing: Option<u32>,
    asks: Asks<Ask>,
}

impl Default for Reader {
    fn default() -> Self {
        Self::sharing(shared())
    }
}

impl Reader {
    /// A reader keeping its spots in `kept`.
    pub fn sharing(kept: Shared) -> Self {
        Self {
            kept,
            series: Comic::default(),
            chapter: Chapter::default(),
            chapters: vec![],
            urls: vec![],
            sizes: vec![],
            first_round: 0,
            round: 0,
            tries: 0,
            waiting: false,
            failed: None,
            start: None,
            open: false,
            layout: None,
            zoom: None,
            strip: Strip::default(),
            pos: 0.,
            go_to: None,
            seq: 0,
            unwritten: false,
            writing: None,
            asks: Asks::default(),
        }
    }

    /// How many pages from the first are in.
    fn landed(&self) -> usize {
        self.sizes.iter().take_while(|s| s.is_some()).count()
    }

    fn complete(&self) -> bool {
        !self.sizes.is_empty() && self.landed() == self.sizes.len()
    }

    fn zoom(&self) -> u32 {
        self.zoom.unwrap_or(ZOOM_RESET)
    }

    /// The column at the zoom kept (and the room for it).
    fn widths(&self) -> Option<(f32, f32)> {
        self.layout.map(|l| (l.column, l.room))
    }

    fn relayout(&mut self) {
        let Some(layout) = self.layout else { return };
        let sizes: Vec<(u32, u32)> = self.sizes.iter().map_while(|s| *s).collect();
        self.strip = Strip::new(&sizes, zoomed_width(layout.column, self.zoom(), layout.room), layout.scale);
    }

    fn ask_pages(&mut self, need: Need) -> Effect {
        Effect::Page { id: self.asks.ask(Ask::Pages), path: pages_path(&self.chapter.path), key: pages_key(&self.chapter.path), need }
    }

    /// Opens once enough is in to read from where it was left (or all of it is), scrolled there.
    fn settle(&mut self) {
        self.relayout();
        if self.open {
            return;
        }
        let (Some(start), Some(layout)) = (self.start, self.layout) else { return };
        if !self.complete() && !self.strip.fills_from(start.page, layout.screen) {
            return;
        }
        self.open = true;
        if start != Spot::default() {
            self.go(self.strip.pos_of(start));
        }
    }

    fn go(&mut self, pos: f32) {
        self.seq += 1;
        self.go_to = Some(GoTo { pos, seq: self.seq });
    }

    /// A try failed: another after a wait, or, the tries spent, why.
    fn failed(&mut self, words: String, offline: bool) -> Vec<Effect> {
        if self.waiting || self.failed.is_some() {
            return vec![];
        }
        self.tries += 1;
        if self.tries < TRIES {
            self.waiting = true;
            vec![Effect::Timer { id: self.asks.ask(Ask::Again), ms: TRY_WAIT_MS * self.tries }]
        } else {
            self.failed = Some((words, offline));
            vec![]
        }
    }

    fn again(&mut self, need: Need) -> Vec<Effect> {
        self.waiting = false;
        self.round += 1;
        if self.sizes.is_empty() {
            vec![self.ask_pages(need)]
        } else {
            vec![]
        }
    }

    /// Where you are written down, and the other pages told.
    fn write(&mut self) -> Vec<Effect> {
        self.unwritten = false;
        let mut effects: Vec<Effect> = self.writing.take().map(|id| {
            self.asks.take(id);
            Effect::Cancel { id }
        }).into_iter().collect();
        if let Some(json) = reading(&self.kept, |r| r.to_json()) {
            effects.push(Effect::WritePref { key: READING_PREF.into(), value: Some(json) });
            effects.push(Effect::Tell { event: AppEvent::PageChanged { key: READING_PREF.into() } });
        }
        effects
    }

    fn opened(&mut self, series: Comic, chapter: Chapter, saved: u32) -> Vec<Effect> {
        let mut effects = if self.unwritten { self.write() } else { vec![] };
        let same_series = self.series.path == series.path;
        let round = self.round + 1;
        let kept = self.kept.clone();
        let old = std::mem::replace(self, Self::sharing(kept));
        self.layout = old.layout;
        self.zoom = old.zoom;
        self.seq = old.seq;
        self.asks = old.asks;
        self.asks.forget();
        if same_series {
            self.chapters = old.chapters;
        }
        (self.first_round, self.round) = (round, round);
        self.series = series;
        self.chapter = chapter;
        self.start = reading(&self.kept, |r| r.spot(&self.chapter.path));
        if self.start.is_none() {
            effects.push(Effect::ReadPref { id: self.asks.ask(Ask::Reading), key: READING_PREF.into() });
        }
        if self.zoom.is_none() {
            effects.push(Effect::ReadPref { id: self.asks.ask(Ask::Zoom), key: ZOOM_PREF.into() });
        }
        let id = self.asks.ask(Ask::Chapters);
        effects.push(Effect::Page { id, path: super::chapters_path(&self.series.path), key: chapters_key(&self.series.path), need: Need::Any });
        if saved > 0 {
            self.urls = vec![None; saved as usize];
            self.sizes = vec![None; saved as usize];
        } else {
            effects.push(self.ask_pages(Need::Any));
        }
        effects
    }

    fn scrolled(&mut self, pos: f32) -> Vec<Effect> {
        self.pos = pos;
        if !self.open || self.go_to.is_some() {
            return vec![];
        }
        let spot = self.strip.spot_at(pos);
        let (series, chapter) = (&self.series.path, &self.chapter.path);
        if !reading(&self.kept, |r| r.save(series, chapter, spot)).unwrap_or(false) {
            return vec![];
        }
        self.unwritten = true;
        if self.writing.is_some() {
            return vec![];
        }
        let id = self.asks.ask(Ask::Write);
        self.writing = Some(id);
        vec![Effect::Timer { id, ms: WRITE_MS }]
    }

    fn zoomed(&mut self, inward: Option<bool>, at: f32) -> Vec<Effect> {
        let Some((column, room)) = self.widths().filter(|_| self.open) else { return vec![] };
        let to = match inward {
            Some(inward) => match zoom_step(self.zoom(), inward, column, room) {
                Some(z) => z,
                None => return vec![],
            },
            None => ZOOM_RESET,
        };
        let spot = self.strip.spot_at(self.pos + at);
        self.zoom = Some(to);
        self.relayout();
        self.go(self.strip.pos_of(spot) - at);
        vec![Effect::WritePref { key: ZOOM_PREF.into(), value: Some(to.to_string()) }]
    }
}

impl Screen for Reader {
    type Event = Event;
    type View = View;

    fn update(&mut self, event: Event, _cx: &Context) -> Vec<Effect> {
        match event {
            Event::Opened { series, chapter, saved } => return self.opened(series, chapter, saved),
            Event::Layout { column, room, screen, scale } => {
                let layout = Some(Layout { column, room, screen, scale });
                if layout != self.layout {
                    self.layout = layout;
                    self.settle();
                }
            }
            Event::Measured { round, index, width, height } if round >= self.first_round => {
                let Some(size) = self.sizes.get_mut(index as usize) else { return vec![] };
                if width == 0 || height == 0 {
                    return self.failed(PAGE_MISSING.into(), false);
                }
                *size = Some((width, height));
                self.settle();
            }
            Event::Missed { round, index, status, offline } if round >= self.first_round && (index as usize) < self.sizes.len() => {
                let words = match status {
                    _ if offline => api::OFFLINE,
                    401 | 403 => SIGN_IN,
                    _ => PAGE_MISSING,
                };
                return self.failed(words.into(), offline);
            }
            Event::Measured { .. } | Event::Missed { .. } => {}
            Event::Scrolled { pos } => return self.scrolled(pos),
            Event::Went { seq } => {
                if self.go_to.is_some_and(|g| g.seq == seq) {
                    self.go_to = None;
                }
            }
            Event::Zoom { inward, at } => return self.zoomed(inward, at),
            Event::Retry => {
                self.tries = 0;
                self.failed = None;
                return self.again(Need::Server);
            }
            Event::Left if self.unwritten => return self.write(),
            Event::Left => {}
        }
        vec![]
    }

    fn answer(&mut self, id: u32, reply: Reply, _cx: &Context) -> Vec<Effect> {
        match (self.asks.take(id), reply) {
            (Some(Ask::Reading), Reply::Pref { value }) => {
                read_back(&self.kept, value.as_deref());
                self.start = reading(&self.kept, |r| r.spot(&self.chapter.path));
                self.settle();
            }
            (Some(Ask::Zoom), Reply::Pref { value }) => {
                let zoom = value.and_then(|v| v.trim().parse().ok()).filter(|z| ZOOMS.contains(z));
                self.zoom = Some(zoom.unwrap_or(ZOOM_RESET));
                self.settle();
            }
            (Some(Ask::Chapters), Reply::Page { page }) => {
                if let Some(Ok(series)) = page.body.as_deref().map(parse_chapters) {
                    self.chapters = series.chapters;
                }
            }
            (Some(Ask::Pages), Reply::Page { page }) if self.sizes.is_empty() => {
                match page.body.as_deref().map(parse_pages) {
                    Some(Ok(urls)) => {
                        self.sizes = vec![None; urls.len()];
                        self.urls = urls.into_iter().map(Some).collect();
                        self.settle();
                    }
                    Some(Err(_)) => return self.failed(CHAPTER_NOT_OUT.into(), false),
                    None => {
                        let (words, offline) = page.failure.map_or((api::UNREACHABLE.into(), false), |f| (f.words, f.kind == Kind::Offline));
                        return self.failed(words, offline);
                    }
                }
            }
            (Some(Ask::Again), Reply::Fired) => return self.again(Need::Any),
            (Some(Ask::Write), Reply::Fired) => {
                self.writing = None;
                return self.write();
            }
            _ => {}
        }
        vec![]
    }

    fn app(&mut self, event: &AppEvent, cx: &Context) -> Vec<Effect> {
        match event {
            AppEvent::PageChanged { key } if !self.series.path.is_empty() && *key == chapters_key(&self.series.path) => {
                let id = self.asks.ask(Ask::Chapters);
                vec![Effect::Page { id, path: super::chapters_path(&self.series.path), key: key.clone(), need: Need::Any }]
            }
            // (back online after it gave up: tried again)
            AppEvent::Connectivity { online: true, .. } if self.failed.is_some() => self.update(Event::Retry, cx),
            _ => vec![],
        }
    }

    fn view(&self) -> View {
        let total = self.sizes.len();
        let landed = self.landed();
        let complete = self.complete();
        let trouble = self.failed.as_ref().map(|(message, offline)| Trouble { message: message.clone(), offline: *offline, busy: false });
        let load = if self.waiting || self.failed.is_some() {
            vec![]
        } else {
            (landed..total.min(landed + AT_ONCE))
                .filter(|i| self.sizes[*i].is_none())
                .map(|i| Load { index: i as u32, file: page_file(i), url: self.urls[i].clone() })
                .collect()
        };
        let pages = if self.open {
            (0..self.strip.len())
                .map(|i| {
                    let (top, tall) = self.strip.page(i);
                    let (width, height) = self.sizes[i].unwrap_or_default();
                    Page { file: page_file(i), url: self.urls[i].clone(), width, height, top, tall }
                })
                .collect()
        } else {
            vec![]
        };
        let (before, after) = neighbours(&self.chapters, &self.chapter.path);
        let zoom = match self.widths() {
            Some((column, room)) => Zoom {
                shown: format!("{}%", zoom_shown(column, self.zoom(), room)),
                can_in: zoom_step(self.zoom(), true, column, room).is_some(),
                can_out: zoom_step(self.zoom(), false, column, room).is_some(),
            },
            None => Zoom::default(),
        };
        View {
            series: self.series.title.clone(),
            chapter: self.chapter.label.clone(),
            open: self.open,
            progress: (!self.open && total > 0).then(|| format!("{landed} / {total} pages")),
            trouble: if self.open { None } else { trouble.clone() },
            column: self.layout.map_or(0., |l| zoomed_width(l.column, self.zoom(), l.room)),
            width: self.strip.width(),
            height: self.strip.height(),
            pages,
            total: total as u32,
            load,
            round: self.round,
            folder: chapter_folder(&self.chapter.path),
            counter: (self.open && total > 0).then(|| format!("{} / {total}", (self.strip.spot_at(self.pos).page + 1).min(total))),
            coming: self.open && !complete && trouble.is_none(),
            stopped: if self.open && !complete { trouble } else { None },
            end: (self.open && complete).then(|| End { title: format!("End of {}", self.chapter.label.to_lowercase()), chapters: CHAPTERS.into() }),
            before: before.map(|i| self.chapters[i].clone()),
            after: after.map(|i| self.chapters[i].clone()),
            previous_label: PREVIOUS.into(),
            next_label: NEXT.into(),
            zoom,
            go_to: self.go_to,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{test_context, PageReply};

    const PAGES: &str = r#"{"images": ["https://cdn.asurascans.com/asura-images/chapters/a/1.webp",
                                       "https://cdn.asurascans.com/asura-images/chapters/a/2.webp",
                                       "https://cdn.asurascans.com/asura-images/chapters/a/3.webp",
                                       "https://cdn.asurascans.com/asura-images/chapters/a/4.webp",
                                       "https://cdn.asurascans.com/asura-images/chapters/a/5.webp"]}"#;

    fn series() -> Comic {
        Comic { path: "/comics/a".into(), title: "A".into(), ..Default::default() }
    }

    fn chapter(n: u32) -> Chapter {
        Chapter { path: format!("/comics/a/chapter/{n}"), label: format!("Chapter {n}"), ..Default::default() }
    }

    fn page(body: Option<&str>, failure: Option<api::Failure>) -> Reply {
        Reply::Page { page: PageReply { body: body.map(String::from), at: 1., failure, stale: false } }
    }

    fn id_of(effects: &[Effect], want: impl Fn(&Effect) -> bool) -> u32 {
        let found = effects.iter().find(|e| want(e)).and_then(|e| match e {
            Effect::Page { id, .. } | Effect::ReadPref { id, .. } | Effect::Timer { id, .. } => Some(*id),
            _ => None,
        });
        found.unwrap_or_else(|| panic!("not in {effects:?}"))
    }

    /// A reader opened on chapter 2 with its spot and zoom read, its chapters and pages in, on a 1000-point screen.
    fn reading_chapter_two(spots: &str) -> (Reader, Shared) {
        let cx = test_context(1000.);
        let kept = Shared::default();
        let mut reader = Reader::sharing(kept.clone());
        reader.update(Event::Layout { column: 800., room: 1500., screen: 1000., scale: 1. }, &cx);
        let effects = reader.update(Event::Opened { series: series(), chapter: chapter(2), saved: 0 }, &cx);
        let spot = id_of(&effects, |e| matches!(e, Effect::ReadPref { key, .. } if key == READING_PREF));
        let zoom = id_of(&effects, |e| matches!(e, Effect::ReadPref { key, .. } if key == ZOOM_PREF));
        let chapters = id_of(&effects, |e| matches!(e, Effect::Page { key, .. } if key == "manhwa-chapters-/comics/a"));
        let pages = id_of(&effects, |e| matches!(e, Effect::Page { path, .. } if path.starts_with("/api/manhwa/chapter?")));
        reader.answer(spot, Reply::Pref { value: Some(spots.into()) }, &cx);
        reader.answer(zoom, Reply::Pref { value: None }, &cx);
        let list = r#"{"chapters": [{"path": "/comics/a/chapter/1", "label": "Chapter 1"}, {"path": "/comics/a/chapter/2", "label": "Chapter 2"}]}"#;
        reader.answer(chapters, page(Some(list), None), &cx);
        reader.answer(pages, page(Some(PAGES), None), &cx);
        (reader, kept)
    }

    #[test]
    fn it_opens_once_a_screen_and_a_half_is_in_and_ends() {
        let cx = test_context(1000.);
        let (mut reader, _) = reading_chapter_two("");
        let view = reader.view();
        assert!(!view.open);
        assert_eq!(view.progress.as_deref(), Some("0 / 5 pages"));
        assert_eq!(view.load.iter().map(|l| l.index).collect::<Vec<_>>(), vec![0, 1, 2, 3]);
        assert_eq!(view.load[0].file, "0.img");
        assert!(view.load[0].url.as_deref().is_some_and(|u| u.ends_with("/1.webp")));
        let round = view.round;
        // (an 800-point column: a page 800 x 1000 is 1000 tall; the second makes it 2000, past 1.5 screens)
        reader.update(Event::Measured { round, index: 1, width: 800, height: 1000 }, &cx);
        reader.update(Event::Measured { round, index: 0, width: 800, height: 1000 }, &cx);
        let view = reader.view();
        assert!(view.open && view.coming);
        assert_eq!(view.go_to, None);
        assert_eq!(view.pages.len(), 2);
        assert_eq!((view.pages[1].top, view.pages[1].tall), (1000., 1000.));
        assert_eq!(view.load.iter().map(|l| l.index).collect::<Vec<_>>(), vec![2, 3, 4]);
        assert_eq!(view.counter.as_deref(), Some("1 / 5"));
        assert_eq!(view.zoom.shown, "100%");
        assert_eq!(view.before.map(|c| c.label), Some("Chapter 1".into()));
        assert_eq!(view.after, None);
        for index in 2..5 {
            reader.update(Event::Measured { round, index, width: 800, height: 1000 }, &cx);
        }
        let view = reader.view();
        assert_eq!(view.end.map(|e| e.title), Some("End of chapter 2".into()));
        assert!(!view.coming && view.load.is_empty());
        assert_eq!(view.height, 5000.);
    }

    #[test]
    fn it_opens_where_it_was_left_and_writes_down_where_you_are() {
        let cx = test_context(1000.);
        let (mut reader, kept) = reading_chapter_two(r#"{"spots": {"/comics/a/chapter/2": [2, 0.5]}}"#);
        let round = reader.view().round;
        for index in 0..4 {
            reader.update(Event::Measured { round, index, width: 800, height: 1000 }, &cx);
        }
        // (from page 3: 2,000 points are in, past 1.5 screens)
        let view = reader.view();
        let go = view.go_to.expect("a scroll to where it was");
        assert_eq!(go.pos, 2500.);
        // (scrolling before it's there writes nothing down)
        assert!(reader.update(Event::Scrolled { pos: 0. }, &cx).is_empty());
        reader.update(Event::Went { seq: go.seq }, &cx);
        let timer = match reader.update(Event::Scrolled { pos: 3100. }, &cx).as_slice() {
            [Effect::Timer { id, ms: WRITE_MS }] => *id,
            other => panic!("{other:?}"),
        };
        assert!(reader.update(Event::Scrolled { pos: 3200. }, &cx).is_empty());
        assert_eq!(reader.view().counter.as_deref(), Some("4 / 5"));
        let written = reader.answer(timer, Reply::Fired, &cx);
        let [Effect::WritePref { key, value: Some(json) }, Effect::Tell { .. }] = written.as_slice() else { panic!("{written:?}") };
        assert_eq!(key, READING_PREF);
        let back = super::super::Reading::from_json(json);
        assert_eq!(back.last("/comics/a"), Some("/comics/a/chapter/2"));
        assert_eq!(back.spot("/comics/a/chapter/2"), Spot { page: 3, share: 0.2 });
        assert_eq!(reading(&kept, |r| r.spot("/comics/a/chapter/2")), Some(Spot { page: 3, share: 0.2 }));
        // (leaving with nothing new: nothing to write)
        assert!(reader.update(Event::Left, &cx).is_empty());
        reader.update(Event::Scrolled { pos: 100. }, &cx);
        assert!(matches!(reader.update(Event::Left, &cx).as_slice(), [Effect::Cancel { .. }, Effect::WritePref { .. }, Effect::Tell { .. }]));
    }

    #[test]
    fn a_page_that_doesnt_come_is_tried_again_then_said() {
        let cx = test_context(1000.);
        let (mut reader, _) = reading_chapter_two("");
        let round = reader.view().round;
        let wait = id_of(&reader.update(Event::Missed { round, index: 0, status: 503, offline: false }, &cx), |_| true);
        assert!(reader.view().load.is_empty());
        // (another page missing in the same try: one wait)
        assert!(reader.update(Event::Missed { round, index: 1, status: 503, offline: false }, &cx).is_empty());
        reader.answer(wait, Reply::Fired, &cx);
        let round = reader.view().round;
        assert_eq!(reader.view().load.len(), 4);
        let wait = id_of(&reader.update(Event::Missed { round, index: 0, status: 403, offline: false }, &cx), |e| matches!(e, Effect::Timer { ms: 3000, .. }));
        reader.answer(wait, Reply::Fired, &cx);
        let round = reader.view().round;
        reader.update(Event::Missed { round, index: 0, status: 403, offline: false }, &cx);
        let view = reader.view();
        assert_eq!(view.trouble.map(|t| t.message), Some(SIGN_IN.into()));
        assert!(view.load.is_empty());
        reader.update(Event::Retry, &cx);
        let view = reader.view();
        assert_eq!(view.trouble, None);
        assert!(view.round > round && view.load.len() == 4);
    }

    #[test]
    fn no_pages_list_is_tried_again_and_a_downloaded_chapter_needs_none() {
        let cx = test_context(1000.);
        let kept = Shared::default();
        read_back(&kept, None);
        let mut reader = Reader::sharing(kept);
        let effects = reader.update(Event::Opened { series: series(), chapter: chapter(1), saved: 0 }, &cx);
        let pages = id_of(&effects, |e| matches!(e, Effect::Page { path, .. } if path.starts_with("/api/manhwa/chapter?")));
        let wait = id_of(&reader.answer(pages, page(Some(r#"{"images": []}"#), None), &cx), |_| true);
        let again = reader.answer(wait, Reply::Fired, &cx);
        assert!(matches!(again.as_slice(), [Effect::Page { .. }]));
        // (a downloaded one: its pages from its files, nothing asked of the server for them)
        let effects = reader.update(Event::Opened { series: series(), chapter: chapter(2), saved: 3 }, &cx);
        assert!(!effects.iter().any(|e| matches!(e, Effect::Page { path, .. } if path.starts_with("/api/manhwa/chapter?"))));
        let view = reader.view();
        assert_eq!(view.load.len(), 3);
        assert_eq!(view.load[2], Load { index: 2, file: "2.img".into(), url: None });
        assert_eq!(view.folder, crate::downloads::chapter_folder("/comics/a/chapter/2"));
        // (the last chapter's answers are dropped)
        assert!(reader.update(Event::Measured { round: 1, index: 0, width: 1, height: 1 }, &cx).is_empty());
        assert_eq!(reader.view().load.len(), 3);
    }

    #[test]
    fn zoom_keeps_the_point_where_it_is() {
        let cx = test_context(1000.);
        let (mut reader, _) = reading_chapter_two("");
        let round = reader.view().round;
        for index in 0..5 {
            reader.update(Event::Measured { round, index, width: 800, height: 1000 }, &cx);
        }
        reader.update(Event::Scrolled { pos: 1000. }, &cx);
        let effects = reader.update(Event::Zoom { inward: Some(true), at: 500. }, &cx);
        assert_eq!(effects, vec![Effect::WritePref { key: ZOOM_PREF.into(), value: Some("110".into()) }]);
        let view = reader.view();
        assert_eq!(view.zoom.shown, "110%");
        assert_eq!(view.width, 880.);
        // (1,500 down the strip was halfway down page 2; at 110% that's 1,650, kept 500 down the screen)
        assert_eq!(view.go_to.map(|g| g.pos), Some(1150.));
        reader.update(Event::Zoom { inward: None, at: 0. }, &cx);
        assert_eq!(reader.view().zoom.shown, "100%");
    }
}
