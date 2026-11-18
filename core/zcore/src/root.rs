//! The app's one root model: it owns every live screen, so it alone decides when a screen's timers and requests end.
//!
//! The app opens screens on it, sends each its events, and carries out the [`Order`]s that come back. Effect ids are
//! the root's own, unique across screens: an answer goes back to the screen that asked, under that screen's own id,
//! and an answer for a screen that has closed (or an effect it cancelled) is dropped. In the background, timers are
//! paused, and run on with the time they had left when the app comes back.
//!
//! It also holds the app's one [`Store`] of server pages: a screen's [`Effect::Page`] is answered from it, and the
//! app's own code asks it for pages with [`Root::page`], each answer coming back as a [`Delivery`].
//!
//! ```
//! use zcore::history::{Event, History};
//! use zcore::root::Root;
//! use zcore::screen::{Clock, Device, Effect, Platform, Reply};
//!
//! let platform = Platform { device: Device::Phone, model: "Pixel".into(), version_name: "1.0".into(),
//!                           version_code: 1, os: "t".into(), stream: "s".into(), codecs: vec![], hdr: false };
//! let clock = Clock { now: 1_794_657_600., utc_offset: 0 };
//! let mut root = Root::new(platform, "https://host.invalid".into(), clock);
//! let history = root.open(History::default());
//! let turn = root.update::<History>(history, Event::Opened { profile: "p1".into() }, clock);
//! let Some(Effect::Load { id, .. }) = turn.orders.first().map(|o| o.effect.clone()) else { panic!() };
//! let turn = root.answer(id, Reply::Missing, clock);
//! assert!(matches!(&turn.orders[0].effect, Effect::Request { request, .. } if request.cookies));
//! // (nothing kept: the screen waits for the server, unchanged)
//! assert!(turn.changed.is_empty());
//! ```

use crate::screen::{AppEvent, Clock, Context, Effect, Platform, Reply, Screen};
use crate::store::{Io, Need, Out, PageReply, Step, Store};
use std::any::Any;
use std::collections::{BTreeMap, HashMap};

/// A live screen, as the root knows it.
pub type ScreenId = u32;

/// An effect for the app to carry out, and the screen it's for (where a page-level one, like Play, goes).
#[derive(Clone, Debug, PartialEq)]
pub struct Order {
    pub screen: ScreenId,
    pub effect: Effect,
}

/// What a call to the root asks of the app: the screens whose view may have changed (draw them again), the effects
/// to carry out, in order, the pages the app asked for with [`Root::page`] that are ready, and the keys of the pages
/// whose body changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Turn {
    pub changed: Vec<ScreenId>,
    pub orders: Vec<Order>,
    pub delivered: Vec<Delivery>,
    pub pages_changed: Vec<String>,
}

/// A page the app asked for with [`Root::page`], under the ticket it gave.
#[derive(Clone, Debug, PartialEq)]
pub struct Delivery {
    pub ticket: u64,
    pub page: PageReply,
}

/// Who waits for a page from the store.
#[derive(Clone, Debug, PartialEq)]
enum Waiter {
    Screen { screen: ScreenId, local: u32 },
    Ticket(u64),
}

use crate::session::maintenance;

/// The screen [`Order`]s that aren't any screen's (the store's reads and requests, signing out) are for.
pub const NO_SCREEN: ScreenId = 0;

/// Any screen, as the root holds it.
trait Live: Send {
    fn answer(&mut self, id: u32, reply: Reply, cx: &Context) -> Vec<Effect>;
    fn app(&mut self, event: &AppEvent, cx: &Context) -> Vec<Effect>;
    fn behind(&self) -> bool;
    fn any(&self) -> &dyn Any;
    fn any_mut(&mut self) -> &mut dyn Any;
}

impl<S: Screen + Send + 'static> Live for S {
    fn answer(&mut self, id: u32, reply: Reply, cx: &Context) -> Vec<Effect> {
        Screen::answer(self, id, reply, cx)
    }
    fn app(&mut self, event: &AppEvent, cx: &Context) -> Vec<Effect> {
        Screen::app(self, event, cx)
    }
    fn behind(&self) -> bool {
        Screen::behind(self)
    }
    fn any(&self) -> &dyn Any {
        self
    }
    fn any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// What an effect id stands for while its answer is awaited.
