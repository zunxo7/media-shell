//! Your TV on a phone or a PC: keeping up with the account's TV while the app shows, playing things there (Play on
//! TV), working it from the Your TV page and the TV bar, and signing it out. The rules of the conversation are
//! [`crate::remote`]'s; this screen holds it: one request waiting at the server for what's on the TV, and what's
//! pressed sent one at a time, in order.
//!
//! It keeps up while an account is signed in (told by [`AppEvent::Account`]) and forgets everything at sign-out; the TV
//! itself has none (it's [`crate::on_tv`]'s other end).
//!
//! ```
//! use zcore::root::Root;
//! use zcore::screen::{AppEvent, Clock, Device, Effect, Platform, Screen};
//! use zcore::session::Account;
//! use zcore::your_tv::YourTv;
//!
//! let platform = Platform { device: Device::Phone, model: "Pixel".into(), version_name: "1.0".into(),
//!                           version_code: 1, os: "t".into(), stream: "s".into(), codecs: vec![], hdr: false };
//! let clock = Clock { now: 1_794_657_600., utc_offset: 0 };
//! let mut root = Root::new(platform, "https://host.invalid".into(), clock);
//! let account = Account { profiles: vec![], current: None, plus: false, max_profiles: 5 };
//! let effects = YourTv::default().app(&AppEvent::Account { account: Some(account) }, root.context());
//! assert!(matches!(&effects[0],
//!                  Effect::Request { request, .. } if request.url.ends_with("/api/remote/now?v=-1")));
//! ```

use crate::api::{self, Outcome};
use crate::model;
use crate::pictures;
use crate::player::clock as time_words;
use crate::remote::{self, carried, Group, Key, Remote};
use crate::screen::{AppEvent, Asks, Context, Device, Effect, Need, Reply, Screen};
use serde_json::{json, Value};
use std::collections::VecDeque;

/// How long the wait for what's on the TV may take: the server answers within 25 s.
pub const LONG_POLL_MS: u32 = 40_000;
/// Presses waiting to go out; the oldest give way (the server keeps 12 for a TV anyway).
const BACKLOG: usize = 16;
/// Where a signed-out TV is taken off the account.
const REMOVE: &str = "/api/devices/remove";

/// What happens to Your TV.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The Your TV page opened: no question asked, no note.
    Opened,
    /// Play on TV switched.
    Switch,
    /// Play a title on the TV (sent only while [`View::plays_on_tv`]).
    Play { title: String, episode: Option<String> },
    Toggle,
    /// Back or forward 10 s.
    Jump { forward: bool },
    /// The bar let go (or tapped) this far along, 0 to 1.
    Scrub { share: f32 },
    Next,
    /// Stop playing.
    Stop,
    /// One of the TV player's choices picked.
    Pick { group: String, choice: String },
    /// One of the TV remote's buttons.
    Key { key: Key },
    /// Sign out TV: the first press asks, the second does it.
    SignOutTv,
}

/// Exactly what Your TV, its bar and the entry to it draw.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// The sidebar's or menu's entry: "Your TV", or "Sign in a TV" while the account has none.
    pub entry: String,
    /// The account has a TV: the entry opens Your TV (else Sign in a TV).
    pub has_tv: bool,
    /// Play anywhere starts it on the TV (a download still plays here).
    pub plays_on_tv: bool,
    /// The page's head.
    pub title: String,
    /// The TV's name, and the line under it.
    pub name: String,
    pub status: String,
    /// Play on TV, and whether it's on.
    pub switch: String,
    pub on: bool,
    /// What's on the TV.
    pub playing: Option<Playing>,
    /// What the page says with nothing on; `waiting`: a title chosen that hasn't started yet.
    pub idle: String,
    pub waiting: bool,
    /// The TV bar over every other page, while Play on TV is on.
    pub bar: Option<Bar>,
    /// The choices the captions button shows, and the settings button's: a button shows only with some.
    pub captions: Vec<Group>,
    pub settings: Vec<Group>,
    /// Those buttons' sheet's button.
    pub done: String,
    pub change: String,
    pub sign_out: String,
    /// Sign out TV asked once: its button says `sure_words`.
    pub sure: bool,
    pub sure_words: String,
    pub stop: String,
    /// Why signing the TV out failed.
    pub note: Option<String>,
    /// Counts up each time the page should close (the TV signed out).
    pub closed: u32,
}

