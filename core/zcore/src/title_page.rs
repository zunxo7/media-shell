//! A title's page as a screen: its facts, how far you are, what Play does, the story, the seasons and episodes (oldest
//! or newest first, as this device keeps it), or, with no episodes, more like it. A newer copy of the page (a show's
//! newest episode) replaces the one showing as soon as the store has it; the season and order stay as they were.
//!
//! ```
//! use zcore::title_page::{Event, TitlePage};
//! use zcore::root::Root;
//! use zcore::screen::{Clock, Device, Effect, Platform};
//!
//! let platform = Platform { device: Device::Tv, model: "TV".into(), version_name: "1.0".into(), version_code: 1,
//!                           os: "t".into(), stream: "s".into(), codecs: vec![], hdr: false };
//! let clock = Clock { now: 1_794_657_600., utc_offset: 0 };
//! let mut root = Root::new(platform, "https://host.invalid".into(), clock);
//! let page = root.open(TitlePage::default());
//! let turn = root.update::<TitlePage>(page, Event::Opened { id: "t".into(), profile: "p".into() }, clock);
//! assert!(turn.orders.iter().any(|o| matches!(&o.effect, Effect::Load { key, .. } if key == "title-t")));
//! assert!(root.view::<TitlePage>(page).is_some_and(|v| v.loading));
//! ```

use crate::cards::{self, Card, Marks};
use crate::home::Spot;
use crate::home_page::title_page;
use crate::model::{self, Item, Title};
use crate::screen::{AppEvent, Asks, Context, Effect, Need, Reply, Screen, Trouble};
use crate::session::mine;
use crate::title::{self as rules, EpisodeKey, SeasonKey};
use serde_json::Value;

/// The device's setting for a show's episodes, newest first.
pub const ORDER_PREF: &str = "epsDesc";

/// What happens on a title's page.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The page opened on title `id`, for `profile`.
    Opened {
        id: String,
        profile: String,
    },
    Retry,
    /// A season's chip.
    Season {
        id: String,
    },
    /// The order pill: newest first, or oldest.
    Order,
    /// The page's Play.
    Play,
    /// From E1: the season showing from its first episode.
    FromStart,
    /// An episode's row.
    Episode {
        id: String,
    },
    /// A card in More like this, by its place.
    Related {
        index: u32,
    },
    /// The story pressed: all of it, or four lines again.
    Story,
    /// The episode the page was to bring into view is in view.
    Revealed,
    /// A second passed: a title not out yet counts down.
    Tick,
}

/// How far you are into a film, or the episode you're on.
#[derive(Clone, Debug, PartialEq)]
pub struct Watching {
    /// "E3  ·  12m left".
    pub line: String,
    /// How far, 0 to 1.
    pub share: f32,
}

/// A season's chip.
#[derive(Clone, Debug, PartialEq)]
pub struct SeasonChip {
    pub id: String,
    pub label: String,
    pub on: bool,
}

/// An episode's row.
#[derive(Clone, Debug, PartialEq)]
pub struct EpisodeRow {
    pub id: String,
    /// "3"; "" when it has no number.
    pub number: String,
    pub name: String,
    /// "45m"; "" when it isn't known.
    pub length: String,
    /// "7 Mar 2024".
    pub date: Option<String>,
    /// Its date and length on one line ("7 Mar 2024  ·  45m").
    pub line: String,
    pub description: Option<String>,
    /// Its still, or the title's cover.
    pub picture: Option<String>,
    /// How far into it, 0 to 1.
    pub share: f32,
}

/// Exactly what a title's page draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// Nothing to show yet: the page's skeleton.
    pub loading: bool,
    pub trouble: Option<Trouble>,
    pub title: Option<Title>,
    /// "2019   ·   2h 28m   ·   Sci-Fi".
    pub facts: String,
    /// Its age rating's badge (none for anime, whose ratings aren't its own).
    pub age: Option<String>,
    pub story: String,
    /// All of the story shows, not just four lines.
    pub story_open: bool,
    pub cast: Option<String>,
    pub genres: Option<String>,
    pub cast_label: String,
    pub genres_label: String,
    /// What Play says: the countdown while it's not out yet.
    pub play: String,
    /// Not out yet: Play and the episodes do nothing; send Tick each second.
    pub locked: bool,
    /// "From E1", once you're into a show.
    pub from_start: Option<String>,
    pub watching: Option<Watching>,
    pub episodes_title: String,
    pub newest: bool,
    /// "Newest first", "Oldest first".
    pub order: String,
    /// The season chips, when there's more than one season.
    pub seasons: Vec<SeasonChip>,
    /// The season showing's episodes, in the order picked.
    pub episodes: Vec<EpisodeRow>,
    pub related_title: String,
    pub related: Vec<Card>,
    /// An episode to bring into view (the one the player was last on); the app says when it has with Revealed.
    pub reveal: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
