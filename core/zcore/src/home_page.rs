//! Home as a screen: the stage turning through its featured titles, then Continue watching, New episodes and the
//! server's rows, each card dressed in its show's own art.
//!
//! Its first frame comes from what the device kept, of any age (Home, this profile's spots and up next, the anime
//! list, and the saved pages of the shows in Continue watching), so the rows are never slotted in after the page is
//! up. Then each page is asked of the app's store by the cache rules, and a newer one redraws it. Offline with nothing
//! kept, it says so, and asks again when the network's back.
//!
//! ```
//! use zcore::home_page::{Event, HomePage};
//! use zcore::root::Root;
//! use zcore::screen::{Clock, Device, Effect, Platform, Reply};
//!
//! let platform = Platform { device: Device::Phone, model: "Pixel".into(), version_name: "1.0".into(),
//!                           version_code: 1, os: "t".into(), stream: "s".into(), codecs: vec![], hdr: false };
//! let clock = Clock { now: 1_794_657_600., utc_offset: 0 };
//! let mut root = Root::new(platform, "https://host.invalid".into(), clock);
//! let home = root.open(HomePage::default());
//! let turn = root.update::<HomePage>(home, Event::Opened { profile: "p1".into() }, clock);
//! // (what the device kept is read first: Home, the spots, up next, the anime list)
//! assert_eq!(turn.orders.iter().filter(|o| matches!(o.effect, Effect::Load { .. })).count(), 4);
//! assert!(root.view::<HomePage>(home).is_some_and(|v| v.loading));
//! ```

use crate::cards::{self, Card, Marks};
use crate::home::{self, Home, Row, Spot, UpNext};
use crate::model::{self, Item};
use crate::screen::{AppEvent, Asks, Context, Effect, Need, Reply, Route, Screen, Trouble};
use crate::session::mine;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

const HOME_PATH: &str = "/api/home";
const UP_NEXT: &str = "/api/up-next";
const ANIME: &str = "/api/anime";
const ANIME_KEY: &str = "anime";

/// A title page's path and the key it's kept under.
pub fn title_page(id: &str) -> (String, String) {
    (format!("/api/title/{id}"), format!("title-{id}"))
}

/// What happens on Home.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Home opened for `profile`.
    Opened {
        profile: String,
    },
    /// Try again, after it couldn't load.
    Retry,
    /// Home is the page showing (nothing over it): only then does the stage turn by itself.
    Showing {
        on: bool,
    },
    /// Something holds the stage still: it has the remote's focus, or a finger on it.
    Hold {
        on: bool,
    },
    /// The stage turned to slide `index` (its small pictures, its dots).
    Hero {
        index: u32,
    },
    /// Swiped on to the next slide, or back to the last.
    Next,
    Previous,
    /// The stage's Play: a film plays, a show opens its page.
    HeroPlay,
    /// The stage's More info.
    HeroInfo,
    /// A card pressed: `row` and `card` its place in the view's rows.
    Tapped {
        row: u32,
        card: u32,
    },
    /// A card's ⓘ (or a long press): its title's page.
    Info {
        row: u32,
        card: u32,
    },
}

/// A row as Home draws it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowView {
    pub title: String,
    pub cards: Vec<Card>,
    /// Landscape cards on the phone too.
    pub wide: bool,
    /// The Top 10: numbered places.
    pub ranked: bool,
    /// Continue watching: its cards play, and offer their page (ⓘ).
    pub continuing: bool,
}

/// Exactly what Home draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub hero: Vec<Item>,
    /// The slide showing.
    pub hero_at: u32,
    /// How long a slide stays (ms): the stage's bar fills over it.
    pub stay_ms: u32,
    /// The next slide's picture, read ahead so it fades in on a picture already there...
    pub warm: Option<String>,
    /// ...this long after the slide came in (ms).
    pub warm_after_ms: u32,
    pub play: String,
    pub info: String,
    pub rows: Vec<RowView>,
    /// Nothing to show yet: the rows' skeleton.
    pub loading: bool,
    pub trouble: Option<Trouble>,
}

