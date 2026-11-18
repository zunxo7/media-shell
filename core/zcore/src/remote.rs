//! Your TV: a phone or a computer playing things on the account's TV and controlling it.
//!
//! The server (`server/remote.py`) holds one request from the device open at `/api/remote/now` and answers it the
//! moment what's on the TV changes, or after 25 s regardless. What's pressed goes to `/api/remote/send`, one at a
//! time and in order. This module is that conversation's rules: reading the answers, what a command looks like, what
//! the device shows before the TV has answered, and the words the pages use. [`crate::your_tv`] holds the
//! conversation, and [`crate::on_tv`] is the TV's end of it.
//!
//! Times (`clock`) are milliseconds on the core's clock.

use crate::model::{b, d, objects, s, t, Title};
use serde_json::{json, Value};

/// Where pressed buttons go.
pub const SEND: &str = "/api/remote/send";
/// How long to wait before asking again when the account has no TV.
pub const NO_TV_WAIT_MS: u64 = 10_000;
/// How long to wait before asking again after the server couldn't be reached.
pub const TROUBLE_WAIT_MS: u64 = 5_000;
/// The TV player's choices that the captions button shows.
pub const CAPTIONS: [&str; 2] = ["Subtitles", "Subtitle size"];
/// The TV player's choices that the settings button shows.
pub const SETTINGS: [&str; 2] = ["Audio", "Quality"];
/// How far the back and forward buttons move, in seconds.
pub const JUMP: f64 = 10.;

/// The path that waits for what's on the TV; `version`: the last one heard (-1 at first, answered at once).
pub fn now_path(version: i64) -> String {
    format!("/api/remote/now?v={version}")
}

/// Where something at `time` of `total` when heard at `at` is up to at `clock` (all ms but the seconds): carried on
/// while it `moves`, never past the end.
pub fn carried(time: f64, total: f64, moves: bool, at: u64, clock: u64) -> f64 {
    let moved = if moves { clock.saturating_sub(at) as f64 / 1000. } else { 0. };
    if total > 0. {
        (time + moved).min(total)
    } else {
        time + moved
    }
}

/// The account's TV: the one the server keeps the device up with.
#[derive(Clone, Debug, PartialEq)]
pub struct Tv {
    pub id: String,
    pub name: String,
}

/// One of the TV player's choices (a subtitle language, a quality), and whether it's the one in use.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub label: String,
    pub on: bool,
}

/// A group of the TV player's choices ("Subtitles", "Audio").
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub title: String,
    pub choices: Vec<Choice>,
}

/// What's on the TV, as it last said. `at`: when that was, on this device's clock, so the time can be carried on
/// between reports (the TV only reports a change, and every 10 s).
#[derive(Clone, Debug, PartialEq)]
pub struct Now {
    pub title_id: String,
    pub episode_id: Option<String>,
    pub title: String,
    /// "Season 1 · Episode 3", "Movie".
    pub place: String,
    pub time: f64,
    pub total: f64,
    pub paused: bool,
    pub buffering: bool,
    /// What went wrong on the TV; empty when nothing did.
    pub error: String,
    /// The next episode's name ("Episode 4"), when there is one.
    pub next: Option<String>,
    pub groups: Vec<Group>,
    pub at: u64,
}

impl Now {
    /// Where it's up to at `clock`: carried on from the last report while it plays, never past the end.
    pub fn position(&self, clock: u64) -> f64 {
        carried(self.time, self.total, !self.paused && !self.buffering, self.at, clock)
    }

    /// The groups one of the page's buttons shows (`CAPTIONS` or `SETTINGS`).
    pub fn groups_for(&self, wanted: &[&str]) -> Vec<&Group> {
        self.groups.iter().filter(|g| wanted.contains(&g.title.as_str())).collect()
    }

    /// The line under the title: what went wrong, else where it is.
    pub fn line(&self) -> &str {
        if self.error.is_empty() {
            &self.place
        } else {
            &self.error
        }
    }

    /// Equal but for when it was heard: a report that changes nothing on screen.
    fn same(&self, other: &Now) -> bool {
        Now { at: other.at, ..self.clone() } == *other
    }
}