enum Ask {
    Page(String),
    Order,
    /// One of More like this's asks of the catalogue.
    Like,
}

/// A title page's model. See the module's docs.
#[derive(Debug, Default)]
pub struct TitlePage {
    id: String,
    profile: String,
    now: f64,
    asks: Asks<Ask>,
    body: Option<String>,
    title: Option<Title>,
    failed: Option<(String, bool)>,
    busy: bool,
    spots: Vec<Spot>,
    season: Option<String>,
    /// The season's been picked (by the viewer, or the player): a newer page keeps it.
    picked: bool,
    newest: bool,
    story_open: bool,
    /// Home's and the anime list's cards, for More like this off Movies & TV's shelf.
    shelf: Vec<Option<String>>,
    pool: Vec<Item>,
    asking_like: usize,
    related: Vec<Item>,
    marks: Marks,
    reveal: Option<String>,
    warmed: Option<(String, Option<String>)>,
}

impl TitlePage {
    fn page(&mut self, path: &str, key: &str, need: Need) -> Effect {
        Effect::Page { id: self.asks.ask(Ask::Page(key.into())), path: path.into(), key: key.into(), need }
    }

    fn progress_key(&self) -> String {
        mine("progress", &self.profile)
    }

    fn keys(&self) -> Vec<EpisodeKey> {
        self.title
            .iter()
            .flat_map(|t| &t.episodes)
            .map(|e| EpisodeKey { id: e.id.clone(), season: e.season_id.clone() })
            .collect()
    }

    fn play_rule(&self) -> Option<rules::Play> {
        let t = self.title.as_ref()?;
        Some(rules::play(
            &t.id,
            t.kind == "movie",
            &self.keys(),
            !t.seasons.is_empty(),
            self.season.as_deref(),
            &self.spots,
        ))
    }

    /// The season showing's episodes, as indices.
    fn in_season(&self) -> Vec<usize> {
        let has_seasons = self.title.as_ref().is_some_and(|t| !t.seasons.is_empty());
        rules::in_season(&self.keys(), has_seasons, self.season.as_deref())
    }

    /// The page's body read again; the season it opens on picked, unless one already was.
    fn read(&mut self, cx: &Context) -> Vec<Effect> {
        let Some(o) = self.body.as_deref().and_then(|b| serde_json::from_str::<Value>(b).ok()) else { return vec![] };
        let first = self.title.is_none();
        let t = model::title(&self.id, &o);
        let seasons: Vec<SeasonKey> =
            t.seasons.iter().map(|s| SeasonKey { id: s.id.clone(), extra: s.extra }).collect();
        self.title = Some(t);
        let keep = self.picked
            && self.title.as_ref().is_some_and(|t| t.seasons.iter().any(|s| Some(&s.id) == self.season.as_ref()));
        if !keep {
            let ids: Vec<String> =
                self.title.iter().flat_map(|t| &t.episodes).map(|e| e.id.clone()).chain([self.id.clone()]).collect();
            let resume = rules::resume(&self.spots, &ids).map(|i| self.spots[i].key.clone());
            self.season = rules::season_shown(&self.keys(), &seasons, resume.as_deref());
        }
        let mut effects = vec![];
        if first && self.title.as_ref().is_some_and(|t| t.episodes.is_empty()) {
            effects.extend(self.ask_like(cx));
        }
        effects
    }

