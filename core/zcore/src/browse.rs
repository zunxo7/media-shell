//! The sections (Dramas, TV Shows, Movies, Anime) and search, as screens: a section's chips and its titles, a page at a
//! time as its end comes near; search as you type, what's known here first and then the server's, ranked one way;
//! a section's own search kept to that section.
//!
//! ```
//! use zcore::browse::{Search, SearchEvent};
//! use zcore::root::Root;
//! use zcore::screen::{Clock, Device, Effect, Platform};
//!
//! let platform = Platform { device: Device::Desktop, model: "PC".into(), version_name: "1.0".into(), version_code: 1,
//!                           os: "t".into(), stream: "s".into(), codecs: vec![], hdr: false };
//! let clock = Clock { now: 1_794_657_600., utc_offset: 0 };
//! let mut root = Root::new(platform, "https://host.invalid".into(), clock);
//! let page = root.open(Search::default());
//! root.update::<Search>(page, SearchEvent::Opened, clock);
//! root.update::<Search>(page, SearchEvent::Query { text: "d".into() }, clock);
//! assert!(root.view::<Search>(page).is_some_and(|v| v.note == "Popular right now"));
//! ```

use crate::cards::{self, Card, Marks};
use crate::model::{self, Item};
use crate::screen::{AppEvent, Asks, Context, Effect, Empty, Glyph, Need, Reply, Screen, Trouble};
use crate::search::{fold, rank};
use crate::sections::{self, Load};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// How long typing must stop before the server's asked (ms).
pub const DEBOUNCE_MS: u32 = 150;
/// The fewest letters searched for; under it, what's popular.
pub const LETTERS: usize = 2;
/// The most titles "Popular right now" shows.
/// A section asks for its next page when less than this many screens are left below.
const NEAR_END: f32 = 1.6;
const POPULAR: usize = 30;

const HOME_PATH: &str = "/api/home";
const ANIME_PATH: &str = "/api/anime";
const ANIME_KEY: &str = "anime";

/// What a search field says before anything's typed: a section's, or search everywhere's (None).
pub fn hint(scope: Option<&str>) -> &'static str {
    match scope {
        Some("movies") => "Search movies",
        Some("shows") => "Search TV shows",
        Some("anime") => "Search anime",
        Some("dramas") => "Search dramas",
        _ => "Movies, shows, dramas, anime",
    }
}

/// A search as it shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchView {
    pub query: String,
    pub hint: String,
    /// "Popular right now", "Searching…", "12 results".
    pub note: String,
    pub cards: Vec<Card>,
    /// Still looking: the field's spinner, and the grid's skeleton while there's nothing yet.
    pub busy: bool,
    /// Two letters or more typed: the results show (a section's own titles, under it).
    pub typed: bool,
    /// Nothing found.
    pub empty: Option<Empty>,
    /// Goes up when a new search's results first show: the grid goes back to its top.
    pub fresh: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Slot {
    Suggest,
    Movies,
    Shows,
}

/// Search as you type, everywhere or in a section (`scope`): what's known on this device first (Home's titles and the
/// anime list, folded once), then [`DEBOUNCE_MS`] after the typing stops, the server's suggestions and its full lists of
/// films and shows, dealt in turn; all of it ranked the same way.
#[derive(Debug, Default)]
struct Finder {
    scope: Option<String>,
    query: String,
    /// (folded title, folded genres, the card): what's known here.
    index: Vec<(String, String, Item)>,
    /// A section's own first list: its popular titles.
    popular: Vec<Item>,
    hits: Vec<Item>,
    busy: bool,
    found: HashMap<Slot, Vec<Item>>,
    timer: Option<u32>,
    asking: Vec<(u32, Slot)>,
    fresh: u32,
}

/// What a finder's effects were asked for, inside the screen that holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FindAsk {
    Wait,
    Ask(Slot),
}

impl Finder {
    fn new(scope: Option<&str>) -> Self {
        Self { scope: scope.map(String::from), ..Self::default() }
    }

    fn fits(&self, item: &Item) -> bool {
        sections::in_scope(item, self.scope.as_deref())
    }

    /// What's known here, from Home's and the anime list's bodies.
    fn index(&mut self, home: Option<&str>, anime: Option<&str>) {
        let mut all: Vec<Item> = vec![];
        if let Some(o) = home.and_then(|b| serde_json::from_str::<Value>(b).ok()) {
            all.extend(crate::home::parse(&o).everything().cloned());
        }
        if let Some(o) = anime.and_then(|b| serde_json::from_str::<Value>(b).ok()) {
            all.extend(model::items(&o, "items"));
        }
        let mut seen = HashSet::new();
        all.retain(|i| seen.insert(i.id.clone()));
        self.index = all.into_iter().map(|i| (fold(&i.title), fold(&i.genres.join(" ")), i)).collect();
    }

