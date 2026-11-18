//! Manhwa's shelf: the chips (sorts, then genres), the covers of the chosen one, more as its end comes near, and a
//! search in the head that shows what Asura has by the name once two letters are typed and typing pauses.

use super::{card_line, catalog_path, parse_catalog, read_back, reading, shared, shelf_query, shelf_key, Comic, Shared};
use super::{READING_PREF, SHELVES};
use crate::api::{self, Outcome};
use crate::screen::{AppEvent, Asks, Context, Effect, Empty, Glyph, Need, Reply, Screen, Trouble};

/// Letters typed before a search is asked.
const LETTERS: usize = 2;
/// How long typing pauses before the search is asked.
const TYPING_MS: u32 = 300;
/// How long after a later page failed before it's asked again: at once, it looped every second while Asura was down.
const COOL_MS: u32 = 10_000;
/// How close to the end (in screens) the shelf asks for its next page.
const NEAR_END: f32 = 1.6;

const TITLE: &str = "Manhwa";
const HINT: &str = "Search manhwa";
const SEARCHING: &str = "Searching…";
const NOTHING: &str = "Nothing found";
const EMPTY_SHELF: &str = "No manhwa came back. Try again in a moment.";

/// What happens on the shelf.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The page is shown: its shelf from the first page, unless it has one already.
    Opened,
    /// A chip chosen (its index).
    Chip { index: u32 },
    /// Scrolled: `to_end` how much of the grid is below the screen, `screen` how tall the screen is.
    Scrolled { to_end: f32, screen: f32 },
    /// Search opened or closed.
    Search { open: bool },
    /// What's typed in the search.
    Typed { text: String },
    /// Try again, after it couldn't load.
    Retry,
}

/// Exactly what the shelf draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub title: String,
    pub chips: Vec<String>,
    pub chip: u32,
    pub cards: Vec<Card>,
    /// Nothing to show yet, and it's coming: the grid's skeleton.
    pub loading: bool,
    /// Another page is coming: a spinner under the cards.
    pub more: bool,
    pub trouble: Option<Trouble>,
    pub search: Search,
}

/// A series' cover on the shelf.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    /// What opening it hands the series page.
    pub comic: Comic,
    pub title: String,
    /// Its cover, whole address.
    pub picture: String,
    /// The chapter you're up to, else its rating.
    pub line: Option<String>,
}

/// The search over the shelf.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Search {
    pub open: bool,
    pub hint: String,
    /// Enough is typed: the results show in place of the shelf.
    pub shown: bool,
    /// Asking: the field's spinner, and the results' skeleton.
    pub busy: bool,
    /// Over the results: "Searching…", "3 results".
    pub note: String,
    pub cards: Vec<Card>,
    pub empty: Option<Empty>,
}

#[derive(Debug)]
enum Ask {
    Reading,
    /// A shelf's first page, for the shelf asked as `turn`.
    First { turn: u32 },
    Next { turn: u32, page: u32 },
    Cool,
    Typing,
    Search { query: String },
}

/// The shelf's model.
#[derive(Debug)]
pub struct Shelf {
    kept: Shared,
    base: String,
    opened: bool,
    chip: usize,
    /// Each shelf asked for counts up, so an answer for one left behind is dropped.
    turn: u32,
    items: Vec<Comic>,
    page: u32,
    more: bool,
    loading: bool,
    failed: Option<(String, bool)>,
    cooling: bool,
    searching: bool,
    query: String,
    found: Option<Vec<Comic>>,
    /// The typing pause or search under way.
    typing: Option<u32>,
    asks: Asks<Ask>,
}

impl Default for Shelf {
    fn default() -> Self {
        Self::sharing(shared())
    }
}

impl Shelf {
    /// A shelf reading the spots in `kept`.
    pub fn sharing(kept: Shared) -> Self {
        Self {
            kept,
            base: String::new(),
            opened: false,
            chip: 0,
            turn: 0,
            items: vec![],
            page: 0,
            more: false,
            loading: false,
            failed: None,
            cooling: false,
            searching: false,
            query: String::new(),
            found: None,
            typing: None,
            asks: Asks::default(),
        }
    }

    fn query(&self) -> (&'static str, &'static str) {
        shelf_query(SHELVES.get(self.chip).map_or("popular", |s| s.0))
    }

    /// The chosen shelf from its first page.
    fn first(&mut self, need: Need) -> Vec<Effect> {
        self.turn += 1;
        self.page = 0;
        self.more = false;
        self.loading = true;
        self.failed = None;
        self.cooling = false;
        vec![self.ask_first(need)]
    }

