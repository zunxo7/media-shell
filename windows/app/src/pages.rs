//! The app's pages, as the phone app has them (design/SPEC.md), laid out for a laptop's window: the sections in a bar
//! along the top (not the phone's tab bar), and wider pages filling their width rather than leaving it empty.
use crate::data::{self, Item, Me};
use crate::gfx::{Font, TextStyle};
use crate::kit::{col, el, icon, k, row, text, Anim, Cb, El, Fill};
use crate::net::{self};
use crate::tokens as t;
use crate::ui::*;
use crate::core_screen::CoreScreen;
use crate::platform::Shell;
use crate::{downloads, ds, spawn, ui as post, App};
use zcore::screen::{Effect, Route as CoreRoute};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

// (for the player, until it's redone: its old pieces, in the new type)

/// A section's own search field (and, with "-page", its results' scroller).
fn find_field(section: &str) -> String {
    format!("find-{section}")
}

/// The sections' keys, as the core's navigation lists its tabs after Home.
const SECTIONS: [&str; 4] = zcore::sections::KEYS;
/// Pages start this far down (there's no bar along the top: the sections are in the rail).
const TOP: f32 = t::SPACE_28;
/// The rail down the left: the sections, search, downloads, history and you. Pages sit beside it.
const RAIL: f32 = t::K_NAV_RAIL;
/// The rail's sides: its icons' rows are the rest of it.
const RAIL_PAD: f32 = t::SPACE_10;
/// How long Who's watching's copy stays over Home once a profile's picked: till its fade and the name's fold are done.
const WHO_LEAVE_MS: f32 = shell::MORPH_MS + 40.;

/// How wide the pages are: the window less the rail's icons (opened out, it goes over the pages, never pushing them).
fn view_w(app: &App) -> f32 {
    (app.kit.width - STAGE_X).max(t::K_PAGES_LEAST_WIDTH)
}
/// Where the pages start: at the rail's icons' edge (its padding is the page's), so a page's gutter on the left,
/// from the icons, is the same as on the right, from the window's edge.
const STAGE_X: f32 = RAIL - RAIL_PAD;

#[derive(Clone, Debug, PartialEq)]
enum Route {
    /// Search through everything (the rail's); a section searches on its own page (`finding`).
    Search,
    Title(String),
    Account,
    Downloads,
    History,
    /// Manhwa's shelf.
    Manhwa,
    /// A manhwa series (its path).
    Series(String),
    /// A chapter being read (its path).
    Reader(String),
    /// Playing on the account's TV.
    YourTv,
}

