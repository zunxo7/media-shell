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
/// The most posters on the sign-in wall.
const WALL_MOST: usize = 24;
/// How long a picked profile shines in the middle before its Home comes in, at least.
pub const SHINE_MS: u32 = 900;

pub const FILL_IN: &str = "Fill in every field.";
pub const BAD_EMAIL: &str = "Enter a valid email.";
pub const BAD_CODE: &str = "Enter the 6-digit code.";
/// How long before a code can be sent again (the server holds to it too).
pub const RESEND_MS: u32 = 60_000;
/// The server's answer for an email that can't sign up (not the uni's): a notice from the top, not under the form.
const NOT_ALLOWED: u16 = 422;

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
    /// Email, username, password: the code goes to the email.
    Create,
    LogIn,
    /// The code sent to the email, to make the account.
    Code,
    /// A forgotten password: the email to send a code to.
    Forgot,
    /// The code sent, and a new password.
    Reset,
}

/// A field of the sign-in card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Email,
    Code,
    Username,
    Password,
}

/// The sign-in card.
#[derive(Clone, Debug, PartialEq)]
pub struct SignInView {
    pub mode: Mode,
    /// The step's fields, in order (the first takes the focus): Create email, username, password; Log in username or
    /// email, password; Code the code; Forgot the email; Reset the code and a new password.
    pub fields: Vec<Field>,
    /// The tabs show (the TV's phone tab, Create account, Log in), not on a code's step or Forgot.
    pub tabs: bool,
    /// "Forgot password?" shows (Log in).
    pub forgot: bool,
    /// Send again shows (a code's step).
    pub resend: bool,
    /// Back shows, and the step it goes to (Code: Create; Forgot, Reset: Log in).
    pub back: Option<Mode>,
    /// The TV offers signing in with a phone.
    pub phone_tab: bool,
    pub line: String,
    pub button: String,
    pub busy: bool,
    pub error: Option<String>,
    /// Goes up with each failure: the card shakes.
    pub shakes: u32,
    /// The "Remember me" tick on Create and Log in, everywhere but the TV (a TV stays signed in); None where it doesn't show.
    pub remember: Option<bool>,
    /// The tabs' names: the TV's phone tab, Create account, Log in.
    pub phone_tab_title: String,
    pub create_tab_title: String,
    pub log_in_tab_title: String,
    pub remember_title: String,
    /// The fields' names (the username's is "Username or email" when logging in; the password's "New password" on Reset).
    pub email_title: String,
    pub username_title: String,
    pub password_title: String,
    pub code_title: String,
    /// "Forgot password?" under Log in; Back on the code, Forgot and Reset steps.
    pub forgot_title: String,
    pub back_title: String,
    /// Send the code again, and whether it may be yet (a minute after the last).
    pub resend_title: String,
    pub resend_ready: bool,
    /// A notice from the top (an email that can't sign up, a code sent again): it comes each time `toasts` goes up, and
    /// goes when `toast` is None again (NOTICE_MS later).
    pub toast: Option<String>,
    pub toasts: u32,
}

/// How long the sign-in card's notice stays.
pub const NOTICE_MS: u32 = 4_000;
/// The splash plays at least this long, from when it starts showing, whatever the check says.
pub const SPLASH_MS: u32 = 2_050;

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
    /// Add profile shows: the account has room for another.
    pub can_add: bool,
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
    /// The splash started showing: it plays through [`SPLASH_MS`] at least. `held`: the shell lets it go on itself
    /// ([`Event::SplashDone`]), as Windows does once its update check has decided.
    SplashShown { held: bool },
    /// The shell lets the splash go on (the check it held it for is over).
    SplashDone,
    Mode { mode: Mode },
    /// The card sent: the fields its step has (Create: email, username, password; Log in: username or email,
    /// password; Code: code; Forgot: email; Reset: code, password).
    Submit { email: String, username: String, password: String, code: String, remember: bool },
    /// Send the code again.
    Resend,
    /// The TV signed in another way (a phone approved its code): the account is checked.
    SignedInElsewhere,
    /// A profile picked (Who's watching, or the menu's switch: the same, on both apps).
    Pick { id: String },
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ask {
    Check,
    Wall,
    Auth,
    Select,
    Shine,
    SignOut,
    /// The splash's minimum time up.
    Splash,
    /// Time to look again while waiting for approval.
    Wait,
    /// Time to ask whether maintenance is over, and the asking.
    Recheck,
    Maintenance,
    /// The sign-up form sent: its code on the way.
    SendCode,
    /// A forgotten password's code on the way.
    Forgot,
    /// A code sent again, and the minute before another may be.
    Resend,
    ResendReady,
    /// A notice's time up (the notice it was).
    Toast(u32),
}

