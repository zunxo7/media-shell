//! Where the app is: the tabs at the bottom of everything (Home and the sections), the pages opened over them, the
//! player over all of it, and the menu and sheets over the pages; what Back (or Esc) closes first, and how each move
//! plays. Links from outside (a TV's QR code, a download's notification) land here too.
//!
//! ```
//! use zcore::nav::{Event, Nav};
//! use zcore::root::Root;
//! use zcore::screen::{Clock, Device, Platform, Route};
//!
//! let platform = Platform { device: Device::Phone, model: "Pixel".into(), version_name: "1.0".into(),
//!                           version_code: 1, os: "t".into(), stream: "s".into(), codecs: vec![], hdr: false };
//! let clock = Clock { now: 1_794_657_600., utc_offset: 0 };
//! let mut root = Root::new(platform, "https://host.invalid".into(), clock);
//! let nav = root.open(Nav::default());
//! root.update::<Nav>(nav, Event::Started { offline: false }, clock);
//! root.update::<Nav>(nav, Event::Open { route: Route::Title { id: "t".into() } }, clock);
//! assert_eq!(root.view::<Nav>(nav).and_then(|v| v.top), Some(Route::Title { id: "t".into() }));
//! root.update::<Nav>(nav, Event::Back, clock);
//! assert_eq!(root.view::<Nav>(nav).and_then(|v| v.top), None);
//! ```

use crate::screen::{AppEvent, Context, Device, Effect, Glyph, Reply, Route, Screen, Trouble};

/// The sheet that signs a TV in (its code typed, or come from a QR code).
pub const SIGN_IN_TV: &str = "sign-in-tv";
/// The sheet that asks for a donation ([`crate::donate`]).
pub const DONATE: &str = "donate";
/// The sheet that edits a profile (the Windows app's).
pub const PROFILE: &str = "profile";
/// The sheet that picks the account's TV's subtitles (the Windows app's Your TV).
pub const TV_CAPTIONS: &str = "tv-captions";
/// The sheet that sets the account's TV's sound and quality (the Windows app's Your TV).
pub const TV_SETTINGS: &str = "tv-settings";

/// A tab: Home, a section, or (a phone's) Manhwa.
#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    /// "home", a section's key, or "manhwa".
    pub key: String,
    pub label: String,
    pub glyph: Glyph,
    pub on: bool,
}

/// A page the rail (or the menu) opens, and whether it's the one showing.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub route: Route,
    pub label: String,
    pub on: bool,
}

/// Something over the pages that isn't a page: its name (as [`SIGN_IN_TV`]) and what it was opened with.
#[derive(Clone, Debug, PartialEq)]
pub struct Sheet {
    pub name: String,
    pub arg: Option<String>,
}

/// Which page lights the window behind the pages (its pictures are that page's `glow`: Home's, a title's, a series').
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lit {
    /// No page lights it.
    #[default]
    Nothing,
    /// Home, nothing over it: its slide showing.
    Home,
    /// A title's page.
    Title,
    /// A Manhwa series' page.
    Series,
}

/// What's playing over everything.
#[derive(Clone, Debug, PartialEq)]
pub struct Playing {
    pub show: String,
    /// The episode asked for; None for the title's own pick.
    pub episode: Option<String>,
    /// A download on this device.
    pub local: bool,
}

/// How the last move plays.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Move {
    /// Nothing moving (the app just opened).
    #[default]
    Still,
    /// A page opened over `under`, which fades away beneath it (None: over the tabs).
    Push { under: Option<Route> },
    /// The page `left` closed, fading out over the one under it.
    Pop { left: Route },
    /// The rail's pages: `old` stays under the new one until it's in (None: the tabs).
    Swap { old: Option<Route> },
    /// Another tab: the tabs slide from `from` (an index).
    Tab { from: u32 },
}

