//! Manhwa, from Asura through the server: what its answers mean, where you are in each series, and how a chapter's
//! pages stack into one strip to scroll. The server's shared Asura+ session unlocks every chapter; the pages
//! themselves come straight from Asura's CDN to the device.

use crate::model::{s, t};
use crate::sections::encode;
use crate::Error;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

pub mod reader;
pub mod series;
pub mod shelf;
pub mod still;

/// Asura's CDN serves a chapter's pages only to a request that says it came from Asura's site.
pub const REFERER: &str = "https://asurascans.com/";
/// The browser a page's request says it's from.
pub const PAGE_AGENT: &str = crate::queue::probe::YOUTUBE_AGENT;

/// A request for one of a chapter's pages on Asura's CDN, with `extra` headers (a Range).
pub fn page_request(url: &str, extra: Vec<crate::screen::Header>) -> crate::screen::Request {
    use crate::screen::Header;
    let mut headers = vec![Header::new("User-Agent", PAGE_AGENT), Header::new("Referer", REFERER)];
    headers.extend(extra);
    crate::screen::Request { headers, ..crate::queue::probe::source_request(url, None, vec![]) }
}

/// Where a chapter's pages may come from: Asura's own chapter images, nothing else the server's answer might carry.
const PAGE_PREFIXES: [&str; 3] = [
    "https://cdn.asurascans.com/asura-images/chapters/",
    "https://cdn.asurascans.com/asura-images/chapters-merged/",
    // (old chapters Asura put back up, e.g. Nano Machine's first ones)
    "https://cdn.asurascans.com/asura-images/chapters-restored/",
];

/// The shelves, as every section's chips: (key, name). The first three are sorts of everything, the rest genres.
pub const SHELVES: [(&str, &str); 11] = [
    ("popular", "All"),
    ("latest", "Latest"),
    ("rating", "Top rated"),
    ("action", "Action"),
    ("fantasy", "Fantasy"),
    ("regression", "Regression"),
    ("murim", "Murim"),
    ("isekai", "Isekai"),
    ("romance", "Romance"),
    ("comedy", "Comedy"),
    ("revenge", "Revenge"),
];

/// A series. `thumb`: its cover made small on the server (~60 KB, not ~900 KB).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Comic {
    pub path: String,
    pub title: String,
    pub cover: String,
    pub thumb: String,
    /// Out of ten, "8.5"; empty when Asura gives none.
    pub rating: String,
}

impl Comic {
    /// The cover for a card.
    pub fn small(&self) -> &str {
        if self.thumb.is_empty() {
            &self.cover
        } else {
            &self.thumb
        }
    }

    /// The cover for the series' own page, where it's shown bigger.
    pub fn large(&self) -> String {
        if self.thumb.is_empty() {
            self.cover.clone()
        } else {
            format!("{}&w=720", self.thumb)
        }
    }
}

/// A chapter: its label ("Chapter 12"), its own name, when it came out as Asura words it ("2 days ago"), and a still
/// from it made on the server.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chapter {
    pub path: String,
    pub label: String,
    pub name: String,
    pub date: String,
    pub thumb: String,
}

/// A page of the catalogue, and whether there's another after it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shelf {
    pub items: Vec<Comic>,
    pub more: bool,
}

/// A series' chapters, oldest first, and what it's about.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Series {
    pub chapters: Vec<Chapter>,
    pub about: Option<String>,
}

/// What a shelf asks the catalogue for: (genre, sort). A sort shelf is every genre; a genre shelf is its popular ones.
pub fn shelf_query(key: &str) -> (&str, &str) {
    if matches!(key, "popular" | "latest" | "rating") {
        ("", key)
    } else {
        (key, "popular")
    }
}

/// The catalogue's address: a shelf's page, or what's called `query`.
pub fn catalog_path(page: u32, query: &str, genre: &str, sort: &str) -> String {
    format!("/api/manhwa?page={page}&q={}&genre={}&sort={}", encode(query.trim()), encode(genre), encode(sort))
}

/// A series' chapters' address.
pub fn chapters_path(series: &str) -> String {
    format!("/api/manhwa/chapters?path={}", encode(series))
}

