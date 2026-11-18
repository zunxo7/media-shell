//! The app from its first frame to a profile in use: the splash while the account is checked, then signing in, or
//! Who's watching (however many profiles: it's where profiles are added and managed); picking or switching a profile; signing out, and being
//! signed out by the server. The apps draw each stage and play their own splash; this decides what comes next.

use crate::api::{self, Kind, Outcome};
use crate::pictures;
use crate::screen::{AppEvent, Context, Device, Effect, Reply, Request, Screen};
use serde_json::{json, Value};

/// Where the account and its profiles come from.
pub const PROFILES: &str = "/api/profiles";
/// The sign-in wall's posters.
const BACKDROP: &str = "/api/backdrop";
/// How long the account check may take before the app goes in without it (offline).
const CHECK_MS: u32 = 5_000;
/// How often an account waiting for the owner's approval is looked at again.
const WAIT_MS: u32 = 15_000;
/// Whether maintenance is over: asked this often while it's on.
pub const MAINTENANCE_PATH: &str = "/api/maintenance";
pub const MAINTENANCE_MS: u32 = 15_000;
/// The header the server's maintenance answers carry.
pub const MAINTENANCE_HEADER: &str = "X-Zunflix-Maintenance";

/// The server's maintenance answer (a 503 with `X-Zunflix-Maintenance`): the owner's message.
pub fn maintenance(reply: &Reply) -> Option<String> {
    let Reply::Response { status: 503, headers, body } = reply else { return None };
    crate::screen::header(headers, MAINTENANCE_HEADER)?;
    let v: Value = serde_json::from_str(body).unwrap_or_default();
    Some(v["message"].as_str().unwrap_or_default().to_string())
}
/// The longest sign-in waits after the splash for the wall's posters (a slow line: the card comes, they follow).
pub const WALL_WAIT_MS: u32 = 1_500;
/// The most posters on the sign-in wall.
const WALL_MOST: usize = 24;
/// How long a picked profile shines in the middle before its Home comes in, at least.
pub const SHINE_MS: u32 = 900;

pub const FILL_IN: &str = "Fill in every field.";

/// A profile of the account.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
    /// Its place among the account's profiles: picks its colour when it has no picture.
    pub index: u32,
    /// Its settings as the server keeps them (JSON).
    pub settings: Option<String>,
}

/// The signed-in account, as `/api/profiles` says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Account {
    pub profiles: Vec<Profile>,
    /// The profile in use; the first when the server names none.
    pub current: Option<String>,
    /// The account has Plus (2K and 4K).
    pub plus: bool,
    /// How many profiles it may have.
    pub max_profiles: u32,
}

impl Account {
    /// The profile in use.
    pub fn me(&self) -> Option<&Profile> {
        self.current.as_ref().and_then(|id| self.profiles.iter().find(|p| &p.id == id))
    }
}

/// The account from `/api/profiles`' answer; None when it can't be read.
/// Whether the account check's answer says the account waits for the owner's approval.
pub fn pending(answer: &str) -> bool {
    serde_json::from_str::<Value>(answer).ok().and_then(|o| o.get("pending")?.as_bool()).unwrap_or(false)
}

pub fn account(answer: &str) -> Option<Account> {
    let o: Value = serde_json::from_str(answer).ok()?;
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
    let profiles: Vec<Profile> = o
        .get("profiles")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter(|p| p.is_object())
                .enumerate()
                .map(|(i, p)| Profile {
                    id: text(p, "id").unwrap_or_default(),
                    name: text(p, "name").unwrap_or_default(),
                    avatar: text(p, "avatar"),
                    index: u32::try_from(i).unwrap_or(u32::MAX),
                    settings: p.get("settings").filter(|s| s.is_object()).map(Value::to_string),
                })
                .collect()
        })
        .unwrap_or_default();
    let named = text(&o, "current").filter(|id| profiles.iter().any(|p| &p.id == id));
    let current = named.or_else(|| profiles.first().map(|p| p.id.clone()));
    let max_profiles = o.get("maxProfiles").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(u32::MAX);
    Some(Account { profiles, current, plus: o.get("plus").and_then(Value::as_bool).unwrap_or(false), max_profiles })
}

/// The name a profile's own page (its saved spots, its up next) is kept under on the device.
pub fn mine(what: &str, profile: &str) -> String {
    format!("me-{what}-{profile}")
}

/// What the app shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    /// The splash, while the account is checked.
    #[default]
    Splash,
    SignIn,
    /// A new account waiting for the owner's approval: in by itself once it's given.
    Waiting,
    /// Who's watching.
    Who,
    /// The app itself.
    Main,
    /// The owner has switched zunflix off for maintenance: once it's over, "We're back" and Continue.
    Maintenance,
}

/// The sign-in card's tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The TV's: sign in with a phone (its own code page).
    Phone,
    Create,
    LogIn,
}

/// The sign-in card.
#[derive(Clone, Debug, PartialEq)]
pub struct SignInView {
    pub mode: Mode,
    /// The TV offers signing in with a phone.
    pub phone_tab: bool,
    pub line: String,
    pub button: String,
    pub busy: bool,
    pub error: Option<String>,
    /// Goes up with each failure: the card shakes.
    pub shakes: u32,
    /// The "Remember me" tick, everywhere but the TV (a TV stays signed in); None there.
    pub remember: Option<bool>,
    /// The tabs' names: the TV's phone tab, Create account, Log in.
    pub phone_tab_title: String,
    pub create_tab_title: String,
    pub log_in_tab_title: String,
    pub remember_title: String,
}