#[derive(Clone, Debug, PartialEq)]
enum Ask {
    /// The device's copy of a page, of any age, for the first frame.
    Kept(String),
    /// A page from the store.
    Page(String),
    Turn,
}

/// Where Home is in getting its first frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Closed,
    /// Reading what the device kept.
    Kept,
    /// Reading the kept pages of the shows its own rows need.
    Titles,
    /// Drawn; the store keeps it current.
    Live,
}

/// Home's model. See the module's docs.
#[derive(Debug, Default)]
pub struct HomePage {
    profile: Option<String>,
    base: String,
    now: f64,
    phase: Phase,
    /// The latest body of each page, by key.
    bodies: HashMap<String, String>,
    /// Why Home's own page didn't come, until one does.
    failed: Option<(String, bool)>,
    busy: bool,
    /// The title pages asked of the store.
    asked: HashSet<String>,
    asks: Asks<Ask>,
    waiting: usize,
    home: Option<Home>,
    rows: Vec<RowView>,
    hero_at: usize,
    showing: bool,
    held: bool,
    turn: Option<u32>,
}

impl HomePage {
    fn keys(&self) -> [(String, String); 4] {
        let p = self.profile.clone().unwrap_or_default();
        [
            (HOME_PATH.into(), home::KEY.into()),
            (crate::progress::PATH.into(), mine("progress", &p)),
            (UP_NEXT.into(), mine("upnext", &p)),
            (ANIME.into(), ANIME_KEY.into()),
        ]
    }

    fn load(&mut self, key: &str) -> Effect {
        self.waiting += 1;
        Effect::Load { id: self.asks.ask(Ask::Kept(key.into())), key: key.into() }
    }

    fn page(&mut self, path: &str, key: &str, need: Need) -> Effect {
        Effect::Page { id: self.asks.ask(Ask::Page(key.into())), path: path.into(), key: key.into(), need }
    }

    /// Every page asked of the store: Home's own as `need` says.
    fn go_live(&mut self, need: Need) -> Vec<Effect> {
        self.phase = Phase::Live;
        let mut effects: Vec<Effect> = self
            .keys()
            .iter()
            .map(|(path, key)| {
                let need = if key == home::KEY { need } else { Need::Any };
                self.page(path, key, need)
            })
            .collect();
        effects.extend(self.look_up());
        effects
    }

    /// The shows the own rows name: Continue watching's and New episodes'.
    fn shows(&self) -> Vec<String> {
        let mut ids: Vec<String> =
            self.spots().iter().map(|s| s.show.clone()).chain(self.up_next().into_iter().map(|n| n.id)).collect();
        let mut seen = HashSet::new();
        ids.retain(|id| !id.is_empty() && seen.insert(id.clone()));
        ids
    }

    /// The title pages the own rows need from the store: a show with no card in Home or the anime list (for its
    /// art), and an anime with none kept (it says which season each episode is in, for "S2 E1").
    fn look_up(&mut self) -> Vec<Effect> {
        let known = self.known();
        let wanted: Vec<String> = self
            .shows()
            .into_iter()
            .filter(|id| {
                !known.contains_key(id) || (id.starts_with("anime-") && !self.bodies.contains_key(&title_page(id).1))
            })
            .filter(|id| !self.asked.contains(id))
            .collect();
        wanted
            .into_iter()
            .map(|id| {
                self.asked.insert(id.clone());
                let (path, key) = title_page(&id);
                self.page(&path, &key, Need::Any)
            })
            .collect()
    }

    fn body(&self, key: &str) -> Option<Value> {
        self.bodies.get(key).and_then(|b| serde_json::from_str(b).ok())
    }

    fn spots(&self) -> Vec<Spot> {
        let key = mine("progress", self.profile.as_deref().unwrap_or_default());
        let saved = self.bodies.get(&key).and_then(|b| crate::progress::saved(b).ok()).unwrap_or_default();
        saved
            .into_iter()
            .map(|s| Spot {
                key: s.key,
                show: s.show,
                title: s.title,
                ep: s.ep,
                cover: s.cover,
                at: s.at,
                length: s.length,
                updated: s.updated,
            })
            .collect()
    }