/// What's on the TV, as the page shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Playing {
    pub title: String,
    /// Where it is ("Season 1 · Episode 3"), or what went wrong on the TV (`failed`).
    pub line: String,
    pub failed: bool,
    /// Its picture at the page's size.
    pub art: Option<String>,
    pub paused: bool,
    pub buffering: bool,
    /// The Next button: "Next: Episode 4".
    pub next: Option<String>,
    /// Where it was when heard, and its length, in seconds; `at`: when heard, ms on the core's clock.
    pub time: f64,
    pub total: f64,
    pub at: u64,
    /// It plays on between reports, so the page redraws as time passes.
    pub moving: bool,
}

/// How far along it is at a moment: the bar's share, and the times either side of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub share: f32,
    /// "4:05", and what's left: "-1:02:03".
    pub at: String,
    pub left: String,
}

impl Playing {
    /// Where it's up to at `now` (seconds since 1970), or where the bar is held while it's dragged (`scrub`, 0 to 1).
    pub fn place(&self, now: f64, scrub: Option<f32>) -> Place {
        let at = match scrub {
            Some(share) => self.total * f64::from(share.clamp(0., 1.)),
            None => carried(self.time, self.total, self.moving, self.at, ms(now)),
        };
        let share = if self.total > 0. { (at / self.total).clamp(0., 1.) as f32 } else { 0. };
        Place { share, at: time_words(at), left: format!("-{}", time_words(self.total - at)) }
    }
}

/// The TV bar: what's on there (or what Play on TV does), its picture, and its play/pause while something is on.
#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    pub title: String,
    pub line: String,
    pub art: Option<String>,
    pub paused: Option<bool>,
}

/// What an effect was asked for.
#[derive(Debug)]
enum Ask {
    /// What's on the TV.
    Now,
    /// The wait before asking again.
    Wait,
    /// A press going out.
    Send,
    /// The title whose picture shows.
    Art((String, Option<String>)),
    /// The TV taken off the account.
    Remove,
}

/// Which title and episode the picture is for, and its address once known.
#[derive(Debug)]
struct Art {
    of: (String, Option<String>),
    url: Option<String>,
}

/// Your TV's model.
#[derive(Debug, Default)]
pub struct YourTv {
    remote: Remote,
    asks: Asks<Ask>,
    /// Keeping up: signed in, on a phone or a PC.
    kept: bool,
    /// The request waiting at the server, or the wait before the next; None while not keeping up.
    listening: Option<u32>,
    version: i64,
    queue: VecDeque<Value>,
    sending: bool,
    art: Option<Art>,
    sure: bool,
    note: Option<String>,
    closed: u32,
    desktop: bool,
    /// The server, for the pictures' addresses.
    base: String,
}

/// The core's clock in ms, as [`crate::remote`] counts.
fn ms(now: f64) -> u64 {
    (now.max(0.) * 1000.) as u64
}

impl YourTv {
    fn ask_now(&mut self, cx: &Context) -> Effect {
        let id = self.asks.ask(Ask::Now);
        self.listening = Some(id);
        let mut request = api::get(&cx.base, &cx.who(), &remote::now_path(self.version), None);
        request.timeout_ms = LONG_POLL_MS;
        Effect::Request { id, request }
    }

    fn wait(&mut self, ms: u64) -> Effect {
        let id = self.asks.ask(Ask::Wait);
        self.listening = Some(id);
        Effect::Timer { id, ms: ms as u32 }
    }

    /// Signed out: everything forgotten (the request or wait under way cancelled).
    fn forget(&mut self) -> Vec<Effect> {
        let effects = self.halt();
        let closed = self.closed;
        *self = Self {
            asks: std::mem::take(&mut self.asks),
            closed,
            desktop: self.desktop,
            base: std::mem::take(&mut self.base),
            ..Self::default()
        };
        self.asks.forget();
        effects
    }