    /// More like this's pool: Movies & TV's own catalogue by the title's rarer genres; anything else, the cards Home
    /// and the anime list know on the same shelf.
    fn ask_like(&mut self, cx: &Context) -> Vec<Effect> {
        let Some(t) = self.title.as_ref() else { return vec![] };
        if t.source == "cinema" {
            let mut genres = rules::related_genres(&t.genres);
            if genres.is_empty() {
                genres.push(String::new());
            }
            let kind = t.media_type.clone().unwrap_or_default();
            self.asking_like = genres.len();
            genres
                .iter()
                .map(|g| {
                    let request = crate::api::get(&cx.base, &cx.who(), &crate::sections::related_path(&kind, g), None);
                    Effect::Request { id: self.asks.ask(Ask::Like), request }
                })
                .collect()
        } else {
            self.shelf = vec![None, None];
            vec![self.page("/api/home", crate::home::KEY, Need::Any), self.page("/api/anime", "anime", Need::Any)]
        }
    }

    fn weigh(&mut self) {
        let Some(t) = self.title.as_ref() else { return };
        let me = rules::Candidate {
            id: t.id.clone(),
            genres: t.genres.clone(),
            year: crate::model::year_number(t.year.as_deref()),
            kind: Some(t.kind.clone()),
        };
        let pool: Vec<rules::Candidate> = self
            .pool
            .iter()
            .map(|i| rules::Candidate {
                id: i.id.clone(),
                genres: i.genres.clone(),
                year: crate::model::year_number(i.year.as_deref()),
                kind: i.kind.clone(),
            })
            .collect();
        self.related = rules::related(&me, &pool, rules::RELATED).into_iter().map(|i| self.pool[i].clone()).collect();
    }

    /// What Play would start, read ahead whenever that changes.
    fn warm(&mut self) -> Vec<Effect> {
        let Some(t) = self.title.as_ref() else { return vec![] };
        let episode = self.play_rule().and_then(|p| p.episode).and_then(|i| t.episodes.get(i)).map(|e| e.id.clone());
        if t.kind != "movie" && episode.is_none() {
            return vec![];
        }
        let target = (t.id.clone(), episode);
        if self.warmed.as_ref() == Some(&target) {
            return vec![];
        }
        self.warmed = Some(target.clone());
        vec![Effect::Warm { show: target.0, episode: target.1 }]
    }

    fn locked(&self) -> bool {
        self.title.as_ref().is_some_and(|t| crate::dates::countdown(t.available_at, self.now).is_some())
    }

    fn play(&self, episode: Option<String>) -> Vec<Effect> {
        vec![Effect::Play { show: self.id.clone(), episode }]
    }

    fn spots_from(body: &str) -> Vec<Spot> {
        crate::progress::saved(body)
            .unwrap_or_default()
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
}

impl Screen for TitlePage {
    type Event = Event;
    type View = View;

    fn update(&mut self, event: Event, cx: &Context) -> Vec<Effect> {
        self.now = cx.clock.now;
        match event {
            Event::Opened { id, profile } => {
                if self.id == id && self.profile == profile {
                    return vec![];
                }
                *self = Self { id: id.clone(), profile, now: cx.clock.now, busy: true, ..Self::default() };
                let (path, key) = title_page(&id);
                let progress = self.progress_key();
                vec![
                    Effect::ReadPref { id: self.asks.ask(Ask::Order), key: ORDER_PREF.into() },
                    self.page(crate::progress::PATH, &progress, Need::Any),
                    self.page(&path, &key, Need::Any),
                ]
            }
            Event::Retry if !self.id.is_empty() => {
                self.busy = true;
                let (path, key) = title_page(&self.id);
                vec![self.page(&path, &key, Need::Server)]
            }
            Event::Retry => vec![],
            Event::Season { id } => {
                self.season = Some(id);
                self.picked = true;
                self.warm()
            }
            Event::Order => {
                self.newest = !self.newest;
                vec![Effect::WritePref { key: ORDER_PREF.into(), value: Some(self.newest.to_string()) }]
            }
            Event::Tick => vec![],
            Event::Play | Event::FromStart | Event::Episode { .. } if self.locked() => vec![],
            Event::Play => {
                let Some(t) = self.title.as_ref() else { return vec![] };
                let episode =
                    self.play_rule().and_then(|p| p.episode).and_then(|i| t.episodes.get(i)).map(|e| e.id.clone());
                self.play(episode)
            }
            Event::FromStart => {
                let first =
                    self.in_season().first().and_then(|&i| self.title.as_ref()?.episodes.get(i)).map(|e| e.id.clone());
                first.map(|e| self.play(Some(e))).unwrap_or_default()
            }
            Event::Episode { id } => self.play(Some(id)),
            Event::Related { index } => self.related.get(index as usize).map(cards::tap).into_iter().collect(),
            Event::Story => {
                self.story_open = !self.story_open;
                vec![]
            }
            Event::Revealed => {
                self.reveal = None;
                vec![]
            }
        }
    }