#[derive(Clone, Debug, PartialEq)]
enum Move {
    /// A page opened over the one under it.
    Push,
    /// A page closed: it plays its way out over the one under it.
    Pop(Route),
    /// Another section: the pages slide, as the phone's pager does (from this one).
    Tab(usize),
    /// One page for another from the rail: the new one fades in over the old, which stays under it until it's in
    /// (nothing beneath flashes through).
    Swap(Route),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Sheet {
    Profile,
    Tv,
    /// The TV player's subtitles, picked on Your TV.
    TvCaptions,
    /// The TV player's sound and quality.
    TvSettings,
    /// Donations: the goal and where to send to.
    Donate,
}

/// The one a thing replaced, fading out, and since when.
type Replaced<T> = Option<(T, Instant)>;

pub struct Pages {
    /// The pages over the tabs, the tab under them and how the last move plays: the core's navigation's
    /// (`zcore::nav`), taken up by `follow_nav`.
    stack: Vec<Route>,
    tab: usize,
    moving: Option<(Instant, Move)>,
    pub(crate) nav: CoreScreen<zcore::nav::Nav>,
    /// The core's moves taken up, and the times Home went back to its top.
    nav_seen: u32,
    home_top_seen: u32,
    /// The rail is opening a page of its own: fresh from wherever you are.
    railing: bool,
    home: CoreScreen<zcore::home_page::HomePage>,
    /// Home's rows as last drawn (their names, and whether ranked or carrying on), to keep the row on screen in place.
    rows_seen: Vec<(String, bool, bool)>,
    /// The hero's slide as last drawn, and the one it came from (with when): noticed as it's drawn, so the very
    /// frame the new one shows is its first, fading in over the old (noticed on the timer's beat, a frame drawn
    /// between had put it there at once).
    hero_seen: Cell<u32>,
    title: CoreScreen<zcore::title_page::TitlePage>,
    /// The title the page is open on.
    title_open: String,
    dramas: CoreScreen<zcore::browse::Section>,
    shows: CoreScreen<zcore::browse::Section>,
    movies: CoreScreen<zcore::browse::Section>,
    anime: CoreScreen<zcore::browse::Section>,
    /// The section scroll last told to the core (section, distance to its end, screen), so only a change is sent.
    scroll_told: Option<(&'static str, i32, i32)>,
    layers_told: Option<(bool, bool, bool)>,
    finder: CoreScreen<zcore::browse::Search>,
    pub signed_in: bool,
    pub checking: bool,
    /// The splash: 0 the z, 1 the f dropped onto it, 2 the name opened out.
    splash: u8,
    splash_gone: Option<Instant>,
    /// When the splash began (it's drawn frame by frame from this, as the website's).
    pub splash_from: Option<Instant>,
    signup: bool,
    /// The sign-in card's step (the core's): Create, Log in, the code, Forgot, Reset.
    sign_mode: zcore::session::Mode,
    /// The sign-in card's notice from the top (an email that can't sign up), and when it came: taken up as it's
    /// drawn, so it shows with the answer that brought it.
    pub(crate) notice: RefCell<Option<(String, Instant, Option<Instant>)>>,
    pub(crate) toasts_seen: Cell<u32>,
    /// The sign-in card's parts folding open or shut as its step changes: (from, to, since) for each.
    pub(crate) folds: RefCell<HashMap<&'static str, (f32, f32, Instant)>>,
    pub(crate) fold_hints: RefCell<HashMap<&'static str, String>>,
    /// When the sign-in card first showed (its parts come in one after another then), None while it isn't showing.
    pub(crate) login_since: Cell<Option<Instant>>,
    /// The sign-in card's line as it is, the one before it, how many changes, and when it changed (it crossfades).
    pub(crate) line_was: RefCell<(String, String, u32, Option<Instant>)>,
    /// "Remember me" ticked: this PC stays signed in; unticked, signing in lasts until the app's closed.
    remember: bool,
    /// A new account waiting for the owner's approval: the Waiting screen.
    waiting: bool,
    /// zunflix under maintenance: its screen, over everything.
    maintenance: bool,
    reveal: bool,
    busy: bool,
    /// The menu and sheet as the core's navigation last said (their way out is played from the change), and the ones
    /// closing, since when.
    menu_seen: bool,
    menu_closed: Option<Instant>,
    sheet_seen: Option<Sheet>,
    sheet_closed: Option<(Sheet, Instant)>,
    error: String,
    shake: Option<Instant>,
    note: String,
    wall: Vec<String>,
    hero_from: Cell<Option<(usize, Instant)>>,
    /// The next slide's picture last read ahead.
    hero_warmed: Option<String>,
    /// Home is the page showing, as the core's Home was last told.
    home_showing: bool,
    /// The profile Home was opened for.
    home_for: Option<String>,
    /// Whether the account has a TV and plays on it, as the core's navigation was last told.
    tv_told: Option<(bool, bool)>,
    /// The section searching right on its page: its name gave way to Back and the field, its titles to the results.
    finding: Option<&'static str>,
    me: Option<Me>,
    history: CoreScreen<zcore::history::History>,
    settings: CoreScreen<zcore::settings::Settings>,
    /// The update's file coming down now.
    updating: bool,
    /// An update being made ready by the app before it closes (the Mac's): how far.
    finishing: Option<f32>,
    /// That install is the one zunflix opens with (the bootstrapper): failing, it carries on opening as it is.
    booting: bool,
    /// The installs the core's updater has asked for so far, and whether it was last told something plays.
    installs_seen: u32,
    playing_told: bool,
    /// The splash has played through once: it goes on as soon as the updater has decided.
    splash_held: bool,
    /// When the update card first showed (it slides in from the right); the card it showed; since when it's been
    /// sliding away.
    toast_at: RefCell<Option<Instant>>,
    toast_card: RefCell<Option<zcore::update::Card>>,
    toast_gone: RefCell<Option<Instant>>,
    /// The picture just picked (its file), and how the cropper frames it: what's saved is exactly what it shows.
    avatar: Option<String>,
    crop: zcore::profiles::Crop,
    /// Dragging the picture in its frame: where the pointer was when it started, and the frame then.
    crop_drag: Option<((f32, f32), zcore::profiles::Crop)>,
    /// Just saved: the new picture's on its way (a skeleton in its place, not the old one).
    avatar_pending: bool,
    /// Sign in a TV (`zcore::tv_link`).
    sign_in_tv: CoreScreen<zcore::tv_link::SignInTv>,
    session: u64,
    /// The core session's stage these pages last took up, and how many times its sign-in card had shaken.
    stage_seen: zcore::session::Stage,
    shakes_seen: u32,
    /// Each page's glow: the colour it has now and the one it had (since when), which the new one fades in over.
    glows: RefCell<HashMap<String, (u32, Replaced<u32>)>>,
    /// The glow showing behind the pages (its page's name), and the one it replaced, fading out, since when.
    glow_shown: RefCell<(Option<String>, Replaced<String>)>,
    /// The rail's "flix" colour, and the one it replaced, fading out, since when.
    flix_shown: RefCell<(u32, Replaced<u32>)>,
    /// The profile edit card was closed by the core's navigation (a Back, a click outside): the session is told.
    profile_left: bool,
    /// A title page coming back from under the player: the episode to show (title, episode, since when).
    showing_ep: Option<(String, String, Instant)>,
    /// Who's watching easing away over Home since then (the profile just picked).
    who_leaving: Option<Instant>,
    /// The pages' search fields open now, and those closing (drawn back into their button) since when.
    bar_search: RefCell<(HashSet<String>, HashMap<String, Instant>)>,
    /// Deletes asking or not, by key, and since when: the disc grows into its "click again" pill (or back) over 240 ms.
    pub(crate) morphs: RefCell<HashMap<String, (bool, Instant)>>,
    /// The card the pointer rests on, and since when; the one previewed.
    resting: Option<(u64, Instant)>,
    preview: Option<u64>,
    /// The account's profiles; Who's watching showing (and in Manage profiles); the profile the edit card is for
    /// (None with `adding`: a new one).
    profiles: Vec<Me>,
    /// The rail: open (names showing, the pages beside it), as you left it last time; and while it's just been opened
    /// or closed, the pages dimmed for a moment as they lay themselves out again beside it.
    rail_open: bool,
    rail_moving: bool,
    who: bool,
    /// Who's watching opened with the pick already made (a switch from the menu): it only fades in, the picked one
    /// simply there, shining.
    who_picked: bool,
    /// How many places Who's watching's row had when the pick was made, kept until it's gone: the profile changing
    /// brings or takes the Add tile (only the main one adds), and the row's new width slid the picked one again.
    who_count: Cell<Option<usize>>,
    /// Donations' bank details: open or not as last drawn, and since when (their height easing to it).
    pub(crate) donate_fold: Cell<(bool, Option<Instant>)>,
    /// The edit card the core last asked Who's watching for.
    editing_seen: Option<String>,
    /// The profile just picked on Who's watching: it stays as the others fade, then Home comes in.
    picking: Option<String>,
    /// Settings' section showing: 0 your profile, 1 TVs and devices, 2 about.
    editing: Option<Me>,
    adding: bool,
    /// The edit card's rules and saving (`zcore::profiles::Editor`), how many times it had closed when last looked at,
    /// and the account as last seen.
    editor: CoreScreen<zcore::profiles::Editor>,
    edits_seen: u32,
    account_seen: Option<zcore::session::Account>,
    /// Counts anything that may change what a page shows (an answer come in, a click, a key, time passing): a page
    /// hidden under another is kept as it is on screen only while this hasn't moved since it was last built.
    pub(crate) changes: u64,
    /// The pages hidden under the top one, and what they were last built at: (changes, width, height, rail open).
    frozen: RefCell<HashMap<String, (u64, u32, u32, bool)>>,
    manhwa: manhwa::Manhwa,
    /// The account's TV (`zcore::your_tv`): kept up with while an account is in (the core's own), the times it has
    /// asked Your TV to close, and how far along its bar is being dragged (0 to 1) until it's let go.
    pub(crate) your_tv: CoreScreen<zcore::your_tv::YourTv>,
    donate: CoreScreen<zcore::donate::Donate>,
    tv_closed: u32,
    tv_scrub: Option<f32>,
}
impl Default for Pages {
    fn default() -> Self {
        Self {
            stack: vec![], tab: 0, moving: None, nav: CoreScreen::new(|a| &mut a.pages.nav, nav_effect), nav_seen: 0, home_top_seen: 0, railing: false,
            home: CoreScreen::new(|a| &mut a.pages.home, page_effect), rows_seen: vec![], hero_seen: Cell::new(0),
            title: CoreScreen::new(|a| &mut a.pages.title, page_effect), title_open: String::new(),
            dramas: CoreScreen::new(|a| &mut a.pages.dramas, page_effect), shows: CoreScreen::new(|a| &mut a.pages.shows, page_effect),
            movies: CoreScreen::new(|a| &mut a.pages.movies, page_effect), anime: CoreScreen::new(|a| &mut a.pages.anime, page_effect),
            scroll_told: None, layers_told: None, finder: CoreScreen::new(|a| &mut a.pages.finder, page_effect),
            signed_in: false, checking: true, splash: 0, splash_gone: None, splash_from: None, signup: true, sign_mode: zcore::session::Mode::LogIn, notice: RefCell::new(None), toasts_seen: Cell::new(0), folds: RefCell::new(HashMap::new()), fold_hints: RefCell::new(HashMap::new()), login_since: Cell::new(None), line_was: RefCell::new(Default::default()), remember: true, waiting: false, maintenance: false, reveal: false, busy: false,
            menu_seen: false, menu_closed: None, sheet_seen: None, sheet_closed: None, error: String::new(), shake: None, note: String::new(), wall: vec![],
            hero_from: Cell::new(None), hero_warmed: None, home_showing: false, home_for: None, tv_told: None,
            finding: None, me: None,
            history: CoreScreen::new(|a| &mut a.pages.history, page_effect), settings: CoreScreen::new(|a| &mut a.pages.settings, |_, _| {}),
            updating: false, finishing: None, booting: false, installs_seen: 0, playing_told: false, splash_held: false, toast_at: RefCell::new(None), toast_card: RefCell::new(None), toast_gone: RefCell::new(None), avatar: None, crop: zcore::profiles::Crop::new(1., 1.), crop_drag: None, avatar_pending: false,
            sign_in_tv: CoreScreen::new(|a| &mut a.pages.sign_in_tv, |_, _| {}), session: 0, stage_seen: zcore::session::Stage::Splash, shakes_seen: 0, glows: RefCell::new(HashMap::new()), glow_shown: RefCell::new((None, None)), flix_shown: RefCell::new((0, None)), profile_left: false, showing_ep: None, who_leaving: None, bar_search: RefCell::new((HashSet::new(), HashMap::new())), morphs: RefCell::new(HashMap::new()),
            resting: None, preview: None,
            profiles: vec![], rail_open: false, rail_moving: false, who: false, who_picked: false, who_count: Cell::new(None), donate_fold: Cell::new((false, None)), editing_seen: None, picking: None, editing: None, adding: false,
            editor: CoreScreen::new(|a| &mut a.pages.editor, |_, _| {}), edits_seen: 0, account_seen: None,
            changes: 0, frozen: RefCell::new(HashMap::new()), manhwa: Default::default(),
            your_tv: CoreScreen::new(|a| &mut a.pages.your_tv, |_, _| {}), donate: CoreScreen::new(|a| &mut a.pages.donate, |_, _| {}), tv_closed: 0, tv_scrub: None,
        }
    }
}


impl Pages {
    // ---------- time passing ----------
    pub fn tick(&mut self, kit: &mut crate::kit::Kit) {
        // (whatever this finds due changes what pages show: counted, so a hidden page is built again for it)
        let was = std::mem::replace(&mut kit.dirty, false);
        self.tick_pages(kit);
        if kit.dirty { self.changes += 1; }
        kit.dirty |= was;
    }
    fn tick_pages(&mut self, kit: &mut crate::kit::Kit) {
        // (a page's way in or out has played: the copy it left, or the page under it, can go)
        if self.moving.as_ref().is_some_and(|(at, _)| at.elapsed().as_millis() > 520) {
            self.moving = None;
            kit.dirty = true;
        }
        if self.hero_from.get().is_some_and(|(_, at)| at.elapsed().as_millis() > home::HERO_HOLD_MS) {
            self.hero_from.set(None);
            kit.dirty = true;
        }
        if self.menu_closed.is_some_and(|at| at.elapsed().as_millis() > 200) {
            self.menu_closed = None;
            kit.dirty = true;
        }
        if self.sheet_closed.is_some_and(|(_, at)| at.elapsed().as_millis() > 260) {
            self.sheet_closed = None;
            kit.dirty = true;
        }
        if self.shake.is_some_and(|at| at.elapsed().as_millis() > 400) {
            self.shake = None;
            kit.dirty = true;
        }
        // (a search field drawn back into its button: gone once it's played)
        {
            let mut bars = self.bar_search.borrow_mut();
            let before = bars.1.len();
            bars.1.retain(|_, at| at.elapsed().as_millis() < 280);
            if bars.1.len() != before { kit.dirty = true; }
        }
        if self.who_leaving.is_some_and(|at| at.elapsed().as_secs_f32() * 1000. > WHO_LEAVE_MS) {
            self.who_leaving = None;
            self.picking = None;
            kit.dirty = true;
        }
        // (the sign-in notice: drawn again as it goes, and gone)
        // (the sign-in card's parts folding: drawn each frame until they rest)
        if self.folds.borrow().values().any(|(from, to, at)| from != to && at.elapsed().as_millis() < crate::pages::shell::FOLD_MS as u128 + 50) {
            kit.dirty = true;
        }
        let notice = self.notice.borrow().as_ref().map(|(_, at, gone)| (at.elapsed().as_millis(), gone.map(|g| g.elapsed().as_millis())));
        if let Some((ms, gone)) = notice {
            if ms < 200 {
                kit.dirty = true;   // (drawn first out of sight, then where it rests: it slides in)
            }
            // (the core's time up: drawn again as it slides away, then let go)
            if let Some(gone) = gone {
                kit.dirty = true;
                if gone > 600 {
                    *self.notice.borrow_mut() = None;
                }
            }
        }
        if self.splash_gone.is_some_and(|at| at.elapsed().as_millis() > 420) {
            self.splash_gone = None;
            kit.dirty = true;
        }
        self.manhwa_tick(kit);
        self.your_tv_tick(kit);
        // (a section's scroll told to the core, which asks for more as the end comes near)
        if self.stack.is_empty() && self.tab > 0 {
            let section = SECTIONS[self.tab - 1];
            let scroll = kit.scroll_of(k(&format!("page-{section}")));
            let told = (section, (scroll.max - scroll.pos) as i32, kit.height as i32);
            if self.scroll_told != Some(told) {
                self.scroll_told = Some(told);
                self.section(section).send(zcore::browse::SectionEvent::Scrolled { to_end: scroll.max - scroll.pos, screen: kit.height });
            }
        }
        self.follow_core(kit);
    }