/// Exactly what to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub tabs: Vec<Tab>,
    /// The tab under the pages.
    pub tab: u32,
    /// The pages over the tabs, the top one last.
    pub stack: Vec<Route>,
    /// The page showing; None: the tabs.
    pub top: Option<Route>,
    pub moving: Move,
    /// Goes up with every move, so the same kind of move twice still plays.
    pub moves: u32,
    pub playing: Option<Playing>,
    pub menu: bool,
    pub sheet: Option<Sheet>,
    /// Back has something to close; when it hasn't, Back leaves the app.
    pub can_back: bool,
    /// What Back closes next (see [`BackTo`]).
    pub back: BackTo,
    /// The button asking "click again" before it does what it says, by its key.
    pub asking: Option<String>,
    /// Search, as the rail and the head offer it.
    pub search: Entry,
    /// The rail's own pages beside the tabs (the Windows app's Manhwa).
    pub pages: Vec<Entry>,
    /// The rail's foot (Downloads, Watch history, a TV, Settings), as this device has them.
    pub foot: Vec<Entry>,
    /// What the profile menu lists besides the profiles (a phone's head has Downloads of its own, so it isn't here).
    pub menu_foot: Vec<Entry>,
    /// Which page lights the window.
    pub lit: Lit,
    /// zunflix can't be reached (told by the app's network: [`AppEvent::Connectivity`]).
    pub offline: bool,
    /// Goes up whenever Home should be back at its top (Home picked again, or a rail page left for it).
    pub home_top: u32,
    /// Home is the page showing, nothing over it (no page, player, menu or sheet): only then does its stage turn.
    pub home_showing: bool,
    /// The profile menu's words: its heading (and the rail's name for it), and signing out.
    pub profiles: String,
    pub sign_out: String,
}

impl Default for View {
    fn default() -> Self {
        Nav::default().view()
    }
}

/// How long a title page waits for the episode it should scroll to before it lets go of it.
pub const REVEAL_MS: u64 = 2000;

/// The episode a title page should show, laid out `top` from the screen's top and `height` tall on a screen
/// `screen` tall: how far its page scrolls (up or down) to bring it to a third of the way down, or None when it is
/// already well in view (its top past the sixth, its foot above 85%).
pub fn reveal_shift(top: f32, height: f32, screen: f32) -> Option<f32> {
    let want = screen * 0.3;
    (top < want * 0.5 || top + height > screen * 0.85).then_some(top - want)
}

/// What Back does next, in the one order every app follows: the player, then what's over the pages (a sheet's own
/// step, the sheet, the menu), then a page's own layers (its search, a detail), then the pages, then the tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BackTo {
    /// Nothing to close: Back leaves the app.
    #[default]
    Leave,
    /// The player closes (send `Back`).
    Player,
    /// The sheet's own step goes back (its sign-in's steps); the shell does it, the sheet stays.
    Step,
    /// The sheet closes (the shell closes it).
    Sheet,
    /// The menu closes (the shell closes it).
    Menu,
    /// The page's search closes (the shell closes it).
    Find,
    /// The page's detail closes (the shell closes it).
    Detail,
    /// The top page closes, or the tabs go back to Home (send `Back`).
    Page,
}

/// What a shell keeps open itself, which the navigation can't know (told with [`Event::Layers`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Layers {
    /// The open sheet has a step of its own for Back to go through first.
    pub step: bool,
    /// The page showing has its search open.
    pub find: bool,
    /// The page showing has a detail open (a download's).
    pub detail: bool,
}

/// What happened.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The app is in, signed in (`offline`: in without the server).
    Started {
        offline: bool,
    },
    /// A tab picked (the tab bar, the rail, the pager settling after a swipe), by its index.
    Tab {
        index: u32,
    },
    /// A page opened over the one showing.
    Open {
        route: Route,
    },
    /// A page from the rail: fresh from wherever you are, just it over the tabs.
    Rail {
        route: Route,
    },
    /// Back, or Esc.
    Back,
    /// Home, as a phone steering this TV asks: the player and every page closed.
    Home,
    Menu {
        open: bool,
    },
    Sheet {
        name: String,
        arg: Option<String>,
    },
    CloseSheet,
    /// Play a title here (or on the TV, when this device plays there).
    Play {
        show: String,
        episode: Option<String>,
        local: bool,
    },
    /// The player moved on to another episode.
    PlayerEpisode {
        episode: String,
    },
    PlayerClosed,
    /// A link opened the app (a TV's QR code, host.invalid/link?code=…).
    Link {
        url: String,
    },
    /// A download's notification tapped.
    OpenDownloads,
    /// The account's TV: whether there is one, and whether Play goes to it.
    YourTv {
        linked: bool,
        on: bool,
    },
    /// A button that asks before it acts was pressed once.
    Ask {
        key: String,
    },
    /// Pressed anywhere else: the asking button lets go.
    Unask,
    /// What the shell keeps open itself, as it changes (see [`BackTo`]).
    Layers {
        step: bool,
        find: bool,
        detail: bool,
    },
}