    fn answer(&mut self, id: u32, reply: Reply, cx: &Context) -> Vec<Effect> {
        self.now = cx.clock.now;
        let Some(ask) = self.asks.take(id) else { return vec![] };
        let mut effects = vec![];
        match (ask, reply) {
            (Ask::Order, Reply::Pref { value }) => self.newest = value.as_deref() == Some("true"),
            (Ask::Page(key), Reply::Page { page }) if key == title_page(&self.id).1 => {
                self.busy = false;
                self.failed = page.failure.as_ref().map(|f| (f.words.clone(), f.kind == crate::api::Kind::Offline));
                if page.body.is_some() && page.body != self.body {
                    self.body = page.body;
                    effects.extend(self.read(cx));
                }
            }
            (Ask::Page(key), Reply::Page { page }) if key == self.progress_key() => {
                if let Some(body) = page.body {
                    self.spots = Self::spots_from(&body);
                }
            }
            (Ask::Page(key), Reply::Page { page }) => {
                let at = usize::from(key != crate::home::KEY);
                let body = page.body.as_deref().and_then(|b| serde_json::from_str::<Value>(b).ok());
                if key == crate::home::KEY {
                    if let Some(o) = &body {
                        self.marks = Marks::new(&crate::home::parse(o).rows);
                    }
                }
                if let Some(slot) = self.shelf.get_mut(at) {
                    *slot = Some(page.body.unwrap_or_default());
                }
                if self.shelf.iter().all(Option::is_some) {
                    let mut all: Vec<Item> = vec![];
                    for (i, body) in self.shelf.iter().flatten().enumerate() {
                        let Ok(o) = serde_json::from_str::<Value>(body) else { continue };
                        if i == 0 {
                            all.extend(crate::home::parse(&o).everything().cloned());
                        } else {
                            all.extend(model::items(&o, "items"));
                        }
                    }
                    let mut seen = std::collections::HashSet::new();
                    let source = self.title.as_ref().map(|t| t.source.clone()).unwrap_or_default();
                    all.retain(|i| seen.insert(i.id.clone()) && rules::same_shelf(&source, i.source.as_deref()));
                    self.pool = all;
                    self.weigh();
                }
            }
            (Ask::Like, reply) => {
                if let crate::api::Outcome::Body { body, .. } = crate::api::outcome(reply, cx.online) {
                    if let Ok(o) = serde_json::from_str::<Value>(&body) {
                        self.pool.extend(model::items(&o, "items"));
                    }
                }
                self.asking_like = self.asking_like.saturating_sub(1);
                if self.asking_like == 0 {
                    self.weigh();
                }
            }
            _ => return vec![],
        }
        effects.extend(self.warm());
        effects
    }

    fn app(&mut self, event: &AppEvent, _cx: &Context) -> Vec<Effect> {
        if self.id.is_empty() {
            return vec![];
        }
        match event {
            // (a newer copy of the page: the newest episode shows at once)
            AppEvent::PageChanged { key } if *key == title_page(&self.id).1 => {
                let (path, key) = title_page(&self.id);
                vec![self.page(&path, &key, Need::Any)]
            }
            AppEvent::PageChanged { key } if *key == self.progress_key() => {
                let key = self.progress_key();
                vec![self.page(crate::progress::PATH, &key, Need::Any)]
            }
            AppEvent::Played { show, episode: Some(episode) } if *show == self.id => {
                let Some(t) = self.title.as_ref().filter(|t| t.kind != "movie") else { return vec![] };
                let Some(e) = t.episodes.iter().find(|e| &e.id == episode) else { return vec![] };
                if !t.seasons.is_empty() && e.season_id.is_some() {
                    self.season = e.season_id.clone();
                    self.picked = true;
                }
                self.reveal = Some(episode.clone());
                vec![]
            }
            _ => vec![],
        }
    }