    /// What the core's navigation and screens say, taken up whatever is on screen (the player too): the layers, the
    /// glow, the TV, and when Home is the page showing.
    pub fn follow_core(&mut self, kit: &mut crate::kit::Kit) {
        self.sync_layers();
        self.follow_layers();
        self.follow_glow(kit);
        // (the account's TV, as the rail names it)
        let tv = (self.your_tv.view().has_tv, self.your_tv.view().plays_on_tv);
        if self.tv_told != Some(tv) {
            self.tv_told = Some(tv);
            self.nav.send(zcore::nav::Event::YourTv { linked: tv.0, on: tv.1 });
        }
        // (the stage turns only while Home is the page showing: the core says when, and its Home times it)
        let showing = self.nav.view().home_showing;
        if self.home_showing != showing {
            self.home_showing = showing;
            self.home.send(zcore::home_page::Event::Showing { on: showing });
        }
    }

    /// A section's page in the core.
    fn section(&self, key: &str) -> &CoreScreen<zcore::browse::Section> {
        match key {
            "dramas" => &self.dramas,
            "shows" => &self.shows,
            "movies" => &self.movies,
            _ => &self.anime,
        }
    }

    /// What the core's navigation, Home and title page say, taken up by the pages: the pages over the tabs and how
    /// they move, Home's top, the stage's slide, the title page opened on its title, and the episode to show.
    pub fn follow_nav(app: &mut App) {
        let v = app.pages.nav.view().clone();
        crate::player::follow_nav(app, v.playing.as_ref());
        let p = &mut app.pages;
        if v.moves != p.nav_seen {
            p.nav_seen = v.moves;
            let from_tab = p.tab;
            p.stack = v.stack.iter().filter_map(route_of).collect();
            p.tab = v.tab as usize;
            let moving = match &v.moving {
                zcore::nav::Move::Pop { left } => route_of(left).map(Move::Pop),
                zcore::nav::Move::Swap { old: Some(old) } => route_of(old).map(Move::Swap).or(Some(Move::Push)),
                zcore::nav::Move::Tab { from } => Some(Move::Tab(*from as usize)),
                zcore::nav::Move::Push { .. } | zcore::nav::Move::Swap { old: None } => Some(Move::Push),
                zcore::nav::Move::Still => None,
            };
            p.moving = moving.map(|m| (Instant::now(), m));
            if p.tab != from_tab && p.tab > 0 {
                p.section(SECTIONS[p.tab - 1]).send(zcore::browse::SectionEvent::Opened { section: SECTIONS[p.tab - 1].into() });
            }
            if let Some(Route::Title(id)) = p.stack.last().cloned() {
                if id != p.title_open {
                    p.title_open = id.clone();
                    p.title.send(zcore::title_page::Event::Opened { id, profile: crate::profile_settings::current_id() });
                }
            }
            p.changes += 1;
            app.kit.dirty = true;
        }
        if v.home_top != app.pages.home_top_seen {
            app.pages.home_top_seen = v.home_top;
            app.kit.reset_scroll(k("page-home"));
        }
        let home = app.pages.home.view().clone();
        if home.warm != app.pages.hero_warmed {
            app.pages.hero_warmed = home.warm.clone();
            if let Some(url) = home.warm.clone() {
                crate::images::preload(url, home.warm_after_ms);
            }
        }
        let rows: Vec<(String, bool, bool)> = home.rows.iter().map(|r| (r.title.clone(), r.ranked, r.continuing)).collect();
        if rows != app.pages.rows_seen {
            Pages::set_home_rows(app, rows, !home.hero.is_empty());
        }
        let reveal = app.pages.title.view().reveal.clone();
        if let Some(ep) = reveal {
            app.pages.showing_ep = Some((app.pages.title_open.clone(), ep, Instant::now()));
            app.pages.title.send(zcore::title_page::Event::Revealed);
        }
    }