/// A chapter's pages' address.
pub fn pages_path(chapter: &str) -> String {
    format!("/api/manhwa/chapter?path={}", encode(chapter))
}

/// Where a shelf's first page is kept on the device.
pub fn shelf_key(genre: &str, sort: &str) -> String {
    format!("manhwa-shelf-{genre}-{sort}")
}

/// Where a series' chapters are kept on the device.
pub fn chapters_key(series: &str) -> String {
    format!("manhwa-chapters-{series}")
}

/// Where a chapter's pages' list is kept on the device (its pages too, by the app, as it reads them).
pub fn pages_key(chapter: &str) -> String {
    format!("manhwa-pages-{chapter}")
}

/// A catalogue page from the server. Searching (`query`), there's never a next page.
///
/// # Errors
/// [`Error::BadAnswer`] when it isn't JSON, or a shelf's first page came back empty (Asura was down).
pub fn parse_catalog(body: &str, page: u32, query: &str, genre: &str) -> Result<Shelf, Error> {
    let o: Value = serde_json::from_str(body)?;
    let items: Vec<Comic> = objects(&o, "items")
        .map(|c| Comic { path: t(c, "path"), title: t(c, "title"), cover: t(c, "cover"), thumb: t(c, "thumb"), rating: t(c, "rating") })
        .filter(|c| !c.path.is_empty())
        .collect();
    let searching = !query.trim().is_empty();
    if !searching && page == 1 && genre.is_empty() && items.is_empty() {
        return Err(Error::BadAnswer("the shelf came back empty".into()));
    }
    let more = !searching && o.get("hasMore").and_then(Value::as_bool).unwrap_or(false);
    Ok(Shelf { items, more })
}

/// A series' chapters from the server, and what it's about (its HTML entities read).
///
/// # Errors
/// [`Error::BadAnswer`] when it isn't JSON or lists no chapters.
pub fn parse_chapters(body: &str) -> Result<Series, Error> {
    let o: Value = serde_json::from_str(body)?;
    let chapters: Vec<Chapter> = objects(&o, "chapters")
        .map(|c| Chapter { path: t(c, "path"), label: t(c, "label"), name: t(c, "name"), date: t(c, "date"), thumb: t(c, "thumb") })
        .filter(|c| !c.path.is_empty())
        .collect();
    if chapters.is_empty() {
        return Err(Error::BadAnswer("no chapters".into()));
    }
    let about = s(&o, "description").map(|d| unescape(&unescape(&d)).trim().to_string()).filter(|d| !d.is_empty());
    Ok(Series { chapters, about })
}

/// A chapter's pages, in order, once each: only Asura's own chapter images.
///
/// # Errors
/// [`Error::BadAnswer`] when it isn't JSON or has no pages.
pub fn parse_pages(body: &str) -> Result<Vec<String>, Error> {
    let o: Value = serde_json::from_str(body)?;
    let mut pages: Vec<String> = Vec::new();
    for url in o.get("images").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        if is_page(url) && !pages.iter().any(|p| p == url) {
            pages.push(url.to_string());
        }
    }
    if pages.is_empty() {
        return Err(Error::BadAnswer("no pages from Asura".into()));
    }
    Ok(pages)
}

/// One of a chapter's pages on Asura's CDN (it wants [`REFERER`]).
pub fn is_page(url: &str) -> bool {
    PAGE_PREFIXES.iter().any(|p| url.starts_with(p))
}

/// "Chapter 12" as a button says it: "Ch 12".
pub fn short_label(label: &str) -> String {
    match label.get(..7) {
        Some(head) if head.eq_ignore_ascii_case("chapter") => format!("Ch {}", label[7..].trim_start()),
        _ => label.to_string(),
    }
}

/// A series card's line: the chapter you're up to ("Continue · Ch 12", from its path), else its rating ("★ 8.5").
pub fn card_line(last: Option<&str>, rating: &str) -> Option<String> {
    if let Some(chapter) = last {
        return Some(format!("Continue · Ch {}", chapter.rsplit('/').next().unwrap_or(chapter)));
    }
    rating.parse::<f32>().ok().filter(|r| *r > 0.).map(|r| format!("★ {r:.1}"))
}

