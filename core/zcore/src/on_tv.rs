//! The TV's end of Your TV: while the app shows on a TV it keeps one request waiting at the server
//! (`/api/remote/wait`), answered the moment a phone asks for something, and it tells the account's phones what's
//! playing (`/api/remote/state`). What each thing asked means is decided here; the app carries it out ([`Order`]):
//! its pages, its player, its keys and volume.
//!
//! ```
//! use zcore::on_tv::{orders, Order, Button};
//! use zcore::screen::Effect;
//! assert_eq!(orders(&["ok".into()]), vec![Effect::Tv { order: Order::Press { button: Button::Ok } }]);
//! ```

use crate::api::{self, Kind, Outcome};
use crate::remote::{Command, Group};
use crate::screen::{AppEvent, Asks, Context, Device, Effect, Reply, Screen};
use serde_json::{json, Value};

/// How long the wait for a phone may take: the server answers within 25 s.
pub const LONG_POLL_MS: u32 = 40_000;
/// After the server couldn't be reached.
pub const TROUBLE_WAIT_MS: u32 = 4_000;
/// After the server said no (signed out, or a server without it): no need to keep asking.
pub const REFUSED_WAIT_MS: u32 = 30_000;
/// How often the player is looked at for what to tell the phones.
pub const SAMPLE_MS: u32 = 500;
/// How often the phones are told what's on even when nothing's new, so they stay sure.
pub const HEARTBEAT_S: f64 = 10.;

/// Something a phone asked of this TV, for the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Order {
    /// End what's playing.
    Stop,
    /// Home, with nothing playing.
    Home,
    /// The search page; `typed`: words typed on the phone to search for.
    Search { typed: Option<String> },
    /// For the player that's open (nothing when none is).
    Toggle,
    Seek { to: f64 },
    Next,
    Pick { group: String, choice: String },
    Volume { change: Volume },
    /// One of the TV remote's own buttons, as if pressed.
    Press { button: Button },
    /// Say what the open player is doing ([`Event::Playing`]).
    Sample,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Volume {
    Up,
    Down,
    Mute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Up,
    Down,
    Left,
    Right,
    Ok,
    Back,
    PlayPause,
    Rewind,
    Forward,
}

/// What the TV's player is doing, as it is: the core words it for the phones.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerState {
    /// Empty until the title is known.
    pub title_id: String,
    pub title: String,
    /// The episode playing, its season's name ("Season 1") and its number.
    pub episode_id: Option<String>,
    pub season: Option<String>,
    pub number: Option<i64>,
    /// What the player says while there's no title yet.
    pub status: String,
    pub time: f64,
    pub total: f64,
    pub paused: bool,
    pub buffering: bool,
    pub started: bool,
    /// What went wrong; empty when nothing did.
    pub fault: String,
    /// There's a next episode, and its number.
    pub has_next: bool,
    pub next_number: Option<i64>,
    pub groups: Vec<Group>,
}

/// What happens on the TV's end.
// (an event is handed over once, never kept, so its size doesn't matter)
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The app's pages showed, signed in: it listens for the phones.
    Opened,
    /// The answer to [`Order::Sample`]: what the open player is doing, None with none open.
    Playing { state: Option<PlayerState> },
}

/// What a phone's asks become, in order. Unknown ones are let go.
pub fn orders(keys: &[String]) -> Vec<Effect> {
    keys.iter().filter_map(|k| order(k)).collect()
}