    fn route(&self) -> Option<&Route> {
        self.stack.last()
    }

    // ---------- signing in and out ----------
    pub fn start(&mut self) {
        self.checking = true;
        self.splash_from = Some(Instant::now());
        let session = self.session;
        // (the splash: the f drops, and a moment after it settles the name opens out)
        spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(t::D_SPLASH_DROP.into())).await;
            post(move |a| if a.pages.session == session { a.pages.splash = 1 });
            tokio::time::sleep(std::time::Duration::from_millis(t::SPRING_DROP.2 as u64 + 120)).await;
            post(move |a| if a.pages.session == session { a.pages.splash = 2 });
        });
        // (the core times the splash; it is held until the updater has decided)
        post(move |a| if a.pages.session == session {
            a.pages.splash_held = true;
            a.session.send(zcore::session::Event::SplashShown { held: true });
            Pages::follow_updater(a);
        });
        spawn(async { net::send_last_crash() });
        crate::images::trim_disk();
    }
    pub fn sign_out(&mut self) {
        data::forget_all();
        net::clear_session();
        downloads::enabled(false);
        let session = self.session + 1;
        let wall = std::mem::take(&mut self.wall);
        let (shakes_seen, installs_seen, toasts_seen) = (self.shakes_seen, self.installs_seen, self.toasts_seen.clone());
        *self = Self { checking: false, splash: 2, wall, session, stage_seen: zcore::session::Stage::SignIn, shakes_seen, installs_seen, toasts_seen, ..Default::default() };
    }
    fn auth(app: &mut App) {
        let (email, username) = (app.kit.text_of("email"), app.kit.text_of("username"));
        let (password, code) = (app.kit.text_of("password"), app.kit.text_of("code"));
        let remember = app.pages.remember;
        app.session.send(zcore::session::Event::Submit { email, username, password, code, remember });
    }

    /// What the core's session says (`zcore::session`), carried into the pages: the sign-in card's state, Who's
    /// watching, and each new stage as it comes (in as a profile, signed out, offline at the start).
    pub fn follow_session(app: &mut App) {
        use zcore::session::Stage;
        let v = app.session.view().clone();
        // (Who's watching's edit card, as the core asks: opened for the profile it names, "" a new one; closed here,
        // the core told)
        if v.editing != app.pages.editing_seen {
            app.pages.editing_seen = v.editing.clone();
            if let Some(id) = &v.editing {
                let me = data::profiles().into_iter().find(|m| &m.id == id);
                Self::edit_profile(app, me);
            }
        } else if v.editing.is_some() && std::mem::take(&mut app.pages.profile_left) {
            app.session.send(zcore::session::Event::EditClosed);
        }
        let p = &mut app.pages;
        p.busy = v.sign_in.busy;
        p.error = v.sign_in.error.clone().unwrap_or_default();
        if v.sign_in.shakes != p.shakes_seen {
            p.shakes_seen = v.sign_in.shakes;
            p.shake = Some(Instant::now());
        }
        let signup = v.sign_in.mode == zcore::session::Mode::Create;
        p.signup = signup;
        p.sign_mode = v.sign_in.mode;
        p.waiting = v.stage == zcore::session::Stage::Waiting;
        if v.stage != zcore::session::Stage::SignIn {
            p.login_since.set(None);   // (the card comes in again next time it shows)
        }
        p.maintenance = v.stage == zcore::session::Stage::Maintenance;
        if p.wall.is_empty() && !v.wall.is_empty() {
            p.wall = v.wall.clone();
            // (read while the splash plays)
            let urls = p.wall.iter().map(|u| net::card_img(u, zcore::pictures::POSTER)).collect();
            crate::images::warm_all(urls);
        }
        if v.picking.is_some() {
            p.picking = v.picking.clone();
        }
        if let Some(note) = &v.who_note {
            if p.note != *note {
                p.note = note.clone();
            }
        }
        let account = v.account.as_ref().and_then(data::account);
        // (the edit card saved or deleted: it closes, a skeleton where a new picture is coming)
        let edit = p.editor.view();
        if edit.closed != p.edits_seen {
            p.edits_seen = edit.closed;
            p.avatar_pending = p.avatar.is_some() || edit.removed;
            p.avatar = None;
            p.close_sheet();
        }
        if v.account != p.account_seen {
            p.account_seen = v.account.clone();
            p.avatar_pending = false;
            p.profiles = data::profiles();
            // (the profile being edited, as it is now: its new picture, not the one it had)
            if let Some(e) = &p.editing {
                if let Some(fresh) = p.profiles.iter().find(|f| f.id == e.id) {
                    p.editing = Some(fresh.clone());
                }
            }
        }
        if v.stage == p.stage_seen {
            if v.stage == Stage::Main && account.is_some() {
                p.me = account;
            }
            return;
        }
        let from = std::mem::replace(&mut p.stage_seen, v.stage);
        if from == Stage::Splash {
            p.checking = false;
            p.splash_gone = Some(Instant::now());
        }
        app.kit.dirty = true;
        match v.stage {
            Stage::Splash | Stage::SignIn | Stage::Waiting => {}
            // (whatever was open closes: the player, a sheet)
            Stage::Maintenance => {
                p.close_sheet();
                app.close_player();
            }
            Stage::Who => {
                p.signed_in = true;
                p.profiles = data::profiles();
                p.who = true;
                p.who_picked = v.picking.is_some();
                downloads::enabled(true);
            }
            Stage::Main => {
                p.profiles = data::profiles();
                match account {
                    // (a profile picked on Who's watching: everything the last one saw let go, Home again as this one)
                    Some(me) if from == Stage::Who && p.home_for.is_some() => {
                        p.enter_profile(me);
                        app.kit.reset_scroll(k("page-home"));
                    }
                    me => {
                        p.me = me;
                        p.who = false;
                        if from == Stage::Who {
                            // (Who's watching stays a moment, the picked one in the middle, fading away over Home)
                            p.who_leaving = Some(Instant::now());
                        }
                        p.signed_in = true;
                        downloads::enabled(true);
                        p.nav.send(zcore::nav::Event::Started { offline: v.offline });
                        p.donate.send(zcore::donate::Event::Opened);
                        if v.offline {
                            net::reached(false);
                            p.note = zcore::api::UNREACHABLE.into();
                        }
                        p.load_home();
                    }
                }
            }
        }
    }
    fn sign_out_now(app: &mut App) {
        app.session.send(zcore::session::Event::SignOut);
    }

    // ---------- home ----------
    /// Home, as the core's Home has it (`zcore::home_page`), opened for the profile in use.
    fn load_home(&mut self) {
        self.error.clear();
        let profile = crate::profile_settings::current_id();
        self.home_for = Some(profile.clone());
        self.home.send(zcore::home_page::Event::Opened { profile });
        self.home_showing = false;
    }

    // ---------- going places ----------
    /// Back, from a page's Back or the keyboard.
    pub fn go_back(app: &mut App) {
        // (Esc out of a search drops its field's focus: what's closing is the core's to say)
        let v = app.pages.nav.view();
        let leaves_search = v.back == zcore::nav::BackTo::Find || (v.back == zcore::nav::BackTo::Page && v.top == Some(CoreRoute::Search));
        app.pages.back();
        if leaves_search {
            app.kit.focus = 0;
        }
    }
    /// What this shell keeps open itself, told to the core's navigation (which says what Back closes) when it changes.
    fn sync_layers(&mut self) {
        let step = self.sheet() == Some(Sheet::Tv) && self.sign_in_tv.view().stage == zcore::tv_link::Stage::Steps;
        let layers = (step, self.finding.is_some(), downloads::view().page.show.is_some());
        if self.layers_told != Some(layers) {
            self.layers_told = Some(layers);
            let (step, find, detail) = layers;
            self.nav.send(zcore::nav::Event::Layers { step, find, detail });
        }
    }
    /// The page lighting the window, as the core's navigation says (its name keys the colours kept).
    pub(crate) fn glow_name(&self) -> Option<String> {
        use zcore::nav::Lit;
        match (self.nav.view().lit, self.stack.last()) {
            (Lit::Home, _) => Some("home".into()),
            (Lit::Title, Some(Route::Title(id))) => Some(format!("title-{id}")),
            (Lit::Series, Some(Route::Series(path))) => Some(format!("series-{path}")),
            _ => None,
        }
    }
    /// The colour of the page lighting the window, worked out from the pictures the core names and kept so a new one
    /// fades in over the old; a title's page is told once its colour is there (its picture waits for that).
    fn follow_glow(&mut self, kit: &mut crate::kit::Kit) {
        use zcore::nav::Lit;
        let Some(name) = self.glow_name() else { return };
        let from = match (self.nav.view().lit, self.stack.last()) {
            (Lit::Home, _) => self.home.view().glow.clone(),
            (Lit::Title, Some(Route::Title(id))) => match self.title.view().title.as_ref().filter(|t| &t.id == id) {
                Some(_) => self.title.view().glow.clone(),
                None => return,
            },
            _ => zcore::pictures::GlowArt { logo: None, art: self.manhwa.series.view().glow.clone() },
        };
        let colour = glow_colour(&from);
        let known = |p: &Self| p.glows.borrow().get(&name).map(|g| g.0);
        let before = known(self);
        self.glow_of(&name, colour);
        if known(self) != before {
            kit.dirty = true;
        }
        if self.nav.view().lit == Lit::Title && known(self).is_some_and(|c| c != 0) && self.title.view().art_held {
            self.title.send(zcore::title_page::Event::GlowReady);
        }
    }
    /// The sheet open, as the core's navigation has it.
    fn sheet(&self) -> Option<Sheet> {
        self.nav.view().sheet.as_ref().and_then(|s| sheet_of(&s.name))
    }
    /// The profile menu is open, as the core's navigation has it.
    pub(crate) fn menu(&self) -> bool {
        self.nav.view().menu
    }
    /// The button asking "click again" is `key`'s, as the core's navigation has it.
    pub(crate) fn asking(&self, key: &str) -> bool {
        self.nav.view().asking.as_deref() == Some(key)
    }
    /// A button that asks before it acts was pressed: true on the second press (the one to act on); the first asks.
    pub(crate) fn ask(&self, key: &str) -> bool {
        if self.asking(key) {
            self.nav.send(zcore::nav::Event::Unask);
            true
        } else {
            self.nav.send(zcore::nav::Event::Ask { key: key.into() });
            false
        }
    }
    /// The menu and the sheet as the core's navigation changed them: their way out played, and the sheet's own
    /// screen started (whoever opened it: a button, a link, the rail's TV).
    pub fn follow_layers(&mut self) {
        let (menu, sheet) = (self.menu(), self.sheet());
        if menu != self.menu_seen {
            self.menu_seen = menu;
            self.menu_closed = (!menu).then(Instant::now);
        }
        if sheet != self.sheet_seen {
            let old = std::mem::replace(&mut self.sheet_seen, sheet);
            self.profile_left = old == Some(Sheet::Profile) && sheet != old;
            self.sheet_closed = old.filter(|_| sheet.is_none()).map(|old| (old, Instant::now()));
            match sheet {
                Some(Sheet::Tv) => self.sign_in_tv.send(zcore::tv_link::SignInEvent::Opened),
                Some(Sheet::Donate) => {
                    self.donate.send(zcore::donate::Event::Refresh);
                    self.donate_fold.set((false, None));   // (folded each time it opens, at once)
                }
                _ => {}
            }
        }
    }
    pub fn back(&mut self) {
        use zcore::nav::BackTo;
        self.sync_layers();
        match self.nav.view().back {
            BackTo::Step => self.sign_in_tv.send(zcore::tv_link::SignInEvent::Back),
            BackTo::Find => self.close_find(),
            BackTo::Detail => downloads::show(None),
            BackTo::Sheet | BackTo::Menu | BackTo::Player | BackTo::Page | BackTo::Leave => self.nav.send(zcore::nav::Event::Back),
        }
        self.nav.send(zcore::nav::Event::Unask);
        self.note.clear();
    }
    /// A page opened over the one showing, or (from the rail, `clear_for` just before) fresh from wherever you are.
    fn push(&mut self, route: Route) {
        self.note.clear();
        let route = core_route(&route);
        let event = if std::mem::take(&mut self.railing) { zcore::nav::Event::Rail { route } } else { zcore::nav::Event::Open { route } };
        self.nav.send(event);
    }
    /// A section, from the rail or a Back; Home always opens at its top.
    fn open_tab(app: &mut App, index: usize) {
        app.pages.go_tab(index);
        if index == 0 {
            app.kit.reset_scroll(k("page-home"));
        }
    }
    /// The next page opened is the rail's: the one showing stays under it as it fades in.
    fn clear_for(&mut self) {
        self.close_find();
        downloads::show(None);
        self.railing = true;
    }
    fn go_tab(&mut self, index: usize) {
        self.close_find();
        self.nav.send(zcore::nav::Event::Tab { index: index as u32 });
    }
    fn close_menu(&mut self) {
        self.nav.send(zcore::nav::Event::Menu { open: false });
    }
    fn toggle_menu(&mut self) {
        self.nav.send(zcore::nav::Event::Menu { open: !self.menu() });
    }
    /// A sheet over the page (the core's navigation opens it; `follow_layers` starts its own screen).
    fn open_sheet(&mut self, sheet: Sheet) {
        self.nav.send(zcore::nav::Event::Sheet { name: sheet_name(sheet).into(), arg: None });
    }
    fn close_sheet(&mut self) {
        self.nav.send(zcore::nav::Event::CloseSheet);
    }

    // ---------- a title ----------
    /// Something laid out afresh every frame just now (Who's watching leaving, a toast coming or going, a delete asking).
    pub fn animating(&self) -> bool {
        self.who_leaving.is_some_and(|at| at.elapsed().as_secs_f32() * 1000. < WHO_LEAVE_MS)
            || self.toast_at.borrow().is_some_and(|at| at.elapsed().as_millis() < 120)
            || self.toast_gone.borrow().is_some_and(|at| at.elapsed().as_millis() < 520)
            || self.morphs.borrow().values().any(|(_, at)| at.elapsed().as_millis() < 260)
            || self.donate_fold.get().1.is_some_and(|at| at.elapsed().as_millis() < u128::from(t::D_FOLD) + 34)
    }
    /// The glow showing behind the pages now: its page's name, its colours, and how strong (as the scroll has it).
    pub(crate) fn current_glow(&self, app: &App) -> Option<(String, u32, Option<u32>, f32)> {
        let name = self.glow_name()?;
        let (now, before) = self.glows.borrow().get(&name).copied().map(shown_glow)?;
        let scroll = if name == "home" { "page-home".to_string() } else { format!("page-{name}") };
        let strength = zcore::pictures::glow_strength(app.kit.scroll_of(k(&scroll)).pos, app.kit.height);
        Some((name, now, before, strength))
    }
    /// How far the pages start from the window's left edge (the rail's width).
    pub(crate) fn rail_width(&self) -> f32 {
        STAGE_X
    }

    /// Play a title (`episode` the one asked for; `local`: a download, from Downloads): the core's navigation says
    /// where (the TV, or here: `follow_nav` then opens the player), and the player loads the title itself.
    pub(crate) fn play_as(&self, show: String, episode: Option<String>, local: bool) {
        self.nav.send(zcore::nav::Event::Play { show, episode, local });
    }

    pub(crate) fn play(&self, show: String, episode: Option<String>) {
        self.play_as(show, episode, false);
    }

    /// The player closed, on `episode`: the core asks again what's watched, and a show's own page shows the episode.
    pub(crate) fn player_closed(&self, episode: Option<&str>) {
        if let Some(e) = episode {
            self.nav.send(zcore::nav::Event::PlayerEpisode { episode: e.into() });
        }
        self.nav.send(zcore::nav::Event::PlayerClosed);
    }

    /// Just laid out with an episode to show: the page scrolled straight to it (in the same frame, so nothing moves
    /// before your eyes), unless it's already well in view.
    pub(crate) fn reveal_laid_out(&mut self, kit: &mut crate::kit::Kit) {
        self.place_reader(kit);
        let Some((id, ep, at)) = self.showing_ep.clone() else { return };
        if at.elapsed().as_millis() > u128::from(zcore::nav::REVEAL_MS) {
            self.showing_ep = None;
            return;
        }
        let Some((_, y, _, h)) = kit.rect_of(k(&format!("ep-{ep}"))) else { return };
        self.showing_ep = None;
        let Some(shift) = zcore::nav::reveal_shift(y, h, kit.height) else { return };
        let key = k(&format!("page-title-{id}"));
        let pos = kit.scroll_of(key).pos;
        kit.scroll_now(key, pos + shift);
    }

    /// A title's page: the core's title page opens on it as it shows (`follow_nav`).
    fn open_title(&mut self, id: String) {
        self.push(Route::Title(id));
    }

    // ---------- search ----------
    /// Search through everything: the rail's Search, a page of its own.
    fn open_search(&mut self, kit: &mut crate::kit::Kit) {
        self.close_find();
        kit.set_text("search", "");
        kit.focus = k("search");
        self.push(Route::Search);
        self.finder.send(zcore::browse::SearchEvent::Opened);
        self.finder.send(zcore::browse::SearchEvent::Query { text: String::new() });
    }
    /// A page's search, right on it: a section's (kept to that section, as the phone's), Downloads' or Watch
    /// history's (the list narrowing as it's typed), or Manhwa's (what Asura has by the name typed).
    fn open_find(&mut self, section: &'static str, kit: &mut crate::kit::Kit) {
        let field = find_field(section);
        if section == "manhwa" {
            self.manhwa_search_reset();
        }
        kit.set_text(&field, "");
        kit.focus = k(&field);
        if SECTIONS.contains(&section) {
            kit.reset_scroll(k(&format!("{field}-page")));
            self.section(section).send(zcore::browse::SectionEvent::Find { open: true });
        }
        self.finding = Some(section);
    }
    /// Back to the section's own titles (the results stay as they were while they fade away).
    fn close_find(&mut self) {
        match self.finding.take() {
            Some("history") => self.history.send(zcore::history::Event::Query { text: String::new() }),
            Some("downloads") => downloads::search(""),
            Some("manhwa") => self.manhwa_search_closed(),
            Some(section) if SECTIONS.contains(&section) => self.section(section).send(zcore::browse::SectionEvent::Find { open: false }),
            _ => {}
        }
    }
    /// What's typed in a search field, to the core's search: search everywhere's, or the section's.
    fn search(app: &mut App) {
        match (app.pages.route(), app.pages.finding) {
            (Some(Route::Search), _) => {
                let text = app.kit.text_of("search");
                app.pages.finder.send(zcore::browse::SearchEvent::Query { text });
            }
            (None, Some(section)) if SECTIONS.contains(&section) => {
                let text = app.kit.text_of(&find_field(section));
                app.pages.section(section).send(zcore::browse::SectionEvent::Query { text });
            }
            _ => {}
        }
    }

    // ---------- the account ----------
    /// Settings, fresh from anywhere, its name field holding this profile's name (edited right there).
    fn open_settings(app: &mut App) {
        app.pages.clear_for();
        app.pages.account();
        app.pages.editing = None;
        app.pages.adding = false;
        Self::open_editor(app);
        let name = app.pages.me.as_ref().map(|m| m.name.clone()).unwrap_or_default();
        app.kit.set_text("profile-name", &name);
    }
    /// What the core's updater (`zcore::update`) asks, carried out: told whether something plays, the install when it
    /// asks for one, and the opening let go on once it has decided.
    pub fn follow_updater(app: &mut App) {
        let playing = app.player.is_some();
        if playing != app.pages.playing_told {
            app.pages.playing_told = playing;
            app.updater.send(zcore::update::Event::Playing { playing });
        }
        let v = app.updater.view();
        let (installs, install, booting, opening) = (v.installs, v.install.clone(), v.booting, v.opening);
        // (the core is fetching the update: the window is the bootstrapper, small in the middle, showing how far; when
        // that ends without the setup running (it failed), the window comes back as it was)
        if v.bootstrap.is_some() && !app.pages.updating {
            app.pages.updating = true;
            app.pages.booting = booting;
            app.pages.checking = true;
            app.window.bootstrap();
        } else if v.bootstrap.is_none() && app.pages.updating {
            app.pages.updating = false;
            app.pages.finishing = None;
            account::maximise_again(app);
            // (as it opened: the splash carries on as it is, the core letting it go on; else back to the pages)
            if !std::mem::take(&mut app.pages.booting) {
                app.pages.checking = false;
            }
        }
        // (down and checked: the setup runs over this app, whose window the installer's takes the place of)
        if installs != app.pages.installs_seen {
            app.pages.installs_seen = installs;
            if let Some(build) = install {
                account::install(build.file);
            }
        }
        if !opening {
            crate::show_opened(app);
        }
        if app.pages.splash_held && !opening {
            app.pages.splash_held = false;
            app.session.send(zcore::session::Event::SplashDone);
        }
    }
    fn account(&mut self) {
        self.push(Route::Account);
        self.settings.send(zcore::settings::Event::Opened { plus: data::plus() });
    }
    fn open_history(&mut self) {
        self.push(Route::History);
        self.history.send(zcore::history::Event::Opened { profile: crate::profile_settings::current_id() });
    }
    // ---------- profiles ----------
    /// A profile picked on Who's watching (or the menu): the core says what it does (managing: its edit card).
    fn choose_profile(app: &mut App, profile: Me) {
        if app.session.view().managing {
            app.session.send(zcore::session::Event::Pick { id: profile.id });
            return;
        }
        let p = &mut app.pages;
        if p.me.as_ref().is_some_and(|m| m.id == profile.id) && p.home_for.is_some() {
            p.who = false;
            return;
        }
        app.session.send(zcore::session::Event::Pick { id: profile.id });
    }
    /// Everything the last profile saw let go of; Home again, as this one.
    fn enter_profile(&mut self, profile: Me) {
        data::forget_all();
        let session = self.session + 1;
        let fresh = Self {
            checking: false, splash: 2, signed_in: true, session, me: Some(profile), profiles: std::mem::take(&mut self.profiles),
            stage_seen: self.stage_seen, shakes_seen: self.shakes_seen,
            wall: std::mem::take(&mut self.wall), installs_seen: self.installs_seen,
            // (Who's watching stays a moment, as it was, the picked one in the middle, fading away over Home)
            picking: self.picking.take(), who_leaving: Some(Instant::now()), who_count: Cell::new(self.who_count.get()),
            // (the colours showing carry on: the new profile's own glide in from them, never grey in between)
            glows: RefCell::new(self.glows.take()), flix_shown: RefCell::new(self.flix_shown.take()),
            // (donations are the account's, not the profile's: kept, so the rail's heart stays)
            donate: std::mem::replace(&mut self.donate, CoreScreen::new(|a| &mut a.pages.donate, |_, _| {})),
            // (so is the account's TV: it keeps up while an account is in, which the core told it once)
            your_tv: std::mem::replace(&mut self.your_tv, CoreScreen::new(|a| &mut a.pages.your_tv, |_, _| {})), tv_closed: self.tv_closed, ..Default::default()
        };
        *self = fresh;
        // (the fresh pages' own Donations drew an empty one over the kept one's view as they opened: drawn again after)
        self.donate.send(zcore::donate::Event::Opened);
        self.load_home();
    }
    /// The small edit card for a profile (None: a new one), over whatever's open.
    fn edit_profile(app: &mut App, profile: Option<Me>) {
        let name = profile.as_ref().map(|p| p.name.clone()).unwrap_or_default();
        app.pages.adding = profile.is_none();
        app.pages.editing = profile;
        app.pages.open_sheet(Sheet::Profile);
        Self::open_editor(app);
        app.kit.set_text("profile-name", &name);
    }
    /// The core's edit card opened for the profile being edited (or added; else this one), by the account it's on.
    fn open_editor(app: &mut App) {
        let Some(account) = app.session.view().account.clone() else { return };
        let p = &mut app.pages;
        let profile = if p.adding {
            None
        } else {
            let id = p.editing.as_ref().or(p.me.as_ref()).map(|m| m.id.clone());
            let Some(found) = account.profiles.iter().find(|f| Some(&f.id) == id.as_ref()).cloned() else { return };
            Some(found)
        };
        p.avatar = None;
        p.crop_drag = None;
        p.editor.send(zcore::profiles::EditEvent::Opened { profile, account });
    }
    /// The picture just picked, in a frame the shape of the profile picture: dragged to move it, the slider to zoom;
    /// what the frame shows is what's saved. `side`: the frame's size.
    pub(super) fn crop_editor(&self, _app: &App, side: f32, radius: f32) -> El {
        let Some(path) = self.avatar.as_ref() else { return el() };
        let crop = self.crop;
        let (pw, ph) = (crop.width, crop.height);
        let short = pw.min(ph);
        // (the picture read once, as big as it's ever shown — zoomed all the way in — then only scaled down and moved
        // by the compositor as it's dragged and zoomed: never read again, never a blank frame. A GIF plays, read at
        // the frame's own size: every one of its frames is kept)
        let gif = path.to_lowercase().ends_with(".gif") || path.to_lowercase().ends_with(".webp");
        let read_at = if gif { 1.25 } else { zcore::profiles::CROP_MOST };
        let most = side * read_at / short;
        let scale = side * crop.zoom / short;
        let mut picture = el().at(0., 0., f32::NAN, f32::NAN).size((pw * most).round(), (ph * most).round()).decorative()
            .image(format!("file:{path}"))
            .transform(side / 2. - crop.x * scale, side / 2. - crop.y * scale, crop.zoom / read_at);
        if gif {
            picture = picture.animate_gif();
        }
        let frame = el().key("avatar-crop").size(side, side).radius(radius).clip().bg(t::C_SKELETON).grab()
            .drag(move |a, _, done| {
                let pointer = a.kit.pointer;
                let p = &mut a.pages;
                let (start, from) = *p.crop_drag.get_or_insert((pointer, p.crop));
                p.crop = from.moved(side, (0., 0.), (pointer.0 - start.0, pointer.1 - start.1), 1.);
                if done { p.crop_drag = None; }
                a.kit.dirty = true;
            })
            .kid(picture);
        // the zoom: a slider from the whole square to four times in
        let track = side.min(t::K_SETTINGS_ZOOM_TRACK);
        let level = crop.level();
        let slider = el().key("avatar-zoom").w(track).h(t::K_SETTINGS_ZOOM_TOUCH)
            .drag(|a, fraction, _| {
                a.pages.crop = a.pages.crop.at_level(fraction);
                a.kit.dirty = true;
            })
            .kid(el().at(0., (t::K_SETTINGS_ZOOM_TOUCH - t::K_SETTINGS_TRACK) / 2., 0., f32::NAN).h(t::K_SETTINGS_TRACK).radius(t::K_SETTINGS_TRACK / 2.).clip().bg(w(t::W25)).decorative()
                .kid(el().at(0., 0., f32::NAN, 0.).pct_w(level).bg(t::C_WHITE)))
            .kid(el().at((track * level - t::S_KNOB / 2.).clamp(0., track - t::S_KNOB), (t::K_SETTINGS_ZOOM_TOUCH - t::S_KNOB) / 2., f32::NAN, f32::NAN).size(t::S_KNOB, t::S_KNOB).radius(t::S_KNOB / 2.).bg(t::C_WHITE).decorative());
        col().items_center().gap(t::SPACE_12)
            .kid(frame)
            .kid(row().items_center().gap(t::SPACE_10)
                .kid(icon("minus", t::K_SETTINGS_SMALL_GLYPH, w(t::W70)))
                .kid(slider)
                .kid(icon("plus", t::K_SETTINGS_SMALL_GLYPH, w(t::W70))))
    }
    fn change_avatar(app: &mut App) {
        use zcore::profiles::{check_picture, EditEvent, UNREADABLE};
        let Some(path) = choose_picture(app) else { return };
        let size = match std::fs::read(&path) {
            Ok(data) => check_picture(data.len(), crate::images::dimensions(&data)),
            Err(_) => Err(UNREADABLE),
        };
        match size {
            Ok((w, h)) => {
                app.pages.avatar = Some(path);
                app.pages.crop = zcore::profiles::Crop::new(w as f32, h as f32);
                app.pages.crop_drag = None;
                app.pages.editor.send(EditEvent::Picked);
            }
            Err(words) => app.pages.editor.send(EditEvent::PickFailed { words: words.into() }),
        }
    }
    /// The name, and the picture cut to the cropper's square, to the core's edit card, which saves them.
    fn save_profile(app: &mut App) {
        use zcore::profiles::{EditEvent, UNREADABLE};
        let p = &app.pages;
        if p.editor.view().busy { return; }
        let name = app.kit.text_of("profile-name");
        let picked = p.avatar.clone().map(|path| (path, p.crop.square(1.)));
        let session = p.session;
        spawn(async move {
            let picture = match picked {
                Some((path, square)) => match tokio::task::spawn_blocking(move || crate::images::avatar(&path, square)).await.ok().flatten() {
                    Some(picture) => Some(picture),
                    None => {
                        post(move |a| if a.pages.session == session { a.pages.editor.send(EditEvent::PickFailed { words: UNREADABLE.into() }) });
                        return;
                    }
                },
                None => None,
            };
            post(move |a| if a.pages.session == session { a.pages.editor.send(EditEvent::Save { name, picture }) });
        });
    }
    fn sign_in_tv(app: &mut App) {
        let name = app.kit.text_of("tv-name");
        let code = app.kit.text_of("tv-code");
        app.pages.sign_in_tv.send(zcore::tv_link::SignInEvent::Submit { name, code });
    }

    /// Scrolling closes a preview (the card has moved from under it).
    pub fn scrolled(&mut self) {
        self.preview = None;
        self.resting = None;
    }

    /// The colour a page glows in, remembered so a new one fades in over the old.
    fn glow_of(&self, page: &str, color: u32) -> (u32, Option<u32>) {
        let mut glows = self.glows.borrow_mut();
        // (a colour not known yet keeps the last one showing, never a blink to none and back)
        if color == 0 {
            return glows.get(page).copied().map_or((0, None), shown_glow);
        }
        let entry = glows.entry(page.to_string()).or_insert((color, None));
        if entry.0 != color {
            *entry = (color, Some((entry.0, Instant::now())));
        }
        shown_glow(*entry)
    }
}