#[derive(Debug)]
enum Waiting {
    /// An answer from the device (a kept page, a setting): nothing to stop.
    Answer,
    /// A request under way: stopped by a Cancel.
    Request,
    /// A timer due at `due` (seconds since 1970).
    Timer { due: f64 },
    /// A timer held while the app is in the background, with this much left.
    Paused { left_ms: u32 },
}

/// Who an effect's answer goes to.
#[derive(Debug)]
enum Owner {
    /// A screen, under its own id for the effect.
    Screen { screen: ScreenId, local: u32 },
    /// The store, for the page kept under `key`.
    Store { key: String, step: Step },
}

#[derive(Debug)]
struct Pending {
    owner: Owner,
    waiting: Waiting,
}

impl Pending {
    fn screen(&self) -> Option<ScreenId> {
        match self.owner {
            Owner::Screen { screen, .. } => Some(screen),
            Owner::Store { .. } => None,
        }
    }
}

/// The root model. See the module's docs.
pub struct Root {
    cx: Context,
    screens: BTreeMap<ScreenId, Box<dyn Live>>,
    store: Store<Waiter>,
    /// By the root's effect id.
    pending: HashMap<u32, Pending>,
    next_screen: ScreenId,
    next_effect: u32,
}

impl Root {
    /// The app starting at `clock`, on the device `platform`, talking to the server at `base`; online and in front
    /// until told otherwise.
    pub fn new(platform: Platform, base: String, clock: Clock) -> Self {
        let base = base.trim_end_matches('/').to_string();
        let cx = Context { platform, base, clock, foreground: true, online: true, metered: false };
        Self { cx, screens: BTreeMap::new(), store: Store::default(), pending: HashMap::new(), next_screen: 1, next_effect: 1 }
    }

    /// How the app is, as screens are told.
    pub fn context(&self) -> &Context {
        &self.cx
    }

