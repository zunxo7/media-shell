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
use crate::downloads::{self, Act, State};
use crate::queue::{self, Snapshot, WantedEpisode, WantedTitle};
use crate::screen::{AppEvent, Asks, Context, Device, Effect, Need, Reply, Route, Screen, Trouble};
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
    /// The film's Download, or the show's Download all, pressed: it does what [`View::download`] says.
    Download,
    /// An episode row's download control pressed: it does what its [`Mark::act`] says.
    EpisodeDownload {
        id: String,
    },
    /// The page's glow colour is known (or it has none): its picture may come in.
    GlowReady,
}

/// How long a title's picture waits for the page's glow to come first, ms.
pub const GLOW_WAIT_MS: u32 = 1200;

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
    /// A small row's name with its number first ("3. Name"); the name alone when it has no number.
    pub numbered: String,
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
    /// Its download control; None where it can't be downloaded here.
    pub download: Option<Mark>,
}

/// A download's control on an episode's row.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    /// Where its download stands; None: not downloaded.
    pub state: Option<State>,
    /// How far along, 0 to 1.
    pub share: f32,
    /// What pressing it does (the one rule for every download's control, [`downloads::tap`]).
    pub act: Act,
}

/// The film's Download, or a show's Download all (its episodes, not its extras): what it says and does for the episodes
/// as they are ([`downloads::batch`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Download {
    pub act: Act,
    pub label: String,
    /// About what it'll take, while it adds ("~1.4 GB"; exact for a Movies & TV film).
    pub chip: Option<String>,
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
    /// Its IMDb rating, drawn before the facts.
    pub rating: Option<String>,
    /// What parts the rating from the facts: only when both are there.
    pub rating_gap: String,
    /// Its logo, when it has one (shown in place of the name).
    pub logo: Option<String>,
    /// What lights the page: its logo and its art.
    pub glow: crate::pictures::GlowArt,
    /// The logo is only shown if it is in Latin letters: the app checks the picture and shows the name when it isn't
    /// (`art::logo_shown(true, latin, None)`).
    pub logo_check: bool,
    /// Its age rating's badge (none for anime, whose ratings aren't its own).
    pub age: Option<String>,
    pub story: String,
    /// All of the story shows, not just four lines.
    pub story_open: bool,
    pub cast: Option<String>,
    pub genres: Option<String>,
    /// "Cast: ", with its colon and space: the line's label, then its value beside it.
    pub cast_label: String,
    pub genres_label: String,
    /// What Play says: the countdown while it's not out yet.
    pub play: String,
    /// Not out yet: Play and the episodes do nothing; Play's words count down each second.
    pub locked: bool,
    /// "From E1", once you're into a show.
    pub from_start: Option<String>,
    pub watching: Option<Watching>,
    /// The second button is Download (a film's, or a title with no episodes), not From E1.
    pub download_one: bool,
    /// The film's Download or the show's Download all; None where it can't be downloaded here.
    pub download: Option<Download>,
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
    /// Its picture waits for the glow to light the page first (till the glow's ready, or [`GLOW_WAIT_MS`]).
    pub art_held: bool,
}