/// Exactly what to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub stage: Stage,
    /// In without the server (offline at the start): the app opens where it works offline.
    pub offline: bool,
    pub sign_in: SignInView,
    /// The sign-in wall's posters, at a poster card's size.
    pub wall: Vec<String>,
    /// The Waiting stage's words, and its way out.
    pub waiting_title: String,
    pub waiting_line: String,
    pub waiting_sign_out: String,
    /// The Maintenance stage's words: its title, the owner's message (or a line of its own), and "back soon".
    pub maintenance_title: String,
    pub maintenance_message: String,
    pub maintenance_back: String,
    /// Maintenance is over: the card says so, with Continue (its words).
    pub maintenance_over: bool,
    pub maintenance_continue: String,
    pub who_title: String,
    /// The profile being picked: it shines in the middle while the others fade.
    pub picking: Option<String>,
    /// Why a profile couldn't be picked.
    pub who_note: Option<String>,
    /// Whether Who's watching wears the wordmark: not when it's only a switch from Home's menu (the pick shining alone).
    pub who_mark: bool,
    pub account: Option<Account>,
    /// Who's watching's managing words: its heading and button, the button that ends it, the add tile.
    pub who_manage: String,
    pub who_done: String,
    pub who_add: String,
    /// Who's watching's heading now: "Who's watching?", or "Manage profiles" while managing.
    pub who_heading: String,
    /// This profile manages the others (the account's first): Manage profiles and Add profile show.
    pub can_manage: bool,
    /// Managing: a tap on a profile opens its edit card instead of watching as it.
    pub managing: bool,
    /// The edit card open from Who's watching: a profile's id, or "" for a new one.
    pub editing: Option<String>,
}

impl Default for View {
    fn default() -> Self {
        Session::default().view()
    }
}

/// What happened.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The app opened.
    Started,
    /// The app's splash has played through once.
    SplashDone,
    Mode { mode: Mode },
    Submit { username: String, password: String, remember: bool },
    /// The TV signed in another way (a phone approved its code): the account is checked.
    SignedInElsewhere,
    /// A profile picked (Who's watching, or the menu's switch).
    Pick { id: String },
    /// Switching profile from the menu: Who's watching again.
    Switch,
    /// The viewer signed out.
    SignOut,
    /// The server no longer knows this device.
    SignedOut,
    /// Manage profiles pressed (`on`), or Done.
    Manage { on: bool },
    /// Add profile pressed on Who's watching.
    Add,
    /// The edit card opened from Who's watching closed.
    EditClosed,
    /// Continue pressed once maintenance is over.
    Continue,
    /// The sign-in wall's first posters are in (the app read them while the splash played).
    WallShown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ask {
    Check,
    Wall,
    Auth,
    Select,
    Shine,
    SignOut,
    /// Time to look again while waiting for approval.
    Wait,
    /// Time to ask whether maintenance is over, and the asking.
    Recheck,
    Maintenance,
    /// Long enough waiting on the wall's posters: on to sign-in without them.
    WallWait,
}

/// The session. See the module's docs.
#[derive(Debug)]
pub struct Session {
    stage: Stage,
    offline: bool,
    splash_done: bool,
    /// The account check's outcome while the splash still plays: Some(None) when it said signed out.
    checked: Option<Option<Account>>,
    /// The account waits for the owner's approval.
    pending: bool,
    checking: bool,
    account: Option<Account>,
    /// The wall's posters are in (or there are none to wait for), or waited for long enough.
    wall_in: bool,
    /// Sign-in held after the splash until the wall's posters are in (WALL_WAIT_MS at most).
    held: bool,
    mode: Mode,
    tv: bool,
    remember: Option<bool>,
    busy: bool,
    error: Option<String>,
    shakes: u32,
    wall: Vec<String>,
    picking: Option<String>,
    /// The pick was made from Home's menu, not on Who's watching.
    switched: bool,
    /// The pick's answer and its shine, each when done: (selected, shone).
    pick: (Option<Result<(), String>>, bool),
    who_note: Option<String>,
    managing: bool,
    editing: Option<String>,
    /// Maintenance's message, and the stage it came over (gone back to once it's over).
    maintenance: String,
    /// The check found it over: waiting on Continue.
    maintenance_done: bool,
    before: Stage,
    asks: crate::screen::Asks<Ask>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            stage: Stage::Splash,
            offline: false,
            splash_done: false,
            checked: None,
            pending: false,
            checking: false,
            account: None,
            mode: Mode::LogIn,
            tv: false,
            remember: None,
            busy: false,
            error: None,
            shakes: 0,
            wall: vec![],
            wall_in: false,
            held: false,
            picking: None,
            switched: false,
            pick: (None, false),
            who_note: None,
            managing: false,
            editing: None,
            maintenance: String::new(),
            maintenance_done: false,
            before: Stage::Splash,
            asks: Default::default(),
        }
    }
}