/// Where Read starts: the chapter you're up to, else the first.
pub fn start(chapters: &[Chapter], last: Option<&str>) -> Option<usize> {
    if chapters.is_empty() {
        return None;
    }
    Some(last.and_then(|l| chapters.iter().position(|c| c.path == l)).unwrap_or(0))
}

/// What the series' first button says: "Continue · Ch 12" once you've started, else "Read".
pub fn read_label(chapters: &[Chapter], last: Option<&str>) -> String {
    match last.and_then(|l| chapters.iter().find(|c| c.path == l)) {
        Some(c) => format!("Continue · {}", short_label(&c.label)),
        None => "Read".into(),
    }
}

/// The chapters before and after `path`, if it's one of them.
pub fn neighbours(chapters: &[Chapter], path: &str) -> (Option<usize>, Option<usize>) {
    match chapters.iter().position(|c| c.path == path) {
        Some(at) => (at.checked_sub(1), (at + 1 < chapters.len()).then_some(at + 1)),
        None => (None, None),
    }
}

/// A chapter row's line: "Reading" on the one you're up to, its name, when it came out; with none of those,
/// "Downloaded" for one kept on the device (`saved`), else "Read online".
pub fn chapter_line(chapter: &Chapter, reading: bool, saved: bool) -> String {
    let parts: Vec<&str> =
        [reading.then_some("Reading"), Some(chapter.name.as_str()), Some(chapter.date.as_str())].into_iter().flatten().filter(|p| !p.is_empty()).collect();
    match (parts.is_empty(), saved) {
        (false, _) => parts.join(" · "),
        (true, true) => "Downloaded".into(),
        (true, false) => "Read online".into(),
    }
}

/// Where you are in a chapter: the page at the top of the screen, and how far down it (0 to 1). A share, not points,
/// so it holds when the window, and so the strip, is another width.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spot {
    pub page: usize,
    pub share: f32,
}

/// Where you are in every series and chapter, on this device: a series' last chapter, a chapter's spot. The app
/// keeps it as the text `to_json` gives.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    last: HashMap<String, String>,
    spots: HashMap<String, Spot>,
}

impl Reading {
    /// What `to_json` wrote; anything unreadable is a fresh start.
    pub fn from_json(text: &str) -> Reading {
        let o: Value = serde_json::from_str(text).unwrap_or(Value::Null);
        let last = o
            .get("last")
            .and_then(Value::as_object)
            .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect())
            .unwrap_or_default();
        let spots = o
            .get("spots")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| {
                        let page = usize::try_from(v.get(0)?.as_u64()?).ok()?;
                        let share = v.get(1)?.as_f64()? as f32;
                        Some((k.clone(), Spot { page, share: share.clamp(0., 1.) }))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Reading { last, spots }
    }

    pub fn to_json(&self) -> String {
        let spots: Map<String, Value> = self.spots.iter().map(|(k, s)| (k.clone(), json!([s.page, s.share]))).collect();
        json!({ "last": self.last, "spots": spots }).to_string()
    }

    /// The chapter last read in a series (its path).
    pub fn last(&self, series: &str) -> Option<&str> {
        self.last.get(series).map(String::as_str)
    }

    /// Where a chapter was left; its top if it never was.
    pub fn spot(&self, chapter: &str) -> Spot {
        self.spots.get(chapter).copied().unwrap_or_default()
    }

    /// You're at `spot` in `chapter` of `series`. True when that changed anything (worth writing down).
    pub fn save(&mut self, series: &str, chapter: &str, spot: Spot) -> bool {
        let changed = self.last(series) != Some(chapter) || self.spots.get(chapter) != Some(&spot);
        self.last.insert(series.to_string(), chapter.to_string());
        self.spots.insert(chapter.to_string(), spot);
        changed
    }
}

/// A chapter's pages one under another, each as tall as it is at the strip's width, laid out in the screen's own
/// pixels: every page's and every piece's top and height a whole number of them, from one running total, so
/// neighbours share an edge exactly.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Strip {
    /// Pixels to a point.
    scale: f64,
    /// The pages' sizes in their files (width, height), and their tops and heights here, in pixels.
    sizes: Vec<(u32, u32)>,
    tops: Vec<u64>,
    heights: Vec<u64>,
    width: u64,
}