    fn short(&self) -> bool {
        fold(self.query.trim()).chars().count() < LETTERS
    }

    fn popular(&self) -> Vec<Item> {
        if self.scope.is_some() {
            let mut list = self.popular.clone();
            list.sort_by_key(|i| !i.own_art());
            list.truncate(POPULAR);
            list
        } else {
            self.index.iter().map(|e| &e.2).filter(|i| self.fits(i) && i.own_art()).take(POPULAR).cloned().collect()
        }
    }

    fn ranked(&self, list: Vec<Item>) -> Vec<Item> {
        let q = fold(self.query.trim());
        let words: Vec<&str> = q.split_whitespace().collect();
        let mut list = list;
        // (stable: what the server sent that doesn't match here stays after, in the order it came)
        list.sort_by_cached_key(|i| rank(&fold(&i.title), &fold(&i.genres.join(" ")), &q, &words));
        list
    }

    /// What shows now: local hits first, then each answer's, once each, ranked.
    fn merge(&mut self) {
        if self.short() {
            self.hits = self.popular();
            return;
        }
        let q = fold(self.query.trim());
        let words: Vec<&str> = q.split_whitespace().collect();
        let mut local: Vec<(u8, usize, &Item)> = self
            .index
            .iter()
            .filter(|e| self.fits(&e.2))
            .map(|e| (rank(&e.0, &e.1, &q, &words), e.0.len(), &e.2))
            .filter(|(r, ..)| *r < 4)
            .collect();
        local.sort_by_key(|(r, len, _)| (*r, *len));
        let mut all: Vec<Item> = local.into_iter().map(|(.., i)| i.clone()).collect();
        if let Some(s) = self.found.get(&Slot::Suggest) {
            all.extend(s.iter().filter(|i| self.fits(i)).cloned());
        }
        if let (Some(m), Some(s)) = (self.found.get(&Slot::Movies), self.found.get(&Slot::Shows)) {
            all.extend(sections::dealt(&[m.clone(), s.clone()]).into_iter().filter(|i| self.fits(i)));
        }
        let mut seen = HashSet::new();
        all.retain(|i| seen.insert(i.id.clone()));
        self.hits = self.ranked(all);
    }

    /// Typed: what's known shows at once; the server's asked once the typing stops.
    fn query(&mut self, text: String, asks: &mut Asks<Ask>) -> Vec<Effect> {
        let changed = fold(text.trim()) != fold(self.query.trim());
        self.query = text;
        if !changed {
            return vec![];
        }
        let mut effects: Vec<Effect> = self
            .timer
            .take()
            .into_iter()
            .chain(self.asking.drain(..).map(|(id, _)| id))
            .map(|id| Effect::Cancel { id })
            .collect();
        self.found.clear();
        self.fresh = self.fresh.wrapping_add(1);
        self.busy = !self.short();
        self.merge();
        if self.busy {
            let id = asks.ask(Ask::Find(FindAsk::Wait));
            self.timer = Some(id);
            effects.push(Effect::Timer { id, ms: DEBOUNCE_MS });
        }
        effects
    }

    /// The typing stopped: the server's suggestions and its films and shows by that name.
    fn ask(&mut self, asks: &mut Asks<Ask>, cx: &Context) -> Vec<Effect> {
        self.timer = None;
        let term = sections::encode(self.query.trim());
        [
            (Slot::Suggest, format!("/api/cinema/suggest?q={term}")),
            (Slot::Movies, format!("/api/cinema?type=movie&search={term}")),
            (Slot::Shows, format!("/api/cinema?type=series&search={term}")),
        ]
        .into_iter()
        .map(|(slot, path)| {
            let id = asks.ask(Ask::Find(FindAsk::Ask(slot)));
            self.asking.push((id, slot));
            Effect::Request { id, request: crate::api::get(&cx.base, &cx.who(), &path, None) }
        })
        .collect()
    }

    fn answer(&mut self, ask: FindAsk, reply: Reply, asks: &mut Asks<Ask>, cx: &Context) -> Vec<Effect> {
        match ask {
            FindAsk::Wait => self.ask(asks, cx),
            FindAsk::Ask(slot) => {
                self.asking.retain(|(_, s)| *s != slot);
                let items = match crate::api::outcome(reply, cx.online) {
                    crate::api::Outcome::Body { body, .. } => {
                        serde_json::from_str::<Value>(&body).map(|o| model::items(&o, "items")).unwrap_or_default()
                    }
                    _ => vec![],
                };
                self.found.insert(slot, items);
                self.busy = self.found.len() < 3;
                self.merge();
                vec![]
            }
        }
    }