fn order(key: &str) -> Option<Effect> {
    let tv = |order| Some(Effect::Tv { order });
    if let Some(typed) = key.strip_prefix("text:") {
        return tv(Order::Search { typed: Some(typed.to_string()) });
    }
    if let Some(command) = Command::parse(key) {
        return match command {
            Command::Play { title, episode } => Some(Effect::Play { show: title, episode }),
            Command::Toggle => tv(Order::Toggle),
            Command::Seek(to) => tv(Order::Seek { to }),
            Command::Next => tv(Order::Next),
            Command::Pick { group, choice } => tv(Order::Pick { group, choice }),
            Command::Stop => tv(Order::Stop),
        };
    }
    let press = |button| tv(Order::Press { button });
    let volume = |change| tv(Order::Volume { change });
    match key {
        "search" => tv(Order::Search { typed: None }),
        "home" => tv(Order::Home),
        "volume_up" => volume(Volume::Up),
        "volume_down" => volume(Volume::Down),
        "mute" => volume(Volume::Mute),
        "up" => press(Button::Up),
        "down" => press(Button::Down),
        "left" => press(Button::Left),
        "right" => press(Button::Right),
        "ok" => press(Button::Ok),
        "back" => press(Button::Back),
        "play" => press(Button::PlayPause),
        "rewind" => press(Button::Rewind),
        "forward" => press(Button::Forward),
        _ => None,
    }
}

/// What's on, as the phones read it (`crate::remote::heard`).
pub fn report(p: &PlayerState) -> Value {
    let place = match &p.episode_id {
        Some(_) => {
            let number = p.number.map(|n| n.to_string()).unwrap_or_default();
            match &p.season {
                Some(season) => format!("{season} · Episode {number}"),
                None => format!("Episode {number}"),
            }
        }
        None if !p.title_id.is_empty() => "Movie".into(),
        None => p.status.clone(),
    };
    let next = if p.has_next { format!("Episode {}", p.next_number.map(|n| n.to_string()).unwrap_or_default()) } else { String::new() };
    let groups: Vec<Value> = p
        .groups
        .iter()
        .map(|g| json!({"title": g.title, "choices": g.choices.iter().map(|c| json!({"label": c.label, "on": c.on})).collect::<Vec<_>>()}))
        .collect();
    json!({
        "titleId": p.title_id, "episodeId": p.episode_id.clone().unwrap_or_default(), "title": p.title, "where": place,
        "time": p.time, "total": p.total, "paused": p.paused, "buffering": p.buffering || !p.started, "error": p.fault,
        "next": next, "groups": groups,
    })
}

/// What tells a report apart from the last one: everything but the time, and where it is only if it jumped (a film
/// moving on as expected is nothing new). `now`: seconds since 1970.
fn news(report: &Value, p: &PlayerState, now: f64) -> String {
    let mut without = report.clone();
    if let Some(o) = without.as_object_mut() {
        o.remove("time");
    }
    let mark = if p.paused || p.buffering { p.time as i64 } else { ((p.time - now) / 2.) as i64 };
    format!("{without}{mark}")
}

#[derive(Debug)]
enum Ask {
    Wait,
    /// The wait before asking again.
    Again,
    Sample,
    Tell,
}

/// The TV's end's model.
#[derive(Debug, Default)]
pub struct OnTv {
    asks: Asks<Ask>,
    opened: bool,
    /// The request waiting at the server, or the wait before the next.
    listening: Option<u32>,
    sampling: Option<u32>,
    /// What the phones were last told (empty: nothing on), and when.
    said: String,
    said_at: f64,
}

impl OnTv {
    fn listen(&mut self, cx: &Context) -> Vec<Effect> {
        if !self.opened || !cx.foreground || cx.platform.device != Device::Tv {
            return vec![];
        }
        let mut effects = vec![];
        if self.listening.is_none() {
            let id = self.asks.ask(Ask::Wait);
            self.listening = Some(id);
            let mut request = api::get(&cx.base, &cx.who(), "/api/remote/wait", None);
            request.timeout_ms = LONG_POLL_MS;
            effects.push(Effect::Request { id, request });
        }
        if self.sampling.is_none() {
            effects.push(self.sample_later());
        }
        effects
    }

    fn sample_later(&mut self) -> Effect {
        let id = self.asks.ask(Ask::Sample);
        self.sampling = Some(id);
        Effect::Timer { id, ms: SAMPLE_MS }
    }

    fn again(&mut self, ms: u32) -> Effect {
        let id = self.asks.ask(Ask::Again);
        self.listening = Some(id);
        Effect::Timer { id, ms }
    }

    fn tell(&mut self, body: String, cx: &Context) -> Effect {
        Effect::Request { id: self.asks.ask(Ask::Tell), request: api::post(&cx.base, &cx.who(), "/api/remote/state", body) }
    }
}