/// One answer from `/api/remote/now`.
#[derive(Clone, Debug, PartialEq)]
pub struct Heard {
    /// None: the account has no TV.
    pub tv: Option<Tv>,
    /// What to ask with next time (`now_path`); None when the answer didn't say.
    pub version: Option<i64>,
    /// zunflix is open on the TV.
    pub online: bool,
    pub now: Option<Now>,
}

/// Reads an answer from `/api/remote/now`, heard at `clock`.
pub fn heard(answer: &Value, clock: u64) -> Heard {
    let Some(tv) = answer.get("tv").filter(|v| v.is_object()) else {
        return Heard { tv: None, version: None, online: false, now: None };
    };
    let age = d(answer, "age");
    let now = answer.get("now").filter(|v| v.is_object()).map(|o| Now {
        title_id: t(o, "titleId"),
        episode_id: s(o, "episodeId"),
        title: t(o, "title"),
        place: t(o, "where"),
        time: d(o, "time"),
        total: d(o, "total"),
        paused: b(o, "paused"),
        buffering: b(o, "buffering"),
        error: t(o, "error"),
        next: s(o, "next"),
        groups: objects(o, "groups")
            .into_iter()
            .map(|g| Group {
                title: t(g, "title"),
                choices: objects(g, "choices")
                    .into_iter()
                    .map(|c| Choice { label: t(c, "label"), on: b(c, "on") })
                    .collect(),
            })
            .collect(),
        at: clock.saturating_sub((age.max(0.) * 1000.) as u64),
    });
    Heard {
        tv: Some(Tv { id: t(tv, "id"), name: t(tv, "name") }),
        version: answer.get("v").and_then(Value::as_i64),
        online: b(answer, "online"),
        now,
    }
}

/// Something for the TV's player to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Start a title (an episode of it, when it's a show).
    Play {
        title: String,
        episode: Option<String>,
    },
    Toggle,
    /// Move to this many seconds in.
    Seek(f64),
    Next,
    /// Pick a choice in one of the player's groups.
    Pick {
        group: String,
        choice: String,
    },
    /// End what's playing.
    Stop,
}

impl Command {
    /// As the server and the TV read it: the command's name, then its details after tabs ("seek\t120.5").
    pub fn text(&self) -> String {
        match self {
            Command::Play { title, episode } => format!("play\t{title}\t{}", episode.as_deref().unwrap_or_default()),
            Command::Toggle => "toggle".into(),
            Command::Seek(to) => format!("seek\t{to}"),
            Command::Next => "next".into(),
            Command::Pick { group, choice } => format!("pick\t{group}\t{choice}"),
            Command::Stop => "stop".into(),
        }
    }

    /// A command as the TV receives it; None for anything else (a remote's key, typed words).
    pub fn parse(text: &str) -> Option<Command> {
        let mut parts = text.split('\t');
        let name = parts.next()?;
        let mut next = || parts.next().map(String::from);
        Some(match name {
            // (a bare "play" is the remote's play/pause key, not a title)
            "play" => Command::Play { title: next().filter(|t| !t.is_empty())?, episode: next().filter(|e| !e.is_empty()) },
            "toggle" => Command::Toggle,
            "seek" => Command::Seek(next()?.parse().ok()?),
            "next" => Command::Next,
            "pick" => Command::Pick { group: next()?, choice: next()? },
            "stop" => Command::Stop,
            _ => return None,
        })
    }
}

/// A button of the TV's own remote, pressed on the TV.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Key {
    VolumeUp,
    VolumeDown,
}

impl Key {
    /// The server's name for it.
    pub fn name(self) -> &'static str {
        match self {
            Key::VolumeUp => "volume_up",
            Key::VolumeDown => "volume_down",
        }
    }
}

/// What the TV bar says: its title line and the line under it.
#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    pub title: String,
    pub line: String,
}

/// Everything a device keeps about the account's TV, changed only by what it hears and what's pressed. What's
/// pressed shows at once, ahead of the TV's answer. Each change that needs the TV hands back the body to POST to
/// `SEND`.
#[derive(Clone, Debug, Default)]
pub struct Remote {
    tv: Option<Tv>,
    /// Play on TV: playing anything starts it on the TV.
    pub on: bool,
    ready: bool,
    now: Option<Now>,
    waiting: Option<String>,
    /// Chosen while zunflix wasn't open on the TV: it starts when it is.
    held: Option<Command>,
}