    fn view(&self, marks: &Marks, now: i64) -> SearchView {
        let typed = !self.short();
        let count = self.hits.len();
        let note = if !typed {
            "Popular right now".to_string()
        } else if self.busy && count == 0 {
            "Searching…".into()
        } else {
            format!(
                "{count}{} {}",
                if self.busy { "+" } else { "" },
                if count == 1 && !self.busy { "result" } else { "results" }
            )
        };
        let query = self.query.trim();
        SearchView {
            query: self.query.clone(),
            hint: hint(self.scope.as_deref()).into(),
            note,
            cards: self.hits.iter().map(|i| marks.grid(i, now)).collect(),
            busy: self.busy,
            typed,
            empty: (typed && !self.busy && count == 0).then(|| Empty {
                glyph: Glyph::Search,
                title: "Nothing found".into(),
                line: format!("Nothing called “{query}” yet."),
            }),
            fresh: self.fresh,
        }
    }
}

// ---------- search everywhere ----------

/// What happens on search everywhere's page.
#[derive(Clone, Debug, PartialEq)]
pub enum SearchEvent {
    Opened,
    /// What's typed (or said, or sent from a phone).
    Query {
        text: String,
    },
    /// A result pressed, by its place.
    Tapped {
        index: u32,
    },
}

#[derive(Clone, Debug, PartialEq)]
enum Ask {
    Page(String),
    Find(FindAsk),
    /// A page of the list asked for `load` (its count), `skip` titles in.
    List {
        load: u32,
        skip: i64,
        key: String,
    },
    /// One of Dramas' lands' first page.
    Land {
        load: u32,
        at: usize,
    },
}

/// Search everywhere. See the module's docs.
#[derive(Debug, Default)]
pub struct Search {
    finder: Finder,
    now: f64,
    asks: Asks<Ask>,
    home: Option<String>,
    anime: Option<String>,
    marks: Marks,
}

impl Search {
    fn pages(&mut self) -> Vec<Effect> {
        vec![
            Effect::Page {
                id: self.asks.ask(Ask::Page(crate::home::KEY.into())),
                path: HOME_PATH.into(),
                key: crate::home::KEY.into(),
                need: Need::Any,
            },
            Effect::Page {
                id: self.asks.ask(Ask::Page(ANIME_KEY.into())),
                path: ANIME_PATH.into(),
                key: ANIME_KEY.into(),
                need: Need::Any,
            },
        ]
    }
}

impl Screen for Search {
    type Event = SearchEvent;
    type View = SearchView;

    fn update(&mut self, event: SearchEvent, cx: &Context) -> Vec<Effect> {
        self.now = cx.clock.now;
        match event {
            SearchEvent::Opened => self.pages(),
            SearchEvent::Query { text } => self.finder.query(text, &mut self.asks),
            SearchEvent::Tapped { index } => self.finder.hits.get(index as usize).map(cards::tap).into_iter().collect(),
        }
    }

    fn answer(&mut self, id: u32, reply: Reply, cx: &Context) -> Vec<Effect> {
        self.now = cx.clock.now;
        match self.asks.take(id) {
            Some(Ask::Page(key)) => {
                if let Reply::Page { page } = reply {
                    if key == crate::home::KEY {
                        self.home = page.body.or(self.home.take());
                        if let Some(o) = self.home.as_deref().and_then(|b| serde_json::from_str::<Value>(b).ok()) {
                            self.marks = Marks::new(&crate::home::parse(&o).rows);
                        }
                    } else {
                        self.anime = page.body.or(self.anime.take());
                    }
                    self.finder.index(self.home.as_deref(), self.anime.as_deref());
                    self.finder.merge();
                }
                vec![]
            }
            Some(Ask::Find(ask)) => self.finder.answer(ask, reply, &mut self.asks, cx),
            _ => vec![],
        }
    }

    fn app(&mut self, event: &AppEvent, _cx: &Context) -> Vec<Effect> {
        match event {
            AppEvent::PageChanged { key } if key == crate::home::KEY || key == ANIME_KEY => self.pages(),
            _ => vec![],
        }
    }

    fn view(&self) -> SearchView {
        self.finder.view(&self.marks, self.now as i64)
    }
}

// ---------- a section ----------