impl Session {
    fn get(&mut self, ask: Ask, path: &str, cx: &Context) -> Effect {
        let mut request = api::get(&cx.base, &cx.who(), path, None);
        if ask == Ask::Check {
            request.timeout_ms = CHECK_MS;
        }
        Effect::Request { id: self.asks.ask(ask), request }
    }

    fn post(&mut self, ask: Ask, path: &str, body: Value, cx: &Context) -> Effect {
        let request: Request = api::post(&cx.base, &cx.who(), path, body.to_string());
        Effect::Request { id: self.asks.ask(ask), request }
    }

    /// The account known (signed in) or not: on to the stage that follows, and the profile in use told.
    fn arrive(&mut self, account: Option<Account>, offline: bool, cx: &Context) -> Vec<Effect> {
        self.offline = offline;
        match account {
            Some(account) => {
                self.account = Some(account);
                self.stage = Stage::Who;
                vec![]
            }
            None if offline => {
                self.stage = Stage::Main;
                vec![]
            }
            // (not before the wall's posters are in: the card never comes over an empty wall)
            None if !self.wall_in => {
                let first = !self.held;
                self.held = true;
                if first { vec![Effect::Timer { id: self.asks.ask(Ask::WallWait), ms: WALL_WAIT_MS }] } else { vec![] }
            }
            None => self.back_to_sign_in(cx),
        }
    }

    /// The wall's posters are in, or won't be: a held sign-in goes on.
    fn wall_in(&mut self, cx: &Context) -> Vec<Effect> {
        self.wall_in = true;
        if std::mem::take(&mut self.held) && self.stage == Stage::Splash {
            return self.back_to_sign_in(cx);
        }
        vec![]
    }

    fn back_to_sign_in(&mut self, cx: &Context) -> Vec<Effect> {
        self.stage = Stage::SignIn;
        self.account = None;
        self.busy = false;
        self.picking = None;
        self.mode = if self.tv { Mode::Phone } else { Mode::LogIn };
        if self.wall.is_empty() {
            vec![self.get(Ask::Wall, BACKDROP, cx)]
        } else {
            vec![]
        }
    }

    fn fail(&mut self, words: String) {
        self.busy = false;
        self.error = Some(words);
        self.shakes += 1;
    }

    /// Waiting for the owner's approval: the account looked at again in a while.
    fn wait(&mut self) -> Vec<Effect> {
        self.stage = Stage::Waiting;
        vec![Effect::Timer { id: self.asks.ask(Ask::Wait), ms: WAIT_MS }]
    }

    /// Maintenance: the poster wall asked for if it isn't here, and whether it's over asked in a while.
    fn under_maintenance(&mut self, message: &str, cx: &Context) -> Vec<Effect> {
        self.maintenance = message.trim().to_string();
        if self.stage == Stage::Maintenance {
            // (on again while saying "We're back": back to the on look, asked again in a while)
            if !std::mem::take(&mut self.maintenance_done) {
                return vec![];
            }
            return vec![Effect::Timer { id: self.asks.ask(Ask::Recheck), ms: MAINTENANCE_MS }];
        }
        self.before = self.stage;
        self.stage = Stage::Maintenance;
        self.busy = false;
        self.picking = None;
        self.editing = None;
        let mut effects = vec![Effect::Timer { id: self.asks.ask(Ask::Recheck), ms: MAINTENANCE_MS }];
        if self.wall.is_empty() {
            effects.push(self.get(Ask::Wall, BACKDROP, cx));
        }
        effects
    }

    /// Maintenance over: back where it was, the account checked again (in the app, by way of Who's watching, so
    /// every page loads afresh).
    fn maintenance_over(&mut self, cx: &Context) -> Vec<Effect> {
        self.maintenance_done = false;
        self.stage = match self.before {
            Stage::Main => Stage::Who,
            Stage::Maintenance => Stage::Splash,
            other => other,
        };
        self.checking = true;
        vec![self.get(Ask::Check, PROFILES, cx)]
    }

    /// This profile manages the account's profiles: the account's first.
    fn manages(&self) -> bool {
        self.account.as_ref().and_then(Account::me).is_some_and(|m| m.index == 0)
    }

    /// The pick's answer and its shine both done: in as that profile, or back on Who's watching saying why not.
    fn picked(&mut self) -> Vec<Effect> {
        let (Some(result), true) = (&self.pick.0, self.pick.1) else { return vec![] };
        let id = self.picking.take().unwrap_or_default();
        let result = result.clone();
        self.pick = (None, false);
        match result {
            Ok(()) => {
                if let Some(account) = self.account.as_mut() {
                    account.current = Some(id.clone());
                }
                self.stage = Stage::Main;
                self.managing = false;
                vec![Effect::Profile { id }]
            }
            Err(words) => {
                self.who_note = Some(words);
                vec![]
            }
        }
    }
}