    /// Starts keeping up, when it should and isn't yet.
    fn start(&mut self, cx: &Context) -> Vec<Effect> {
        if !self.kept || !cx.foreground || cx.platform.device == Device::Tv || self.listening.is_some() {
            return vec![];
        }
        self.version = -1;
        vec![self.ask_now(cx)]
    }

    /// Stops keeping up: the request or wait under way is cancelled.
    fn halt(&mut self) -> Vec<Effect> {
        self.listening.take().map(|id| Effect::Cancel { id }).into_iter().collect()
    }

    /// A press for the TV queued (nothing: no TV, or held until it's ready), and the next sent if none is going.
    fn send(&mut self, body: Option<Value>, cx: &Context) -> Vec<Effect> {
        if let Some(body) = body {
            if self.queue.len() >= BACKLOG {
                self.queue.pop_front();
            }
            self.queue.push_back(body);
        }
        self.pump(cx)
    }

    fn pump(&mut self, cx: &Context) -> Vec<Effect> {
        if self.sending {
            return vec![];
        }
        let Some(body) = self.queue.pop_front() else { return vec![] };
        self.sending = true;
        let id = self.asks.ask(Ask::Send);
        vec![Effect::Request { id, request: api::post(&cx.base, &cx.who(), remote::SEND, body.to_string()) }]
    }

    /// The picture for what's on, asked for once for each title and episode.
    fn want_art(&mut self) -> Vec<Effect> {
        let Some(now) = self.remote.now() else { return vec![] };
        let wanted = (now.title_id.clone(), now.episode_id.clone());
        if wanted.0.is_empty() || self.art.as_ref().is_some_and(|art| art.of == wanted) {
            return vec![];
        }
        self.art = Some(Art { of: wanted.clone(), url: None });
        let (path, key) = (format!("/api/title/{}", wanted.0), format!("title-{}", wanted.0));
        vec![Effect::Page { id: self.asks.ask(Ask::Art(wanted)), path, key, need: Need::Any }]
    }

    /// The picture for what's on at a card's `size`, when it's known.
    fn art(&self, size: &str) -> Option<String> {
        let now = self.remote.now()?;
        let art = self.art.as_ref()?;
        (art.of.0 == now.title_id && art.of.1 == now.episode_id)
            .then(|| art.url.as_deref().map(|u| pictures::card(&self.base, u, size)))
            .flatten()
    }

    fn heard(&mut self, body: &str, cx: &Context) -> Vec<Effect> {
        let Ok(answer) = serde_json::from_str::<Value>(body) else {
            return vec![self.wait(remote::TROUBLE_WAIT_MS)];
        };
        let heard = remote::heard(&answer, ms(cx.clock.now));
        self.version = heard.version.unwrap_or(self.version);
        let no_tv = heard.tv.is_none();
        let (_, send) = self.remote.hear(heard);
        let mut effects = self.send(send, cx);
        effects.extend(self.want_art());
        effects.push(if no_tv { self.wait(remote::NO_TV_WAIT_MS) } else { self.ask_now(cx) });
        effects
    }
}

impl Screen for YourTv {
    type Event = Event;
    type View = View;

    fn update(&mut self, event: Event, cx: &Context) -> Vec<Effect> {
        self.desktop = cx.platform.device == Device::Desktop;
        self.base.clone_from(&cx.base);
        let clock = ms(cx.clock.now);
        match event {
            Event::Opened => {
                self.sure = false;
                self.note = None;
                vec![]
            }
            Event::Switch => {
                self.remote.on = !self.remote.on;
                vec![]
            }
            Event::Play { title, episode } => {
                if !self.remote.on || self.remote.tv().is_none() {
                    return vec![];
                }
                let body = self.remote.play(&title, episode.as_deref());
                self.send(body, cx)
            }
            Event::Toggle => {
                let body = self.remote.toggle(clock);
                self.send(body, cx)
            }
            Event::Jump { forward } => {
                let body = self.remote.jump(forward, clock);
                self.send(body, cx)
            }
            Event::Scrub { share } => {
                let total = self.remote.now().map_or(0., |n| n.total);
                let body = self.remote.seek(total * f64::from(share.clamp(0., 1.)), clock);
                self.send(body, cx)
            }
            Event::Next => {
                let body = self.remote.next_episode();
                self.send(body, cx)
            }
            Event::Stop => {
                let body = self.remote.stop();
                self.send(body, cx)
            }
            Event::Pick { group, choice } => {
                let body = self.remote.pick(&group, &choice);
                self.send(body, cx)
            }
            Event::Key { key } => {
                let body = self.remote.key(key);
                self.send(body, cx)
            }
            Event::SignOutTv => {
                if !self.sure {
                    self.sure = true;
                    self.note = None;
                    return vec![];
                }
                self.sure = false;
                let Some(tv) = self.remote.tv() else { return vec![] };
                let body = json!({"id": tv.id}).to_string();
                vec![Effect::Request { id: self.asks.ask(Ask::Remove), request: api::post(&cx.base, &cx.who(), REMOVE, body) }]
            }
        }
    }