    fn ask_first(&mut self, need: Need) -> Effect {
        let (genre, sort) = self.query();
        let id = self.asks.ask(Ask::First { turn: self.turn });
        Effect::Page { id, path: catalog_path(1, "", genre, sort), key: shelf_key(genre, sort), need }
    }

    fn stop_typing(&mut self) -> Vec<Effect> {
        self.typing.take().map(|id| {
            self.asks.take(id);
            Effect::Cancel { id }
        }).into_iter().collect()
    }

    fn card(&self, comic: &Comic) -> Card {
        let last = reading(&self.kept, |r| r.last(&comic.path).map(String::from)).flatten();
        Card {
            comic: comic.clone(),
            title: comic.title.clone(),
            picture: api::url(&self.base, comic.small()),
            line: card_line(last.as_deref(), &comic.rating),
        }
    }

    fn lettered(&self) -> bool {
        self.searching && self.query.trim().chars().count() >= LETTERS
    }
}

impl Screen for Shelf {
    type Event = Event;
    type View = View;

    fn update(&mut self, event: Event, cx: &Context) -> Vec<Effect> {
        match event {
            Event::Opened => {
                let mut effects = vec![];
                if !self.opened {
                    self.opened = true;
                    self.base = cx.base.clone();
                    if reading(&self.kept, |_| ()).is_none() {
                        effects.push(Effect::ReadPref { id: self.asks.ask(Ask::Reading), key: READING_PREF.into() });
                    }
                }
                if self.items.is_empty() && !self.loading {
                    effects.extend(self.first(Need::Any));
                }
                effects
            }
            Event::Chip { index } => {
                let index = index as usize;
                if index == self.chip || index >= SHELVES.len() {
                    return vec![];
                }
                self.chip = index;
                self.items.clear();
                self.first(Need::Any)
            }
            Event::Retry => self.first(Need::Server),
            Event::Scrolled { to_end, screen } => {
                let due = !self.searching && self.more && !self.loading && !self.cooling && self.failed.is_none();
                if !due || to_end > screen * NEAR_END {
                    return vec![];
                }
                let (genre, sort) = self.query();
                let page = self.page + 1;
                self.loading = true;
                let id = self.asks.ask(Ask::Next { turn: self.turn, page });
                vec![Effect::Request { id, request: api::get(&cx.base, &cx.who(), &catalog_path(page, "", genre, sort), None) }]
            }
            Event::Search { open } => {
                self.searching = open;
                self.query.clear();
                self.found = None;
                self.stop_typing()
            }
            Event::Typed { text } => {
                if text == self.query {
                    return vec![];
                }
                self.query = text;
                self.found = None;
                let mut effects = self.stop_typing();
                if self.lettered() {
                    let id = self.asks.ask(Ask::Typing);
                    self.typing = Some(id);
                    effects.push(Effect::Timer { id, ms: TYPING_MS });
                }
                effects
            }
        }
    }

    fn answer(&mut self, id: u32, reply: Reply, cx: &Context) -> Vec<Effect> {
        let Some(ask) = self.asks.take(id) else { return vec![] };
        match ask {
            Ask::Reading => {
                if let Reply::Pref { value } = reply {
                    read_back(&self.kept, value.as_deref());
                }
            }
            Ask::First { turn } if turn == self.turn => {
                let Reply::Page { page } = reply else { return vec![] };
                self.loading = false;
                let (genre, _) = self.query();
                match page.body.as_deref().map(|body| parse_catalog(body, 1, "", genre)) {
                    Some(Ok(shelf)) => {
                        self.items = shelf.items;
                        self.more = shelf.more;
                        self.page = 1;
                        self.failed = None;
                    }
                    Some(Err(_)) => self.failed = Some((EMPTY_SHELF.into(), false)),
                    None => self.failed = page.failure.map(|f| (f.words, f.kind == api::Kind::Offline)),
                }
            }
            Ask::Next { turn, page } if turn == self.turn => {
                self.loading = false;
                let (genre, _) = self.query();
                let shelf = match api::outcome(reply, cx.online) {
                    Outcome::Body { body, .. } => parse_catalog(&body, page, "", genre).ok(),
                    _ => None,
                };
                match shelf {
                    Some(shelf) => {
                        for comic in shelf.items {
                            if !self.items.iter().any(|c| c.path == comic.path) {
                                self.items.push(comic);
                            }
                        }
                        self.more = shelf.more;
                        self.page = page;
                    }
                    None => {
                        self.cooling = true;
                        return vec![Effect::Timer { id: self.asks.ask(Ask::Cool), ms: COOL_MS }];
                    }
                }
            }
            Ask::Cool => self.cooling = false,
            Ask::Typing => {
                let query = self.query.trim().to_string();
                let id = self.asks.ask(Ask::Search { query: query.clone() });
                self.typing = Some(id);
                let path = catalog_path(1, &query, "", "popular");
                return vec![Effect::Request { id, request: api::get(&cx.base, &cx.who(), &path, None) }];
            }
            Ask::Search { query } if query == self.query.trim() => {
                self.typing = None;
                let found = match api::outcome(reply, cx.online) {
                    Outcome::Body { body, .. } => parse_catalog(&body, 1, &query, "").map(|s| s.items).unwrap_or_default(),
                    _ => vec![],
                };
                self.found = Some(found);
            }
            Ask::First { .. } | Ask::Next { .. } | Ask::Search { .. } => {}
        }
        vec![]
    }