    /// A screen made live; it gets its events through [`Root::update`] under the id returned.
    pub fn open<S: Screen + Send + 'static>(&mut self, screen: S) -> ScreenId {
        let id = loop {
            let id = self.next_screen;
            self.next_screen = self.next_screen.wrapping_add(1).max(1);
            if !self.screens.contains_key(&id) {
                break id;
            }
        };
        self.screens.insert(id, Box::new(screen));
        id
    }

    /// An event for the screen `screen`, which must be an `S`; nothing happens if it isn't, or has closed.
    pub fn update<S: Screen + 'static>(&mut self, screen: ScreenId, event: S::Event, clock: Clock) -> Turn {
        self.cx.clock = clock;
        let Some(model) = self.screens.get_mut(&screen).and_then(|s| s.any_mut().downcast_mut::<S>()) else {
            return Turn::default();
        };
        let effects = model.update(event, &self.cx);
        let mut turn = Turn { changed: vec![screen], ..Turn::default() };
        self.route(screen, effects, &mut turn);
        turn
    }

    /// What `screen` draws, if it's a live `S`.
    pub fn view<S: Screen + 'static>(&self, screen: ScreenId) -> Option<S::View> {
        self.screens.get(&screen)?.any().downcast_ref::<S>().map(Screen::view)
    }

    /// The app's answer to the effect `id`, passed to the screen (or the store) that asked; dropped when nothing waits
    /// for it.
    pub fn answer(&mut self, id: u32, reply: Reply, clock: Clock) -> Turn {
        self.cx.clock = clock;
        let mut turn = Turn::default();
        let Some(pending) = self.pending.remove(&id) else { return turn };
        // (maintenance told to every screen first, so the one that asked already knows why its answer failed)
        if let Some(message) = maintenance(&reply) {
            self.route(NO_SCREEN, vec![Effect::Tell { event: AppEvent::Maintenance { message } }], &mut turn);
        }
        match pending.owner {
            Owner::Screen { screen, local } => self.give(screen, local, reply, &mut turn),
            Owner::Store { key, step } => {
                let out = self.store.answer(&key, step, reply, &self.cx);
                self.take(out, &mut turn);
            }
        }
        turn
    }

    /// The app's own code wants the server page at `path`, kept under `key`, as `need` says; it comes back as a
    /// [`Delivery`] under `ticket`, in this turn or a later one.
    pub fn page(&mut self, path: &str, key: &str, need: Need, ticket: u64, clock: Clock) -> Turn {
        self.cx.clock = clock;
        let mut turn = Turn::default();
        let out = self.store.want(path, key, Waiter::Ticket(ticket), need, &self.cx);
        self.take(out, &mut turn);
        turn
    }

    /// Every page forgotten from memory (signed out): each is read again when next wanted.
    pub fn forget_pages(&mut self) {
        self.store.forget();
    }

    /// An answer for `screen`'s own effect `local`.
    fn give(&mut self, screen: ScreenId, local: u32, reply: Reply, turn: &mut Turn) {
        let Some(model) = self.screens.get_mut(&screen) else { return };
        let effects = model.answer(local, reply, &self.cx);
        turn.changed.push(screen);
        self.route(screen, effects, turn);
    }

    /// What the store asks for, carried out: its reads and requests under the root's ids, its pages to their waiters,
    /// and its changed pages told to every screen.
    fn take(&mut self, out: Out<Waiter>, turn: &mut Turn) {
        for (key, step, io) in out.io {
            let effect = match io {
                Io::Load { key: kept } => {
                    let id = self.wait_for(Owner::Store { key, step }, Waiting::Answer);
                    Effect::Load { id, key: kept }
                }
                Io::Ask { path, etag } => {
                    let request = crate::api::get(&self.cx.base, &self.cx.who(), &path, etag.as_deref());
                    Effect::Request { id: self.wait_for(Owner::Store { key, step }, Waiting::Request), request }
                }
                Io::Keep { key, body, etag } => Effect::Store { key, body, etag },
            };
            turn.orders.push(Order { screen: NO_SCREEN, effect });
        }
        if out.signed_out {
            turn.orders.push(Order { screen: NO_SCREEN, effect: Effect::SignedOut });
        }
        for (waiter, page) in out.give {
            match waiter {
                Waiter::Screen { screen, local } => self.give(screen, local, Reply::Page { page }, turn),
                Waiter::Ticket(ticket) => turn.delivered.push(Delivery { ticket, page }),
            }
        }
        for key in out.changed {
            let event = AppEvent::PageChanged { key: key.clone() };
            let ids: Vec<ScreenId> = self.screens.keys().copied().collect();
            for screen in ids {
                let Some(model) = self.screens.get_mut(&screen) else { continue };
                let effects = model.app(&event, &self.cx);
                if !effects.is_empty() {
                    self.route(screen, effects, turn);
                }
            }
            turn.pages_changed.push(key);
        }
    }

    fn wait_for(&mut self, owner: Owner, waiting: Waiting) -> u32 {
        let id = self.effect_id();
        self.pending.insert(id, Pending { owner, waiting });
        id
    }

    /// Something happened to the whole app: timers pause behind and run on in front, and every screen is told.
    pub fn app(&mut self, event: AppEvent, clock: Clock) -> Turn {
        self.cx.clock = clock;
        let mut turn = Turn::default();
        match event {
            AppEvent::Lifecycle { foreground } if foreground != self.cx.foreground => {
                self.cx.foreground = foreground;
                if foreground {
                    self.resume_timers(&mut turn);
                } else {
                    self.pause_timers(&mut turn);
                }
            }
            AppEvent::Lifecycle { .. } => {}
            AppEvent::Connectivity { online, metered } => (self.cx.online, self.cx.metered) = (online, metered),
            // (only the core tells of these)
            AppEvent::PageChanged { .. }
            | AppEvent::ProfilesChanged
            | AppEvent::ProfileGone { .. }
            | AppEvent::Account { .. }
            | AppEvent::Maintenance { .. } => return turn,
            AppEvent::Watched | AppEvent::Played { .. } => {}
        }
        let ids: Vec<ScreenId> = self.screens.keys().copied().collect();
        for screen in ids {
            let Some(model) = self.screens.get_mut(&screen) else { continue };
            let effects = model.app(&event, &self.cx);
            turn.changed.push(screen);
            self.route(screen, effects, &mut turn);
        }
        turn
    }

    /// The screen has gone: its timers and requests are cancelled, and any answer still to come for it is dropped.
    pub fn close(&mut self, screen: ScreenId, clock: Clock) -> Turn {
        self.cx.clock = clock;
        self.screens.remove(&screen);
        let mut ids: Vec<u32> = self.pending.iter().filter(|(_, p)| p.screen() == Some(screen)).map(|(id, _)| *id).collect();
        ids.sort_unstable();
        let mut turn = Turn::default();
        for id in ids {
            if let Some(pending) = self.pending.remove(&id) {
                stop(id, pending, &mut turn);
            }
        }
        turn
    }

    fn effect_id(&mut self) -> u32 {
        loop {
            let id = self.next_effect;
            self.next_effect = self.next_effect.wrapping_add(1).max(1);
            if !self.pending.contains_key(&id) {
                return id;
            }
        }
    }

    /// A screen's effects, under the root's ids, onto the turn.
    fn route(&mut self, screen: ScreenId, effects: Vec<Effect>, turn: &mut Turn) {
        let wait = |root: &mut Self, local: u32, waiting: Waiting| root.wait_for(Owner::Screen { screen, local }, waiting);
        for effect in effects {
            let effect = match effect {
                Effect::Request { id, request } => Effect::Request { id: wait(self, id, Waiting::Request), request },
                Effect::Fetch { id, request, file } => {
                    Effect::Fetch { id: wait(self, id, Waiting::Request), request, file }
                }
                Effect::Load { id, key } => Effect::Load { id: wait(self, id, Waiting::Answer), key },
                Effect::ReadPref { id, key } => Effect::ReadPref { id: wait(self, id, Waiting::Answer), key },
                Effect::Disk { id, op } => Effect::Disk { id: wait(self, id, Waiting::Answer), op },
                Effect::Timer { id, ms } if self.cx.foreground || self.screens.get(&screen).is_some_and(|s| s.behind()) => {
                    let due = self.cx.clock.now + f64::from(ms) / 1000.;
                    Effect::Timer { id: wait(self, id, Waiting::Timer { due }), ms }
                }
                Effect::Timer { id, ms } => {
                    wait(self, id, Waiting::Paused { left_ms: ms });
                    continue;
                }
                Effect::Page { id, path, key, need } => {
                    let out = self.store.want(&path, &key, Waiter::Screen { screen, local: id }, need, &self.cx);
                    self.take(out, turn);
                    continue;
                }
                Effect::Tell { event } => {
                    let ids: Vec<ScreenId> = self.screens.keys().copied().collect();
                    for to in ids {
                        let Some(model) = self.screens.get_mut(&to) else { continue };
                        let effects = model.app(&event, &self.cx);
                        turn.changed.push(to);
                        self.route(to, effects, turn);
                    }
                    continue;
                }
                Effect::Forget => {
                    self.store.forget();
                    Effect::Forget
                }
                // (another profile's Home is its own)
                Effect::Profile { id } => {
                    self.store.forget_one(crate::home::KEY);
                    Effect::Profile { id }
                }
                Effect::Cancel { id: local } => {
                    let found = self.pending.iter().find(|(_, p)| matches!(p.owner, Owner::Screen { screen: s, local: l } if s == screen && l == local));
                    if let Some(id) = found.map(|(id, _)| *id) {
                        if let Some(pending) = self.pending.remove(&id) {
                            stop(id, pending, turn);
                        }
                    }
                    continue;
                }
                other => other,
            };
            turn.orders.push(Order { screen, effect });
        }
    }

    /// Every running timer stopped, with what it had left kept under a new id, so an answer from before the pause, if
    /// one slips through, can't fire it.
    fn pause_timers(&mut self, turn: &mut Turn) {
        let now = self.cx.clock.now;
        let behind = |p: &Pending| p.screen().and_then(|s| self.screens.get(&s)).is_some_and(|s| s.behind());
        let mut running: Vec<u32> = self.pending.iter()
            .filter(|(_, p)| matches!(p.waiting, Waiting::Timer { .. }) && !behind(p)).map(|(id, _)| *id).collect();
        running.sort_unstable();
        for old in running {
            let Some(Pending { owner, waiting: Waiting::Timer { due } }) = self.pending.remove(&old) else {
                continue;
            };
            let screen = owner_screen(&owner);
            let left_ms = ((due - now).max(0.) * 1000.).round().min(f64::from(u32::MAX)) as u32;
            self.wait_for(owner, Waiting::Paused { left_ms });
            turn.orders.push(Order { screen, effect: Effect::Cancel { id: old } });
        }
    }

    /// Every paused timer started again for the time it had left.
    fn resume_timers(&mut self, turn: &mut Turn) {
        let mut paused: Vec<u32> =
            self.pending.iter().filter(|(_, p)| matches!(p.waiting, Waiting::Paused { .. })).map(|(id, _)| *id).collect();
        paused.sort_unstable();
        for old in paused {
            let Some(Pending { owner, waiting: Waiting::Paused { left_ms } }) = self.pending.remove(&old) else {
                continue;
            };
            let screen = owner_screen(&owner);
            let due = self.cx.clock.now + f64::from(left_ms) / 1000.;
            let id = self.wait_for(owner, Waiting::Timer { due });
            turn.orders.push(Order { screen, effect: Effect::Timer { id, ms: left_ms } });
        }
    }
}