static HEIGHT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x44610000);
/// The window's height in points, as last drawn (for shapes that fill it).
pub fn window_height() -> f32 {
    f32::from_bits(HEIGHT.load(std::sync::atomic::Ordering::Relaxed))
}
pub fn set_window_height(h: f32) {
    HEIGHT.store(h.to_bits(), std::sync::atomic::Ordering::Relaxed);
}

static SCALE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3FA00000);
/// The window's pixels per point (1.25 at 125%), as last drawn.
pub fn scale() -> f32 {
    f32::from_bits(SCALE.load(std::sync::atomic::Ordering::Relaxed))
}
pub fn set_scale(s: f32) {
    SCALE.store(s.to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// The tab at `index` under the pages: Home, then the sections.
fn tab_key(index: usize) -> &'static str {
    index.checked_sub(1).and_then(|i| SECTIONS.get(i)).copied().unwrap_or("home")
}

/// A Lucide icon the core names, as this app's icon set calls it.
fn glyph_icon(glyph: zcore::screen::Glyph) -> &'static str {
    use zcore::screen::Glyph;
    match glyph {
        Glyph::History => "history",
        Glyph::Search => "search",
        Glyph::Download => "download",
        Glyph::House => "house",
        Glyph::Clapperboard => "clapperboard",
        Glyph::Tv => "tv",
        Glyph::Film => "film",
        Glyph::Sparkles => "sparkles",
        Glyph::BookOpen => "book_open",
    }
}