    fn app(&mut self, event: &AppEvent, _cx: &Context) -> Vec<Effect> {
        let (genre, sort) = self.query();
        match event {
            AppEvent::PageChanged { key } if self.opened && *key == shelf_key(genre, sort) => vec![self.ask_first(Need::Any)],
            // (back online with nothing to show: asked again)
            AppEvent::Connectivity { online: true, .. } if self.failed.is_some() && self.items.is_empty() => self.first(Need::Any),
            _ => vec![],
        }
    }

    fn view(&self) -> View {
        let shown = self.lettered();
        let typed = self.query.trim();
        let search = Search {
            open: self.searching,
            hint: HINT.into(),
            shown,
            busy: shown && self.found.is_none(),
            note: match &self.found {
                None => SEARCHING.into(),
                Some(f) => format!("{} {}", f.len(), if f.len() == 1 { "result" } else { "results" }),
            },
            cards: self.found.iter().flatten().map(|c| self.card(c)).collect(),
            empty: self.found.as_ref().filter(|f| f.is_empty() && shown).map(|_| Empty {
                glyph: Glyph::Search,
                title: NOTHING.into(),
                line: format!("Nothing called “{typed}” yet."),
            }),
        };
        View {
            title: TITLE.into(),
            chips: SHELVES.iter().map(|s| s.1.to_string()).collect(),
            chip: self.chip as u32,
            cards: self.items.iter().map(|c| self.card(c)).collect(),
            loading: self.items.is_empty() && self.loading,
            more: !self.items.is_empty() && self.loading,
            trouble: match (&self.failed, self.items.is_empty()) {
                (Some((message, offline)), true) => Some(Trouble { message: message.clone(), offline: *offline, busy: self.loading }),
                _ => None,
            },
            search,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{response, test_context, PageReply};

    const SHELF: &str = r#"{"items": [{"path": "/comics/a", "title": "A", "cover": "https://cdn/a.webp", "thumb": "/mimg/a", "rating": "9.1"}], "hasMore": true}"#;

    fn page(body: Option<&str>) -> Reply {
        Reply::Page { page: PageReply { body: body.map(String::from), at: 1., failure: None, stale: false } }
    }

    fn opened(shelf: &mut Shelf, cx: &Context) -> u32 {
        let effects = shelf.update(Event::Opened, cx);
        let id = effects.iter().find_map(|e| match e {
            Effect::Page { id, path, key, .. } => {
                assert_eq!(path, "/api/manhwa?page=1&q=&genre=&sort=popular");
                assert_eq!(key, "manhwa-shelf--popular");
                Some(*id)
            }
            _ => None,
        });
        id.expect("the first page")
    }

    #[test]
    fn a_shelf_opens_pages_on_and_cools_after_a_failure() {
        let cx = test_context(1000.);
        let kept = Shared::default();
        let mut shelf = Shelf::sharing(kept.clone());
        let first = opened(&mut shelf, &cx);
        assert!(shelf.view().loading);
        shelf.answer(first, page(Some(SHELF)), &cx);
        let view = shelf.view();
        assert_eq!(view.cards[0].picture, "https://host.invalid/mimg/a");
        assert_eq!(view.cards[0].line.as_deref(), Some("★ 9.1"));
        assert_eq!(view.chips[0], "All");
        // (far from the end: nothing asked)
        assert!(shelf.update(Event::Scrolled { to_end: 2000., screen: 800. }, &cx).is_empty());
        let next = match shelf.update(Event::Scrolled { to_end: 1000., screen: 800. }, &cx).as_slice() {
            [Effect::Request { id, request }] => {
                assert!(request.url.ends_with("/api/manhwa?page=2&q=&genre=&sort=popular"));
                *id
            }
            other => panic!("{other:?}"),
        };
        assert!(shelf.view().more);
        let cool = match shelf.answer(next, response(500, "", None), &cx).as_slice() {
            [Effect::Timer { id, ms: COOL_MS }] => *id,
            other => panic!("{other:?}"),
        };
        assert!(shelf.update(Event::Scrolled { to_end: 0., screen: 800. }, &cx).is_empty());
        shelf.answer(cool, Reply::Fired, &cx);
        assert_eq!(shelf.update(Event::Scrolled { to_end: 0., screen: 800. }, &cx).len(), 1);
    }

    #[test]
    fn a_chip_asks_its_shelf_and_drops_the_last_ones_answer() {
        let cx = test_context(1000.);
        let mut shelf = Shelf::sharing(Shared::default());
        let first = opened(&mut shelf, &cx);
        let murim = match shelf.update(Event::Chip { index: 6 }, &cx).as_slice() {
            [Effect::Page { id, path, .. }] => {
                assert_eq!(path, "/api/manhwa?page=1&q=&genre=murim&sort=popular");
                *id
            }
            other => panic!("{other:?}"),
        };
        shelf.answer(first, page(Some(SHELF)), &cx);
        assert!(shelf.view().cards.is_empty());
        shelf.answer(murim, page(Some(r#"{"items": []}"#)), &cx);
        assert_eq!(shelf.view().trouble, None);
        assert!(!shelf.view().loading);
    }

    #[test]
    fn nothing_kept_and_no_connection_is_trouble() {
        let cx = test_context(1000.);
        let mut shelf = Shelf::sharing(Shared::default());
        let first = opened(&mut shelf, &cx);
        let away = api::Failure { kind: api::Kind::Offline, words: api::OFFLINE.into() };
        shelf.answer(first, Reply::Page { page: PageReply { body: None, at: 0., failure: Some(away), stale: false } }, &cx);
        assert_eq!(shelf.view().trouble.map(|t| t.offline), Some(true));
        assert!(matches!(shelf.update(Event::Retry, &cx).as_slice(), [Effect::Page { need: Need::Server, .. }]));
        shelf.failed = Some((api::OFFLINE.into(), true));
        assert_eq!(shelf.app(&AppEvent::Connectivity { online: true, metered: false }, &cx).len(), 1);
    }

    #[test]
    fn a_search_waits_for_two_letters_and_a_pause() {
        let cx = test_context(1000.);
        let mut shelf = Shelf::sharing(Shared::default());
        opened(&mut shelf, &cx);
        shelf.update(Event::Search { open: true }, &cx);
        assert!(shelf.update(Event::Typed { text: "s".into() }, &cx).is_empty());
        assert!(!shelf.view().search.shown);
        let pause = match shelf.update(Event::Typed { text: "so".into() }, &cx).as_slice() {
            [Effect::Timer { id, ms: TYPING_MS }] => *id,
            other => panic!("{other:?}"),
        };
        // (typed on: the pause starts again)
        let again = match shelf.update(Event::Typed { text: "sol".into() }, &cx).as_slice() {
            [Effect::Cancel { id }, Effect::Timer { id: next, .. }] if *id == pause => *next,
            other => panic!("{other:?}"),
        };
        assert!(shelf.answer(pause, Reply::Fired, &cx).is_empty());
        let view = shelf.view();
        assert!(view.search.shown && view.search.busy);
        assert_eq!(view.search.note, SEARCHING);
        let ask = match shelf.answer(again, Reply::Fired, &cx).as_slice() {
            [Effect::Request { id, request }] => {
                assert!(request.url.ends_with("/api/manhwa?page=1&q=sol&genre=&sort=popular"));
                *id
            }
            other => panic!("{other:?}"),
        };
        shelf.answer(ask, response(200, r#"{"items": []}"#, None), &cx);
        let view = shelf.view();
        assert_eq!(view.search.note, "0 results");
        assert_eq!(view.search.empty.map(|e| e.line), Some("Nothing called “sol” yet.".into()));
    }

    #[test]
    fn cards_say_where_you_are() {
        let cx = test_context(1000.);
        let kept = Shared::default();
        let mut shelf = Shelf::sharing(kept.clone());
        let effects = shelf.update(Event::Opened, &cx);
        let Some(Effect::ReadPref { id, key }) = effects.first() else { panic!("{effects:?}") };
        assert_eq!(key, READING_PREF);
        shelf.answer(*id, Reply::Pref { value: Some(r#"{"last": {"/comics/a": "/comics/a/chapter/12"}}"#.into()) }, &cx);
        let Some(Effect::Page { id, .. }) = effects.get(1) else { panic!("{effects:?}") };
        shelf.answer(*id, page(Some(SHELF)), &cx);
        assert_eq!(shelf.view().cards[0].line.as_deref(), Some("Continue · Ch 12"));
    }
}