    fn up_next(&self) -> Vec<UpNext> {
        self.body(&mine("upnext", self.profile.as_deref().unwrap_or_default()))
            .map(|o| home::up_next(&o))
            .unwrap_or_default()
    }

    /// Each title's card as the catalogue has it: Home's lists, the anime list, then the title pages read.
    fn known(&self) -> HashMap<String, Item> {
        let mut known: HashMap<String, Item> = HashMap::new();
        let anime = self.body(ANIME_KEY).map(|o| model::items(&o, "items")).unwrap_or_default();
        for i in self.home.iter().flat_map(Home::everything).chain(anime.iter()) {
            known.entry(i.id.clone()).or_insert_with(|| i.clone());
        }
        for id in self.shows() {
            if let (false, Some(o)) = (known.contains_key(&id), self.body(&title_page(&id).1)) {
                known.insert(id.clone(), model::card_of(&model::title(&id, &o)));
            }
        }
        known
    }

    /// "S2 E1" for a spot when its title's page says the episode's season, else "E1".
    fn episode_name(&self, show: &str, episode: Option<&str>, n: Option<i64>) -> Option<String> {
        let season = episode.and_then(|id| {
            let o = self.body(&title_page(show).1)?;
            model::objects(&o, "episodes")
                .into_iter()
                .find(|e| model::t(e, "id") == id)
                .and_then(|e| model::i(e, "season"))
        });
        home::episode_short(season, n)
    }

    /// The rows worked out again from the pages as they are now.
    fn rebuild(&mut self) {
        let shown = self.home.as_ref().and_then(|h| h.hero.get(self.hero_at)).map(|i| i.id.clone());
        self.home = self.body(home::KEY).map(|o| home::parse(&o));
        // (a newer Home keeps the slide showing, wherever it is now)
        self.hero_at = shown.and_then(|id| self.home.as_ref()?.hero.iter().position(|i| i.id == id)).unwrap_or(0);
        let Some(home) = self.home.as_ref() else {
            self.rows.clear();
            return;
        };
        let spots = self.spots();
        let known = self.known();
        let dress = |card: Item| match known.get(&card.id) {
            Some(k) => home::dress(card, k),
            None => card,
        };
        let resume: Vec<&Spot> = home::continue_watching(&spots).into_iter().map(|i| &spots[i]).collect();
        // (each with when its show was last watched: a next episode's show, by the episode it finished)
        let watched = |show: &str| spots.iter().filter(|s| s.show == show).map(|s| s.updated).fold(0., f64::max);
        let mut cards: Vec<(f64, Item)> = resume
            .iter()
            .map(|s| (s.updated, dress(home::continue_card(s, self.episode_name(&s.show, Some(&s.key), s.ep).as_deref()))))
            .collect();
        cards.extend(
            self.up_next()
                .iter()
                .map(|n| (watched(&n.id), dress(home::up_next_card(n, self.episode_name(&n.id, n.resume.as_deref(), n.ep).as_deref())))),
        );
        let share: HashMap<&str, f32> = resume
            .iter()
            .map(|s| (s.key.as_str(), if s.length > 0. { (s.at / s.length).clamp(0., 1.) as f32 } else { 0. }))
            .collect();
        let marks = Marks::new(&home.rows);
        let now = self.now as i64;
        let view = |row: &Row| {
            let mut cards = marks.row(row, now);
            if row.continuing {
                for c in &mut cards {
                    // (a next episode, not started: no bar)
                    c.progress = c.item.resume.as_deref().and_then(|k| share.get(k)).copied();
                }
            }
            RowView { title: row.title.clone(), cards, wide: row.wide, ranked: row.ranked, continuing: row.continuing }
        };
        let own = home::own_rows(cards);
        self.rows = own.iter().chain(home.rows.iter()).filter(|r| !r.items.is_empty()).map(view).collect();
    }