/// A page as the core's navigation names it.
fn core_route(route: &Route) -> CoreRoute {
    match route {
        Route::Search => CoreRoute::Search,
        Route::Title(id) => CoreRoute::Title { id: id.clone() },
        Route::Account => CoreRoute::Settings,
        Route::Downloads => CoreRoute::Downloads,
        Route::History => CoreRoute::History,
        Route::Manhwa => CoreRoute::Manhwa,
        Route::Series(path) => CoreRoute::Other { name: "manhwa-series".into(), arg: path.clone() },
        Route::Reader(path) => CoreRoute::Other { name: "manhwa-reader".into(), arg: path.clone() },
        Route::YourTv => CoreRoute::YourTv,
    }
}

/// The page the core's navigation names, as these pages draw it.
fn route_of(route: &CoreRoute) -> Option<Route> {
    Some(match route {
        CoreRoute::Search => Route::Search,
        CoreRoute::Title { id } => Route::Title(id.clone()),
        CoreRoute::Settings => Route::Account,
        CoreRoute::Downloads => Route::Downloads,
        CoreRoute::History => Route::History,
        CoreRoute::Manhwa => Route::Manhwa,
        CoreRoute::YourTv => Route::YourTv,
        CoreRoute::Other { name, arg } if name == "manhwa-series" => Route::Series(arg.clone()),
        CoreRoute::Other { name, arg } if name == "manhwa-reader" => Route::Reader(arg.clone()),
        CoreRoute::Other { .. } | CoreRoute::Section { .. } => return None,
    })
}