/// A piece of a page drawn as one picture (a page can be taller than the GPU takes in one).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    /// Where it starts down the strip, and how much of the strip it takes, in points (whole pixels).
    pub top: f32,
    pub height: f32,
    /// How tall its picture is drawn, in points: [`BLEED`] more than it takes, under the next piece.
    pub drawn: f32,
    /// The rows of the page's file it shows: (first, how many).
    pub rows: (u32, u32),
}

/// Pixels a piece's picture reaches under the next one. Mid-glide the strip sits between pixels, where the compositor
/// draws a picture's edge soft; two soft edges meeting let the background through as a thin line (a seam). With this,
/// the next piece's soft top edge has the right picture under it instead.
pub const BLEED: u64 = 1;

impl Strip {
    /// The pages, by their sizes (width, height in pixels), `width` points wide on a screen with `scale` pixels to a
    /// point.
    pub fn new(sizes: &[(u32, u32)], width: f32, scale: f32) -> Strip {
        let scale = f64::from(scale.max(f32::EPSILON));
        let width = (f64::from(width.max(0.)) * scale).round() as u64;
        let heights: Vec<u64> = sizes.iter().map(|&(w, h)| (width as f64 * f64::from(h) / f64::from(w.max(1))).round() as u64).collect();
        let mut tops = Vec::with_capacity(heights.len());
        let mut y = 0;
        for h in &heights {
            tops.push(y);
            y += h;
        }
        Strip { scale, sizes: sizes.to_vec(), tops, heights, width }
    }

    fn points(&self, px: u64) -> f32 {
        (px as f64 / self.scale) as f32
    }

    pub fn len(&self) -> usize {
        self.heights.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heights.is_empty()
    }

    /// How wide it is, in points (a whole number of pixels).
    pub fn width(&self) -> f32 {
        self.points(self.width)
    }

    /// Where page `i` starts, and how tall it is, in points.
    pub fn page(&self, i: usize) -> (f32, f32) {
        match (self.tops.get(i), self.heights.get(i)) {
            (Some(&top), Some(&h)) => (self.points(top), self.points(h)),
            _ => (self.height(), 0.),
        }
    }

    pub fn height(&self) -> f32 {
        self.points(self.tops.last().zip(self.heights.last()).map_or(0, |(t, h)| t + h))
    }

    /// Page `i` cut into pieces no taller than `most` points, on whole pixels: each piece's place, and the rows of the
    /// page's file it shows (its [`BLEED`] too, where the page has rows left).
    pub fn pieces(&self, i: usize, most: f32) -> Vec<Piece> {
        let (Some(&top), Some(&tall), Some(&(_, src_h))) = (self.tops.get(i), self.heights.get(i), self.sizes.get(i)) else { return vec![] };
        if tall == 0 {
            return vec![];
        }
        let most = ((f64::from(most) * self.scale).floor() as u64).max(1);
        let n = tall.div_ceil(most);
        let row = |px: u64| ((px.min(tall) as f64 / tall as f64 * f64::from(src_h)).round() as u32).min(src_h);
        (0..n)
            .map(|j| {
                let (from, to) = (tall * j / n, tall * (j + 1) / n);
                let (first, last) = (row(from), row(to + BLEED));
                Piece {
                    top: self.points(top + from),
                    height: self.points(to - from),
                    drawn: self.points(to - from + BLEED),
                    rows: (first, last.saturating_sub(first).max(1)),
                }
            })
            .collect()
    }

    /// The spot at `pos` down the strip.
    pub fn spot_at(&self, pos: f32) -> Spot {
        if self.is_empty() {
            return Spot::default();
        }
        let px = f64::from(pos) * self.scale;
        let page = self.tops.partition_point(|top| *top as f64 <= px).saturating_sub(1);
        let (top, h) = (self.tops[page] as f64, self.heights[page] as f64);
        Spot { page, share: if h > 0. { ((px - top) / h).clamp(0., 1.) as f32 } else { 0. } }
    }