impl Session {
    fn on_event(&mut self, event: Event, cx: &Context) -> Vec<Effect> {
        match event {
            Event::Started => {
                self.tv = cx.platform.device == Device::Tv;
                self.remember = (!self.tv).then_some(true);
                self.mode = if self.tv { Mode::Phone } else { Mode::LogIn };
                self.checking = true;
                vec![self.get(Ask::Check, PROFILES, cx), self.get(Ask::Wall, BACKDROP, cx)]
            }
            Event::SplashDone => {
                self.splash_done = true;
                if self.stage == Stage::Maintenance {
                    self.checked = None;
                    return vec![];
                }
                if self.pending {
                    self.checked = None;
                    return self.wait();
                }
                match self.checked.take() {
                    Some(account) => {
                        let offline = account.is_none() && self.offline;
                        self.arrive(account, offline, cx)
                    }
                    None => vec![],
                }
            }
            Event::Mode { mode } => {
                if mode != Mode::Phone || self.tv {
                    self.mode = mode;
                    self.error = None;
                }
                vec![]
            }
            Event::Submit { username, password, remember } => {
                if self.busy || self.mode == Mode::Phone {
                    return vec![];
                }
                let create = self.mode == Mode::Create;
                if username.trim().is_empty() || password.is_empty() {
                    self.fail(FILL_IN.into());
                    return vec![];
                }
                self.busy = true;
                self.error = None;
                if self.remember.is_some() {
                    self.remember = Some(remember);
                }
                let body = json!({"username": username.trim(), "password": password, "remember": self.remember.unwrap_or(true)});
                vec![self.post(Ask::Auth, if create { "/api/signup" } else { "/api/signin" }, body, cx)]
            }
            Event::SignedInElsewhere => vec![self.get(Ask::Check, PROFILES, cx)],
            Event::Pick { id } => {
                if self.picking.is_some() {
                    return vec![];
                }
                if self.managing {
                    self.editing = Some(id);
                    return vec![];
                }
                self.switched = self.stage == Stage::Main;
                self.stage = Stage::Who;
                self.who_note = None;
                self.picking = Some(id.clone());
                let mine = self.account.as_ref().and_then(|a| a.current.as_deref()) == Some(id.as_str());
                self.pick = (if mine { Some(Ok(())) } else { None }, false);
                let mut effects = vec![Effect::Timer { id: self.asks.ask(Ask::Shine), ms: SHINE_MS }];
                if !mine {
                    let path = format!("{PROFILES}/{}/select", crate::sections::encode(&id));
                    effects.push(self.post(Ask::Select, &path, json!({}), cx));
                }
                effects
            }
            Event::Switch => {
                self.switched = false;
                self.stage = Stage::Who;
                self.who_note = None;
                vec![]
            }
            Event::Manage { on } => {
                self.managing = on && self.manages();
                self.editing = None;
                vec![]
            }
            Event::Add => {
                if self.manages() {
                    self.editing = Some(String::new());
                }
                vec![]
            }
            Event::EditClosed => {
                self.editing = None;
                vec![]
            }
            Event::SignOut => {
                let out = self.post(Ask::SignOut, "/api/signout", json!({}), cx);
                let mut effects = vec![out, Effect::Forget];
                effects.extend(self.back_to_sign_in(cx));
                effects
            }
            Event::WallShown => self.wall_in(cx),
            Event::Continue => {
                if self.stage != Stage::Maintenance || !self.maintenance_done {
                    return vec![];
                }
                self.maintenance_over(cx)
            }
            Event::SignedOut => {
                if self.stage == Stage::SignIn {
                    return vec![];
                }
                let mut effects = vec![Effect::Forget];
                effects.extend(self.back_to_sign_in(cx));
                effects
            }
        }
    }