impl Screen for OnTv {
    type Event = Event;
    type View = ();

    fn update(&mut self, event: Event, cx: &Context) -> Vec<Effect> {
        match event {
            Event::Opened => {
                self.opened = true;
                self.listen(cx)
            }
            Event::Playing { state: None } if self.said.is_empty() => vec![],
            Event::Playing { state: None } => {
                self.said.clear();
                self.said_at = cx.clock.now;
                vec![self.tell("{}".into(), cx)]
            }
            Event::Playing { state: Some(p) } => {
                let report = report(&p);
                let news = news(&report, &p, cx.clock.now);
                if news == self.said && cx.clock.now - self.said_at <= HEARTBEAT_S {
                    return vec![];
                }
                self.said = news;
                self.said_at = cx.clock.now;
                vec![self.tell(report.to_string(), cx)]
            }
        }
    }

    fn answer(&mut self, id: u32, reply: Reply, cx: &Context) -> Vec<Effect> {
        let Some(ask) = self.asks.take(id) else { return vec![] };
        match ask {
            Ask::Wait => {
                self.listening = None;
                match api::outcome(reply, cx.online) {
                    Outcome::Body { body, .. } => {
                        let keys: Vec<String> = serde_json::from_str::<Value>(&body)
                            .ok()
                            .and_then(|o| o.get("keys").cloned())
                            .and_then(|k| serde_json::from_value(k).ok())
                            .unwrap_or_default();
                        let mut effects = orders(&keys);
                        effects.extend(self.listen(cx));
                        effects
                    }
                    Outcome::Failed { failure } if matches!(failure.kind, Kind::Offline | Kind::Unreachable) => {
                        vec![self.again(TROUBLE_WAIT_MS)]
                    }
                    _ => vec![self.again(REFUSED_WAIT_MS)],
                }
            }
            Ask::Again => {
                self.listening = None;
                self.listen(cx)
            }
            Ask::Sample => {
                self.sampling = None;
                let mut effects = vec![Effect::Tv { order: Order::Sample }];
                effects.extend(self.listen(cx));
                effects
            }
            Ask::Tell => vec![],
        }
    }

    fn app(&mut self, event: &AppEvent, cx: &Context) -> Vec<Effect> {
        match event {
            AppEvent::Lifecycle { foreground: false } => {
                let ids = [self.listening.take(), self.sampling.take()];
                ids.into_iter().flatten().map(|id| Effect::Cancel { id }).collect()
            }
            AppEvent::Lifecycle { foreground: true } => self.listen(cx),
            _ => vec![],
        }
    }