    /// How far down the strip `spot` is (its page's start past the end: the end).
    pub fn pos_of(&self, spot: Spot) -> f32 {
        let (top, h) = self.page(spot.page);
        top + h * spot.share
    }

    /// Enough has come to open the chapter at page `start`: more than a screen and a half (`screen` tall) of pages
    /// from there. A short first page, a banner, used to open it on a strip with nothing under it.
    pub fn fills_from(&self, start: usize, screen: f32) -> bool {
        self.len() > start && self.height() - self.page(start).0 >= screen * 1.5
    }
}

/// The reader's zoom steps, in percent of its readable column: a browser's (Chrome's), from half to three times.
pub const ZOOMS: [u32; 13] = [50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300];
/// The zoom a reader starts at, and goes back to (Ctrl+0).
pub const ZOOM_RESET: u32 = 100;

/// The strip's width at `zoom` percent of its readable `column`: no wider than the `room` the window has.
pub fn zoomed_width(column: f32, zoom: u32, room: f32) -> f32 {
    (column * zoom as f32 / 100.).min(room).max(1.)
}

/// How much the strip is zoomed as it shows (the window may hold it narrower than its step): percent of the column.
pub fn zoom_shown(column: f32, zoom: u32, room: f32) -> u32 {
    (zoomed_width(column, zoom, room) / column.max(1.) * 100.).round() as u32
}

/// The next step in (or out) from how it shows now; None when there's none (in: the strip already fills the window).
pub fn zoom_step(zoom: u32, inward: bool, column: f32, room: f32) -> Option<u32> {
    let shown = zoom_shown(column, zoom, room);
    if inward {
        if zoomed_width(column, zoom, room) >= room {
            return None;
        }
        ZOOMS.iter().copied().find(|z| *z > shown)
    } else {
        ZOOMS.iter().rev().copied().find(|z| *z < shown)
    }
}

/// The app setting the reading spots are kept under, as [`Reading::to_json`] writes them.
pub const READING_PREF: &str = "manhwaReading";

/// The reading spots as every Manhwa screen shares them, read from [`READING_PREF`] once (None until then). Only the
/// reader changes them; it tells the others with [`crate::screen::AppEvent::PageChanged`] under [`READING_PREF`].
pub type Shared = Arc<Mutex<Option<Reading>>>;

/// The app's one copy of the reading spots.
pub(crate) fn shared() -> Shared {
    static READING: OnceLock<Shared> = OnceLock::new();
    Arc::clone(READING.get_or_init(Shared::default))
}

/// What `f` says of the reading spots, once they've been read.
pub(crate) fn reading<T>(kept: &Shared, f: impl FnOnce(&mut Reading) -> T) -> Option<T> {
    kept.lock().unwrap_or_else(PoisonError::into_inner).as_mut().map(f)
}

/// The reading spots as the setting had them (None: never written), unless they've been read already.
pub(crate) fn read_back(kept: &Shared, value: Option<&str>) {
    let mut kept = kept.lock().unwrap_or_else(PoisonError::into_inner);
    if kept.is_none() {
        *kept = Some(Reading::from_json(value.unwrap_or_default()));
    }
}

/// The reading spots as the very first Android app kept them (its "manhwa-reader" settings, each value as text: a
/// series' last chapter under the series' path, a chapter's page under "<chapter>:page"), in [`Reading::to_json`]'s
/// form, each chapter at its page's top (the old offsets were one screen's pixels).
pub fn moved_reading(entries: &[(String, String)]) -> String {
    let mut reading = Reading::default();
    for (key, value) in entries {
        match key.strip_suffix(":page") {
            Some(chapter) => {
                if let Ok(page) = value.trim().parse::<usize>() {
                    reading.spots.insert(chapter.to_string(), Spot { page, share: 0. });
                }
            }
            None if !key.contains(':') => {
                reading.last.insert(key.clone(), value.clone());
            }
            None => {}
        }
    }
    reading.to_json()
}

/// The five HTML entities Asura's descriptions carry, read once (`&amp;` last, so `&amp;lt;` stays `&lt;`).
fn unescape(text: &str) -> String {
    text.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&")
}