/// The navigation model. See the module's docs.
#[derive(Debug)]
pub struct Nav {
    device: Device,
    tab: u32,
    stack: Vec<Route>,
    moving: Move,
    moves: u32,
    playing: Option<Playing>,
    played: Option<String>,
    menu: bool,
    sheet: Option<Sheet>,
    asking: Option<String>,
    tv_linked: bool,
    tv_on: bool,
    home_top: u32,
    layers: Layers,
    offline: bool,
}

impl Default for Nav {
    fn default() -> Self {
        Self {
            device: Device::Phone,
            tab: 0,
            stack: vec![],
            moving: Move::Still,
            moves: 0,
            playing: None,
            played: None,
            menu: false,
            sheet: None,
            asking: None,
            tv_linked: false,
            tv_on: false,
            home_top: 0,
            layers: Layers::default(),
            offline: false,
        }
    }
}

/// The tabs a device has: Home and the sections; a phone's Manhwa too (the Windows app's is a page of the rail's; a
/// TV has none).
fn tabs(device: Device) -> Vec<(&'static str, &'static str, Glyph)> {
    let mut tabs = vec![
        ("home", "Home", Glyph::House),
        ("dramas", crate::sections::title("dramas"), Glyph::Clapperboard),
        ("shows", crate::sections::title("shows"), Glyph::Tv),
        ("movies", crate::sections::title("movies"), Glyph::Film),
        ("anime", crate::sections::title("anime"), Glyph::Sparkles),
    ];
    if device == Device::Phone {
        tabs.push(("manhwa", crate::sections::title("manhwa"), Glyph::BookOpen));
    }
    tabs
}

/// A TV's code in a link to host.invalid/link (its QR code): four to eight letters and digits, as capitals.
pub fn link_code(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://host.invalid/link").or_else(|| url.strip_prefix("http://host.invalid/link"))?;
    let query = rest.strip_prefix('?').or_else(|| rest.strip_prefix("/?"))?;
    let code = query.split('&').find_map(|pair| pair.strip_prefix("code="))?;
    let ok = (4..=8).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_alphanumeric());
    ok.then(|| code.to_ascii_uppercase())
}

/// What a page that couldn't load says: its title, its line, and its buttons (Downloads, offline, where it has them).
#[derive(Clone, Debug, PartialEq)]
pub struct TroubleWords {
    pub title: String,
    pub line: String,
    pub retry: String,
    pub downloads: Option<String>,
}

/// The words for `trouble` (`downloads`: the device keeps downloads, so offline offers them).
pub fn trouble_words(trouble: &Trouble, downloads: bool) -> TroubleWords {
    let (title, line) = if trouble.offline {
        ("You're offline", "Your downloads still work.".to_string())
    } else if trouble.message.is_empty() {
        ("Couldn't load this", "Give it a moment and try again.".to_string())
    } else {
        ("Couldn't load this", trouble.message.clone())
    };
    TroubleWords {
        title: title.into(),
        line,
        retry: "Try again".into(),
        downloads: (trouble.offline && downloads).then(|| "Downloads".into()),
    }
}

impl Nav {
    fn moved(&mut self, moving: Move) {
        self.moving = moving;
        self.moves = self.moves.wrapping_add(1);
        self.asking = None;
    }

    fn top(&self) -> Option<&Route> {
        self.stack.last()
    }

    fn push(&mut self, route: Route) {
        self.menu = false;
        // (a double tap counts once)
        if self.top() == Some(&route) {
            return;
        }
        let under = self.top().cloned();
        self.stack.push(route);
        self.moved(Move::Push { under });
    }

    /// Fresh from wherever you are: just `route` over the tabs.
    fn rail(&mut self, route: Route) {
        self.menu = false;
        if self.stack == [route.clone()] {
            return;
        }
        let old = self.top().cloned();
        self.stack = vec![route];
        self.moved(Move::Swap { old });
    }

    fn go_tab(&mut self, index: u32) {
        self.menu = false;
        if index as usize >= tabs(self.device).len() {
            return;
        }
        if let Some(top) = self.top().cloned() {
            // (from a page: it fades out over the tab, whichever tab that is)
            self.stack.clear();
            self.tab = index;
            self.moved(Move::Pop { left: top });
        } else if index != self.tab {
            let from = std::mem::replace(&mut self.tab, index);
            self.moved(Move::Tab { from });
        }
        if index == 0 {
            self.home_top = self.home_top.wrapping_add(1);
        }
    }