    fn answer(&mut self, id: u32, reply: Reply, cx: &Context) -> Vec<Effect> {
        let Some(ask) = self.asks.take(id) else { return vec![] };
        match ask {
            Ask::Now => {
                self.listening = None;
                match api::outcome(reply, cx.online) {
                    Outcome::Body { body, .. } => self.heard(&body, cx),
                    _ => vec![self.wait(remote::TROUBLE_WAIT_MS)],
                }
            }
            Ask::Wait => {
                self.listening = None;
                vec![self.ask_now(cx)]
            }
            Ask::Send => {
                self.sending = false;
                let mut effects = vec![];
                if let Outcome::Failed { failure } = api::outcome(reply, cx.online) {
                    if failure.status() >= 500 {
                        effects.push(Effect::Report { place: "remote.send".into(), message: failure.words });
                    }
                }
                effects.extend(self.pump(cx));
                effects
            }
            Ask::Art(wanted) => {
                let Reply::Page { page } = reply else { return vec![] };
                let found = page.body.as_deref().and_then(|b| serde_json::from_str::<Value>(b).ok()).and_then(|o| {
                    remote::art(&model::title(&wanted.0, &o), wanted.1.as_deref())
                });
                if let Some(art) = self.art.as_mut().filter(|art| art.of == wanted) {
                    art.url = found;
                }
                vec![]
            }
            Ask::Remove => {
                match api::outcome(reply, cx.online) {
                    Outcome::Failed { failure } => self.note = Some(failure.words),
                    _ => {
                        self.remote.forget_tv();
                        self.closed += 1;
                    }
                }
                vec![]
            }
        }
    }

    fn app(&mut self, event: &AppEvent, cx: &Context) -> Vec<Effect> {
        match event {
            AppEvent::Lifecycle { foreground: false } => self.halt(),
            AppEvent::Lifecycle { foreground: true } => self.start(cx),
            // (signed in: keep up with the TV, in the background stopping by itself; signed out: forget it)
            AppEvent::Account { account: Some(_) } => {
                self.desktop = cx.platform.device == Device::Desktop;
                self.base.clone_from(&cx.base);
                self.kept = true;
                self.start(cx)
            }
            AppEvent::Account { account: None } => self.forget(),
            _ => vec![],
        }
    }