    fn slides(&self) -> usize {
        self.home.as_ref().map_or(0, |h| h.hero.len())
    }

    /// The stage set turning again from now, when it should; else stopped.
    fn rearm(&mut self) -> Vec<Effect> {
        let mut effects: Vec<Effect> = self.turn.take().map(|id| Effect::Cancel { id }).into_iter().collect();
        if self.showing && !self.held && self.slides() > 1 {
            let id = self.asks.ask(Ask::Turn);
            self.turn = Some(id);
            effects.push(Effect::Timer { id, ms: home::HERO_STAY as u32 });
        }
        effects
    }

    fn turn_to(&mut self, index: usize) -> Vec<Effect> {
        let n = self.slides();
        if n > 0 {
            self.hero_at = index % n;
        }
        self.rearm()
    }

    fn card(&self, row: u32, card: u32) -> Option<&Item> {
        self.rows.get(row as usize)?.cards.get(card as usize).map(|c| &c.item)
    }

    fn hero(&self) -> Option<&Item> {
        self.home.as_ref()?.hero.get(self.hero_at)
    }

    fn open(item: &Item) -> Effect {
        Effect::Open { route: Route::Title { id: item.id.clone() } }
    }

    /// Answers in: the first frame's reads, then the store's pages.
    fn took(&mut self) -> Vec<Effect> {
        match self.phase {
            Phase::Kept if self.waiting == 0 => {
                // (the kept pages of the shows the own rows name: their art and seasons, on the first frame too)
                self.phase = Phase::Titles;
                let wanted: Vec<String> = self
                    .shows()
                    .into_iter()
                    .map(|id| title_page(&id).1)
                    .filter(|k| !self.bodies.contains_key(k))
                    .collect();
                let effects: Vec<Effect> = wanted.iter().map(|k| self.load(k)).collect();
                if effects.is_empty() {
                    return self.took();
                }
                effects
            }
            Phase::Titles if self.waiting == 0 => {
                self.rebuild();
                let mut effects = self.go_live(Need::Any);
                effects.extend(self.rearm());
                effects
            }
            Phase::Live => {
                let before = self.slides();
                self.rebuild();
                let mut effects = self.look_up();
                if self.slides() != before || self.turn.is_none() {
                    effects.extend(self.rearm());
                }
                effects
            }
            _ => vec![],
        }
    }
}

impl Screen for HomePage {
    type Event = Event;
    type View = View;

    fn update(&mut self, event: Event, cx: &Context) -> Vec<Effect> {
        self.now = cx.clock.now;
        self.base = cx.base.clone();
        match event {
            Event::Opened { profile } => {
                if self.profile.as_ref() == Some(&profile) {
                    return vec![];
                }
                let showing = self.showing;
                *self = Self {
                    profile: Some(profile),
                    base: cx.base.clone(),
                    now: cx.clock.now,
                    phase: Phase::Kept,
                    showing,
                    ..Self::default()
                };
                let keys: Vec<String> = self.keys().into_iter().map(|(_, key)| key).collect();
                keys.iter().map(|k| self.load(k)).collect()
            }
            Event::Retry if self.phase == Phase::Live => {
                self.busy = true;
                self.asked.clear();
                self.go_live(Need::Server)
            }
            Event::Retry => vec![],
            Event::Showing { on } => {
                self.showing = on;
                self.rearm()
            }
            Event::Hold { on } => {
                self.held = on;
                self.rearm()
            }
            Event::Hero { index } => self.turn_to(index as usize),
            Event::Next => self.turn_to(self.hero_at + 1),
            Event::Previous => self.turn_to(self.hero_at + self.slides().max(1) - 1),
            Event::HeroPlay => match self.hero() {
                Some(i) if i.is_movie() => vec![Effect::Play { show: i.id.clone(), episode: None }],
                Some(i) => vec![Self::open(i)],
                None => vec![],
            },
            Event::HeroInfo => self.hero().map(Self::open).into_iter().collect(),
            Event::Tapped { row, card } => self.card(row, card).map(cards::tap).into_iter().collect(),
            Event::Info { row, card } => self.card(row, card).map(Self::open).into_iter().collect(),
        }
    }