impl Remote {
    /// The account's TV, if it has one.
    pub fn tv(&self) -> Option<&Tv> {
        self.tv.as_ref()
    }

    /// zunflix is open on the TV.
    pub fn ready(&self) -> bool {
        self.ready
    }

    /// What's on the TV.
    pub fn now(&self) -> Option<&Now> {
        self.now.as_ref()
    }

    /// The title chosen that the TV hasn't started yet.
    pub fn waiting(&self) -> Option<&str> {
        self.waiting.as_deref()
    }

    /// Takes in an answer. Returns whether anything shown changed, and what to send now (a title chosen while
    /// zunflix wasn't open on the TV, once it is).
    pub fn hear(&mut self, heard: Heard) -> (bool, Option<Value>) {
        let before = (self.tv.clone(), self.on, self.ready, self.waiting.clone());
        let was = self.now.clone();
        if heard.tv.is_none() {
            self.on = false;
        }
        self.tv = heard.tv;
        self.ready = heard.online;
        self.now = heard.now;
        if self.now.as_ref().is_some_and(|n| Some(&n.title_id) == self.waiting.as_ref()) {
            self.waiting = None;
        }
        let send = if self.ready && self.on { self.held.take().and_then(|c| self.command(c)) } else { None };
        let same_now = match (&was, &self.now) {
            (Some(a), Some(b)) => a.same(b),
            (None, None) => true,
            _ => false,
        };
        let changed = !same_now || before != (self.tv.clone(), self.on, self.ready, self.waiting.clone());
        (changed, send)
    }

    /// Starts a title on the TV: at once when zunflix is open there, else as soon as it is.
    pub fn play(&mut self, title: &str, episode: Option<&str>) -> Option<Value> {
        let command = Command::Play { title: title.into(), episode: episode.map(String::from) };
        self.waiting = Some(title.into());
        if self.ready {
            self.command(command)
        } else {
            self.held = Some(command);
            None
        }
    }

    /// Play or pause.
    pub fn toggle(&mut self, clock: u64) -> Option<Value> {
        if let Some(n) = &mut self.now {
            n.time = n.position(clock);
            n.at = clock;
            n.paused = !n.paused;
        }
        self.command(Command::Toggle)
    }

    /// Moves to `to` seconds in (never before the start).
    pub fn seek(&mut self, to: f64, clock: u64) -> Option<Value> {
        let to = to.max(0.);
        if let Some(n) = &mut self.now {
            n.time = to;
            n.at = clock;
        }
        self.command(Command::Seek(to))
    }

    /// Back or forward `JUMP` seconds from where it's up to.
    pub fn jump(&mut self, forward: bool, clock: u64) -> Option<Value> {
        let at = self.now.as_ref()?.position(clock);
        self.seek(if forward { at + JUMP } else { at - JUMP }, clock)
    }

    /// The next episode.
    pub fn next_episode(&mut self) -> Option<Value> {
        self.command(Command::Next)
    }

    /// Ends what's on the TV.
    pub fn stop(&mut self) -> Option<Value> {
        self.now = None;
        self.command(Command::Stop)
    }

    pub fn pick(&mut self, group: &str, choice: &str) -> Option<Value> {
        if let Some(g) = self.now.as_mut().and_then(|n| n.groups.iter_mut().find(|g| g.title == group)) {
            for c in &mut g.choices {
                c.on = c.label == choice;
            }
        }
        self.command(Command::Pick { group: group.into(), choice: choice.into() })
    }

    /// One of the TV remote's buttons.
    pub fn key(&self, key: Key) -> Option<Value> {
        Some(json!({"key": key.name(), "tv": self.tv.as_ref()?.id}))
    }

    /// The TV was signed out of the account (`/api/devices/remove` with its id).
    pub fn forget_tv(&mut self) {
        self.tv = None;
        self.on = false;
        self.now = None;
    }

    fn command(&self, command: Command) -> Option<Value> {
        Some(json!({"do": command.text(), "tv": self.tv.as_ref()?.id}))
    }

    /// The TV's name, or "Your TV" before it's known.
    pub fn name(&self) -> &str {
        self.tv.as_ref().map_or("Your TV", |tv| &tv.name)
    }