    fn on_answer(&mut self, id: u32, reply: Reply, cx: &Context) -> Vec<Effect> {
        let Some(ask) = self.asks.take(id) else { return vec![] };
        if ask == Ask::Shine {
            self.pick.1 = true;
            return self.picked();
        }
        if ask == Ask::WallWait {
            return self.wall_in(cx);
        }
        if ask == Ask::Wait {
            return if self.stage == Stage::Waiting { vec![self.get(Ask::Check, PROFILES, cx)] } else { vec![] };
        }
        if ask == Ask::Recheck {
            return if self.stage == Stage::Maintenance && !self.maintenance_done { vec![self.get(Ask::Maintenance, MAINTENANCE_PATH, cx)] } else { vec![] };
        }
        // (its own asks refused for maintenance, the account check at start too: the session runs on its own in the apps,
        // so the root never sees these)
        if let Some(message) = maintenance(&reply).filter(|_| ask != Ask::Maintenance) {
            if ask == Ask::Check {
                self.checking = false;
            }
            return self.under_maintenance(&message, cx);
        }
        if ask == Ask::Maintenance {
            if self.stage != Stage::Maintenance || self.maintenance_done {
                return vec![];
            }
            let answer = api::outcome(reply, cx.online);
            let state = match &answer {
                Outcome::Body { body, .. } => serde_json::from_str::<Value>(body).ok(),
                _ => None,
            };
            // (offline, or no clear answer: asked again in a while)
            return match state {
                // (over: the card says so and waits on Continue)
                Some(v) if v["on"] == json!(false) => {
                    self.maintenance_done = true;
                    vec![]
                }
                Some(v) => {
                    self.maintenance = v["message"].as_str().unwrap_or_default().trim().to_string();
                    vec![Effect::Timer { id: self.asks.ask(Ask::Recheck), ms: MAINTENANCE_MS }]
                }
                None => vec![Effect::Timer { id: self.asks.ask(Ask::Recheck), ms: MAINTENANCE_MS }],
            };
        }
        // (an answer refused for maintenance changes nothing: the Maintenance stage has it)
        if self.stage == Stage::Maintenance && ask != Ask::Wall {
            if ask == Ask::Check {
                self.checking = false;
            }
            return vec![];
        }
        let outcome = api::outcome(reply, cx.online);
        match (ask, outcome) {
            (Ask::Check, Outcome::Body { body, .. }) => {
                self.checking = false;
                self.pending = pending(&body);
                if self.pending {
                    self.busy = false;
                    if self.stage == Stage::Splash && !self.splash_done {
                        return vec![];
                    }
                    return self.wait();
                }
                let account = account(&body);
                if self.stage == Stage::Splash && !self.splash_done {
                    self.checked = Some(account);
                    return vec![];
                }
                // (signed in just now, or the profiles changed while in)
                if self.stage == Stage::Main || self.stage == Stage::Who {
                    self.account = account;
                    return vec![];
                }
                self.busy = false;
                self.arrive(account, false, cx)
            }
            (Ask::Check, Outcome::Failed { failure }) => {
                self.checking = false;
                let signed_out = failure.kind == Kind::SignedOut;
                if self.stage == Stage::Splash {
                    // (offline, or zunflix couldn't be reached: in all the same; the app says so where it can)
                    self.offline = !signed_out;
                    if !self.splash_done {
                        self.checked = Some(None);
                        return vec![];
                    }
                    return self.arrive(None, !signed_out, cx);
                }
                if self.busy {
                    self.fail(failure.words);
                }
                vec![]
            }
            (Ask::Wall, Outcome::Body { body, .. }) => {
                let posters = serde_json::from_str::<Value>(&body).ok();
                self.wall = posters
                    .as_ref()
                    .and_then(|o| o.get("posters")?.as_array().cloned())
                    .unwrap_or_default()
                    .iter()
                    .filter_map(Value::as_str)
                    .take(WALL_MOST)
                    .map(|p| pictures::card(&cx.base, p, pictures::POSTER))
                    .collect();
                // (no posters: nothing to wait for)
                if self.wall.is_empty() { self.wall_in(cx) } else { vec![] }
            }
            (Ask::Wall, Outcome::Failed { .. }) => self.wall_in(cx),
            (Ask::Auth, Outcome::Body { .. }) => vec![self.get(Ask::Check, PROFILES, cx)],
            (Ask::Auth, Outcome::Failed { failure }) => {
                self.fail(failure.words);
                vec![]
            }
            (Ask::Select, Outcome::Body { .. } | Outcome::Unchanged) => {
                self.pick.0 = Some(Ok(()));
                self.picked()
            }
            (Ask::Select, Outcome::Failed { failure }) => {
                self.pick.0 = Some(Err(failure.words));
                self.picked()
            }
            _ => vec![],
        }
    }

    /// Every screen told of the account when it changed (the settings and spots are the profile's in use).
    fn told(&self, before: Option<Account>, mut effects: Vec<Effect>) -> Vec<Effect> {
        if self.account != before {
            effects.push(Effect::Tell { event: AppEvent::Account { account: self.account.clone() } });
        }
        effects
    }
}

impl Screen for Session {
    type Event = Event;
    type View = View;

    fn update(&mut self, event: Event, cx: &Context) -> Vec<Effect> {
        let before = self.account.clone();
        let effects = self.on_event(event, cx);
        self.told(before, effects)
    }

    fn answer(&mut self, id: u32, reply: Reply, cx: &Context) -> Vec<Effect> {
        let before = self.account.clone();
        let effects = self.on_answer(id, reply, cx);
        self.told(before, effects)
    }

    fn app(&mut self, event: &AppEvent, cx: &Context) -> Vec<Effect> {
        match event {
            AppEvent::Maintenance { message } => self.under_maintenance(message, cx),
            AppEvent::ProfilesChanged if self.account.is_some() => vec![self.get(Ask::Check, PROFILES, cx)],
            // (the profile in use deleted: this device picks again)
            AppEvent::ProfileGone { id } if self.account.as_ref().and_then(|a| a.current.as_ref()) == Some(id) => {
                self.stage = Stage::Who;
                vec![]
            }
            _ => vec![],
        }
    }