/// What happens on a section's page.
#[derive(Clone, Debug, PartialEq)]
pub enum SectionEvent {
    /// The section `section` opened ("dramas", "shows", "movies", "anime").
    Opened {
        section: String,
    },
    /// A chip picked, by its place.
    Chip {
        index: u32,
    },
    /// The list scrolled: `to_end` how much of it is below the screen, `screen` how tall the screen is (the same
    /// unit). Near the end the next page is asked for, once for each length the list reaches.
    Scrolled {
        to_end: f32,
        screen: f32,
    },
    Retry,
    /// A title pressed, by its place.
    Tapped {
        index: u32,
    },
    /// The section's own search opened, or closed.
    Find {
        open: bool,
    },
    Query {
        text: String,
    },
    /// A search result pressed, by its place.
    Found {
        index: u32,
    },
}

/// A chip.
#[derive(Clone, Debug, PartialEq)]
pub struct Chip {
    pub label: String,
    pub on: bool,
}

/// Exactly what a section's page draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SectionView {
    pub title: String,
    pub chips: Vec<Chip>,
    pub cards: Vec<Card>,
    /// Its first page on its way: the grid's skeleton.
    pub loading: bool,
    /// Another page on its way, at the foot.
    pub more: bool,
    /// The server's still putting this list together: it says so.
    pub building: Option<String>,
    pub trouble: Option<Trouble>,
    /// Searching right on the page: its name gave way to Back and the field, its titles to the results.
    pub finding: bool,
    pub search: SearchView,
    /// Goes up when another chip's picked: the grid goes back to its top.
    pub fresh: u32,
}

/// A section's model. See the module's docs.
#[derive(Debug, Default)]
pub struct Section {
    key: String,
    tabs: Vec<sections::Tab>,
    tab: usize,
    now: f64,
    year: i32,
    asks: Asks<Ask>,
    /// Counts the lists asked for: an answer for an earlier one is let go.
    loads: u32,
    /// The list said it was still being put together, and the server's been asked again.
    rechecked: bool,
    items: Vec<Item>,
    next: i64,
    has_more: bool,
    loading: bool,
    /// The length the list had when the next page was last asked for.
    asked_at: usize,
    building: bool,
    failed: Option<(String, bool)>,
    /// Dramas: Home's (Pakistan's) and each land's list, as they come.
    home: Option<String>,
    lands: Vec<Option<Vec<Item>>>,
    anime: Option<String>,
    marks: Marks,
    finding: bool,
    finder: Finder,
    fresh: u32,
}

impl Section {
    fn page(&mut self, ask: Ask, path: &str, key: &str, need: Need) -> Effect {
        Effect::Page { id: self.asks.ask(ask), path: path.into(), key: key.into(), need }
    }

    fn cinema(&self, kind: &str, collection: &str, genre: &str, skip: i64) -> (String, String) {
        let path = sections::cinema_path(kind, collection, genre, self.year, skip);
        let key = format!("cinema-{path}");
        (path, key)
    }

    /// Chip `tab`'s list from its start (`need` as the store should take it).
    fn load(&mut self, tab: usize, need: Need) -> Vec<Effect> {
        let Some(t) = self.tabs.get(tab).copied() else { return vec![] };
        self.loads = self.loads.wrapping_add(1);
        let load = self.loads;
        self.rechecked = false;
        self.loading = true;
        self.failed = None;
        self.next = 0;
        self.has_more = false;
        self.asked_at = 0;
        self.lands.clear();
        let mut effects = vec![self.page(Ask::Page(crate::home::KEY.into()), HOME_PATH, crate::home::KEY, Need::Any)];
        match t.load {
            Load::Cinema { kind, collection, genre } => {
                let (path, key) = self.cinema(kind, collection, genre, 0);
                effects.push(self.page(Ask::List { load, skip: 0, key: key.clone() }, &path, &key, need));
            }
            Load::FromHome { .. } => {}
            Load::Dramas { genre, .. } => {
                self.lands = vec![None; sections::DRAMA_LANDS.len()];
                for (at, land) in sections::DRAMA_LANDS.iter().enumerate() {
                    let (path, key) = self.cinema("series", land, genre, 0);
                    effects.push(self.page(Ask::Land { load, at }, &path, &key, need));
                }
            }
            Load::Anime { .. } => effects.push(self.page(Ask::Page(ANIME_KEY.into()), ANIME_PATH, ANIME_KEY, need)),
        }
        effects
    }