    fn can_back(&self) -> bool {
        self.playing.is_some() || self.sheet.is_some() || self.menu || !self.stack.is_empty() || self.tab != 0
    }

    /// What Back closes next.
    fn back_to(&self) -> BackTo {
        let l = &self.layers;
        let sheet = self.sheet.is_some();
        // (a page's search shows on the tabs, Downloads, Watch history and Manhwa; a detail on Downloads)
        let find_page = matches!(self.top(), None | Some(Route::Downloads | Route::History | Route::Manhwa));
        if self.playing.is_some() {
            BackTo::Player
        } else if sheet && l.step {
            BackTo::Step
        } else if sheet {
            BackTo::Sheet
        } else if self.menu {
            BackTo::Menu
        } else if l.find && find_page {
            BackTo::Find
        } else if l.detail && self.top() == Some(&Route::Downloads) {
            BackTo::Detail
        } else if self.can_back() {
            BackTo::Page
        } else {
            BackTo::Leave
        }
    }

    fn back(&mut self) -> Vec<Effect> {
        if self.playing.is_some() {
            return self.close_player();
        }
        if self.sheet.take().is_some() {
            self.asking = None;
        } else if self.menu {
            self.menu = false;
        } else if let Some(left) = self.stack.pop() {
            // (a rail page left for Home lands at Home's top, as going Home does)
            if self.stack.is_empty() && self.tab == 0 && is_rail_page(&left) {
                self.home_top = self.home_top.wrapping_add(1);
            }
            self.moved(Move::Pop { left });
        } else if self.tab != 0 {
            self.go_tab(0);
        }
        vec![]
    }

    fn close_player(&mut self) -> Vec<Effect> {
        let Some(playing) = self.playing.take() else { return vec![] };
        let mut effects = vec![Effect::Tell { event: AppEvent::Watched }];
        if self.top() == Some(&Route::Title { id: playing.show.clone() }) {
            let episode = self.played.take().or(playing.episode);
            effects.push(Effect::Tell { event: AppEvent::Played { show: playing.show, episode } });
        }
        self.played = None;
        effects
    }

    fn entry(&self, route: Route, label: &str) -> Entry {
        let on = match (&route, self.top()) {
            (Route::Manhwa, Some(Route::Other { name, .. })) => name.starts_with("manhwa-"),
            (Route::YourTv, _) if self.sheet.as_ref().is_some_and(|s| s.name == SIGN_IN_TV) => true,
            (r, top) => top == Some(r),
        };
        Entry { route, label: label.into(), on }
    }
}

/// The rail's pages, which Back leaves for Home's top.
fn is_rail_page(route: &Route) -> bool {
    matches!(route, Route::Downloads | Route::History | Route::Settings | Route::Manhwa | Route::YourTv)
}

impl Screen for Nav {
    type Event = Event;
    type View = View;

    fn update(&mut self, event: Event, cx: &Context) -> Vec<Effect> {
        self.device = cx.platform.device;
        // (a fresh navigation (a new profile, signed out and in) starts as the network is, not as it was told once)
        self.offline = !cx.online;
        match event {
            Event::Started { offline } => {
                // (in without the server: where it works offline)
                if offline && self.device != Device::Tv {
                    self.push(Route::Downloads);
                }
            }
            Event::Tab { index } => self.go_tab(index),
            Event::Open { route } => self.push(route),
            // (no TV here to sign in or play on: the sheet that signs one in instead)
            Event::Rail { route: Route::YourTv } if !self.tv_linked => {
                self.menu = false;
                self.sheet = Some(Sheet { name: SIGN_IN_TV.into(), arg: None });
            }
            Event::Rail { route } => self.rail(route),
            Event::Back => return self.back(),
            Event::Home => {
                let effects = self.close_player();
                (self.sheet, self.menu) = (None, false);
                self.go_tab(0);
                return effects;
            }
            Event::Menu { open } => self.menu = open,
            Event::Sheet { name, arg } => {
                self.menu = false;
                self.asking = None;
                self.sheet = Some(Sheet { name, arg });
            }
            Event::CloseSheet => {
                self.sheet = None;
                self.asking = None;
            }
            Event::Play { show, episode, local } => {
                self.menu = false;
                if self.tv_on && !local {
                    return vec![Effect::Play { show, episode }];
                }
                self.played = None;
                self.playing = Some(Playing { show, episode, local });
            }
            Event::PlayerEpisode { episode } => self.played = Some(episode),
            Event::PlayerClosed => return self.close_player(),
            Event::Link { url } => {
                if let Some(code) = link_code(&url) {
                    self.sheet = Some(Sheet { name: SIGN_IN_TV.into(), arg: Some(code) });
                }
            }
            Event::OpenDownloads => {
                if self.device != Device::Tv {
                    self.push(Route::Downloads);
                }
            }
            Event::YourTv { linked, on } => (self.tv_linked, self.tv_on) = (linked, on),
            Event::Ask { key } => self.asking = Some(key),
            Event::Unask => self.asking = None,
            Event::Layers { step, find, detail } => self.layers = Layers { step, find, detail },
        }
        vec![]
    }