    fn view(&self) -> View {
        let r = &self.remote;
        let playing = r.now().map(|n| Playing {
            title: n.title.clone(),
            line: n.line().to_string(),
            failed: !n.error.is_empty(),
            art: self.art(pictures::LANDSCAPE),
            paused: n.paused,
            buffering: n.buffering,
            next: n.next.as_ref().map(|next| format!("Next: {next}")),
            time: n.time,
            total: n.total,
            at: n.at,
            moving: !n.paused && !n.buffering,
        });
        let groups = |wanted: &[&str]| -> Vec<Group> {
            r.now().map(|n| n.groups_for(wanted).into_iter().cloned().collect()).unwrap_or_default()
        };
        let bar = r.on.then(|| {
            let bar = r.bar();
            Bar { title: bar.title, line: bar.line, art: self.art(pictures::THUMB), paused: r.now().map(|n| n.paused) }
        });
        View {
            entry: if r.tv().is_some() { "Your TV" } else { "Sign in a TV" }.into(),
            has_tv: r.tv().is_some(),
            plays_on_tv: r.on && r.tv().is_some(),
            title: "Your TV".into(),
            name: r.name().into(),
            status: r.status().into(),
            switch: "Play on TV".into(),
            on: r.on,
            playing,
            idle: r.idle().into(),
            waiting: r.waiting().is_some(),
            bar,
            captions: groups(&remote::CAPTIONS),
            settings: groups(&remote::SETTINGS),
            done: "Done".into(),
            change: "Change TV".into(),
            sign_out: "Sign out TV".into(),
            sure: self.sure,
            sure_words: if self.desktop { "Click again to sign it out" } else { "Press again to sign it out" }.into(),
            stop: "Stop playing".into(),
            note: self.note.clone(),
            closed: self.closed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{response, test_context, Method, PageReply};

    fn playing() -> String {
        json!({"tv": {"id": "tv1", "name": "Living room"}, "v": 7, "online": true, "age": 0.0, "now": {
            "titleId": "tt1", "episodeId": "e2", "title": "Dune", "where": "Season 1 · Episode 2", "time": 100.0,
            "total": 9000.0, "paused": false, "buffering": false, "error": "", "next": "Episode 3",
            "groups": [{"title": "Subtitles", "choices": [{"label": "Off", "on": true}]}]}})
        .to_string()
    }

    fn request(effects: &[Effect]) -> (u32, crate::screen::Request) {
        effects
            .iter()
            .find_map(|e| match e {
                Effect::Request { id, request } => Some((*id, request.clone())),
                _ => None,
            })
            .expect("a request")
    }

    fn account() -> crate::session::Account {
        crate::session::Account { profiles: vec![], current: None, plus: false, max_profiles: 5 }
    }

    fn kept() -> (YourTv, Context, u32) {
        let cx = test_context(1000.);
        let mut tv = YourTv::default();
        let (id, request) = request(&tv.app(&AppEvent::Account { account: Some(account()) }, &cx));
        assert_eq!(request.url, "https://host.invalid/api/remote/now?v=-1");
        assert_eq!(request.timeout_ms, LONG_POLL_MS);
        assert!(request.cookies);
        (tv, cx, id)
    }

    #[test]
    fn keeps_up_and_asks_again_with_the_version() {
        let (mut tv, cx, id) = kept();
        let effects = tv.answer(id, response(200, &playing(), None), &cx);
        assert!(matches!(&effects[0], Effect::Page { path, key, .. } if path == "/api/title/tt1" && key == "title-tt1"));
        let (_, again) = request(&effects);
        assert!(again.url.ends_with("?v=7"));
        let view = tv.view();
        assert_eq!(view.entry, "Your TV");
        assert_eq!(view.name, "Living room");
        let p = view.playing.expect("playing");
        assert_eq!(p.next.as_deref(), Some("Next: Episode 3"));
        assert_eq!(p.place(1003., None).at, "1:43");
        assert_eq!(p.place(1003., Some(0.5)).left, "-1:15:00");
        assert_eq!(view.captions.len(), 1);
    }

    #[test]
    fn no_tv_waits_and_trouble_waits() {
        let (mut tv, cx, id) = kept();
        let effects = tv.answer(id, response(200, r#"{"tv": null}"#, None), &cx);
        assert_eq!(effects, vec![Effect::Timer { id: 2, ms: remote::NO_TV_WAIT_MS as u32 }]);
        assert_eq!(tv.view().entry, "Sign in a TV");
        let (id, _) = request(&tv.answer(2, Reply::Fired, &cx));
        let effects = tv.answer(id, Reply::Unreachable { offline: false, message: "x".into() }, &cx);
        assert!(matches!(effects[..], [Effect::Timer { ms, .. }] if ms == remote::TROUBLE_WAIT_MS as u32));
    }

    #[test]
    fn stops_behind_and_starts_in_front() {
        let (mut tv, mut cx, id) = kept();
        cx.foreground = false;
        assert_eq!(tv.app(&AppEvent::Lifecycle { foreground: false }, &cx), vec![Effect::Cancel { id }]);
        cx.foreground = true;
        let (_, request) = request(&tv.app(&AppEvent::Lifecycle { foreground: true }, &cx));
        assert!(request.url.ends_with("?v=-1"));
        // (a TV keeps up with no TV)
        let mut tv_cx = test_context(0.);
        tv_cx.platform.device = Device::Tv;
        assert!(YourTv::default().app(&AppEvent::Account { account: Some(account()) }, &tv_cx).is_empty());
    }

    #[test]
    fn presses_go_out_one_at_a_time_in_order() {
        let (mut tv, cx, id) = kept();
        tv.answer(id, response(200, &playing(), None), &cx);
        let (first, sent) = request(&tv.update(Event::Toggle, &cx));
        assert_eq!(sent.method, Method::Post);
        assert!(sent.url.ends_with(remote::SEND));
        assert!(tv.update(Event::Jump { forward: true }, &cx).is_empty());
        let effects = tv.answer(first, response(500, "", None), &cx);
        assert!(matches!(&effects[0], Effect::Report { place, .. } if place == "remote.send"));
        let (_, next) = request(&effects);
        assert!(matches!(next.body, Some(crate::screen::Body::Text { ref text, .. }) if text.contains("seek")));
    }

    #[test]
    fn plays_on_the_tv_only_with_play_on_tv() {
        let (mut tv, cx, id) = kept();
        tv.answer(id, response(200, &playing(), None), &cx);
        assert!(!tv.view().plays_on_tv);
        assert!(tv.update(Event::Play { title: "tt2".into(), episode: None }, &cx).is_empty());
        tv.update(Event::Switch, &cx);
        let view = tv.view();
        assert!(view.plays_on_tv);
        assert_eq!(view.bar.as_ref().map(|b| b.title.as_str()), Some("Dune"));
        let (_, sent) = request(&tv.update(Event::Play { title: "tt2".into(), episode: None }, &cx));
        assert!(matches!(sent.body, Some(crate::screen::Body::Text { ref text, .. }) if text.contains("play\\ttt2")));
    }

    #[test]
    fn the_picture_comes_from_the_title() {
        let (mut tv, cx, id) = kept();
        let effects = tv.answer(id, response(200, &playing(), None), &cx);
        let Effect::Page { id: art, .. } = effects[0] else { panic!() };
        let title = json!({"title": "Dune", "cover": "https://image.tmdb.org/t/p/w500/c.jpg",
                           "episodes": [{"id": "e2", "thumb": "https://image.tmdb.org/t/p/w500/s.jpg"}]});
        let page = PageReply { body: Some(title.to_string()), at: 1., failure: None, stale: false };
        tv.answer(art, Reply::Page { page }, &cx);
        let art = tv.view().playing.and_then(|p| p.art).expect("art");
        assert!(art.contains("s.jpg"), "{art}");
    }

    #[test]
    fn signing_the_tv_out_asks_first() {
        let (mut tv, cx, id) = kept();
        tv.answer(id, response(200, &playing(), None), &cx);
        assert!(tv.update(Event::SignOutTv, &cx).is_empty());
        assert!(tv.view().sure);
        assert_eq!(tv.view().sure_words, "Press again to sign it out");
        let (remove, request) = request(&tv.update(Event::SignOutTv, &cx));
        assert!(request.url.ends_with(REMOVE));
        tv.answer(remove, response(200, "{}", None), &cx);
        let view = tv.view();
        assert_eq!((view.closed, view.has_tv), (1, false));
        // (with no TV left, nothing to sign out)
        tv.update(Event::SignOutTv, &cx);
        assert!(tv.update(Event::SignOutTv, &cx).is_empty());
    }

    #[test]
    fn signing_out_forgets_the_tv() {
        let (mut tv, cx, id) = kept();
        tv.answer(id, response(200, &playing(), None), &cx);
        let effects = tv.app(&AppEvent::Account { account: None }, &cx);
        assert!(matches!(effects[..], [Effect::Cancel { .. }]));
        assert!(tv.view().playing.is_none());
        assert!(!tv.view().has_tv);
    }
}