    fn view(&self) -> View {
        let trouble = match (&self.title, &self.failed) {
            (None, Some((message, offline))) => {
                Some(Trouble { message: message.clone(), offline: *offline, busy: self.busy })
            }
            _ => None,
        };
        let mut view = View {
            loading: self.title.is_none() && trouble.is_none(),
            trouble,
            cast_label: "Cast".into(),
            genres_label: "Genres".into(),
            episodes_title: "Episodes".into(),
            related_title: "More like this".into(),
            newest: self.newest,
            order: if self.newest { "Newest first" } else { "Oldest first" }.into(),
            story_open: self.story_open,
            reveal: self.reveal.clone(),
            ..View::default()
        };
        let Some(t) = &self.title else { return view };
        let movie = t.kind == "movie";
        let seasons = t.seasons.iter().filter(|s| !s.extra).count();
        let length = rules::length(movie, t.minutes, t.duration.as_deref(), seasons, t.episodes.len());
        view.facts = rules::facts_line(t.year.as_deref(), length.as_deref(), t.genres.first().map(String::as_str));
        view.age = t.age.clone().filter(|_| t.source != "anime");
        view.story = rules::story(t.description.as_deref(), t.synopsis.as_deref());
        view.cast = (!t.cast.is_empty()).then(|| t.cast.iter().take(5).cloned().collect::<Vec<_>>().join(", "));
        view.genres = (!t.genres.is_empty()).then(|| t.genres.iter().take(4).cloned().collect::<Vec<_>>().join(", "));
        if let Some(play) = self.play_rule() {
            let spot = play.watching.and_then(|i| self.spots.get(i)).filter(|s| s.length > 0. && s.at > 0.);
            view.watching = spot.map(|s| Watching {
                line: rules::watching_line(movie, s.ep, s.at, s.length),
                share: (s.at / s.length).clamp(0., 1.) as f32,
            });
            view.from_start = (play.watching.is_some() && !t.episodes.is_empty()).then(|| "From E1".into());
            view.play = play.label;
        }
        if let Some(left) = crate::dates::countdown(t.available_at, self.now) {
            view.play = left;
            view.locked = true;
        }
        if t.seasons.len() > 1 {
            view.seasons = t
                .seasons
                .iter()
                .map(|s| SeasonChip {
                    id: s.id.clone(),
                    label: s.label.clone(),
                    on: Some(&s.id) == self.season.as_ref(),
                })
                .collect();
        }
        let mut shown = self.in_season();
        if self.newest {
            shown.reverse();
        }
        view.episodes = shown
            .into_iter()
            .filter_map(|i| t.episodes.get(i))
            .map(|e| {
                let length = rules::episode_length(e.length);
                let date = e.date.as_deref().and_then(crate::dates::short_date);
                let line = [date.clone(), Some(length.clone()).filter(|l| !l.is_empty())]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("  ·  ");
                let share = self
                    .spots
                    .iter()
                    .find(|s| s.key == e.id)
                    .filter(|s| s.length > 0.)
                    .map_or(0., |s| (s.at / s.length).min(1.) as f32);
                EpisodeRow {
                    id: e.id.clone(),
                    number: e.n.map(|n| n.to_string()).unwrap_or_default(),
                    name: rules::episode_name(e.name.as_deref(), e.n),
                    length,
                    date,
                    line,
                    description: e.description.clone(),
                    picture: e.thumb.clone().or_else(|| t.cover.clone()),
                    share,
                }
            })
            .collect();
        if t.episodes.is_empty() {
            let now = self.now as i64;
            view.related = self.related.iter().map(|i| self.marks.grid(i, now)).collect();
        }
        view.title = Some(t.clone());
        view
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root::{Root, ScreenId, Turn};
    use crate::screen::{response, test_context, Clock};
    use serde_json::json;
    use std::collections::HashMap;

    const NOW: f64 = 1_794_657_600.;
    const CLOCK: Clock = Clock { now: NOW, utc_offset: 0 };

    struct Page {
        root: Root,
        id: ScreenId,
        server: HashMap<String, String>,
        kept: HashMap<String, (String, f64)>,
        pref: Option<String>,
    }

    impl Page {
        fn new() -> Self {
            let mut root = Root::new(test_context(NOW).platform, "https://host.invalid".into(), CLOCK);
            let id = root.open(TitlePage::default());
            Self { root, id, server: HashMap::new(), kept: HashMap::new(), pref: None }
        }

        fn send(&mut self, event: Event) -> Vec<Effect> {
            let turn = self.root.update::<TitlePage>(self.id, event, CLOCK);
            self.run(turn)
        }

        fn run(&mut self, turn: Turn) -> Vec<Effect> {
            let mut queue: Vec<Effect> = turn.orders.into_iter().map(|o| o.effect).collect();
            let mut out = vec![];
            while !queue.is_empty() {
                let effect = queue.remove(0);
                let turn = match &effect {
                    Effect::Load { id, key } => {
                        let reply = self.kept.get(key).map_or(Reply::Missing, |(b, at)| Reply::Kept {
                            body: b.clone(),
                            etag: None,
                            at: *at,
                        });
                        self.root.answer(*id, reply, CLOCK)
                    }
                    Effect::ReadPref { id, .. } => {
                        self.root.answer(*id, Reply::Pref { value: self.pref.clone() }, CLOCK)
                    }
                    Effect::Request { id, request } => {
                        let path = request.url.trim_start_matches("https://host.invalid").to_string();
                        let reply = self
                            .server
                            .get(&path)
                            .map_or(Reply::Unreachable { offline: true, message: "x".into() }, |b| {
                                response(200, b, None)
                            });
                        self.root.answer(*id, reply, CLOCK)
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
            self.root.view::<TitlePage>(self.id).unwrap_or_default()
        }
    }

    fn show(episodes: usize) -> String {
        let eps: Vec<Value> = (1..=episodes)
            .map(|n| json!({"id": format!("e{n}"), "n": n, "seasonId": if n > 2 { "s2" } else { "s1" }, "length": 2700, "date": format!("2026-11-0{n}")}))
            .collect();
        json!({"id": "dar", "title": "Dar-e-Nijaat", "type": "show", "source": "ary", "year": "2026", "genres": ["Drama"], "age": "13+",
               "seasons": [{"id": "s1", "label": "Season 1"}, {"id": "s2", "label": "Season 2"}], "episodes": eps, "cast": ["A", "B"]})
        .to_string()
    }

    #[test]
    fn a_shows_page_and_its_newest_episode_the_moment_it_comes() {
        let mut page = Page::new();
        page.server.insert("/api/title/dar".into(), show(3));
        page.server.insert(
            "/api/progress".into(),
            json!({"e2": {"show": "dar", "title": "D", "ep": 2, "t": 900, "d": 2700, "updated": NOW}}).to_string(),
        );
        let warmed = page.send(Event::Opened { id: "dar".into(), profile: "p".into() });
        assert!(warmed.contains(&Effect::Warm { show: "dar".into(), episode: Some("e2".into()) }));
        let v = page.view();
        assert_eq!(v.facts, "2026   ·   2 Seasons   ·   Drama");
        assert_eq!(v.play, "Resume E2");
        assert_eq!(v.from_start.as_deref(), Some("From E1"));
        assert_eq!(v.watching.as_ref().map(|w| w.line.as_str()), Some("E2  ·  30m left"));
        assert_eq!(v.seasons.iter().filter(|s| s.on).map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s1"]);
        assert_eq!(v.episodes.len(), 2);
        assert_eq!(v.episodes[1].line, "2 Nov 2026  ·  45m");
        assert_eq!(v.cast.as_deref(), Some("A, B"));
        // (the server has a newer page: told as changed, it's shown at once, the season kept)
        page.send(Event::Season { id: "s2".into() });
        page.kept.clear();
        page.server.insert("/api/title/dar".into(), show(4));
        let asked = page.root.update::<TitlePage>(page.id, Event::Retry, CLOCK);
        page.run(asked);
        let v = page.view();
        assert_eq!(v.episodes.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["e3", "e4"]);
    }

    #[test]
    fn a_film_not_out_yet_counts_down_and_wont_play() {
        let mut page = Page::new();
        let soon = NOW as i64 + 2 * 86_400 + 3 * 3600;
        page.server.insert("/api/title/f".into(),
            json!({"id": "f", "title": "Clayface", "type": "movie", "source": "cinema", "availableAt": soon}).to_string());
        page.send(Event::Opened { id: "f".into(), profile: "p".into() });
        let v = page.view();
        assert!(v.locked);
        assert_eq!(v.play, "Available in 2d 3h");
        assert!(page.send(Event::Play).is_empty());
    }

    #[test]
    fn a_page_changed_in_the_store_is_read_again() {
        let mut page = Page::new();
        page.server.insert("/api/title/dar".into(), show(2));
        page.send(Event::Opened { id: "dar".into(), profile: "p".into() });
        assert_eq!(page.view().episodes.len(), 2);
        // (another screen asked the server and got a newer copy)
        page.server.insert("/api/title/dar".into(), show(3));
        let fetched = page.root.page("/api/title/dar", "title-dar", Need::Server, 1, CLOCK);
        let fetched = page.run(fetched);
        assert!(fetched.is_empty());
        assert_eq!(page.view().seasons.len(), 2);
        assert_eq!(page.view().episodes.len(), 2);
        page.send(Event::Season { id: "s2".into() });
        assert_eq!(page.view().episodes.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["e3"]);
    }

    #[test]
    fn play_from_start_episodes_and_the_order() {
        let mut page = Page::new();
        page.pref = Some("true".into());
        page.server.insert("/api/title/dar".into(), show(2));
        page.send(Event::Opened { id: "dar".into(), profile: "p".into() });
        let v = page.view();
        assert!(v.newest && v.order == "Newest first" && v.from_start.is_none() && v.play == "Play E1");
        assert_eq!(v.episodes[0].id, "e2");
        assert_eq!(page.send(Event::Play), [Effect::Play { show: "dar".into(), episode: Some("e1".into()) }]);
        assert_eq!(page.send(Event::FromStart), [Effect::Play { show: "dar".into(), episode: Some("e1".into()) }]);
        assert_eq!(
            page.send(Event::Episode { id: "e2".into() }),
            [Effect::Play { show: "dar".into(), episode: Some("e2".into()) }]
        );
        assert_eq!(
            page.send(Event::Order),
            [Effect::WritePref { key: ORDER_PREF.into(), value: Some("false".into()) }]
        );
        assert_eq!(page.view().episodes[0].id, "e1");
    }

    #[test]
    fn after_playing_the_page_shows_the_episode_last_played() {
        let mut page = Page::new();
        page.server.insert("/api/title/dar".into(), show(3));
        page.send(Event::Opened { id: "dar".into(), profile: "p".into() });
        let mut nav = crate::nav::Nav::default();
        let cx = test_context(NOW);
        nav.update(crate::nav::Event::Open { route: crate::screen::Route::Title { id: "dar".into() } }, &cx);
        nav.update(crate::nav::Event::Play { show: "dar".into(), episode: Some("e1".into()), local: false }, &cx);
        nav.update(crate::nav::Event::PlayerEpisode { episode: "e3".into() }, &cx);
        for effect in nav.update(crate::nav::Event::PlayerClosed, &cx) {
            if let Effect::Tell { event } = effect {
                let turn = page.root.app(event, CLOCK);
                page.run(turn);
            }
        }
        let v = page.view();
        assert_eq!(v.reveal.as_deref(), Some("e3"));
        assert!(v.seasons[1].on);
        page.send(Event::Revealed);
        assert_eq!(page.view().reveal, None);
    }

    #[test]
    fn a_film_with_more_like_it_and_trouble_offline() {
        let mut page = Page::new();
        page.server.insert(
            "/api/title/m".into(),
            json!({"id": "m", "title": "M", "type": "movie", "source": "cinema", "mediaType": "movie",
            "genres": ["Horror", "Drama"], "minutes": 100})
            .to_string(),
        );
        page.server.insert("/api/cinema?type=movie&genre=Horror".into(), json!({"items": [
            {"id": "h1", "title": "H1", "type": "movie", "genres": ["Horror"]}, {"id": "m", "title": "M", "genres": ["Horror"]}]}).to_string());
        let warmed = page.send(Event::Opened { id: "m".into(), profile: "p".into() });
        assert!(warmed.contains(&Effect::Warm { show: "m".into(), episode: None }));
        let v = page.view();
        assert_eq!(v.related.iter().map(|c| c.item.id.as_str()).collect::<Vec<_>>(), ["h1"]);
        assert_eq!(v.play, "Play");
        assert_eq!(
            page.send(Event::Related { index: 0 }),
            [Effect::Open { route: crate::screen::Route::Title { id: "h1".into() } }]
        );

        let mut gone = Page::new();
        gone.send(Event::Opened { id: "x".into(), profile: "p".into() });
        let v = gone.view();
        assert!(!v.loading && v.trouble.is_some_and(|t| t.offline));
    }
}