/// Download and Download all as they stand ([`TitlePage::plan`]).
struct Plan {
    wanted: WantedTitle,
    /// A film's one (or a title with no episodes), not a show's Download all.
    one: bool,
    /// The show's episodes Download all takes (empty for a film).
    episodes: Vec<model::Episode>,
    act: Act,
    label: String,
    /// Which of them (their places) a press acts on.
    targets: Vec<usize>,
    of: usize,
    /// Its size is exact when it comes: one film file from Movies & TV.
    exact: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Ask {
    Page(String),
    Order,
    /// One of More like this's asks of the catalogue.
    Like,
    /// A second gone: a title not out yet counts down.
    Tick,
    /// The picture has waited long enough for the glow.
    GlowWait,
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
    /// The countdown's second is running.
    ticking: bool,
    /// This profile's downloads, as the queue last told them.
    held: Snapshot,
    /// What the queue is to be told.
    outbox: Vec<queue::Event>,
    /// The title the queue was last asked to weigh.
    weighed: Option<String>,
    device: Option<Device>,
    /// The picture is held for the glow to come first.
    glow_held: bool,
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

    /// A title not out yet: its countdown's next second.
    fn tick(&mut self) -> Option<Effect> {
        if self.ticking || !self.locked() {
            return None;
        }
        self.ticking = true;
        Some(Effect::Timer { id: self.asks.ask(Ask::Tick), ms: 1_000 })
    }

    /// Whether a film (`ep` None) or an episode can be downloaded here.
    fn offered(&self, title: &WantedTitle, ep: Option<&model::Episode>) -> bool {
        queue::offered(self.device.unwrap_or(Device::Phone), title, ep.map(WantedEpisode::of).as_ref(), &self.held.anime_audio)
    }

    /// Download and Download all as they stand: a film's one, or the show's episodes (not its extras); None where it
    /// can't be downloaded here.
    fn plan(&self) -> Option<Plan> {
        let t = self.title.as_ref()?;
        let one = t.film || t.episodes.is_empty();
        let episodes: Vec<model::Episode> =
            if one { vec![] } else { t.episodes.iter().filter(|e| downloads::in_download_all(e.season)).cloned().collect() };
        if !one && episodes.is_empty() {
            return None;
        }
        let wanted = WantedTitle::of(t);
        if !self.offered(&wanted, episodes.first()) {
            return None;
        }
        let items: Vec<(Option<State>, f32)> = if one {
            vec![self.held.video(&t.id, None).map_or((None, 0.), |h| (Some(h.state), h.share))]
        } else {
            episodes.iter().map(|e| self.held.video(&t.id, Some(&e.id)).map_or((None, 0.), |h| (Some(h.state), h.share))).collect()
        };
        let (act, label) = downloads::batch(&items, one);
        let states: Vec<Option<State>> = items.iter().map(|i| i.0).collect();
        let targets = downloads::batch_targets(&states, act);
        Some(Plan { wanted, one, episodes, act, label, targets, of: items.len(), exact: t.film && t.source == "cinema" })
    }

    /// What the title will weigh, asked of the queue once (when it's known that it can be downloaded).
    fn weigh_download(&mut self) {
        let Some(plan) = self.plan() else { return };
        if self.weighed.as_deref() == Some(plan.wanted.id.as_str()) {
            return;
        }
        self.weighed = Some(plan.wanted.id.clone());
        let episodes = plan.episodes.iter().map(WantedEpisode::of).collect();
        self.outbox.push(queue::Event::Weigh { title: plan.wanted, episodes });
    }

    fn press_download(&mut self) -> Vec<Effect> {
        let Some(plan) = self.plan() else { return vec![] };
        let id = plan.wanted.id.clone();
        let key = |i: usize| if plan.one { id.clone() } else { plan.episodes[i].id.clone() };
        match plan.act {
            Act::Open => return vec![Effect::Open { route: Route::Downloads }],
            Act::Add => {
                let episodes = if plan.one { vec![] } else { plan.targets.iter().map(|&i| WantedEpisode::of(&plan.episodes[i])).collect() };
                self.outbox.push(queue::Event::Add { title: plan.wanted.clone(), episodes });
            }
            act => self.outbox.extend(plan.targets.iter().map(|&i| queue::Event::Act { key: key(i), act })),
        }
        vec![]
    }

    fn press_episode(&mut self, id: &str) -> Vec<Effect> {
        let Some(t) = self.title.as_ref() else { return vec![] };
        let Some(e) = t.episodes.iter().find(|e| e.id == id) else { return vec![] };
        let wanted = WantedTitle::of(t);
        if !self.offered(&wanted, Some(e)) {
            return vec![];
        }
        match downloads::tap(self.held.video(&t.id, Some(id)).map(|h| h.state)) {
            Act::Open => return vec![Effect::Open { route: Route::Downloads }],
            Act::Add => self.outbox.push(queue::Event::Add { title: wanted, episodes: vec![WantedEpisode::of(e)] }),
            act => self.outbox.push(queue::Event::Act { key: id.into(), act }),
        }
        vec![]
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
                let held = std::mem::take(&mut self.held);
                *self = Self { id: id.clone(), profile, now: cx.clock.now, busy: true, glow_held: true, held, ..Self::default() };
                self.device = Some(cx.platform.device);
                let (path, key) = title_page(&id);
                let progress = self.progress_key();
                vec![
                    Effect::Timer { id: self.asks.ask(Ask::GlowWait), ms: GLOW_WAIT_MS },
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
            Event::Download => self.press_download(),
            Event::EpisodeDownload { id } => self.press_episode(&id),
            Event::GlowReady => {
                self.glow_held = false;
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
                    self.weigh_download();
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
            (Ask::Tick, _) => self.ticking = false,
            (Ask::GlowWait, _) => self.glow_held = false,
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
        effects.extend(self.tick());
        effects
    }

    fn downloads(&mut self, now: &Snapshot, _cx: &Context) -> Vec<Effect> {
        self.held = now.clone();
        self.weigh_download();
        vec![]
    }

    fn to_queue(&mut self) -> Vec<queue::Event> {
        std::mem::take(&mut self.outbox)
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
            cast_label: "Cast: ".into(),
            genres_label: "Genres: ".into(),
            episodes_title: "Episodes".into(),
            related_title: "More like this".into(),
            newest: self.newest,
            order: if self.newest { "Newest first" } else { "Oldest first" }.into(),
            story_open: self.story_open,
            reveal: self.reveal.clone(),
            art_held: self.glow_held,
            ..View::default()
        };
        let Some(t) = &self.title else { return view };
        let movie = t.kind == "movie";
        let seasons = t.seasons.iter().filter(|s| !s.extra).count();
        let length = rules::length(movie, t.minutes, t.duration.as_deref(), seasons, t.episodes.len());
        view.facts = rules::facts_line(t.year.as_deref(), length.as_deref(), t.genres.first().map(String::as_str));
        view.rating = t.rating.clone();
        if t.rating.is_some() && !view.facts.is_empty() {
            view.rating_gap = "   ·   ".into();
        }
        view.logo = t.logo.clone().filter(|l| !l.is_empty());
        view.glow = crate::pictures::GlowArt::of(t.logo.as_deref(), t.cover_big.as_deref(), t.cover.as_deref(), t.poster.as_deref());
        view.logo_check = crate::art::worth_asking(Some(&t.source), t.textless);
        view.download_one = t.film || t.episodes.is_empty();
        view.download = self.plan().map(|p| Download {
            chip: downloads::size_chip(p.act, self.held.weighed(&t.id), p.targets.len(), p.of, p.exact),
            act: p.act,
            label: p.label,
        });
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
            view.from_start = (play.watching.is_some() && !view.download_one).then(|| "From E1".into());
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
        let wanted = WantedTitle::of(t);
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
                    numbered: rules::numbered_name(e.name.as_deref(), e.n),
                    name: rules::episode_name(e.name.as_deref(), e.n),
                    length,
                    date,
                    line,
                    description: e.description.clone(),
                    picture: e.thumb.clone().or_else(|| t.cover.clone()),
                    share,
                    download: self.offered(&wanted, Some(e)).then(|| {
                        let held = self.held.video(&t.id, Some(&e.id));
                        let state = held.map(|h| h.state);
                        Mark { state, share: held.map_or(0., |h| h.share), act: downloads::tap(state) }
                    }),
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
    use crate::queue::Queue;
    use crate::root::{Root, ScreenId, Turn};
    use crate::screen::{response, test_context, Clock, DiskReply};
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
                    Effect::Disk { id, .. } => {
                        self.root.answer(*id, Reply::Disk { disk: DiskReply { ok: true, ..Default::default() } }, CLOCK)
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
    fn the_buttons_and_the_name_are_decided_here() {
        let mut page = Page::new();
        page.server.insert("/api/title/dar".into(), show(3));
        page.send(Event::Opened { id: "dar".into(), profile: "p".into() });
        let v = page.view();
        assert!(!v.download_one);
        assert!(v.rating.is_none() && v.rating_gap.is_empty() && v.logo.is_none() && !v.logo_check);

        let mut film = Page::new();
        film.server.insert("/api/title/f".into(), json!({"id": "f", "title": "F", "type": "movie", "source": "cinema",
            "year": "2020", "rating": "7.1", "logo": "https://x/l.png"}).to_string());
        film.send(Event::Opened { id: "f".into(), profile: "p".into() });
        let v = film.view();
        assert!(v.download_one && v.from_start.is_none());
        // (no IMDb id, so no copy to download from)
        assert!(v.download.is_none());
        assert_eq!((v.rating.as_deref(), v.rating_gap.as_str()), (Some("7.1"), "   ·   "));
        assert_eq!(v.logo.as_deref(), Some("https://x/l.png"));
        assert!(!v.logo_check);

        let mut anime = Page::new();
        anime.server.insert("/api/title/a".into(), json!({"id": "a", "title": "A", "type": "show", "source": "anime",
            "logo": "https://x/a.png", "episodes": [{"id": "x0", "n": 1, "season": 0}, {"id": "x1", "n": 2, "season": 1}]}).to_string());
        anime.send(Event::Opened { id: "a".into(), profile: "p".into() });
        let v = anime.view();
        assert!(v.logo_check && !v.download_one);
    }

    /// A page with the real queue beside it, started and answering the disk with nothing kept.
    fn with_queue(page: &mut Page) {
        let queue = page.root.open(Queue::default());
        let turn = page.root.update::<Queue>(queue, queue::Event::Start, CLOCK);
        page.run(turn);
    }

    #[test]
    fn download_and_download_all_are_the_pages_own_and_go_through_the_queue() {
        let mut page = Page::new();
        with_queue(&mut page);
        page.server.insert("/api/title/s".into(), json!({"id": "s", "title": "S", "type": "show", "source": "cinema", "imdb": "tt2",
            "episodes": [{"id": "x0", "n": 1, "season": 0}, {"id": "x1", "n": 2, "season": 1}, {"id": "x2", "n": 3, "season": 1}]}).to_string());
        page.send(Event::Opened { id: "s".into(), profile: "p".into() });
        let v = page.view();
        // (the episodes' controls; the extras aren't part of Download all)
        assert_eq!(v.download.as_ref().map(|d| (d.act, d.label.as_str())), Some((Act::Add, "Download all")));
        assert_eq!(v.episodes.iter().map(|e| e.download.as_ref().map(|m| m.act)).collect::<Vec<_>>(), [Some(Act::Add); 3]);
        // (pressing one episode's control adds it, and its row says it's going; Download all then takes the other)
        page.send(Event::EpisodeDownload { id: "x1".into() });
        let v = page.view();
        assert_eq!(v.episodes[1].download.as_ref().map(|m| (m.state.is_some(), m.act)), Some((true, Act::Pause)));
        assert_eq!(v.episodes[0].download.as_ref().map(|m| m.state), Some(None));
        assert_eq!(v.download.as_ref().map(|d| d.label.as_str()), Some("Download all"));
        page.send(Event::Download);
        let v = page.view();
        assert_eq!(v.episodes[2].download.as_ref().map(|m| m.state.is_some()), Some(true));
        assert_eq!(v.episodes[0].download.as_ref().map(|m| m.state), Some(None));
        // (every one of Download all's is there: its label says how they go)
        assert!(v.download.as_ref().is_some_and(|d| d.act != Act::Add), "{:?}", v.download);
    }

    #[test]
    fn a_film_not_out_yet_counts_down_and_wont_play() {
        let mut page = Page::new();
        let soon = NOW as i64 + 2 * 86_400 + 3 * 3600;
        page.server.insert("/api/title/f".into(),
            json!({"id": "f", "title": "Clayface", "type": "movie", "source": "cinema", "availableAt": soon}).to_string());
        let opened = page.send(Event::Opened { id: "f".into(), profile: "p".into() });
        let v = page.view();
        assert!(v.locked);
        assert_eq!(v.play, "Available in 2d 3h");
        assert!(page.send(Event::Play).is_empty());
        // (the core counts the seconds itself: an hour on, Play's words have moved, and the next second is asked for)
        let Some(Effect::Timer { id, ms: 1_000 }) = opened.into_iter().find(|e| matches!(e, Effect::Timer { ms: 1_000, .. })) else { panic!("no countdown") };
        let later = Clock { now: NOW + 3600., utc_offset: 0 };
        let next = page.root.answer(id, Reply::Fired, later);
        assert_eq!(page.view().play, "Available in 2d 2h");
        assert!(next.orders.iter().any(|o| matches!(o.effect, Effect::Timer { ms: 1_000, .. })));
    }

    #[test]
    fn the_picture_waits_for_the_glow_a_moment_at_most() {
        let mut page = Page::new();
        page.server.insert("/api/title/dar".into(), show(2));
        let opened = page.send(Event::Opened { id: "dar".into(), profile: "p".into() });
        assert!(page.view().art_held);
        // (the glow's colour is there: the picture comes in at once)
        page.send(Event::GlowReady);
        assert!(!page.view().art_held);
        // (a glow that never comes: the picture comes in anyway, after the wait)
        let mut other = Page::new();
        other.server.insert("/api/title/dar".into(), show(2));
        let opened_too = other.send(Event::Opened { id: "dar".into(), profile: "p".into() });
        assert!(other.view().art_held);
        let wait = |effects: Vec<Effect>| effects.into_iter().find_map(|e| match e {
            Effect::Timer { id, ms: GLOW_WAIT_MS } => Some(id),
            _ => None,
        });
        assert!(wait(opened).is_some());
        let id = wait(opened_too).expect("a wait for the glow");
        other.root.answer(id, Reply::Fired, CLOCK);
        assert!(!other.view().art_held);
        // (the picture and the pictures that light the page come from the title's own)
        assert_eq!(other.view().glow.logo, None);
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