    fn answer(&mut self, id: u32, reply: Reply, cx: &Context) -> Vec<Effect> {
        self.now = cx.clock.now;
        let Some(ask) = self.asks.take(id) else { return vec![] };
        match (ask, reply) {
            (Ask::Turn, Reply::Fired) => {
                self.turn = None;
                return self.turn_to(self.hero_at + 1);
            }
            (Ask::Kept(key), reply) => {
                self.waiting = self.waiting.saturating_sub(1);
                if let Reply::Kept { body, .. } = reply {
                    self.bodies.entry(key).or_insert(body);
                }
            }
            (Ask::Page(key), Reply::Page { page }) => {
                if key == home::KEY {
                    self.busy = false;
                    self.failed = page.failure.as_ref().map(|f| (f.words.clone(), f.kind == crate::api::Kind::Offline));
                }
                if let Some(body) = page.body {
                    self.bodies.insert(key, body);
                }
            }
            _ => return vec![],
        }
        self.took()
    }

    fn app(&mut self, event: &AppEvent, cx: &Context) -> Vec<Effect> {
        if self.phase != Phase::Live {
            return vec![];
        }
        self.now = cx.clock.now;
        match event {
            AppEvent::PageChanged { key } => {
                let path = self.keys().into_iter().find(|(_, k)| k == key).map(|(p, _)| p);
                let path = path.or_else(|| {
                    key.strip_prefix("title-").filter(|id| self.asked.contains(*id)).map(|id| title_page(id).0)
                });
                path.map(|p| self.page(&p, key, Need::Any)).into_iter().collect()
            }
            AppEvent::Watched => {
                let [_, progress, next, _] = self.keys();
                vec![self.page(&progress.0, &progress.1, Need::Server), self.page(&next.0, &next.1, Need::Server)]
            }
            AppEvent::Connectivity { online: true, .. } if self.failed.is_some() && self.home.is_none() => {
                self.busy = true;
                vec![self.page(HOME_PATH, home::KEY, Need::Server)]
            }
            _ => vec![],
        }
    }