    /// The chip's list put together from what's come, once all of it has.
    fn gather(&mut self) {
        let Some(t) = self.tabs.get(self.tab).copied() else { return };
        let home =
            || self.home.as_deref().and_then(|b| serde_json::from_str::<Value>(b).ok()).map(|o| crate::home::parse(&o));
        let list = match t.load {
            Load::Cinema { .. } => return,
            Load::FromHome { movies } => home().map(|h| sections::from_home(h.everything(), movies)),
            Load::Dramas { genre, newest } => match (home(), self.lands.iter().all(Option::is_some)) {
                (Some(h), true) => Some(sections::dramas(
                    sections::from_home(h.everything(), false),
                    self.lands.iter().flatten().cloned().collect(),
                    genre,
                    newest,
                )),
                _ => None,
            },
            Load::Anime { genre, newest } => self
                .anime
                .as_deref()
                .and_then(|b| serde_json::from_str::<Value>(b).ok())
                .map(|o| sections::anime(&model::items(&o, "items"), genre, newest)),
        };
        if let Some(list) = list {
            self.show(list, true);
            self.loading = false;
        }
    }

    /// A list (or the next page of one) shown; its own-art titles first on its first page.
    fn show(&mut self, mut list: Vec<Item>, first: bool) {
        if first {
            list.sort_by_key(|i| !i.own_art());
            self.items = list;
        } else {
            let mut seen: HashSet<String> = self.items.iter().map(|i| i.id.clone()).collect();
            self.items.extend(list.into_iter().filter(|i| seen.insert(i.id.clone())));
        }
        if self.tab == 0 {
            self.finder.popular = self.items.clone();
            self.finder.merge();
        }
    }

    fn fail(&mut self, page: &crate::store::PageReply) {
        if let Some(f) = &page.failure {
            self.failed = Some((f.words.clone(), f.kind == crate::api::Kind::Offline));
        }
    }
}

impl Screen for Section {
    type Event = SectionEvent;
    type View = SectionView;

    fn update(&mut self, event: SectionEvent, cx: &Context) -> Vec<Effect> {
        self.now = cx.clock.now;
        self.year = i32::try_from(crate::dates::year(cx.clock.now, cx.clock.utc_offset)).unwrap_or_default();
        match event {
            SectionEvent::Opened { section } => {
                if self.key == section {
                    return vec![];
                }
                *self = Self {
                    tabs: sections::tabs(&section),
                    finder: Finder::new(Some(&section)),
                    key: section,
                    now: self.now,
                    year: self.year,
                    ..Self::default()
                };
                let mut effects = self.load(0, Need::Any);
                effects.push(Effect::Page {
                    id: self.asks.ask(Ask::Page(ANIME_KEY.into())),
                    path: ANIME_PATH.into(),
                    key: ANIME_KEY.into(),
                    need: Need::Any,
                });
                effects
            }
            SectionEvent::Chip { index } if (index as usize) < self.tabs.len() && index as usize != self.tab => {
                self.tab = index as usize;
                self.items.clear();
                self.fresh = self.fresh.wrapping_add(1);
                self.load(self.tab, Need::Any)
            }
            SectionEvent::Chip { .. } => vec![],
            SectionEvent::Scrolled { to_end, screen } => {
                if to_end > screen * NEAR_END || self.items.is_empty() || self.asked_at == self.items.len() {
                    return vec![];
                }
                let Some(Load::Cinema { kind, collection, genre }) = self.tabs.get(self.tab).map(|t| t.load) else {
                    return vec![];
                };
                if !self.has_more || self.loading {
                    return vec![];
                }
                self.loading = true;
                self.asked_at = self.items.len();
                let (path, key) = self.cinema(kind, collection, genre, self.next);
                let load = self.loads;
                vec![self.page(Ask::List { load, skip: self.next, key: key.clone() }, &path, &key, Need::Any)]
            }
            SectionEvent::Retry => {
                self.items.clear();
                self.load(self.tab, Need::Server)
            }
            SectionEvent::Tapped { index } => self.items.get(index as usize).map(cards::tap).into_iter().collect(),
            SectionEvent::Find { open } => {
                self.finding = open;
                self.finder.query(String::new(), &mut self.asks)
            }
            SectionEvent::Query { text } => self.finder.query(text, &mut self.asks),
            SectionEvent::Found { index } => self.finder.hits.get(index as usize).map(cards::tap).into_iter().collect(),
        }
    }