    fn answer(&mut self, _id: u32, _reply: Reply, _cx: &Context) -> Vec<Effect> {
        vec![]
    }

    fn app(&mut self, event: &AppEvent, _cx: &Context) -> Vec<Effect> {
        if let AppEvent::Connectivity { online, .. } = event {
            self.offline = !online;
        }
        vec![]
    }

    fn view(&self) -> View {
        let tabs = tabs(self.device)
            .into_iter()
            .enumerate()
            .map(|(i, (key, label, glyph))| Tab {
                key: key.into(),
                label: label.into(),
                glyph,
                on: self.stack.is_empty() && i as u32 == self.tab,
            })
            .collect();
        let tv = if self.tv_linked { "Your TV" } else { "Sign in a TV" };
        let foot = match self.device {
            Device::Tv => vec![self.entry(Route::History, "Watch history"), self.entry(Route::Settings, "Settings")],
            _ => vec![
                self.entry(Route::Downloads, "Downloads"),
                self.entry(Route::History, "Watch history"),
                self.entry(Route::YourTv, tv),
                self.entry(Route::Settings, "Settings"),
            ],
        };
        let menu_foot = match self.device {
            Device::Phone => foot.iter().filter(|e| e.route != Route::Downloads).cloned().collect(),
            _ => foot.clone(),
        };
        let lit = match (self.top(), self.tab) {
            (None, 0) => Lit::Home,
            (Some(Route::Title { .. }), _) => Lit::Title,
            (Some(Route::Other { name, .. }), _) if name == "manhwa-series" => Lit::Series,
            _ => Lit::Nothing,
        };
        let pages = match self.device {
            Device::Desktop => vec![self.entry(Route::Manhwa, crate::sections::title("manhwa"))],
            _ => vec![],
        };
        View {
            tabs,
            tab: self.tab,
            stack: self.stack.clone(),
            top: self.top().cloned(),
            moving: self.moving.clone(),
            moves: self.moves,
            playing: self.playing.clone(),
            menu: self.menu,
            sheet: self.sheet.clone(),
            can_back: self.can_back(),
            back: self.back_to(),
            asking: self.asking.clone(),
            search: self.entry(Route::Search, "Search"),
            pages,
            foot,
            menu_foot,
            lit,
            offline: self.offline,
            home_top: self.home_top,
            home_showing: self.tab == 0
                && self.stack.is_empty()
                && self.playing.is_none()
                && !self.menu
                && self.sheet.is_none(),
            profiles: "Profiles".into(),
            sign_out: "Sign out".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::test_context;

    fn nav(device: Device) -> (Nav, Context) {
        let mut cx = test_context(0.);
        cx.platform.device = device;
        let mut nav = Nav::default();
        nav.update(Event::Started { offline: false }, &cx);
        (nav, cx)
    }

    fn title(id: &str) -> Route {
        Route::Title { id: id.into() }
    }

    #[test]
    fn the_tabs_after_home_are_the_sections() {
        let keys: Vec<&str> = tabs(Device::Desktop).iter().skip(1).map(|t| t.0).collect();
        assert_eq!(keys, crate::sections::KEYS);
    }

    #[test]
    fn home_is_showing_only_with_nothing_over_it() {
        let (mut n, cx) = nav(Device::Phone);
        assert!(n.view().home_showing);
        n.update(Event::Menu { open: true }, &cx);
        assert!(!n.view().home_showing);
        n.update(Event::Menu { open: false }, &cx);
        n.update(Event::Open { route: title("a") }, &cx);
        assert!(!n.view().home_showing);
        n.update(Event::Back, &cx);
        assert!(n.view().home_showing);
        n.update(Event::Tab { index: 1 }, &cx);
        assert!(!n.view().home_showing);
    }

    #[test]
    fn pages_push_and_pop_and_a_double_tap_counts_once() {
        let (mut n, cx) = nav(Device::Phone);
        n.update(Event::Open { route: title("a") }, &cx);
        n.update(Event::Open { route: title("a") }, &cx);
        n.update(Event::Open { route: title("b") }, &cx);
        let v = n.view();
        assert_eq!(v.stack, [title("a"), title("b")]);
        assert_eq!(v.moving, Move::Push { under: Some(title("a")) });
        n.update(Event::Back, &cx);
        assert_eq!(n.view().moving, Move::Pop { left: title("b") });
        n.update(Event::Back, &cx);
        let v = n.view();
        assert_eq!((v.top, v.can_back), (None, false));
    }

    #[test]
    fn back_closes_the_player_then_sheets_then_the_menu_then_pages_then_goes_home() {
        let (mut n, cx) = nav(Device::Desktop);
        n.update(Event::Tab { index: 2 }, &cx);
        assert_eq!(n.view().moving, Move::Tab { from: 0 });
        n.update(Event::Open { route: title("t") }, &cx);
        n.update(Event::Menu { open: true }, &cx);
        n.update(Event::Sheet { name: "profile".into(), arg: None }, &cx);
        n.update(Event::Play { show: "t".into(), episode: Some("e1".into()), local: false }, &cx);
        assert!(n.view().playing.is_some() && !n.view().menu);
        n.update(Event::PlayerEpisode { episode: "e2".into() }, &cx);
        let closed = n.update(Event::Back, &cx);
        assert_eq!(
            closed,
            [
                Effect::Tell { event: AppEvent::Watched },
                Effect::Tell { event: AppEvent::Played { show: "t".into(), episode: Some("e2".into()) } }
            ]
        );
        n.update(Event::Back, &cx);
        assert_eq!(n.view().sheet, None);
        assert_eq!(n.view().top, Some(title("t")));
        n.update(Event::Back, &cx);
        assert_eq!(n.view().top, None);
        let home_top = n.view().home_top;
        n.update(Event::Back, &cx);
        let v = n.view();
        assert_eq!((v.tab, v.can_back, v.home_top), (0, false, home_top + 1));
    }

    #[test]
    fn back_names_what_it_closes_in_the_one_order() {
        let (mut n, cx) = nav(Device::Desktop);
        let layers = |n: &mut Nav, step, find, detail| {
            n.update(Event::Layers { step, find, detail }, &cx);
            n.view().back
        };
        assert_eq!(n.view().back, BackTo::Leave);
        n.update(Event::Open { route: Route::Downloads }, &cx);
        assert_eq!(n.view().back, BackTo::Page);
        assert_eq!(layers(&mut n, false, false, true), BackTo::Detail);
        assert_eq!(layers(&mut n, false, true, true), BackTo::Find);
        n.update(Event::Menu { open: true }, &cx);
        assert_eq!(n.view().back, BackTo::Menu);
        n.update(Event::Sheet { name: PROFILE.into(), arg: None }, &cx);
        // (a sheet closes the menu under it)
        assert!(!n.view().menu);
        assert_eq!(layers(&mut n, false, true, true), BackTo::Sheet);
        assert_eq!(layers(&mut n, true, true, true), BackTo::Step);
        n.update(Event::CloseSheet, &cx);
        // (a step belongs to a sheet; with none open it is nothing)
        assert_eq!(layers(&mut n, true, false, false), BackTo::Page);
        n.update(Event::Sheet { name: DONATE.into(), arg: None }, &cx);
        n.update(Event::Play { show: "t".into(), episode: None, local: false }, &cx);
        assert_eq!(layers(&mut n, true, true, true), BackTo::Player);
    }

    #[test]
    fn the_app_is_offline_while_the_network_says_so() {
        let (mut n, cx) = nav(Device::Desktop);
        assert!(!n.view().offline);
        n.app(&AppEvent::Connectivity { online: false, metered: false }, &cx);
        assert!(n.view().offline);
        n.app(&AppEvent::Connectivity { online: true, metered: false }, &cx);
        assert!(!n.view().offline);
    }

    #[test]
    fn a_fresh_navigation_knows_the_network_is_down() {
        let mut cx = test_context(0.);
        cx.online = false;
        let mut n = Nav::default();
        n.update(Event::Started { offline: false }, &cx);
        assert!(n.view().offline);
    }

    #[test]
    fn an_episode_is_scrolled_to_only_when_not_well_in_view() {
        assert_eq!(reveal_shift(300., 100., 1000.), None);
        assert_eq!(reveal_shift(100., 100., 1000.), Some(-200.));
        assert_eq!(reveal_shift(900., 100., 1000.), Some(600.));
        assert_eq!(reveal_shift(150., 700., 1000.), None);
    }

    #[test]
    fn a_page_layer_is_closed_only_on_the_pages_that_have_it() {
        let (mut n, cx) = nav(Device::Desktop);
        n.update(Event::Layers { step: false, find: true, detail: true }, &cx);
        // (a section's search, on the tabs)
        assert_eq!(n.view().back, BackTo::Find);
        // (a title over it: Back closes the title first; and a detail is Downloads' alone)
        n.update(Event::Open { route: title("t") }, &cx);
        assert_eq!(n.view().back, BackTo::Page);
        n.update(Event::Rail { route: Route::History }, &cx);
        assert_eq!(n.view().back, BackTo::Find);
        n.update(Event::Layers { step: false, find: false, detail: true }, &cx);
        assert_eq!(n.view().back, BackTo::Page);
    }

    #[test]
    fn the_sign_in_tv_sheet_opens_from_the_rail_while_there_is_no_tv_and_keeps_its_entry_lit() {
        let (mut n, cx) = nav(Device::Desktop);
        n.update(Event::Rail { route: Route::YourTv }, &cx);
        let v = n.view();
        assert_eq!(v.sheet.as_ref().map(|s| s.name.as_str()), Some(SIGN_IN_TV));
        assert!(v.stack.is_empty() && v.foot[2].on && v.foot[2].label == "Sign in a TV");
        n.update(Event::CloseSheet, &cx);
        n.update(Event::YourTv { linked: true, on: false }, &cx);
        n.update(Event::Rail { route: Route::YourTv }, &cx);
        let v = n.view();
        assert_eq!((v.sheet, v.top), (None, Some(Route::YourTv)));
        assert!(v.foot[2].on && v.foot[2].label == "Your TV");
    }

    #[test]
    fn the_button_asking_lets_go_on_unask_a_move_or_a_sheet_closing() {
        let (mut n, cx) = nav(Device::Desktop);
        n.update(Event::Ask { key: "a".into() }, &cx);
        assert_eq!(n.view().asking.as_deref(), Some("a"));
        n.update(Event::Ask { key: "b".into() }, &cx);
        assert_eq!(n.view().asking.as_deref(), Some("b"));
        n.update(Event::Unask, &cx);
        assert_eq!(n.view().asking, None);
        n.update(Event::Ask { key: "a".into() }, &cx);
        n.update(Event::Open { route: title("t") }, &cx);
        assert_eq!(n.view().asking, None);
        n.update(Event::Sheet { name: PROFILE.into(), arg: None }, &cx);
        n.update(Event::Ask { key: "c".into() }, &cx);
        n.update(Event::CloseSheet, &cx);
        assert_eq!(n.view().asking, None);
    }

    #[test]
    fn the_phones_menu_lists_no_downloads_and_the_glow_is_the_page_showings() {
        let (mut n, cx) = nav(Device::Phone);
        let routes = |v: &View| v.menu_foot.iter().map(|e| e.route.clone()).collect::<Vec<_>>();
        assert_eq!(routes(&n.view()), [Route::History, Route::YourTv, Route::Settings]);
        assert_eq!(n.view().lit, Lit::Home);
        n.update(Event::Tab { index: 2 }, &cx);
        assert_eq!(n.view().lit, Lit::Nothing);
        n.update(Event::Open { route: title("t") }, &cx);
        assert_eq!(n.view().lit, Lit::Title);
        n.update(Event::Open { route: Route::Other { name: "manhwa-series".into(), arg: "s".into() } }, &cx);
        assert_eq!(n.view().lit, Lit::Series);
        let (n, _) = nav(Device::Desktop);
        assert_eq!(routes(&n.view()).len(), 4);
    }

    #[test]
    fn the_rail_opens_fresh_and_its_pages_go_back_to_homes_top() {
        let (mut n, cx) = nav(Device::Tv);
        n.update(Event::Open { route: title("t") }, &cx);
        n.update(Event::Rail { route: Route::History }, &cx);
        let v = n.view();
        assert_eq!(v.stack, [Route::History]);
        assert_eq!(v.moving, Move::Swap { old: Some(title("t")) });
        assert!(v.foot[0].on && v.foot.len() == 2 && v.tabs.len() == 5);
        n.update(Event::Back, &cx);
        let v = n.view();
        assert_eq!((v.top, v.home_top), (None, 1));
    }

    #[test]
    fn a_tab_from_a_page_closes_it_over_the_tab() {
        let (mut n, cx) = nav(Device::Phone);
        n.update(Event::Open { route: Route::Search }, &cx);
        n.update(Event::Tab { index: 5 }, &cx);
        let v = n.view();
        assert_eq!((v.tab, v.top.clone(), v.moving), (5, None, Move::Pop { left: Route::Search }));
        assert!(v.tabs[5].on && v.tabs[5].key == "manhwa");
        n.update(Event::Tab { index: 9 }, &cx);
        assert_eq!(n.view().tab, 5);
    }

    #[test]
    fn links_and_notifications() {
        assert_eq!(link_code("https://host.invalid/link?code=ab12"), Some("AB12".into()));
        assert_eq!(link_code("https://host.invalid/link/?x=1&code=ABCD1234"), Some("ABCD1234".into()));
        assert_eq!(link_code("https://host.invalid/link?code=a-b"), None);
        assert_eq!(link_code("https://evil.com/link?code=ABCD"), None);
        let (mut n, cx) = nav(Device::Phone);
        n.update(Event::Link { url: "https://host.invalid/link?code=wxyz".into() }, &cx);
        assert_eq!(n.view().sheet, Some(Sheet { name: SIGN_IN_TV.into(), arg: Some("WXYZ".into()) }));
        n.update(Event::OpenDownloads, &cx);
        assert_eq!(n.view().top, Some(Route::Downloads));
        let (mut tv, cx) = nav(Device::Tv);
        tv.update(Event::OpenDownloads, &cx);
        tv.update(Event::Started { offline: true }, &cx);
        assert_eq!(tv.view().top, None);
        let mut cx = test_context(0.);
        cx.platform.device = Device::Phone;
        let mut offline = Nav::default();
        offline.update(Event::Started { offline: true }, &cx);
        assert_eq!(offline.view().top, Some(Route::Downloads));
    }

    #[test]
    fn your_tv_plays_there_and_signs_one_in_when_there_is_none() {
        let (mut n, cx) = nav(Device::Phone);
        n.update(Event::Rail { route: Route::YourTv }, &cx);
        assert_eq!(n.view().sheet.map(|s| s.name), Some(SIGN_IN_TV.into()));
        assert_eq!(n.view().foot[2].label, "Sign in a TV");
        n.update(Event::YourTv { linked: true, on: true }, &cx);
        let sent = n.update(Event::Play { show: "s".into(), episode: None, local: false }, &cx);
        assert_eq!(sent, [Effect::Play { show: "s".into(), episode: None }]);
        assert!(n.view().playing.is_none());
        n.update(Event::Play { show: "s".into(), episode: None, local: true }, &cx);
        assert!(n.view().playing.is_some());
        n.update(Event::CloseSheet, &cx);
        n.update(Event::Rail { route: Route::YourTv }, &cx);
        assert_eq!(n.view().top, Some(Route::YourTv));
    }

    #[test]
    fn asking_lets_go_on_any_move_and_home_closes_everything() {
        let (mut n, cx) = nav(Device::Desktop);
        n.update(Event::Ask { key: "delete".into() }, &cx);
        assert_eq!(n.view().asking.as_deref(), Some("delete"));
        n.update(Event::Open { route: Route::Settings }, &cx);
        assert_eq!(n.view().asking, None);
        n.update(Event::Play { show: "s".into(), episode: None, local: false }, &cx);
        let effects = n.update(Event::Home, &cx);
        assert_eq!(effects, [Effect::Tell { event: AppEvent::Watched }]);
        let v = n.view();
        assert!(v.playing.is_none() && v.top.is_none() && v.pages[0].label == "Manhwa");
        n.update(Event::Open { route: Route::Other { name: "manhwa-series".into(), arg: "/s".into() } }, &cx);
        assert!(n.view().pages[0].on);
    }

    #[test]
    fn trouble_says() {
        let t = |offline, message: &str| Trouble { message: message.into(), offline, busy: false };
        let words = trouble_words(&t(true, "x"), true);
        assert_eq!((words.title.as_str(), words.downloads.as_deref()), ("You're offline", Some("Downloads")));
        assert_eq!(trouble_words(&t(true, ""), false).downloads, None);
        assert_eq!(trouble_words(&t(false, ""), true).line, "Give it a moment and try again.");
        assert_eq!(trouble_words(&t(false, "Server down"), true).line, "Server down");
    }
}