/// The session. See the module's docs.
#[derive(Debug)]
pub struct Session {
    stage: Stage,
    offline: bool,
    splash_done: bool,
    /// The splash's minimum time is still running, or the shell holds it.
    splash_timing: bool,
    splash_held: bool,
    /// The account check's outcome while the splash still plays: Some(None) when it said signed out.
    checked: Option<Option<Account>>,
    /// The account waits for the owner's approval.
    pending: bool,
    checking: bool,
    account: Option<Account>,
    mode: Mode,
    tv: bool,
    remember: Option<bool>,
    busy: bool,
    error: Option<String>,
    shakes: u32,
    /// Where the code went.
    email: String,
    resend_ready: bool,
    toast: Option<String>,
    toasts: u32,
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
            splash_timing: false,
            splash_held: false,
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
            email: String::new(),
            resend_ready: false,
            toast: None,
            toasts: 0,
            wall: vec![],
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

/// Enough like an email to send a code to (the server checks it fully).
fn looks_like_email(email: &str) -> bool {
    email.split_once('@').is_some_and(|(name, domain)| !name.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.'))
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
            None => self.back_to_sign_in(cx),
        }
    }

    /// The wall's posters are in (the apps say so; nothing waits on it any more).
    fn wall_in(&mut self, _cx: &Context) -> Vec<Effect> {
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

    /// A refusal: an email that can't sign up as a notice from the top, anything else under the form.
    fn refused(&mut self, failure: crate::api::Failure) -> Vec<Effect> {
        if failure.status() == NOT_ALLOWED {
            self.busy = false;
            self.notice(failure.words)
        } else {
            self.fail(failure.words);
            vec![]
        }
    }

    fn notice(&mut self, words: String) -> Vec<Effect> {
        self.toast = Some(words);
        self.toasts += 1;
        vec![Effect::Timer { id: self.asks.ask(Ask::Toast(self.toasts)), ms: NOTICE_MS }]
    }

    /// On to a code's step: the minute before it may be sent again starts.
    fn code_sent(&mut self, mode: Mode) -> Vec<Effect> {
        self.busy = false;
        self.error = None;
        self.mode = mode;
        self.resend_ready = false;
        vec![Effect::Timer { id: self.asks.ask(Ask::ResendReady), ms: RESEND_MS }]
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
    /// The splash may end once its minimum time is up and the shell no longer holds it.
    fn splash_over(&mut self, cx: &Context) -> Vec<Effect> {
        if self.splash_timing || self.splash_held {
            return vec![];
        }
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

    fn on_event(&mut self, event: Event, cx: &Context) -> Vec<Effect> {
        match event {
            Event::Started => {
                self.tv = cx.platform.device == Device::Tv;
                self.remember = (!self.tv).then_some(true);
                self.mode = if self.tv { Mode::Phone } else { Mode::LogIn };
                self.checking = true;
                vec![self.get(Ask::Check, PROFILES, cx), self.get(Ask::Wall, BACKDROP, cx)]
            }
            Event::SplashShown { held } => {
                if self.splash_timing || self.splash_done {
                    return vec![];
                }
                self.splash_timing = true;
                self.splash_held = held;
                vec![Effect::Timer { id: self.asks.ask(Ask::Splash), ms: SPLASH_MS }]
            }
            Event::SplashDone => {
                self.splash_held = false;
                self.splash_over(cx)
            }
            Event::Mode { mode } => {
                // (a code's step comes only once its code is sent)
                if matches!(mode, Mode::Create | Mode::LogIn | Mode::Forgot) || (mode == Mode::Phone && self.tv) {
                    self.mode = mode;
                    self.error = None;
                }
                vec![]
            }
            Event::Submit { email, username, password, code, remember } => {
                if self.busy || self.mode == Mode::Phone {
                    return vec![];
                }
                let email = email.trim().to_lowercase();
                let code: String = code.chars().filter(char::is_ascii_digit).collect();
                let missing = match self.mode {
                    Mode::Create => email.is_empty() || username.trim().is_empty() || password.is_empty(),
                    Mode::LogIn => username.trim().is_empty() || password.is_empty(),
                    Mode::Code => code.is_empty(),
                    Mode::Forgot => email.is_empty(),
                    Mode::Reset => code.is_empty() || password.is_empty(),
                    Mode::Phone => true,
                };
                if missing {
                    self.fail(FILL_IN.into());
                    return vec![];
                }
                if matches!(self.mode, Mode::Create | Mode::Forgot) && !looks_like_email(&email) {
                    self.fail(BAD_EMAIL.into());
                    return vec![];
                }
                if matches!(self.mode, Mode::Code | Mode::Reset) && code.len() != 6 {
                    self.fail(BAD_CODE.into());
                    return vec![];
                }
                self.busy = true;
                self.error = None;
                if self.remember.is_some() && matches!(self.mode, Mode::Create | Mode::LogIn) {
                    self.remember = Some(remember);
                }
                let remember = self.remember.unwrap_or(true);
                match self.mode {
                    Mode::Create => {
                        self.email = email.clone();
                        let body = json!({"email": email, "username": username.trim(), "password": password, "remember": remember});
                        vec![self.post(Ask::SendCode, "/api/signup", body, cx)]
                    }
                    Mode::Code => {
                        let body = json!({"email": self.email, "code": code, "remember": remember});
                        vec![self.post(Ask::Auth, "/api/signup/verify", body, cx)]
                    }
                    Mode::Forgot => {
                        self.email = email.clone();
                        vec![self.post(Ask::Forgot, "/api/password/forgot", json!({"email": email}), cx)]
                    }
                    Mode::Reset => {
                        let body = json!({"email": self.email, "code": code, "password": password, "remember": remember});
                        vec![self.post(Ask::Auth, "/api/password/reset", body, cx)]
                    }
                    _ => {
                        let body = json!({"username": username.trim(), "password": password, "remember": remember});
                        vec![self.post(Ask::Auth, "/api/signin", body, cx)]
                    }
                }
            }
            Event::Resend => {
                if !self.resend_ready || !matches!(self.mode, Mode::Code | Mode::Reset) {
                    return vec![];
                }
                self.resend_ready = false;
                let purpose = if self.mode == Mode::Reset { "reset" } else { "signup" };
                let body = json!({"email": self.email, "purpose": purpose});
                vec![
                    self.post(Ask::Resend, "/api/code/resend", body, cx),
                    Effect::Timer { id: self.asks.ask(Ask::ResendReady), ms: RESEND_MS },
                ]
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
        if ask == Ask::Splash {
            self.splash_timing = false;
            return self.splash_over(cx);
        }
        if ask == Ask::Shine {
            self.pick.1 = true;
            return self.picked();
        }
        if ask == Ask::ResendReady {
            self.resend_ready = true;
            return vec![];
        }
        if let Ask::Toast(n) = ask {
            // (a newer notice keeps its own time)
            if n == self.toasts {
                self.toast = None;
            }
            return vec![];
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
            (Ask::Auth, Outcome::Failed { failure }) => self.refused(failure),
            (Ask::SendCode, Outcome::Body { .. }) if self.mode == Mode::Create => self.code_sent(Mode::Code),
            (Ask::Forgot, Outcome::Body { .. }) if self.mode == Mode::Forgot => self.code_sent(Mode::Reset),
            (Ask::SendCode | Ask::Forgot, Outcome::Failed { failure }) => self.refused(failure),
            (Ask::Resend, Outcome::Body { .. }) => self.notice(format!("A new code is on its way to {}.", self.email)),
            (Ask::Resend, Outcome::Failed { failure }) => self.notice(failure.words),
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
        let line = match self.mode {
            Mode::Create => "New here? Make an account with your email.".into(),
            Mode::Code => format!("Enter the 6-digit code we sent to {}.", self.email),
            Mode::Forgot => "Enter your email and we'll send you a code.".into(),
            Mode::Reset => format!("Enter the code we sent to {} and a new password.", self.email),
            _ => "Welcome back. Log in to keep watching.".into(),
        };
        let button = match self.mode {
            _ if self.busy => "One moment…",
            Mode::Create => "Continue",
            Mode::Code => "Create account",
            Mode::Forgot => "Send code",
            Mode::Reset => "Set password",
            _ => "Log in",
        };
        View {
            stage: self.stage,
            offline: self.offline,
            sign_in: SignInView {
                mode: self.mode,
                fields: match self.mode {
                    Mode::Create => vec![Field::Email, Field::Username, Field::Password],
                    Mode::LogIn => vec![Field::Username, Field::Password],
                    Mode::Code => vec![Field::Code],
                    Mode::Forgot => vec![Field::Email],
                    Mode::Reset => vec![Field::Code, Field::Password],
                    Mode::Phone => vec![],
                },
                tabs: matches!(self.mode, Mode::Create | Mode::LogIn | Mode::Phone),
                forgot: self.mode == Mode::LogIn,
                resend: matches!(self.mode, Mode::Code | Mode::Reset),
                back: match self.mode {
                    Mode::Code => Some(Mode::Create),
                    Mode::Forgot | Mode::Reset => Some(Mode::LogIn),
                    _ => None,
                },
                phone_tab: self.tv,
                line,
                button: button.into(),
                busy: self.busy,
                error: self.error.clone(),
                shakes: self.shakes,
                remember: self.remember.filter(|_| matches!(self.mode, Mode::Create | Mode::LogIn)),
                phone_tab_title: "Use your phone".into(),
                create_tab_title: "Create account".into(),
                log_in_tab_title: "Log in".into(),
                remember_title: "Remember me".into(),
                email_title: "Email".into(),
                username_title: if self.mode == Mode::LogIn { "Username or email" } else { "Username" }.into(),
                password_title: if self.mode == Mode::Reset { "New password" } else { "Password" }.into(),
                code_title: "Code".into(),
                forgot_title: "Forgot password?".into(),
                back_title: "Back".into(),
                resend_title: "Send again".into(),
                resend_ready: self.resend_ready,
                toast: self.toast.clone(),
                toasts: self.toasts,
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
            can_add: self.account.as_ref().is_some_and(|a| crate::profiles::tile(a).add.is_some()),
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
    fn the_splash_plays_its_minimum_and_a_held_one_waits_for_the_shell() {
        let cx = test_context(0.);
        let mut s = Session::default();
        let (check, _) = started(&mut s);
        let effects = s.update(Event::SplashShown { held: true }, &cx);
        let Some(Effect::Timer { id: timer, ms }) = effects.into_iter().next() else { panic!("no timer") };
        assert_eq!(ms, SPLASH_MS);
        // (the check answered signed out: still the splash until the time is up and the shell lets go)
        s.answer(check, response(401, "{}", None), &cx);
        assert_eq!(s.view().stage, Stage::Splash);
        s.answer(timer, Reply::Fired, &cx);
        assert_eq!(s.view().stage, Stage::Splash);
        s.update(Event::SplashDone, &cx);
        assert_ne!(s.view().stage, Stage::Splash);
        // (not held: the time alone ends it)
        let mut s = Session::default();
        let (check, _) = started(&mut s);
        let Some(Effect::Timer { id: timer, .. }) = s.update(Event::SplashShown { held: false }, &cx).into_iter().next() else { panic!("no timer") };
        s.answer(check, response(401, "{}", None), &cx);
        assert_eq!(s.view().stage, Stage::Splash);
        s.answer(timer, Reply::Fired, &cx);
        assert_ne!(s.view().stage, Stage::Splash);
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
        // (a switch from Home's menu is a pick, as on Who's watching: it shines without the wordmark)
        s.update(Event::Pick { id: "p1".into() }, &cx);
        assert_eq!((s.view().stage, s.view().who_mark), (Stage::Who, false));
    }

    #[test]
    fn managing_a_tap_edits_and_only_the_main_profile_manages() {
        let cx = test_context(0.);
        let mut s = Session::default();
        let (check, _) = started(&mut s);
        s.update(Event::SplashDone, &cx);
        s.answer(check, response(200, &one(3), None), &cx);
        assert!(s.view().can_manage && s.view().can_add);
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
    fn signed_out_shows_the_wall_and_sign_in_goes_in() {
        let cx = test_context(0.);
        let mut s = Session::default();
        let (check, wall) = started(&mut s);
        s.answer(wall, response(200, r#"{"posters": ["poster/a.jpg"]}"#, None), &cx);
        s.answer(check, response(401, "", None), &cx);
        // (straight on once the splash has played: the wall's posters come in under the card)
        s.update(Event::SplashDone, &cx);
        let view = s.view();
        assert_eq!((view.stage, view.sign_in.mode), (Stage::SignIn, Mode::LogIn));
        assert_eq!(view.wall, ["https://host.invalid/card/268x402/poster/a.jpg"]);
        assert!(s.update(Event::Submit { email: String::new(), username: " ".into(), password: "x".into(), code: String::new(), remember: true }, &cx).is_empty());
        assert_eq!((s.view().sign_in.error.as_deref(), s.view().sign_in.shakes), (Some(FILL_IN), 1));
        let effects = s.update(Event::Submit { email: String::new(), username: "zf".into(), password: "pw".into(), code: String::new(), remember: true }, &cx);
        let (auth, url) = requests(&effects)[0].clone();
        assert!(url.ends_with("/api/signin"));
        assert_eq!(s.view().sign_in.button, "One moment…");
        let (check, _) = requests(&s.answer(auth, response(200, "{}", None), &cx))[0].clone();
        // (one profile, still Who's watching: Add profile and Manage profiles are there)
        assert!(own(&s.answer(check, response(200, &one(1), None), &cx)).is_empty());
        assert_eq!(s.view().stage, Stage::Who);
    }

    #[test]
    fn a_new_account_comes_with_its_emails_code_and_a_wrong_password_shakes() {
        let cx = test_context(0.);
        let mut s = Session::default();
        let (check, _) = started(&mut s);
        s.answer(check, response(401, "", None), &cx);
        s.update(Event::SplashDone, &cx);
        s.update(Event::Mode { mode: Mode::Create }, &cx);
        let submit = |s: &mut Session, email: &str, code: &str| {
            let event = Event::Submit { email: email.into(), username: "a".into(), password: "b".into(), code: code.into(), remember: true };
            s.update(event, &cx)
        };
        assert!(submit(&mut s, "not an email", "").is_empty());
        assert_eq!(s.view().sign_in.error.as_deref(), Some(BAD_EMAIL));
        // (an email that can't sign up: a notice from the top, the form as it was)
        let (auth, url) = requests(&submit(&mut s, "a@gmail.com", ""))[0].clone();
        assert!(url.ends_with("/api/signup"));
        let effects = s.answer(auth, response(422, r#"{"error":"You are not on the list."}"#, None), &cx);
        let Some(Effect::Timer { id: gone, ms: NOTICE_MS }) = effects.first().cloned() else { panic!("the notice never goes") };
        let view = s.view().sign_in;
        assert_eq!((view.mode, view.toast.as_deref(), view.toasts, view.error, view.busy), (Mode::Create, Some("You are not on the list."), 1, None, false));
        assert_eq!((view.fields, view.tabs, view.forgot, view.resend, view.back, view.remember), (vec![Field::Email, Field::Username, Field::Password], true, false, false, None, Some(true)));
        s.answer(gone, Reply::Fired, &cx);
        assert_eq!((s.view().sign_in.toast, s.view().sign_in.toasts), (None, 1));
        // (the uni's: its code sent, then the code makes the account)
        let (auth, _) = requests(&submit(&mut s, " I24@isb.nu.edu.pk ", ""))[0].clone();
        let effects = s.answer(auth, response(200, r#"{"code": true}"#, None), &cx);
        let view = s.view().sign_in;
        assert_eq!((view.mode, view.line.as_str(), view.button.as_str(), view.resend_ready), (Mode::Code, "Enter the 6-digit code we sent to i24@isb.nu.edu.pk.", "Create account", false));
        assert_eq!((view.fields, view.tabs, view.resend, view.back, view.remember), (vec![Field::Code], false, true, Some(Mode::Create), None));
        let Some(Effect::Timer { id: ready, ms: RESEND_MS }) = effects.first().cloned() else { panic!("no resend wait") };
        assert!(s.update(Event::Resend, &cx).is_empty());   // (not within the minute)
        s.answer(ready, Reply::Fired, &cx);
        assert!(s.view().sign_in.resend_ready);
        let (resent, url) = requests(&s.update(Event::Resend, &cx))[0].clone();
        assert!(url.ends_with("/api/code/resend"));
        s.answer(resent, response(200, "{}", None), &cx);
        assert_eq!(s.view().sign_in.toasts, 2);
        assert!(submit(&mut s, "", "12 34").is_empty());
        assert_eq!(s.view().sign_in.error.as_deref(), Some(BAD_CODE));
        let (verify, url) = requests(&submit(&mut s, "", "123 456"))[0].clone();
        assert!(url.ends_with("/api/signup/verify"));
        let (check, _) = requests(&s.answer(verify, response(200, r#"{"ok": true}"#, None), &cx))[0].clone();
        s.answer(check, response(200, &one(1), None), &cx);
        assert_eq!(s.view().stage, Stage::Who);

        s.update(Event::SignOut, &cx);
        s.update(Event::Mode { mode: Mode::LogIn }, &cx);
        assert_eq!(s.view().sign_in.username_title, "Username or email");
        let (auth, _) = requests(&submit(&mut s, "", ""))[0].clone();
        s.answer(auth, response(401, r#"{"error":"Wrong password."}"#, None), &cx);
        let view = s.view();
        assert_eq!((view.stage, view.sign_in.error.as_deref(), view.sign_in.busy), (Stage::SignIn, Some("Wrong password."), false));
    }

    #[test]
    fn a_forgotten_password_is_set_again_with_a_code() {
        let cx = test_context(0.);
        let mut s = Session::default();
        let (check, _) = started(&mut s);
        s.answer(check, response(401, "", None), &cx);
        s.update(Event::SplashDone, &cx);
        assert!(s.update(Event::Mode { mode: Mode::Reset }, &cx).is_empty());   // (only once its code is sent)
        assert_eq!(s.view().sign_in.mode, Mode::LogIn);
        s.update(Event::Mode { mode: Mode::Forgot }, &cx);
        let event = Event::Submit { email: "i24@isb.nu.edu.pk".into(), username: String::new(), password: String::new(), code: String::new(), remember: true };
        let (forgot, url) = requests(&s.update(event, &cx))[0].clone();
        assert!(url.ends_with("/api/password/forgot"));
        s.answer(forgot, response(200, r#"{"code": true}"#, None), &cx);
        assert_eq!((s.view().sign_in.mode, s.view().sign_in.password_title.as_str()), (Mode::Reset, "New password"));
        let event = Event::Submit { email: String::new(), username: String::new(), password: "newpass".into(), code: "654321".into(), remember: true };
        let (reset, url) = requests(&s.update(event, &cx))[0].clone();
        assert!(url.ends_with("/api/password/reset"));
        let (check, _) = requests(&s.answer(reset, response(200, r#"{"ok": true}"#, None), &cx))[0].clone();
        s.answer(check, response(200, &one(1), None), &cx);
        assert_eq!(s.view().stage, Stage::Who);
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