    fn answer(&mut self, id: u32, reply: Reply, cx: &Context) -> Vec<Effect> {
        self.now = cx.clock.now;
        let Some(ask) = self.asks.take(id) else { return vec![] };
        let page = match reply {
            Reply::Page { page } => page,
            other => {
                if let Ask::Find(ask) = ask {
                    return self.finder.answer(ask, other, &mut self.asks, cx);
                }
                return vec![];
            }
        };
        let o = page.body.as_deref().and_then(|b| serde_json::from_str::<Value>(b).ok());
        let mut effects = vec![];
        match ask {
            Ask::Page(key) if key == crate::home::KEY => {
                if let Some(o) = &o {
                    self.marks = Marks::new(&crate::home::parse(o).rows);
                }
                self.home = page.body.clone().or(self.home.take());
                self.finder.index(self.home.as_deref(), self.anime.as_deref());
                if self.home.is_none()
                    && matches!(
                        self.tabs.get(self.tab).map(|t| t.load),
                        Some(Load::FromHome { .. } | Load::Dramas { .. })
                    )
                {
                    self.fail(&page);
                    self.loading = false;
                }
                self.gather();
            }
            Ask::Page(_) => {
                self.anime = page.body.clone().or(self.anime.take());
                self.finder.index(self.home.as_deref(), self.anime.as_deref());
                if self.anime.is_none() && matches!(self.tabs.get(self.tab).map(|t| t.load), Some(Load::Anime { .. })) {
                    self.fail(&page);
                    self.loading = false;
                }
                self.gather();
            }
            Ask::List { load, skip, key } if load == self.loads => {
                self.loading = false;
                match o {
                    Some(o) => {
                        let building = model::b(&o, "building");
                        // (a list the server was still putting together, as kept: the server's asked again for it)
                        if building && skip == 0 && !self.rechecked {
                            self.rechecked = true;
                            let path = key.trim_start_matches("cinema-").to_string();
                            effects.push(self.page(
                                Ask::List { load, skip, key: key.clone() },
                                &path,
                                &key,
                                Need::Server,
                            ));
                        }
                        self.building = building;
                        let next = model::i(&o, "next").unwrap_or(0);
                        self.has_more = model::b(&o, "hasMore") && next > skip;
                        self.next = next;
                        self.show(model::items(&o, "items"), skip == 0);
                    }
                    None => self.fail(&page),
                }
            }
            Ask::Land { load, at } if load == self.loads => {
                if let Some(slot) = self.lands.get_mut(at) {
                    *slot = Some(o.map(|o| model::items(&o, "items")).unwrap_or_default());
                }
                self.gather();
            }
            _ => {}
        }
        effects
    }

    fn app(&mut self, event: &AppEvent, _cx: &Context) -> Vec<Effect> {
        let AppEvent::PageChanged { key } = event else { return vec![] };
        match self.tabs.get(self.tab).map(|t| t.load) {
            // (a newer first page, while it's all that's shown: it takes the list's place)
            Some(Load::Cinema { kind, collection, genre }) if self.next <= 0 || self.items.len() <= 60 => {
                let (path, first) = self.cinema(kind, collection, genre, 0);
                if *key != first || self.loading {
                    return vec![];
                }
                let load = self.loads;
                vec![self.page(Ask::List { load, skip: 0, key: first.clone() }, &path, &first, Need::Any)]
            }
            _ if key == crate::home::KEY => {
                vec![self.page(Ask::Page(crate::home::KEY.into()), HOME_PATH, crate::home::KEY, Need::Any)]
            }
            _ if key == ANIME_KEY => vec![self.page(Ask::Page(ANIME_KEY.into()), ANIME_PATH, ANIME_KEY, Need::Any)],
            _ => vec![],
        }
    }