/// What the core's navigation asks of the app: a title played on the account's TV (here, it has the player opened).
fn nav_effect(app: &mut App, effect: Effect) {
    if let Effect::Play { show, episode } = effect {
        app.pages.your_tv.send(zcore::your_tv::Event::Play { title: show, episode });
    }
}

/// What a page in the core asks of the app: a title played (here, or on the TV), a title's page, what Play would start
/// read ahead.
fn page_effect(app: &mut App, effect: Effect) {
    match effect {
        Effect::Play { show, episode } => app.pages.play(show, episode),
        Effect::Open { route } => match route_of(&route) {
            Some(Route::Title(id)) => app.pages.open_title(id),
            Some(route) => app.pages.push(route),
            None => {}
        },
        Effect::Warm { show, episode } => warm_play(show, episode),
        _ => {}
    }
}

/// What Play on a title's page would start, made ready: YouTube's stream found, a Movies & TV title's copy and first
/// seconds in.
fn warm_play(show: String, episode: Option<String>) {
    crate::player::warm(show, episode);
}

/// What a section's search looks through.
pub(crate) mod shell;
mod home;
mod title;
mod account;
mod manhwa;
mod your_tv;
use account::choose_picture;

/// The sheet the core's navigation names, as these pages draw it.
fn sheet_of(name: &str) -> Option<Sheet> {
    Some(match name {
        zcore::nav::PROFILE => Sheet::Profile,
        zcore::nav::SIGN_IN_TV => Sheet::Tv,
        zcore::nav::TV_CAPTIONS => Sheet::TvCaptions,
        zcore::nav::TV_SETTINGS => Sheet::TvSettings,
        zcore::nav::DONATE => Sheet::Donate,
        _ => return None,
    })
}