    /// Under the TV's name on the Your TV page.
    pub fn status(&self) -> &'static str {
        if self.ready {
            "zunflix is open on it"
        } else {
            "Open zunflix on the TV to use it"
        }
    }

    /// What the Your TV page says with nothing on the TV.
    pub fn idle(&self) -> &'static str {
        match (self.waiting.is_some(), self.ready) {
            (true, true) => "Starting on the TV…",
            (true, false) => "It'll start as soon as zunflix is open on the TV.",
            _ => "Switch Play on TV on and play anything. It starts on the TV and shows up here.",
        }
    }

    /// The TV bar, shown on every page while Play on TV is on.
    pub fn bar(&self) -> Bar {
        let name = self.tv.as_ref().map_or("your TV", |tv| &tv.name);
        let title = match &self.now {
            Some(n) => n.title.clone(),
            None => format!("Playing on {name}"),
        };
        let line = if !self.ready {
            "Open zunflix on the TV".to_string()
        } else if self.now.is_some() {
            format!("On {name}")
        } else {
            "Play anything and it starts there".to_string()
        };
        Bar { title, line }
    }
}

/// The picture for what's on: its episode's still, else the title's big cover, else its cover.
pub fn art(title: &Title, episode_id: Option<&str>) -> Option<String> {
    let thumb = episode_id.and_then(|id| title.episodes.iter().find(|e| e.id == id)).and_then(|e| e.thumb.as_deref());
    thumb.or(title.cover_big.as_deref()).or(title.cover.as_deref()).map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Episode;

    fn playing() -> Value {
        json!({"tv": {"id": "tv1", "name": "Living room"}, "v": 7, "online": true, "age": 2.0, "now": {
            "titleId": "tt1", "episodeId": "", "title": "Dune", "where": "Movie", "time": 100.0, "total": 9000.0,
            "paused": false, "buffering": false, "error": "", "next": "",
            "groups": [{"title": "Subtitles", "choices": [{"label": "Off", "on": true}, {"label": "English", "on": false}]},
                       {"title": "Quality", "choices": [{"label": "1080p", "on": true}]}]}})
    }

    #[test]
    fn reads_what_the_tv_says() {
        let h = heard(&playing(), 10_000);
        assert_eq!(h.tv, Some(Tv { id: "tv1".into(), name: "Living room".into() }));
        assert_eq!(h.version, Some(7));
        assert!(h.online);
        let now = h.now.expect("playing");
        assert_eq!(now.episode_id, None);
        assert_eq!(now.next, None);
        assert_eq!(now.at, 8_000);
        assert_eq!(now.groups_for(&CAPTIONS).len(), 1);
        assert_eq!(now.groups_for(&SETTINGS)[0].choices[0].label, "1080p");
    }

    #[test]
    fn no_tv() {
        let h = heard(&json!({"tv": null}), 0);
        assert_eq!(h, Heard { tv: None, version: None, online: false, now: None });
        let mut r = Remote { on: true, ..Default::default() };
        r.hear(heard(&playing(), 0));
        let (changed, _) = r.hear(h);
        assert!(changed);
        assert!(!r.on && r.tv().is_none() && r.now().is_none());
    }

    #[test]
    fn time_carries_on_while_playing() {
        let now = heard(&playing(), 10_000).now.expect("playing");
        assert_eq!(now.position(13_000), 105.);
        assert_eq!(Now { paused: true, ..now.clone() }.position(13_000), 100.);
        assert_eq!(Now { buffering: true, ..now.clone() }.position(13_000), 100.);
        assert_eq!(now.position(100_000_000), 9000.);
    }

    #[test]
    fn commands_as_the_server_reads_them() {
        let play = |e: Option<&str>| Command::Play { title: "tt1".into(), episode: e.map(String::from) }.text();
        assert_eq!(play(Some("tt1:1:2")), "play\ttt1\ttt1:1:2");
        assert_eq!(play(None), "play\ttt1\t");
        assert_eq!(Command::Seek(120.5).text(), "seek\t120.5");
        assert_eq!(Command::Pick { group: "Audio".into(), choice: "English".into() }.text(), "pick\tAudio\tEnglish");
        assert_eq!(Command::Stop.text(), "stop");
    }

    #[test]
    fn the_tv_reads_commands_back() {
        for c in [
            Command::Play { title: "tt1".into(), episode: Some("e2".into()) },
            Command::Play { title: "tt1".into(), episode: None },
            Command::Toggle,
            Command::Seek(120.5),
            Command::Next,
            Command::Pick { group: "Audio".into(), choice: "English".into() },
            Command::Stop,
        ] {
            assert_eq!(Command::parse(&c.text()), Some(c));
        }
        assert_eq!(Command::parse("volume_up"), None);
        assert_eq!(Command::parse("seek\tsoon"), None);
        assert_eq!(Command::parse("pick\tAudio"), None);
    }

    #[test]
    fn play_waits_for_the_tv() {
        let mut r = Remote { on: true, ..Default::default() };
        let mut off = playing();
        off["online"] = json!(false);
        off["now"] = Value::Null;
        r.hear(heard(&off, 0));
        assert_eq!(r.play("tt2", None), None);
        assert_eq!(r.waiting(), Some("tt2"));
        assert_eq!(r.idle(), "It'll start as soon as zunflix is open on the TV.");
        off["online"] = json!(true);
        let (_, send) = r.hear(heard(&off, 0));
        assert_eq!(send, Some(json!({"do": "play\ttt2\t", "tv": "tv1"})));
        assert_eq!(r.idle(), "Starting on the TV…");
        let mut on = playing();
        on["now"]["titleId"] = json!("tt2");
        r.hear(heard(&on, 0));
        assert_eq!(r.waiting(), None);
    }

    #[test]
    fn presses_show_at_once() {
        let mut r = Remote::default();
        r.hear(heard(&playing(), 10_000));
        assert_eq!(r.toggle(12_000), Some(json!({"do": "toggle", "tv": "tv1"})));
        let n = r.now().expect("playing");
        assert!(n.paused);
        assert_eq!(n.time, 104.);
        assert_eq!(r.jump(false, 20_000), Some(json!({"do": "seek\t94", "tv": "tv1"})));
        assert_eq!(r.seek(-5., 20_000), Some(json!({"do": "seek\t0", "tv": "tv1"})));
        r.pick("Subtitles", "English");
        let subs = &r.now().expect("playing").groups[0].choices;
        assert!(!subs[0].on && subs[1].on);
        assert_eq!(r.key(Key::VolumeUp), Some(json!({"key": "volume_up", "tv": "tv1"})));
        r.stop();
        assert!(r.now().is_none());
    }

    #[test]
    fn nothing_sent_without_a_tv() {
        let mut r = Remote::default();
        assert_eq!(r.toggle(0), None);
        assert_eq!(r.key(Key::VolumeDown), None);
    }

    #[test]
    fn a_report_that_changes_nothing() {
        let mut r = Remote::default();
        assert!(r.hear(heard(&playing(), 10_000)).0);
        assert!(!r.hear(heard(&playing(), 35_000)).0);
        let mut later = playing();
        later["now"]["paused"] = json!(true);
        assert!(r.hear(heard(&later, 36_000)).0);
    }

    #[test]
    fn words() {
        let mut r = Remote::default();
        assert_eq!(r.name(), "Your TV");
        assert_eq!(r.bar(), Bar { title: "Playing on your TV".into(), line: "Open zunflix on the TV".into() });
        let mut idle = playing();
        idle["now"] = Value::Null;
        r.hear(heard(&idle, 0));
        assert_eq!(r.status(), "zunflix is open on it");
        assert_eq!(r.bar().line, "Play anything and it starts there");
        r.hear(heard(&playing(), 0));
        assert_eq!(r.bar(), Bar { title: "Dune".into(), line: "On Living room".into() });
    }

    #[test]
    fn art_prefers_the_episode() {
        let title = Title {
            cover: Some("c".into()),
            cover_big: Some("cb".into()),
            episodes: vec![Episode { id: "e1".into(), thumb: Some("still".into()), ..Default::default() }],
            ..Default::default()
        };
        assert_eq!(art(&title, Some("e1")).as_deref(), Some("still"));
        assert_eq!(art(&title, Some("e2")).as_deref(), Some("cb"));
        assert_eq!(art(&Title { cover_big: None, ..title }, None).as_deref(), Some("c"));
    }
}