    fn view(&self) -> View {
        let create = self.mode == Mode::Create;
        View {
            stage: self.stage,
            offline: self.offline,
            sign_in: SignInView {
                mode: self.mode,
                phone_tab: self.tv,
                line: if create { "New here? Make an account, and you're in once it's approved." } else { "Welcome back. Log in to keep watching." }.into(),
                button: if self.busy { "One moment…" } else if create { "Create account" } else { "Log in" }.into(),
                busy: self.busy,
                error: self.error.clone(),
                shakes: self.shakes,
                remember: self.remember,
                phone_tab_title: "Use your phone".into(),
                create_tab_title: "Create account".into(),
                log_in_tab_title: "Log in".into(),
                remember_title: "Remember me".into(),
            },
            wall: self.wall.clone(),
            waiting_title: "Hang tight".into(),
            waiting_line: "Your account is waiting for approval.".into(),
            waiting_sign_out: "Sign out".into(),
            maintenance_title: if self.maintenance_done { "We're back" } else { "Under maintenance" }.into(),
            maintenance_message: if self.maintenance_done { String::new() } else { self.maintenance.clone() },
            // (only when the owner left no message of their own)
            maintenance_back: if self.maintenance.is_empty() && !self.maintenance_done { "We'll be back soon.".into() } else { String::new() },
            maintenance_over: self.maintenance_done,
            maintenance_continue: "Continue".into(),
            who_title: "Who's watching?".into(),
            picking: self.picking.clone(),
            who_note: self.who_note.clone(),
            who_mark: !self.switched,
            account: self.account.clone(),
            who_manage: "Manage profiles".into(),
            who_done: "Done".into(),
            who_add: "Add profile".into(),
            who_heading: if self.managing { "Manage profiles" } else { "Who's watching?" }.into(),
            can_manage: self.manages(),
            managing: self.managing,
            editing: self.editing.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{response, test_context};

    fn requests(effects: &[Effect]) -> Vec<(u32, String)> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Request { id, request } => Some((*id, request.url.clone())),
                _ => None,
            })
            .collect()
    }

    /// The effects but the account told to every screen.
    fn own(effects: &[Effect]) -> Vec<Effect> {
        effects.iter().filter(|e| !matches!(e, Effect::Tell { event: AppEvent::Account { .. } })).cloned().collect()
    }

    fn one(n: usize) -> String {
        let profiles: Vec<Value> = (0..n).map(|i| json!({"id": format!("p{i}"), "name": format!("P{i}")})).collect();
        json!({"profiles": profiles, "current": "p0", "plus": true, "maxProfiles": 5}).to_string()
    }

    fn started(s: &mut Session) -> (u32, u32) {
        let asked = requests(&s.update(Event::Started, &test_context(0.)));
        let check = asked.iter().find(|(_, u)| u.ends_with(PROFILES)).map(|a| a.0).unwrap_or_default();
        let wall = asked.iter().find(|(_, u)| u.ends_with(BACKDROP)).map(|a| a.0).unwrap_or_default();
        (check, wall)
    }

    #[test]
    fn the_account_check_answered_with_maintenance_goes_straight_to_it() {
        let cx = test_context(0.);
        let mut s = Session::default();
        let (check, _) = started(&mut s);
        let mut refused = response(503, r#"{"maintenance": true, "message": "Back at 9."}"#, None);
        if let Reply::Response { headers, .. } = &mut refused {
            headers.push(crate::screen::Header::new(MAINTENANCE_HEADER, "1"));
        }
        s.answer(check, refused, &cx);
        s.update(Event::SplashDone, &cx);
        let view = s.view();
        assert_eq!((view.stage, view.offline, view.maintenance_message.as_str()), (Stage::Maintenance, false, "Back at 9."));
        // (a plain 503 is the server's trouble, not maintenance)
        let mut s = Session::default();
        let (check, _) = started(&mut s);
        s.answer(check, response(503, "{}", None), &cx);
        assert_ne!(s.view().stage, Stage::Maintenance);
    }

    #[test]
    fn maintenance_over_waits_on_continue() {
        let cx = test_context(0.);
        let mut s = Session::default();
        let (check, _) = started(&mut s);
        let mut refused = response(503, r#"{"maintenance": true, "message": "Back at 9."}"#, None);
        if let Reply::Response { headers, .. } = &mut refused {
            headers.push(crate::screen::Header::new(MAINTENANCE_HEADER, "1"));
        }
        let effects = s.answer(check, refused, &cx);
        let Some(Effect::Timer { id: timer, .. }) = effects.iter().find(|e| matches!(e, Effect::Timer { .. })).cloned() else { panic!("no timer") };
        let (ask, _) = requests(&s.answer(timer, Reply::Fired, &cx))[0].clone();
        assert!(s.answer(ask, response(200, r#"{"on": false}"#, None), &cx).is_empty());
        let view = s.view();
        assert_eq!((view.stage, view.maintenance_over, view.maintenance_title.as_str(), view.maintenance_message.as_str()),
            (Stage::Maintenance, true, "We're back", ""));
        assert!(s.update(Event::SplashDone, &cx).is_empty());
        let effects = s.update(Event::Continue, &cx);
        assert!(requests(&effects).iter().any(|(_, url)| url.ends_with(PROFILES)));
        assert!(!s.view().maintenance_over);
    }

    #[test]
    fn one_profile_still_meets_who_is_watching_after_the_splash() {
        let cx = test_context(0.);
        let mut s = Session::default();
        let (check, _) = started(&mut s);
        assert!(s.answer(check, response(200, &one(1), None), &cx).is_empty());
        assert_eq!(s.view().stage, Stage::Splash);
        let effects = s.update(Event::SplashDone, &cx);
        assert!(own(&effects).is_empty());
        assert!(matches!(effects.last(), Some(Effect::Tell { event: AppEvent::Account { account: Some(_) } })));
        let view = s.view();
        assert_eq!((view.stage, view.offline), (Stage::Who, false));
        assert_eq!(view.account.map(|a| (a.plus, a.max_profiles)), Some((true, 5)));
    }

    #[test]
    fn many_profiles_ask_who_and_the_pick_shines_first() {
        let cx = test_context(0.);
        let mut s = Session::default();
        let (check, _) = started(&mut s);
        s.update(Event::SplashDone, &cx);
        assert!(own(&s.answer(check, response(200, &one(3), None), &cx)).is_empty());
        assert_eq!(s.view().stage, Stage::Who);
        let effects = s.update(Event::Pick { id: "p2".into() }, &cx);
        let Some(Effect::Timer { id: shine, ms: SHINE_MS }) = effects.first().cloned() else { panic!("no shine") };
        let (select, url) = requests(&effects)[0].clone();
        assert!(url.ends_with("/api/profiles/p2/select"));
        assert!(s.answer(select, response(200, "{}", None), &cx).is_empty());
        assert_eq!(s.view().picking.as_deref(), Some("p2"));
        assert!(s.view().who_mark);
        assert_eq!(own(&s.answer(shine, Reply::Fired, &cx)), vec![Effect::Profile { id: "p2".into() }]);
        let view = s.view();
        assert_eq!(view.stage, Stage::Main);
        assert_eq!(view.account.and_then(|a| a.current), Some("p2".into()));
        // (a switch from Home's menu: the pick shines without the wordmark, which Who's watching itself brings back)
        s.update(Event::Pick { id: "p1".into() }, &cx);
        assert_eq!((s.view().stage, s.view().who_mark), (Stage::Who, false));
        s.update(Event::Switch, &cx);
        assert!(s.view().who_mark);
    }

    #[test]
    fn managing_a_tap_edits_and_only_the_main_profile_manages() {
        let cx = test_context(0.);
        let mut s = Session::default();
        let (check, _) = started(&mut s);
        s.update(Event::SplashDone, &cx);
        s.answer(check, response(200, &one(3), None), &cx);
        assert!(s.view().can_manage);
        s.update(Event::Manage { on: true }, &cx);
        assert_eq!((s.view().managing, s.view().who_heading.as_str()), (true, "Manage profiles"));
        assert!(s.update(Event::Pick { id: "p2".into() }, &cx).is_empty());
        assert_eq!((s.view().editing.as_deref(), s.view().picking), (Some("p2"), None));
        s.update(Event::EditClosed, &cx);
        s.update(Event::Add, &cx);
        assert_eq!(s.view().editing.as_deref(), Some(""));
        s.update(Event::Manage { on: false }, &cx);
        assert_eq!((s.view().managing, s.view().editing), (false, None));
        assert_eq!(s.view().who_heading, "Who's watching?");

        // (another profile in use: no managing)
        let other = json!({"profiles": [{"id": "p0", "name": "A"}, {"id": "p1", "name": "B"}], "current": "p1"}).to_string();
        let mut s = Session::default();
        let (check, _) = started(&mut s);
        s.update(Event::SplashDone, &cx);
        s.answer(check, response(200, &other, None), &cx);
        s.update(Event::Manage { on: true }, &cx);
        assert!(!s.view().can_manage && !s.view().managing);
    }

    #[test]
    fn a_refused_pick_stays_on_who_saying_why() {
        let cx = test_context(0.);
        let mut s = Session::default();
        let (check, _) = started(&mut s);
        s.update(Event::SplashDone, &cx);
        s.answer(check, response(200, &one(2), None), &cx);
        let effects = s.update(Event::Pick { id: "p1".into() }, &cx);
        let Some(Effect::Timer { id: shine, .. }) = effects.first().cloned() else { panic!() };
        let (select, _) = requests(&effects)[0].clone();
        s.answer(shine, Reply::Fired, &cx);
        assert!(s.answer(select, response(403, r#"{"error":"Not yours."}"#, None), &cx).is_empty());
        let view = s.view();
        assert_eq!((view.stage, view.picking, view.who_note), (Stage::Who, None, Some("Not yours.".into())));
    }

    #[test]
    fn sign_in_waits_for_the_wall_no_longer_than_its_cap_and_not_at_all_without_posters() {
        let cx = test_context(0.);
        let mut s = Session::default();
        let (check, wall) = started(&mut s);
        s.answer(wall, response(200, r#"{"posters": ["poster/a.jpg"]}"#, None), &cx);
        s.answer(check, response(401, "", None), &cx);
        let Some(Effect::Timer { id: wait, .. }) = s.update(Event::SplashDone, &cx).first().cloned() else { panic!("no wait") };
        s.answer(wait, Reply::Fired, &cx);
        assert_eq!(s.view().stage, Stage::SignIn);
        // (the wall couldn't be had: straight on)
        let mut s = Session::default();
        let (check, wall) = started(&mut s);
        s.answer(wall, response(500, "", None), &cx);
        s.answer(check, response(401, "", None), &cx);
        s.update(Event::SplashDone, &cx);
        assert_eq!(s.view().stage, Stage::SignIn);
    }

    #[test]
    fn signed_out_shows_the_wall_and_sign_in_goes_in() {
        let cx = test_context(0.);
        let mut s = Session::default();
        let (check, wall) = started(&mut s);
        s.answer(wall, response(200, r#"{"posters": ["poster/a.jpg"]}"#, None), &cx);
        s.answer(check, response(401, "", None), &cx);
        // (held on the splash until the wall's posters are in)
        let held = s.update(Event::SplashDone, &cx);
        assert!(matches!(held.as_slice(), [Effect::Timer { ms: WALL_WAIT_MS, .. }]));
        assert_eq!(s.view().stage, Stage::Splash);
        s.update(Event::WallShown, &cx);
        let view = s.view();
        assert_eq!((view.stage, view.sign_in.mode), (Stage::SignIn, Mode::LogIn));
        assert_eq!(view.wall, ["https://host.invalid/card/268x402/poster/a.jpg"]);
        assert!(s.update(Event::Submit { username: " ".into(), password: "x".into(), remember: true }, &cx).is_empty());
        assert_eq!((s.view().sign_in.error.as_deref(), s.view().sign_in.shakes), (Some(FILL_IN), 1));
        let effects = s.update(Event::Submit { username: "zf".into(), password: "pw".into(), remember: true }, &cx);
        let (auth, url) = requests(&effects)[0].clone();
        assert!(url.ends_with("/api/signin"));
        assert_eq!(s.view().sign_in.button, "One moment…");
        let (check, _) = requests(&s.answer(auth, response(200, "{}", None), &cx))[0].clone();
        // (one profile, still Who's watching: Add profile and Manage profiles are there)
        assert!(own(&s.answer(check, response(200, &one(1), None), &cx)).is_empty());
        assert_eq!(s.view().stage, Stage::Who);
    }

    #[test]
    fn a_new_account_waits_for_approval_and_a_wrong_password_shakes() {
        let cx = test_context(0.);
        let mut s = Session::default();
        let (check, _) = started(&mut s);
        s.answer(check, response(401, "", None), &cx);
        s.update(Event::SplashDone, &cx);
        s.update(Event::Mode { mode: Mode::Create }, &cx);
        let effects = s.update(Event::Submit { username: "a".into(), password: "b".into(), remember: true }, &cx);
        let (auth, url) = requests(&effects)[0].clone();
        assert!(url.ends_with("/api/signup"));
        let (check, _) = requests(&s.answer(auth, response(200, r#"{"ok": true}"#, None), &cx))[0].clone();
        // (made, not yet approved: it waits, looking again now and then, and goes in once it's approved)
        let effects = s.answer(check, response(200, r#"{"pending": true}"#, None), &cx);
        assert_eq!(s.view().stage, Stage::Waiting);
        let Some(Effect::Timer { id: wait, ms: WAIT_MS }) = effects.first().cloned() else { panic!("no wait") };
        let (check, _) = requests(&s.answer(wait, Reply::Fired, &cx))[0].clone();
        s.answer(check, response(200, &one(1), None), &cx);
        assert_eq!(s.view().stage, Stage::Who);

        s.update(Event::SignOut, &cx);
        s.update(Event::Mode { mode: Mode::LogIn }, &cx);
        let effects = s.update(Event::Submit { username: "a".into(), password: "b".into(), remember: true }, &cx);
        let (auth, _) = requests(&effects)[0].clone();
        s.answer(auth, response(401, r#"{"error":"Wrong password."}"#, None), &cx);
        let view = s.view();
        assert_eq!((view.stage, view.sign_in.error.as_deref(), view.sign_in.busy), (Stage::SignIn, Some("Wrong password."), false));
    }

    #[test]
    fn offline_at_the_start_goes_in_all_the_same() {
        let cx = test_context(0.);
        let mut s = Session::default();
        let (check, _) = started(&mut s);
        s.update(Event::SplashDone, &cx);
        assert!(s.answer(check, Reply::Unreachable { offline: true, message: String::new() }, &cx).is_empty());
        let view = s.view();
        assert_eq!((view.stage, view.offline), (Stage::Main, true));
    }

    #[test]
    fn signing_out_forgets_and_the_tv_starts_on_its_phone_tab() {
        let mut cx = test_context(0.);
        cx.platform.device = Device::Tv;
        let mut s = Session::default();
        s.update(Event::Started, &cx);
        let effects = s.update(Event::SignOut, &cx);
        assert!(effects.contains(&Effect::Forget));
        assert!(requests(&effects).iter().any(|(_, u)| u.ends_with("/api/signout")));
        let view = s.view();
        assert_eq!((view.stage, view.sign_in.mode, view.sign_in.phone_tab, view.sign_in.remember), (Stage::SignIn, Mode::Phone, true, None));
        assert!(s.update(Event::SignedOut, &cx).is_empty());
    }

    #[test]
    fn the_account_reads_leniently() {
        let a = account(r#"{"profiles": [{"id": "a", "name": "A", "avatar": ""}, "junk", {"id": "b", "name": "B", "avatar": "/x.png"}], "current": "gone"}"#);
        let a = a.unwrap_or_default();
        assert_eq!(a.profiles.len(), 2);
        assert_eq!((a.profiles[1].index, a.profiles[1].avatar.as_deref()), (1, Some("/x.png")));
        assert_eq!(a.current.as_deref(), Some("a"));
        assert_eq!(a.max_profiles, u32::MAX);
        assert_eq!(mine("progress", "a"), "me-progress-a");
    }
}