    fn view(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{heard, Choice};
    use crate::screen::{response, test_context};

    fn tv(now: f64) -> Context {
        let mut cx = test_context(now);
        cx.platform.device = Device::Tv;
        cx
    }

    fn episode() -> PlayerState {
        PlayerState {
            title_id: "tt1".into(),
            title: "Dune".into(),
            episode_id: Some("e2".into()),
            season: Some("Season 1".into()),
            number: Some(2),
            time: 100.,
            total: 3000.,
            started: true,
            has_next: true,
            next_number: Some(3),
            groups: vec![Group { title: "Audio".into(), choices: vec![Choice { label: "English".into(), on: true }] }],
            ..Default::default()
        }
    }

    #[test]
    fn what_each_ask_becomes() {
        let keys: Vec<String> =
            ["play\ttt1\te2", "play", "text:dune", "search", "home", "seek\t12", "volume_down", "nonsense", "stop"]
                .map(String::from)
                .into();
        assert_eq!(
            orders(&keys),
            vec![
                Effect::Play { show: "tt1".into(), episode: Some("e2".into()) },
                Effect::Tv { order: Order::Press { button: Button::PlayPause } },
                Effect::Tv { order: Order::Search { typed: Some("dune".into()) } },
                Effect::Tv { order: Order::Search { typed: None } },
                Effect::Tv { order: Order::Home },
                Effect::Tv { order: Order::Seek { to: 12. } },
                Effect::Tv { order: Order::Volume { change: Volume::Down } },
                Effect::Tv { order: Order::Stop },
            ]
        );
    }

    #[test]
    fn listens_on_a_tv_only_and_backs_off() {
        assert!(OnTv::default().update(Event::Opened, &test_context(0.)).is_empty());
        let cx = tv(0.);
        let mut on = OnTv::default();
        let effects = on.update(Event::Opened, &cx);
        let Effect::Request { id, request } = &effects[0] else { panic!() };
        assert!(request.url.ends_with("/api/remote/wait"));
        assert_eq!(request.timeout_ms, LONG_POLL_MS);
        assert!(matches!(effects[1], Effect::Timer { ms: SAMPLE_MS, .. }));
        let effects = on.answer(*id, response(200, r#"{"keys": ["ok"]}"#, None), &cx);
        assert_eq!(effects[0], Effect::Tv { order: Order::Press { button: Button::Ok } });
        let Effect::Request { id, .. } = &effects[1] else { panic!() };
        let effects = on.answer(*id, Reply::Unreachable { offline: false, message: "".into() }, &cx);
        let [Effect::Timer { id, ms: TROUBLE_WAIT_MS }] = effects[..] else { panic!("{effects:?}") };
        let effects = on.answer(id, Reply::Fired, &cx);
        let Effect::Request { id, .. } = &effects[0] else { panic!() };
        let effects = on.answer(*id, response(401, "", None), &cx);
        assert!(matches!(effects[..], [Effect::Timer { ms: REFUSED_WAIT_MS, .. }]));
    }

    #[test]
    fn stops_behind() {
        let cx = tv(0.);
        let mut on = OnTv::default();
        on.update(Event::Opened, &cx);
        assert_eq!(on.app(&AppEvent::Lifecycle { foreground: false }, &cx).len(), 2);
        assert_eq!(on.app(&AppEvent::Lifecycle { foreground: true }, &cx).len(), 2);
    }

    #[test]
    fn the_phones_read_what_it_says() {
        let report = report(&episode());
        let now = heard(&json!({"tv": {"id": "t", "name": "TV"}, "online": true, "now": report}), 0).now.expect("on");
        assert_eq!(now.place, "Season 1 · Episode 2");
        assert_eq!(now.next.as_deref(), Some("Episode 3"));
        assert_eq!(now.groups[0].choices[0].label, "English");
        let film = PlayerState { episode_id: None, has_next: false, ..episode() };
        assert_eq!(super::report(&film)["where"], "Movie");
        assert_eq!(super::report(&PlayerState { status: "Loading…".into(), ..Default::default() })["where"], "Loading…");
        assert_eq!(super::report(&PlayerState::default())["buffering"], true);
    }

    #[test]
    fn tells_what_is_new_and_every_ten_seconds() {
        let mut on = OnTv::default();
        let told = |effects: Vec<Effect>| effects.len();
        assert_eq!(told(on.update(Event::Playing { state: None }, &tv(0.))), 0);
        assert_eq!(told(on.update(Event::Playing { state: Some(episode()) }, &tv(1000.))), 1);
        // (playing on as expected: nothing new)
        let later = PlayerState { time: 102., ..episode() };
        assert_eq!(told(on.update(Event::Playing { state: Some(later) }, &tv(1002.))), 0);
        let jumped = PlayerState { time: 600., ..episode() };
        assert_eq!(told(on.update(Event::Playing { state: Some(jumped.clone()) }, &tv(1003.))), 1);
        let on_time = PlayerState { time: 611., ..jumped };
        assert_eq!(told(on.update(Event::Playing { state: Some(on_time) }, &tv(1014.))), 1);
        let effects = on.update(Event::Playing { state: None }, &tv(1015.));
        assert!(matches!(&effects[..], [Effect::Request { request, .. }] if request.body.as_ref().is_some_and(|b| matches!(b, crate::screen::Body::Text { text, .. } if text == "{}"))));
        assert_eq!(told(on.update(Event::Playing { state: None }, &tv(1016.))), 0);
    }
}