    fn view(&self) -> SectionView {
        let now = self.now as i64;
        let trouble = match (&self.failed, self.items.is_empty()) {
            (Some((message, offline)), true) => {
                Some(Trouble { message: message.clone(), offline: *offline, busy: self.loading })
            }
            _ => None,
        };
        SectionView {
            title: sections::title(&self.key).into(),
            chips: self
                .tabs
                .iter()
                .enumerate()
                .map(|(i, t)| Chip { label: t.label.into(), on: i == self.tab })
                .collect(),
            cards: self.items.iter().map(|i| self.marks.grid(i, now)).collect(),
            loading: self.loading && self.items.is_empty() && trouble.is_none(),
            more: self.loading && !self.items.is_empty(),
            building: self.building.then(|| sections::BUILDING.into()),
            trouble,
            finding: self.finding,
            search: self.finder.view(&self.marks, now),
            fresh: self.fresh,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root::{Root, ScreenId, Turn};
    use crate::screen::{response, test_context, Clock, Route};
    use serde_json::json;

    const NOW: f64 = 1_794_657_600.;
    const CLOCK: Clock = Clock { now: NOW, utc_offset: 0 };

    /// A screen on a root, the server answering by path; timers kept to fire by hand.
    struct On<S: Screen + 'static> {
        root: Root,
        id: ScreenId,
        server: HashMap<String, String>,
        asked: Vec<String>,
        timers: Vec<u32>,
        cancelled: Vec<u32>,
        _s: std::marker::PhantomData<S>,
    }

    impl<S: Screen + Default + Send + 'static> On<S> {
        fn new(server: &[(&str, String)]) -> Self {
            let mut root = Root::new(test_context(NOW).platform, "https://host.invalid".into(), CLOCK);
            let id = root.open(S::default());
            let server = server.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
            Self { root, id, server, asked: vec![], timers: vec![], cancelled: vec![], _s: std::marker::PhantomData }
        }

        fn send(&mut self, event: S::Event) -> Vec<Effect> {
            let turn = self.root.update::<S>(self.id, event, CLOCK);
            self.run(turn)
        }

        fn fire(&mut self) -> Vec<Effect> {
            let id = self.timers.pop().unwrap_or_default();
            let turn = self.root.answer(id, Reply::Fired, CLOCK);
            self.run(turn)
        }

        fn run(&mut self, turn: Turn) -> Vec<Effect> {
            let mut queue: Vec<Effect> = turn.orders.into_iter().map(|o| o.effect).collect();
            let mut out = vec![];
            while !queue.is_empty() {
                let effect = queue.remove(0);
                let turn = match &effect {
                    Effect::Load { id, .. } => self.root.answer(*id, Reply::Missing, CLOCK),
                    Effect::Request { id, request } => {
                        let path = request.url.trim_start_matches("https://host.invalid").to_string();
                        self.asked.push(path.clone());
                        let reply = self
                            .server
                            .get(&path)
                            .map_or(Reply::Unreachable { offline: true, message: "x".into() }, |b| {
                                response(200, b, None)
                            });
                        self.root.answer(*id, reply, CLOCK)
                    }
                    Effect::Timer { id, .. } => {
                        self.timers.push(*id);
                        continue;
                    }
                    Effect::Cancel { id } => {
                        self.cancelled.push(*id);
                        continue;
                    }
                    Effect::Store { .. } => continue,
                    _ => {
                        out.push(effect);
                        continue;
                    }
                };
                queue.extend(turn.orders.into_iter().map(|o| o.effect));
            }
            out
        }

        fn view(&self) -> S::View {
            self.root.view::<S>(self.id).unwrap_or_else(|| S::default().view())
        }
    }

    fn card(id: &str, title: &str, art: bool) -> Value {
        let mut c = json!({"id": id, "title": title, "source": "cinema", "type": "series"});
        if art {
            c["posterClean"] = json!(true);
            c["logo"] = json!("/l.png");
        }
        c
    }

    fn home() -> String {
        json!({"rows": [{"title": "Top 10", "ranked": true, "items": [card("dark", "Dark", true)]},
                        {"title": "More", "items": [card("drama", "The Drama Club", false), json!({"id": "pk", "title": "Pak Drama", "type": "show"})]}]})
        .to_string()
    }

    #[test]
    fn search_shows_whats_known_then_the_servers_after_the_typing_stops() {
        let mut s: On<Search> = On::new(&[("/api/home", home())]);
        s.send(SearchEvent::Opened);
        s.send(SearchEvent::Query { text: "d".into() });
        let v = s.view();
        assert_eq!((v.note.as_str(), v.typed, v.busy), ("Popular right now", false, false));
        assert_eq!(v.cards.iter().map(|c| c.item.id.as_str()).collect::<Vec<_>>(), ["dark"]);
        assert!(v.cards[0].top10);
        s.send(SearchEvent::Query { text: "dr".into() });
        let v = s.view();
        assert_eq!(v.cards.iter().map(|c| c.item.id.as_str()).collect::<Vec<_>>(), ["pk", "drama"]);
        assert_eq!((v.note.as_str(), v.busy), ("2+ results", true));
        assert!(s.asked.iter().all(|p| !p.contains("search")));
        s.server
            .insert("/api/cinema/suggest?q=dr".into(), json!({"items": [card("dr2", "Dr. No", false)]}).to_string());
        s.server.insert(
            "/api/cinema?type=movie&search=dr".into(),
            json!({"items": [card("m", "Mr Drew", false)]}).to_string(),
        );
        s.server.insert(
            "/api/cinema?type=series&search=dr".into(),
            json!({"items": [card("drama", "The Drama Club", false)]}).to_string(),
        );
        s.fire();
        let v = s.view();
        assert!(!v.busy);
        assert_eq!(v.cards.iter().map(|c| c.item.id.as_str()).collect::<Vec<_>>(), ["dr2", "pk", "drama", "m"]);
        assert_eq!(v.note, "4 results");
        assert_eq!(
            s.send(SearchEvent::Tapped { index: 0 }),
            [Effect::Open { route: Route::Title { id: "dr2".into() } }]
        );
    }