/// What it takes to stop an effect no one waits for any more: a running timer or request is cancelled; anything
/// else just has its answer dropped.
fn stop(id: u32, pending: Pending, turn: &mut Turn) {
    if matches!(pending.waiting, Waiting::Request | Waiting::Timer { .. }) {
        turn.orders.push(Order { screen: owner_screen(&pending.owner), effect: Effect::Cancel { id } });
    }
}

fn owner_screen(owner: &Owner) -> ScreenId {
    match owner {
        Owner::Screen { screen, .. } => *screen,
        Owner::Store { .. } => NO_SCREEN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{response, test_context};

    /// A screen that asks for whatever it's told to, and remembers what came back.
    #[derive(Default)]
    struct Probe {
        got: Vec<(u32, Reply)>,
        heard: Vec<AppEvent>,
    }

    impl Screen for Probe {
        type Event = Vec<Effect>;
        type View = usize;
        fn update(&mut self, event: Vec<Effect>, _: &Context) -> Vec<Effect> {
            event
        }
        fn answer(&mut self, id: u32, reply: Reply, _: &Context) -> Vec<Effect> {
            self.got.push((id, reply));
            vec![]
        }
        fn view(&self) -> usize {
            self.got.len()
        }
        fn app(&mut self, event: &AppEvent, _: &Context) -> Vec<Effect> {
            self.heard.push(event.clone());
            vec![]
        }
    }

    fn at(now: f64) -> Clock {
        Clock { now, utc_offset: 0 }
    }

    fn root() -> Root {
        let cx = test_context(0.);
        Root::new(cx.platform, "https://host.invalid/".into(), at(1000.))
    }

    fn request(id: u32) -> Effect {
        Effect::Request { id, request: crate::api::get("https://host.invalid", &[], "/x", None) }
    }

    fn ids(turn: &Turn) -> Vec<u32> {
        turn.orders
            .iter()
            .filter_map(|o| match o.effect {
                Effect::Request { id, .. } | Effect::Timer { id, .. } | Effect::Load { id, .. } => Some(id),
                _ => None,
            })
            .collect()
    }

    fn probe(root: &Root, screen: ScreenId) -> &Probe {
        root.screens.get(&screen).and_then(|s| s.any().downcast_ref::<Probe>()).unwrap_or_else(|| panic!("no probe"))
    }

    #[test]
    fn ids_are_unique_across_screens_and_answers_find_their_screen() {
        let mut root = root();
        let (a, b) = (root.open(Probe::default()), root.open(Probe::default()));
        let first = ids(&root.update::<Probe>(a, vec![request(1)], at(1000.)));
        let second = ids(&root.update::<Probe>(b, vec![request(1)], at(1000.)));
        assert_ne!(first, second);
        let turn = root.answer(second[0], response(200, "b", None), at(1001.));
        assert_eq!(turn.changed, [b]);
        assert_eq!(probe(&root, b).got, [(1, response(200, "b", None))]);
        assert!(probe(&root, a).got.is_empty());
        assert_eq!(root.view::<Probe>(b), Some(1));
        assert_eq!(root.context().base, "https://host.invalid");
    }

    #[test]
    fn closing_a_screen_cancels_its_request_and_drops_the_late_answer() {
        let mut root = root();
        let screen = root.open(Probe::default());
        let load = Effect::Load { id: 2, key: "k".into() };
        let asked = ids(&root.update::<Probe>(screen, vec![request(1), load], at(1000.)));
        let turn = root.close(screen, at(1001.));
        assert_eq!(turn.orders, [Order { screen, effect: Effect::Cancel { id: asked[0] } }]);
        assert_eq!(root.answer(asked[0], response(200, "late", None), at(1002.)), Turn::default());
        assert_eq!(root.answer(asked[1], Reply::Missing, at(1002.)), Turn::default());
        assert_eq!(root.view::<Probe>(screen), None);
        assert_eq!(root.update::<Probe>(screen, vec![request(3)], at(1003.)), Turn::default());
    }

    #[test]
    fn a_screen_cancelling_its_own_effect_cancels_it_under_the_roots_id() {
        let mut root = root();
        let screen = root.open(Probe::default());
        let asked = ids(&root.update::<Probe>(screen, vec![Effect::Timer { id: 7, ms: 500 }], at(1000.)));
        let turn = root.update::<Probe>(screen, vec![Effect::Cancel { id: 7 }], at(1000.1));
        assert_eq!(turn.orders, [Order { screen, effect: Effect::Cancel { id: asked[0] } }]);
        assert_eq!(root.answer(asked[0], Reply::Fired, at(1000.5)), Turn::default());
        assert_eq!(root.update::<Probe>(screen, vec![Effect::Cancel { id: 99 }], at(1001.)).orders, []);
    }

    #[test]
    fn a_timer_paused_in_the_background_resumes_with_its_time_left() {
        let mut root = root();
        let screen = root.open(Probe::default());
        let asked = ids(&root.update::<Probe>(screen, vec![Effect::Timer { id: 4, ms: 10_000 }], at(1000.)));
        let behind = root.app(AppEvent::Lifecycle { foreground: false }, at(1003.));
        assert_eq!(behind.orders, [Order { screen, effect: Effect::Cancel { id: asked[0] } }]);
        assert_eq!(behind.changed, [screen]);
        // (the cancelled timer's answer, had it slipped through, is ignored)
        assert_eq!(root.answer(asked[0], Reply::Fired, at(1010.)), Turn::default());
        // (a timer asked for while behind waits too)
        assert_eq!(root.update::<Probe>(screen, vec![Effect::Timer { id: 5, ms: 2000 }], at(1020.)).orders, []);
        assert_eq!(root.app(AppEvent::Lifecycle { foreground: false }, at(1030.)).orders, []);

        let front = root.app(AppEvent::Lifecycle { foreground: true }, at(2000.));
        let timers: Vec<(u32, u32)> = front
            .orders
            .iter()
            .filter_map(|o| match o.effect {
                Effect::Timer { id, ms } => Some((id, ms)),
                _ => None,
            })
            .collect();
        assert_eq!(timers.iter().map(|t| t.1).collect::<Vec<_>>(), [7000, 2000]);
        assert!(!timers.iter().any(|t| t.0 == asked[0]));
        root.answer(timers[0].0, Reply::Fired, at(2007.));
        assert_eq!(probe(&root, screen).got, [(4, Reply::Fired)]);
        let heard = &probe(&root, screen).heard;
        assert_eq!(heard, &[AppEvent::Lifecycle { foreground: false }, AppEvent::Lifecycle { foreground: false }, AppEvent::Lifecycle { foreground: true }]);
    }

    #[test]
    fn connectivity_is_told_to_every_screen_and_kept_in_the_context() {
        let mut root = root();
        let (a, b) = (root.open(Probe::default()), root.open(Probe::default()));
        let event = AppEvent::Connectivity { online: false, metered: true };
        let turn = root.app(event.clone(), at(1000.));
        assert_eq!(turn.changed, [a, b]);
        assert_eq!(probe(&root, a).heard, std::slice::from_ref(&event));
        assert_eq!(probe(&root, b).heard, [event]);
        assert!(!root.context().online && root.context().metered);
    }

    #[test]
    fn page_level_effects_go_out_as_they_are_for_their_screen() {
        let mut root = root();
        let screen = root.open(Probe::default());
        let play = Effect::Play { show: "s".into(), episode: None };
        let write = Effect::WritePref { key: "k".into(), value: None };
        let turn = root.update::<Probe>(screen, vec![play.clone(), write.clone()], at(1000.));
        assert_eq!(turn.orders, [Order { screen, effect: play }, Order { screen, effect: write }]);
    }

    /// A screen showing one page from the store.
    #[derive(Default)]
    struct Shows {
        source: Option<crate::screen::Source>,
    }

    impl Screen for Shows {
        type Event = ();
        type View = Option<String>;
        fn update(&mut self, _: (), cx: &Context) -> Vec<Effect> {
            self.source.get_or_insert_with(|| crate::screen::Source::new("/api/home", "home")).open(cx)
        }
        fn answer(&mut self, id: u32, reply: Reply, _: &Context) -> Vec<Effect> {
            self.source.as_mut().and_then(|s| s.answer(id, reply)).unwrap_or_default()
        }
        fn view(&self) -> Option<String> {
            self.source.as_ref().and_then(|s| s.shown.clone())
        }
        fn app(&mut self, event: &AppEvent, _: &Context) -> Vec<Effect> {
            match (event, self.source.as_mut()) {
                (AppEvent::PageChanged { key }, Some(source)) => source.changed(key),
                _ => vec![],
            }
        }
    }

    fn only(turn: &Turn) -> &Effect {
        match turn.orders.as_slice() {
            [order] => &order.effect,
            other => panic!("expected one order, got {other:?}"),
        }
    }

    #[test]
    fn two_screens_and_the_app_share_one_page() {
        let mut root = root();
        let (a, b) = (root.open(Shows::default()), root.open(Shows::default()));
        let Effect::Load { id: load, .. } = only(&root.update::<Shows>(a, (), at(1000.))).clone() else { panic!() };
        assert_eq!(root.update::<Shows>(b, (), at(1000.)), Turn { changed: vec![b], ..Turn::default() });
        assert!(root.page("/api/home", "home", Need::Any, 9, at(1000.)).orders.is_empty());
        let Effect::Request { id: ask, request } = only(&root.answer(load, Reply::Missing, at(1000.))).clone() else { panic!() };
        assert!(request.cookies && request.url.ends_with("/api/home"));
        let came = root.answer(ask, response(200, "page", None), at(1001.));
        assert_eq!(came.changed, [a, b]);
        assert_eq!(came.delivered.iter().map(|d| d.ticket).collect::<Vec<_>>(), [9]);
        assert!(matches!(only(&came), Effect::Store { key, .. } if key == "home"));
        assert_eq!(root.view::<Shows>(a), Some(Some("page".into())));
        assert_eq!(root.view::<Shows>(b), Some(Some("page".into())));
    }

    #[test]
    fn a_newer_page_is_told_to_the_screens_showing_it() {
        let mut root = root();
        let screen = root.open(Shows::default());
        let Effect::Load { id: load, .. } = only(&root.update::<Shows>(screen, (), at(100_000.))).clone() else { panic!() };
        // (a minute old: past fresh but usable, so shown at once and asked again behind it)
        let kept = Reply::Kept { body: "old".into(), etag: None, at: 99_940. };
        let Effect::Request { id: ask, .. } = only(&root.answer(load, kept, at(100_000.))).clone() else { panic!() };
        assert_eq!(root.view::<Shows>(screen), Some(Some("old".into())));
        let came = root.answer(ask, response(200, "new", None), at(100_001.));
        assert_eq!(came.pages_changed, ["home".to_string()]);
        assert_eq!(root.view::<Shows>(screen), Some(Some("new".into())));
    }

    #[test]
    fn a_401_from_the_store_signs_out() {
        let mut root = root();
        let Effect::Load { id: load, .. } = only(&root.page("/api/home", "home", Need::Any, 1, at(1000.))).clone() else { panic!() };
        let Effect::Request { id: ask, .. } = only(&root.answer(load, Reply::Missing, at(1000.))).clone() else { panic!() };
        let gone = root.answer(ask, response(401, "", None), at(1001.));
        assert!(gone.orders.iter().any(|o| o.effect == Effect::SignedOut));
        assert_eq!(gone.delivered[0].page.body, None);
    }

    #[test]
    fn an_event_for_the_wrong_kind_of_screen_does_nothing() {
        let mut root = root();
        let screen = root.open(Probe::default());
        let turn = root.update::<crate::history::History>(screen, crate::history::Event::Retry, at(1000.));
        assert_eq!(turn, Turn::default());
        assert_eq!(root.view::<crate::history::History>(screen), None);
    }

    #[test]
    fn maintenance_from_any_screen_takes_the_session_there_and_its_end_brings_it_back() {
        use crate::session::{Event as S, Session, Stage, MAINTENANCE_HEADER, MAINTENANCE_MS};
        let mut root = root();
        let session = root.open(Session::default());
        let (page, other) = (root.open(Probe::default()), root.open(Probe::default()));
        let started = root.update::<Session>(session, S::Started, at(1000.));
        root.update::<Session>(session, S::SplashDone, at(1000.));
        let check = ids(&started)[0];
        let body = r#"{"profiles": [{"id": "p1", "name": "A"}], "current": "p1"}"#;
        root.answer(check, response(200, body, None), at(1000.));
        root.update::<Session>(session, S::Pick { id: "p1".into() }, at(1000.));
        // (a plain 503 is only that screen's failure)
        let asked = ids(&root.update::<Probe>(page, vec![request(1)], at(1000.)))[0];
        root.answer(asked, response(503, "{}", None), at(1000.));
        assert_ne!(root.view::<Session>(session).map(|v| v.stage), Some(Stage::Maintenance));
        // (one with the maintenance header: every screen hears it, the session shows it)
        let asked = ids(&root.update::<Probe>(page, vec![request(2)], at(1000.)))[0];
        let mut reply = response(503, r#"{"maintenance": true, "message": "Back at 9."}"#, None);
        if let Reply::Response { headers, .. } = &mut reply {
            headers.push(crate::screen::Header::new(MAINTENANCE_HEADER, "1"));
        }
        let turn = root.answer(asked, reply, at(1000.));
        let view = root.view::<Session>(session).unwrap_or_default();
        assert_eq!((view.stage, view.maintenance_message.as_str()), (Stage::Maintenance, "Back at 9."));
        assert!(probe(&root, other).heard.contains(&AppEvent::Maintenance { message: "Back at 9.".into() }));
        // (15 s on, it asks; still on, it waits again; off, "We're back" until Continue)
        let timer = turn.orders.iter().find_map(|o| match o.effect { Effect::Timer { id, ms } if ms == MAINTENANCE_MS => Some(id), _ => None });
        let ask = ids(&root.answer(timer.unwrap_or_default(), Reply::Fired, at(1015.)));
        let turn = root.answer(ask[0], response(200, r#"{"on": true, "message": "Soon."}"#, None), at(1015.));
        assert_eq!(root.view::<Session>(session).unwrap_or_default().maintenance_message, "Soon.");
        let ask = ids(&root.answer(ids(&turn)[0], Reply::Fired, at(1030.)));
        let turn = root.answer(ask[0], response(200, r#"{"on": false, "message": ""}"#, None), at(1030.));
        assert!(ids(&turn).is_empty());
        let view = root.view::<Session>(session).unwrap_or_default();
        assert_eq!((view.stage, view.maintenance_over, view.maintenance_title.as_str()), (Stage::Maintenance, true, "We're back"));
        // (Continue: the account checked, back by Who's watching)
        let turn = root.update::<Session>(session, S::Continue, at(1040.));
        assert!(turn.orders.iter().any(|o| matches!(&o.effect, Effect::Request { request, .. } if request.url.ends_with(crate::session::PROFILES))));
        assert_eq!(root.view::<Session>(session).map(|v| v.stage), Some(Stage::Who));
    }

    #[test]
    fn starting_into_maintenance_stays_there_past_the_splash() {
        use crate::session::{Event as S, Session, Stage, MAINTENANCE_HEADER};
        let mut root = root();
        let session = root.open(Session::default());
        let started = root.update::<Session>(session, S::Started, at(1000.));
        let mut reply = response(503, r#"{"maintenance": true, "message": ""}"#, None);
        if let Reply::Response { headers, .. } = &mut reply {
            headers.push(crate::screen::Header::new(MAINTENANCE_HEADER, "1"));
        }
        root.answer(ids(&started)[0], reply, at(1000.));
        root.update::<Session>(session, S::SplashDone, at(1001.));
        let view = root.view::<Session>(session).unwrap_or_default();
        assert_eq!((view.stage, view.offline, view.maintenance_back.as_str()), (Stage::Maintenance, false, "We'll be back soon."));
    }
}