fn objects<'a>(o: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    o.get(key).and_then(Value::as_array).into_iter().flatten().filter(|v| v.is_object())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chapter(n: u32) -> Chapter {
        Chapter { path: format!("/comics/a/chapter/{n}"), label: format!("Chapter {n}"), ..Default::default() }
    }

    #[test]
    fn shelves_ask_by_sort_or_genre() {
        assert_eq!(shelf_query("latest"), ("", "latest"));
        assert_eq!(shelf_query("murim"), ("murim", "popular"));
        assert_eq!(catalog_path(2, " solo leveling ", "", "popular"), "/api/manhwa?page=2&q=solo+leveling&genre=&sort=popular");
        assert_eq!(chapters_path("/comics/a-1"), "/api/manhwa/chapters?path=%2Fcomics%2Fa-1");
        assert_eq!(pages_path("/comics/a/chapter/1.5"), "/api/manhwa/chapter?path=%2Fcomics%2Fa%2Fchapter%2F1.5");
    }

    #[test]
    fn catalogue_pages() {
        let page = parse_catalog(
            r#"{"items": [{"path": "/comics/a", "title": "A", "cover": "https://cdn/a.webp", "thumb": "/mimg/cover?u=x&k=y", "rating": "9.1"},
                          {"title": "no path"}, "junk"], "hasMore": true}"#,
            1,
            "",
            "",
        );
        let page = page.expect("a shelf");
        assert_eq!(page.items.len(), 1);
        assert!(page.more);
        assert_eq!(page.items[0].small(), "/mimg/cover?u=x&k=y");
        assert_eq!(page.items[0].large(), "/mimg/cover?u=x&k=y&w=720");
        let bare = Comic { cover: "https://cdn/a.webp".into(), ..Default::default() };
        assert_eq!((bare.small(), bare.large().as_str()), ("https://cdn/a.webp", "https://cdn/a.webp"));
    }

    #[test]
    fn searches_never_have_more_and_an_empty_shelf_is_an_error() {
        let found = parse_catalog(r#"{"items": [], "hasMore": true}"#, 1, "zzz", "").expect("a search");
        assert_eq!(found, Shelf::default());
        assert!(matches!(parse_catalog(r#"{"items": []}"#, 1, "", ""), Err(Error::BadAnswer(_))));
        assert!(parse_catalog(r#"{"items": []}"#, 1, "", "murim").is_ok());
        assert!(parse_catalog(r#"{"items": []}"#, 2, "", "").is_ok());
    }

    #[test]
    fn chapters_and_their_story() {
        let series = parse_chapters(
            r#"{"chapters": [{"path": "/comics/a/chapter/1", "label": "Chapter 1", "name": "Start", "date": "2 days ago"}],
                "description": " Tom &amp;amp; Jerry &lt;3 "}"#,
        )
        .expect("a series");
        assert_eq!(series.chapters[0].name, "Start");
        assert_eq!(series.about.as_deref(), Some("Tom & Jerry <3"));
        assert!(matches!(parse_chapters(r#"{"chapters": []}"#), Err(Error::BadAnswer(_))));
        assert_eq!(parse_chapters(r#"{"chapters": [{"path": "/c"}], "description": " "}"#).map(|s| s.about), Ok(None));
    }

    #[test]
    fn only_asuras_chapter_images_once_each() {
        let pages = parse_pages(
            r#"{"images": ["https://cdn.asurascans.com/asura-images/chapters/a/1.webp",
                           "https://cdn.asurascans.com/asura-images/chapters/a/1.webp",
                           "https://cdn.asurascans.com/asura-images/chapters-restored/a/2.webp",
                           "http://cdn.asurascans.com/asura-images/chapters/a/3.webp",
                           "https://cdn.asurascans.com.evil.io/asura-images/chapters/a/4.webp",
                           "https://cdn.asurascans.com/asura-images/covers/a.webp"]}"#,
        )
        .expect("pages");
        assert_eq!(pages.len(), 2);
        assert!(pages[1].contains("chapters-restored"));
        assert!(matches!(parse_pages(r#"{"images": ["https://elsewhere/1.jpg"]}"#), Err(Error::BadAnswer(_))));
    }

    #[test]
    fn words_for_where_you_are() {
        let chapters = vec![chapter(1), chapter(2), chapter(3)];
        assert_eq!(short_label("Chapter 12"), "Ch 12");
        assert_eq!(short_label("Prologue"), "Prologue");
        assert_eq!(card_line(Some("/comics/a/chapter/12.5"), "9.1").as_deref(), Some("Continue · Ch 12.5"));
        assert_eq!(card_line(None, "9.14").as_deref(), Some("★ 9.1"));
        assert_eq!(card_line(None, ""), None);
        assert_eq!(read_label(&chapters, Some("/comics/a/chapter/2")), "Continue · Ch 2");
        assert_eq!(read_label(&chapters, Some("/comics/gone/chapter/9")), "Read");
        assert_eq!(start(&chapters, Some("/comics/a/chapter/3")), Some(2));
        assert_eq!(start(&chapters, None), Some(0));
        assert_eq!(start(&[], None), None);
        assert_eq!(neighbours(&chapters, "/comics/a/chapter/1"), (None, Some(1)));
        assert_eq!(neighbours(&chapters, "/comics/a/chapter/3"), (Some(1), None));
        assert_eq!(neighbours(&chapters, "/nowhere"), (None, None));
        let named = Chapter { name: "Start".into(), date: "2 days ago".into(), ..chapter(1) };
        assert_eq!(chapter_line(&named, true, false), "Reading · Start · 2 days ago");
        assert_eq!(chapter_line(&chapter(1), false, false), "Read online");
        assert_eq!(chapter_line(&chapter(1), false, true), "Downloaded");
    }

    #[test]
    fn reading_is_kept_and_read_back() {
        let mut reading = Reading::from_json("not json");
        assert_eq!(reading.last("/comics/a"), None);
        assert_eq!(reading.spot("/comics/a/chapter/1"), Spot::default());
        assert!(reading.save("/comics/a", "/comics/a/chapter/1", Spot { page: 3, share: 0.5 }));
        assert!(!reading.save("/comics/a", "/comics/a/chapter/1", Spot { page: 3, share: 0.5 }));
        let back = Reading::from_json(&reading.to_json());
        assert_eq!(back, reading);
        assert_eq!(back.last("/comics/a"), Some("/comics/a/chapter/1"));
        assert_eq!(back.spot("/comics/a/chapter/1"), Spot { page: 3, share: 0.5 });
    }

    #[test]
    fn the_first_apps_spots_move_over() {
        let entries = [
            ("/comics/a".to_string(), "/comics/a/chapter/2".to_string()),
            ("/comics/a/chapter/2:page".to_string(), "7".to_string()),
            ("/comics/a/chapter/3:offset".to_string(), "120".to_string()),
        ];
        let moved = Reading::from_json(&moved_reading(&entries));
        assert_eq!(moved.last("/comics/a"), Some("/comics/a/chapter/2"));
        assert_eq!(moved.spot("/comics/a/chapter/2"), Spot { page: 7, share: 0. });
        assert_eq!(moved.spot("/comics/a/chapter/3"), Spot::default());
        assert_eq!(Reading::from_json(&moved_reading(&[])), Reading::default());
    }

    #[test]
    fn a_strip_of_pages() {
        // two pages at 400 wide: 800 x 1600 (800 tall), 400 x 200 (200 tall)
        let strip = Strip::new(&[(800, 1600), (400, 200)], 400., 1.);
        assert_eq!(strip.height(), 1000.);
        assert_eq!(strip.page(1), (800., 200.));
        assert_eq!(strip.spot_at(0.), Spot { page: 0, share: 0. });
        assert_eq!(strip.spot_at(400.), Spot { page: 0, share: 0.5 });
        assert_eq!(strip.spot_at(900.), Spot { page: 1, share: 0.5 });
        assert_eq!(strip.spot_at(5000.), Spot { page: 1, share: 1. });
        assert_eq!(strip.pos_of(Spot { page: 1, share: 0.5 }), 900.);
        assert_eq!(strip.pos_of(Spot { page: 7, share: 0.5 }), 1000.);
        // the same spot at another width: the same place in the chapter
        let wider = Strip::new(&[(800, 1600), (400, 200)], 800., 1.25);
        assert_eq!(wider.spot_at(wider.pos_of(strip.spot_at(900.))), Spot { page: 1, share: 0.5 });
        assert!(strip.fills_from(0, 600.));
        assert!(!strip.fills_from(1, 600.));
        assert!(!strip.fills_from(2, 1.));
        assert_eq!(Strip::default().spot_at(10.), Spot::default());
    }

    #[test]
    fn zoom_steps_as_a_browser() {
        // an 800-point column in a 1,500-point window: in up to 175% (1,400), then once more to fill it (1,500)
        assert_eq!(zoom_step(100, true, 800., 1500.), Some(110));
        assert_eq!(zoom_step(175, true, 800., 1500.), Some(200));
        assert_eq!(zoomed_width(800., 200, 1500.), 1500.);
        assert_eq!(zoom_shown(800., 200, 1500.), 188);
        assert_eq!(zoom_step(200, true, 800., 1500.), None);
        // out from how it shows: 188% steps to 175%
        assert_eq!(zoom_step(200, false, 800., 1500.), Some(175));
        assert_eq!(zoom_step(50, false, 800., 1500.), None);
        assert_eq!(zoom_step(300, true, 800., 4000.), None);
        assert_eq!(zoom_step(ZOOM_RESET, false, 800., 1500.), Some(90));
        // (a window narrower than the column: it fits the window, and shows as less)
        assert_eq!(zoom_shown(800., 100, 600.), 75);
        assert_eq!(zoom_step(100, false, 800., 600.), Some(67));
    }

    /// Every page and piece on whole pixels, meeting edge to edge, at the scales Windows offers.
    #[test]
    fn pieces_meet_on_whole_pixels() {
        // a merged chapter's tall page, an odd one and a banner
        let sizes = [(800, 20_011), (720, 1_333), (1080, 97)];
        for scale in [1., 1.25, 1.5, 1.75] {
            for width in [720., 733.3, 1001.7] {
                let strip = Strip::new(&sizes, width, scale);
                let px = |points: f32| f64::from(points) * f64::from(scale);
                let whole = |points: f32| (px(points) - px(points).round()).abs() < 0.02;
                assert!(whole(strip.width()), "width {width} at {scale}");
                let mut y = 0.;
                for (i, &(_, src_h)) in sizes.iter().enumerate() {
                    let (top, tall) = strip.page(i);
                    assert!(whole(top) && whole(tall));
                    assert!((px(top) - y).abs() < 1e-2, "page {i} starts where the last ended");
                    let pieces = strip.pieces(i, 1200.);
                    let mut at = px(top);
                    for p in &pieces {
                        assert!(whole(p.top) && whole(p.height) && whole(p.drawn));
                        assert!((px(p.top) - at).abs() < 1e-2, "a piece starts where the last ended");
                        assert!(px(p.height) <= 1200. * f64::from(scale) + 0.02);
                        assert!((px(p.drawn) - px(p.height) - 1.).abs() < 0.02, "drawn a pixel further");
                        at = px(p.top) + px(p.height);
                    }
                    assert!((at - px(top + tall)).abs() < 1e-2, "the pieces fill the page");
                    // (the rows run on from piece to piece, the last to the page's foot)
                    for pair in pieces.windows(2) {
                        assert!(pair[1].rows.0 <= pair[0].rows.0 + pair[0].rows.1);
                    }
                    let last = pieces.last().expect("a piece");
                    assert_eq!(last.rows.0 + last.rows.1, src_h);
                    y = px(top + tall);
                }
            }
        }
        // (125%: a 733.3-point strip is 917 pixels; 20,011 rows at that width, 22,938 pixels in 16 pieces of 1,500 or less)
        let strip = Strip::new(&sizes, 733.3, 1.25);
        assert_eq!((f64::from(strip.width()) * 1.25).round(), 917.);
        assert_eq!((f64::from(strip.page(0).1) * 1.25).round(), 22_938.);
        assert_eq!(strip.pieces(0, 1200.).len(), 16);
        assert!(strip.pieces(9, 1200.).is_empty());
    }
}