fn sheet_name(sheet: Sheet) -> &'static str {
    match sheet {
        Sheet::Profile => zcore::nav::PROFILE,
        Sheet::Tv => zcore::nav::SIGN_IN_TV,
        Sheet::TvCaptions => zcore::nav::TV_CAPTIONS,
        Sheet::TvSettings => zcore::nav::TV_SETTINGS,
        Sheet::Donate => zcore::nav::DONATE,
    }
}

/// The colour the pictures the core names light a page in (`zcore::pictures::glow_from_code`: the logo's hue, else the
/// art's colour); 0 while they're being read, or when there's no colour to them.
fn glow_colour(from: &zcore::pictures::GlowArt) -> u32 {
    let code = match from.logo.as_deref() {
        Some(logo) => match crate::images::logo_hue(&net::img(logo)) {
            Some(code) => code,
            None => return 0,   // (still being read)
        },
        None => zcore::pictures::HUE_NONE,
    };
    zcore::pictures::glow_from_code(code, || from.art.as_deref().and_then(|a| art_color(&net::img(a)))).unwrap_or(0)
}

/// A glow as it's drawn: the colour before only while it's fading out under the new one (900 ms); then its layer goes.
pub(crate) fn shown_glow((now, before): (u32, Replaced<u32>)) -> (u32, Option<u32>) {
    (now, before.filter(|(_, at)| at.elapsed().as_millis() < 950).map(|(c, _)| c))
}

/// The glow's layers, across the whole window (`left`: how far the window's edge is to the left): the colour now, and
/// the one before fading out under it as the featured title changes.
pub(crate) fn glow_layers(key: &str, now: u32, before: Option<u32>, left: f32) -> El {
    let layer = |c: u32, on: bool| el().key(&format!("{key}-{:06X}", c & 0xFFFFFF)).cover().decorative()
        .image(format!("glow:{:06X}", c & 0xFFFFFF)).backdrop(0., 900).opacity(if on { 1. } else { 0. }).ease(900);
    let mut g = el().key(key).at(-left, 0., 0., 0.).decorative();
    if let Some(b) = before.filter(|b| *b != 0 && *b != now) {
        g = g.kid(layer(b, false));
    }
    if now != 0 {
        g = g.kid(layer(now, true));
    }
    g
}

fn hero_url(item: &Item) -> Option<String> {
    zcore::pictures::stage(item.cover_big.as_deref(), item.cover.as_deref(), item.poster.as_deref()).map(net::img)
}