    fn view(&self) -> View {
        let hero = self.home.as_ref().map(|h| h.hero.clone()).unwrap_or_default();
        let warm = (hero.len() > 1)
            .then(|| hero.get((self.hero_at + 1) % hero.len()))
            .flatten()
            .and_then(|i| crate::pictures::stage(i.cover_big.as_deref(), i.cover.as_deref(), i.poster.as_deref()))
            .map(|p| crate::pictures::url(&self.base, p));
        let trouble = match (&self.home, &self.failed) {
            (None, Some((message, offline))) => {
                Some(Trouble { message: message.clone(), offline: *offline, busy: self.busy })
            }
            _ => None,
        };
        View {
            hero,
            hero_at: self.hero_at as u32,
            stay_ms: home::HERO_STAY as u32,
            warm,
            warm_after_ms: crate::pictures::WARM_AFTER_MS,
            play: "Play".into(),
            info: "More info".into(),
            loading: self.rows.is_empty() && trouble.is_none(),
            rows: self.rows.clone(),
            trouble,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root::{Root, ScreenId, Turn};
    use crate::screen::{response, test_context, Clock};
    use serde_json::json;

    const NOW: f64 = 1_794_657_600.;
    const CLOCK: Clock = Clock { now: NOW, utc_offset: 0 };

    /// Home on a root: what the device kept and what the server says, by key.
    struct Page {
        root: Root,
        id: ScreenId,
        kept: HashMap<String, String>,
        server: HashMap<String, Reply>,
        asked: Vec<String>,
        timers: Vec<u32>,
    }

    impl Page {
        fn new(kept: &[(&str, String)], server: &[(&str, String)]) -> Self {
            let mut root = Root::new(test_context(NOW).platform, "https://host.invalid".into(), CLOCK);
            let id = root.open(HomePage::default());
            Self {
                root,
                id,
                kept: kept.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
                server: server.iter().map(|(k, v)| (k.to_string(), response(200, v, None))).collect(),
                asked: vec![],
                timers: vec![],
            }
        }

        fn send(&mut self, event: Event) -> Vec<Effect> {
            let turn = self.root.update::<HomePage>(self.id, event, CLOCK);
            self.run(turn)
        }

        /// Runs a turn's effects to the end; what's left for the app's pages (play, open).
        fn run(&mut self, turn: Turn) -> Vec<Effect> {
            let mut queue: Vec<Effect> = turn.orders.into_iter().map(|o| o.effect).collect();
            let mut out = vec![];
            while !queue.is_empty() {
                let effect = queue.remove(0);
                let turn = match &effect {
                    Effect::Load { id, key } => {
                        let reply = self.kept.get(key).map_or(Reply::Missing, |b| Reply::Kept {
                            body: b.clone(),
                            etag: None,
                            at: 0.,
                        });
                        self.root.answer(*id, reply, CLOCK)
                    }
                    Effect::Request { id, request } => {
                        let path = request.url.trim_start_matches("https://host.invalid").to_string();
                        self.asked.push(path.clone());
                        let key = match path.as_str() {
                            "/api/home" => "home".to_string(),
                            "/api/progress" => "me-progress-p".into(),
                            "/api/up-next" => "me-upnext-p".into(),
                            "/api/anime" => "anime".into(),
                            p => format!("title-{}", p.trim_start_matches("/api/title/")),
                        };
                        let reply = self
                            .server
                            .get(&key)
                            .cloned()
                            .unwrap_or(Reply::Unreachable { offline: true, message: "x".into() });
                        self.root.answer(*id, reply, CLOCK)
                    }
                    Effect::Timer { id, .. } => {
                        self.timers.push(*id);
                        continue;
                    }
                    Effect::Store { .. } | Effect::Cancel { .. } => continue,
                    _ => {
                        out.push(effect);
                        continue;
                    }
                };
                queue.extend(turn.orders.into_iter().map(|o| o.effect));
            }
            out
        }

        fn view(&self) -> View {
            self.root.view::<HomePage>(self.id).unwrap_or_default()
        }
    }

    fn home_body() -> String {
        json!({
            "hero": [{"id": "h1", "title": "One", "type": "movie", "coverBig": "/big1.jpg"}, {"id": "h2", "title": "Two", "coverBig": "/big2.jpg"}],
            "rows": [
                {"title": "Top 10", "ranked": true, "noTags": true, "items": [{"id": "show", "title": "Show", "cover": "/show.jpg", "logo": "/logo.png"}]},
                {"title": "Dramas", "items": [{"id": "d1", "title": "D1", "newAt": NOW as i64 - 100, "newKind": "episode"}]},
                {"title": "Empty", "items": []}
            ]
        })
        .to_string()
    }

    fn progress_body() -> String {
        json!({"e2": {"show": "show", "title": "Show", "ep": 2, "t": 600, "d": 3000, "updated": NOW - 60.}}).to_string()
    }

    fn title_body() -> String {
        json!({"id": "show", "title": "Show", "episodes": [{"id": "e2", "n": 2, "season": 3}]}).to_string()
    }

    #[test]
    fn the_first_frame_is_whats_kept_with_continue_watching_already_in() {
        let kept = [("home", home_body()), ("me-progress-p", progress_body()), ("title-show", title_body())];
        let mut page = Page::new(&kept, &[]);
        page.send(Event::Opened { profile: "p".into() });
        let v = page.view();
        assert_eq!(
            v.rows.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            ["Continue watching", "Top 10", "Dramas"]
        );
        let cw = &v.rows[0].cards[0];
        assert_eq!(cw.item.sub.as_deref(), Some("S3 E2 · 40m left"));
        assert_eq!(cw.item.logo.as_deref(), Some("/logo.png"));
        assert_eq!(cw.progress, Some(0.2));
        assert!(cw.top10);
        assert_eq!(v.rows[2].cards[0].tag.as_deref(), Some("New Episode"));
        assert_eq!(v.hero.len(), 2);
        assert_eq!(v.warm.as_deref(), Some("https://host.invalid/big2.jpg"));
        assert!(!v.loading && v.trouble.is_none());
    }

    #[test]
    fn the_stage_turns_only_while_home_shows_and_nothing_holds_it() {
        let mut page = Page::new(&[], &[("home", home_body())]);
        page.send(Event::Opened { profile: "p".into() });
        assert!(page.timers.is_empty());
        page.send(Event::Showing { on: true });
        let first = *page.timers.last().unwrap_or(&0);
        let turn = page.root.answer(first, Reply::Fired, CLOCK);
        page.run(turn);
        assert_eq!(page.view().hero_at, 1);
        page.send(Event::Next);
        assert_eq!(page.view().hero_at, 0);
        page.send(Event::Previous);
        assert_eq!(page.view().hero_at, 1);
        let before = page.timers.len();
        page.send(Event::Hold { on: true });
        assert_eq!(page.timers.len(), before);
        assert_eq!(page.send(Event::HeroPlay), [Effect::Open { route: Route::Title { id: "h2".into() } }]);
        page.send(Event::Hero { index: 0 });
        assert_eq!(page.send(Event::HeroPlay), [Effect::Play { show: "h1".into(), episode: None }]);
    }

    #[test]
    fn cards_play_or_open_and_their_info_opens() {
        let kept = [("home", home_body()), ("me-progress-p", progress_body())];
        let mut page = Page::new(&kept, &[]);
        page.send(Event::Opened { profile: "p".into() });
        assert_eq!(
            page.send(Event::Tapped { row: 0, card: 0 }),
            [Effect::Play { show: "show".into(), episode: Some("e2".into()) }]
        );
        assert_eq!(
            page.send(Event::Info { row: 0, card: 0 }),
            [Effect::Open { route: Route::Title { id: "show".into() } }]
        );
        assert_eq!(
            page.send(Event::Tapped { row: 2, card: 0 }),
            [Effect::Open { route: Route::Title { id: "d1".into() } }]
        );
        assert!(page.send(Event::Tapped { row: 9, card: 0 }).is_empty());
    }

    #[test]
    fn offline_with_nothing_kept_is_trouble_until_the_network_is_back() {
        let mut page = Page::new(&[], &[]);
        page.send(Event::Opened { profile: "p".into() });
        let v = page.view();
        assert_eq!(v.trouble.as_ref().map(|t| t.offline), Some(true));
        assert!(!v.loading);
        page.server.insert("home".into(), response(200, &home_body(), None));
        let turn = page.root.app(AppEvent::Connectivity { online: true, metered: false }, CLOCK);
        page.run(turn);
        let v = page.view();
        assert!(v.trouble.is_none() && v.rows.len() == 2);
    }

    #[test]
    fn a_show_not_in_the_lists_is_looked_up_and_watching_asks_again() {
        let progress =
            json!({"x1": {"show": "anime-x", "title": "X", "ep": 1, "t": 60, "d": 1400, "updated": NOW}}).to_string();
        let title = json!({"id": "anime-x", "title": "X", "source": "anime", "coverBig": "/x.jpg", "textless": true, "cover": "/xc.jpg",
                           "episodes": [{"id": "x1", "n": 1, "season": 2}]}).to_string();
        let mut page = Page::new(&[("home", home_body()), ("me-progress-p", progress)], &[("title-anime-x", title)]);
        page.send(Event::Opened { profile: "p".into() });
        assert!(page.asked.contains(&"/api/title/anime-x".to_string()));
        let v = page.view();
        assert_eq!(v.rows[0].cards[0].item.sub.as_deref(), Some("S2 E1 · 22m left"));
        assert_eq!(v.rows[0].cards[0].item.cover.as_deref(), Some("/xc.jpg"));
        page.asked.clear();
        let turn = page.root.app(AppEvent::Watched, CLOCK);
        page.run(turn);
        assert_eq!(page.asked, ["/api/progress", "/api/up-next"]);
    }
}