    #[test]
    fn typing_again_stops_the_last_search_and_nothing_found_says_so() {
        let mut s: On<Search> = On::new(&[("/api/home", home())]);
        s.send(SearchEvent::Opened);
        s.send(SearchEvent::Query { text: "zz".into() });
        let waiting = s.timers.clone();
        s.send(SearchEvent::Query { text: "zzz".into() });
        assert_eq!(s.cancelled, waiting);
        let fresh = s.view().fresh;
        s.send(SearchEvent::Query { text: " zzz ".into() });
        assert_eq!(s.view().fresh, fresh);
        s.fire();
        let v = s.view();
        assert_eq!(v.empty.map(|e| e.line), Some("Nothing called \u{201c}zzz\u{201d} yet.".into()));
        assert_eq!(v.hint, "Movies, shows, dramas, anime");
    }

    fn page(ids: &[(&str, bool)], next: i64, more: bool) -> String {
        json!({"items": ids.iter().map(|(id, art)| card(id, id, *art)).collect::<Vec<_>>(), "next": next, "hasMore": more}).to_string()
    }

    #[test]
    fn a_section_pages_on_and_its_chips_start_again() {
        let all = "/api/cinema?type=series&collection=popular&skip=0";
        let more = "/api/cinema?type=series&collection=popular&skip=2";
        let latest = "/api/cinema?type=series&collection=year&year=2026&skip=0";
        let mut s: On<Section> = On::new(&[
            (all, page(&[("a", false), ("b", true)], 2, true)),
            (more, page(&[("b", true), ("c", false)], 4, false)),
            (latest, page(&[("n", false)], 1, false)),
            ("/api/home", home()),
        ]);
        s.send(SectionEvent::Opened { section: "shows".into() });
        let v = s.view();
        assert_eq!(v.title, "TV Shows");
        assert_eq!(v.cards.iter().map(|c| c.item.id.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        assert!(v.chips[0].on && !v.loading);
        s.asked.clear();
        s.send(SectionEvent::Scrolled { to_end: 5000., screen: 800. });
        assert!(s.asked.is_empty(), "far from the end asks for nothing");
        s.send(SectionEvent::Scrolled { to_end: 1000., screen: 800. });
        assert_eq!(s.view().cards.iter().map(|c| c.item.id.as_str()).collect::<Vec<_>>(), ["b", "a", "c"]);
        s.asked.clear();
        s.send(SectionEvent::Scrolled { to_end: 0., screen: 800. });
        assert!(s.asked.is_empty());
        s.send(SectionEvent::Chip { index: 1 });
        let v = s.view();
        assert_eq!(v.cards.iter().map(|c| c.item.id.as_str()).collect::<Vec<_>>(), ["n"]);
        assert_eq!(v.fresh, 1);
        assert_eq!(
            s.send(SectionEvent::Tapped { index: 0 }),
            [Effect::Open { route: Route::Title { id: "n".into() } }]
        );
        // (a section's search keeps to it, and its popular titles are what it opened on)
        s.send(SectionEvent::Find { open: true });
        let v = s.view();
        assert!(v.finding);
        assert_eq!(v.search.cards.iter().map(|c| c.item.id.as_str()).collect::<Vec<_>>(), ["b", "a", "c"]);
        assert_eq!(v.search.hint, "Search TV shows");
        s.send(SectionEvent::Query { text: "pak".into() });
        assert!(s.view().search.cards.is_empty());
    }

    #[test]
    fn a_list_still_being_made_is_asked_for_again_and_offline_is_trouble() {
        let all = "/api/cinema?type=movie&collection=popular&skip=0";
        let mut s: On<Section> = On::new(&[(all, json!({"items": [], "building": true}).to_string())]);
        s.send(SectionEvent::Opened { section: "movies".into() });
        assert_eq!(s.asked.iter().filter(|p| p.as_str() == all).count(), 2);
        assert_eq!(s.view().building.as_deref(), Some(sections::BUILDING));
        let mut gone: On<Section> = On::new(&[]);
        gone.send(SectionEvent::Opened { section: "movies".into() });
        let v = gone.view();
        assert!(v.trouble.is_some_and(|t| t.offline) && !v.loading);
    }

    #[test]
    fn dramas_deal_pakistans_and_koreas_in_turn() {
        let korean = "/api/cinema?type=series&collection=korean&skip=0";
        let mut s: On<Section> =
            On::new(&[("/api/home", home()), (korean, page(&[("k1", false), ("k2", false)], 0, false))]);
        s.send(SectionEvent::Opened { section: "dramas".into() });
        let v = s.view();
        assert_eq!(v.cards.iter().map(|c| c.item.id.as_str()).collect::<Vec<_>>(), ["pk", "k1", "k2"]);
    }
}
