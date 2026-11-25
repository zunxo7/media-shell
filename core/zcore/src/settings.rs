//! Settings: which tiles show and in what order, their words and choices, the devices this account is signed in on
//! (removing one, or every other), and the "Saved" note after a change. The profile's settings themselves are read and
//! saved by the profile settings store; the profiles tile is [`crate::profiles`], the version card [`crate::update`].
//!
//! ```
//! use zcore::root::Root;
//! use zcore::screen::{Clock, Device, Effect, Platform, Reply};
//! use zcore::settings::{Event, Settings};
//!
//! let platform = Platform { device: Device::Phone, model: "Pixel".into(), version_name: "2.0".into(),
//!                           version_code: 1, os: "t".into(), stream: "s".into(), codecs: vec![], hdr: false };
//! let clock = Clock { now: 1_794_657_600., utc_offset: 0 };
//! let mut root = Root::new(platform, "https://host.invalid".into(), clock);
//! let page = root.open(Settings::default());
//! let turn = root.update::<Settings>(page, Event::Opened { plus: false }, clock);
//! let Some(Effect::Request { id, request }) = turn.orders.first().map(|o| o.effect.clone()) else { panic!() };
//! assert_eq!(request.url, "https://host.invalid/api/devices");
//! let body = r#"{"devices": [{"id": "a", "name": "Pixel", "kind": "phone", "seen": 1794657600, "current": true}], "max": 4}"#;
//! root.answer(id, Reply::Response { status: 200, headers: vec![], body: body.into() }, clock);
//! let view = root.view::<Settings>(page).unwrap_or_default();
//! let devices = view.devices.unwrap_or_default();
//! assert_eq!(devices.count, Some("Signed in  ·  1 of 4".into()));
//! assert_eq!(devices.rows[0].line, "This phone");
//! ```

use crate::api::{self, Kind, Outcome};
use crate::screen::{Asks, Context, Device, Effect, Reply, Screen};
use crate::{captions, quality};
use serde_json::{json, Value};

/// Where the account's devices are listed...
pub const DEVICES: &str = "/api/devices";
/// ...and signed out.
pub const REMOVE: &str = "/api/devices/remove";
/// What shows for a moment after a change, and for how long.
pub const SAVED: &str = "Saved";
pub const SAVED_MS: u32 = 1_600;
/// A device seen this recently is in use now.
const ACTIVE_SECONDS: f64 = 10. * 60.;

/// A tile, in the order the page lays them out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Profiles,
    Subtitles,
    Playback,
    Downloads,
    Languages,
    Devices,
    Version,
}

/// A setting's options, as the profile's settings save them: its group and key, its name, and (value, label) pairs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Choice {
    pub group: String,
    pub key: String,
    pub name: String,
    pub options: Vec<Pick>,
}

/// One option of a [`Choice`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pick {
    pub value: String,
    pub label: String,
}

/// A setting that's on or off: its group and key, its name and a quiet line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Switch {
    pub group: String,
    pub key: String,
    pub name: String,
    pub line: String,
}

/// A tile's name and the quiet line under it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Head {
    pub title: String,
    pub line: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Subtitles {
    pub head: Head,
    /// The line the preview shows, as the player draws it.
    pub preview: String,
    pub on: Switch,
    pub size: Choice,
    pub colour: Choice,
    /// Saved as how dark (`captions::shade_value`); shown as the nearest shade (`captions::shade`).
    pub background: Choice,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Playback {
    pub head: Head,
    pub autoplay: Switch,
    /// Shown as the nearest choice to what's saved (`quality::shown_choice`).
    pub quality: Choice,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Languages {
    pub head: Head,
    pub anime: Choice,
    pub dramas: Choice,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Downloads {
    pub head: Head,
    pub quality: Choice,
    /// A phone's: whether downloads may use mobile data (this device's own setting, not the profile's).
    pub mobile_data: Option<Switch>,
}

/// What a device is, for its icon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeviceKind {
    #[default]
    Phone,
    Tv,
    Computer,
}

/// A destructive button: its label, the words that ask again, and whether it's asking.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Danger {
    pub label: String,
    pub confirm: String,
    pub asking: bool,
}

/// A device the account is signed in on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceRow {
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    /// "This phone", or when it was last used ("Used 2 h ago").
    pub line: String,
    /// Any but this one can be signed out.
    pub remove: Option<Danger>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Devices {
    pub head: Head,
    /// Nothing listed yet: a skeleton.
    pub loading: bool,
    /// "Signed in  ·  2 of 4".
    pub count: Option<String>,
    pub rows: Vec<DeviceRow>,
    /// Sign out every other device, when there are others.
    pub others: Option<Danger>,
    /// Why the list or a removal failed.
    pub note: Option<String>,
}

/// Exactly what the Settings page draws (beside the profiles tile and the version card).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub title: String,
    /// The tiles in one column, and in two (left, then right).
    pub single: Vec<Part>,
    pub left: Vec<Part>,
    pub right: Vec<Part>,
    /// "Saved", for a moment after a change.
    pub saved: Option<String>,
    pub subtitles: Subtitles,
    pub playback: Playback,
    pub languages: Languages,
    pub downloads: Option<Downloads>,
    pub devices: Option<Devices>,
}

/// What happened on Settings.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The page opened; `plus`: the account may stream 4K.
    Opened { plus: bool },
    /// A setting was changed (and saved behind): "Saved" shows.
    Changed,
    /// Remove pressed on a device (once to ask, again to do it).
    Remove { id: String },
    /// Sign out every other device pressed (once to ask, again to do it).
    RemoveOthers,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Asking {
    Device(String),
    Others,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ask {
    List,
    Remove,
    Saved,
}

#[derive(Clone, Debug)]
struct Signed {
    id: String,
    name: String,
    kind: DeviceKind,
    seen: f64,
    current: bool,
}

/// The Settings page. See the module's docs.
#[derive(Debug, Default)]
pub struct Settings {
    asks: Asks<Ask>,
    device: Option<Device>,
    plus: bool,
    now: f64,
    devices: Option<(Vec<Signed>, i64)>,
    asking: Option<Asking>,
    note: Option<String>,
    saved: Option<u32>,
}

impl Settings {
    fn list(&mut self, cx: &Context) -> Effect {
        Effect::Request { id: self.asks.ask(Ask::List), request: api::get(&cx.base, &cx.who(), DEVICES, None) }
    }

    fn remove(&mut self, body: Value, cx: &Context) -> Vec<Effect> {
        self.asking = None;
        self.note = None;
        vec![Effect::Request {
            id: self.asks.ask(Ask::Remove),
            request: api::post(&cx.base, &cx.who(), REMOVE, body.to_string()),
        }]
    }

    fn failed(&mut self, kind: Kind, words: String) -> Vec<Effect> {
        if kind == Kind::SignedOut {
            return vec![Effect::SignedOut];
        }
        self.note = Some(words);
        vec![]
    }
}

impl Screen for Settings {
    type Event = Event;
    type View = View;

    fn update(&mut self, event: Event, cx: &Context) -> Vec<Effect> {
        self.now = cx.clock.now;
        match event {
            Event::Opened { plus } => {
                self.device = Some(cx.platform.device);
                self.plus = plus;
                self.asking = None;
                self.note = None;
                if cx.platform.device == Device::Tv {
                    return vec![];
                }
                vec![self.list(cx)]
            }
            Event::Changed => {
                let mut out: Vec<Effect> = self.saved.take().map(|id| Effect::Cancel { id }).into_iter().collect();
                let id = self.asks.ask(Ask::Saved);
                self.saved = Some(id);
                out.push(Effect::Timer { id, ms: SAVED_MS });
                out
            }
            Event::Remove { id } => {
                if self.asking.as_ref() != Some(&Asking::Device(id.clone())) {
                    self.asking = Some(Asking::Device(id));
                    return vec![];
                }
                self.remove(json!({"id": id}), cx)
            }
            Event::RemoveOthers => {
                if self.asking != Some(Asking::Others) {
                    self.asking = Some(Asking::Others);
                    return vec![];
                }
                self.remove(json!({"others": true}), cx)
            }
        }
    }

    fn answer(&mut self, id: u32, reply: Reply, cx: &Context) -> Vec<Effect> {
        self.now = cx.clock.now;
        let Some(ask) = self.asks.take(id) else { return vec![] };
        if ask == Ask::Saved {
            // (an earlier note's timer, cancelled too late, leaves the newer one showing)
            if self.saved == Some(id) {
                self.saved = None;
            }
            return vec![];
        }
        match (ask, api::outcome(reply, cx.online)) {
            (Ask::List, Outcome::Body { body, .. }) => {
                self.devices = Some(parse(&body));
                vec![]
            }
            (Ask::Remove, Outcome::Body { .. } | Outcome::Unchanged) => vec![self.list(cx)],
            (_, Outcome::Failed { failure }) => self.failed(failure.kind, failure.words),
            _ => vec![],
        }
    }

    fn view(&self) -> View {
        let device = self.device.unwrap_or(Device::Phone);
        let tv = device == Device::Tv;
        let parts = |list: &[Part]| -> Vec<Part> {
            list.iter()
                .copied()
                .filter(|p| !(tv && matches!(p, Part::Profiles | Part::Devices | Part::Downloads)))
                .collect()
        };
        let press = if device == Device::Desktop { "Click" } else { "Press" };
        View {
            title: "Settings".into(),
            single: parts(&[
                Part::Profiles,
                Part::Subtitles,
                Part::Playback,
                Part::Downloads,
                Part::Languages,
                Part::Devices,
                Part::Version,
            ]),
            // (Subtitles is the tallest tile: Languages goes on the right, above Playback, so the columns end about even)
            left: parts(&[Part::Profiles, Part::Subtitles]),
            right: parts(&[Part::Languages, Part::Playback, Part::Downloads, Part::Devices, Part::Version]),
            saved: self.saved.map(|_| SAVED.into()),
            subtitles: Subtitles {
                head: head("Subtitles", "How they look while you watch, as you see them here."),
                preview: "I've waited a long time for this.\nDon't let me down.".into(),
                on: switch("subtitles", "on", "Subtitles", "Show English subtitles when there are some"),
                size: choice("subtitles", "size", "Size", captions::SIZES.iter().map(|(v, l)| (*v, *l))),
                colour: choice("subtitles", "color", "Colour", captions::INKS.iter().map(|(v, l, _)| (*v, *l))),
                background: choice(
                    "subtitles",
                    "background",
                    "Background",
                    captions::SHADES.iter().map(|(v, l, _)| (*v, *l)),
                ),
            },
            playback: Playback {
                head: head("Playback", "How things play."),
                autoplay: switch(
                    "playback",
                    "autoplayNext",
                    "Autoplay next episode",
                    "Plays the next one as the credits roll",
                ),
                quality: choice(
                    "playback",
                    "quality",
                    "Streaming quality",
                    quality::stream_choices(self.plus).into_iter(),
                ),
            },
            languages: Languages {
                head: head("Languages", "What things play in."),
                anime: choice("languages", "anime", "Anime", [("sub", "Japanese"), ("dub", "English")].into_iter()),
                dramas: choice(
                    "languages",
                    "dramas",
                    "Foreign dramas",
                    [("original", "Original"), ("dub", "English")].into_iter(),
                ),
            },
            downloads: (!tv).then(|| Downloads {
                head: head("Downloads", "How films and episodes are saved to watch offline."),
                quality: choice("playback", "downloadQuality", "Quality", quality::DOWNLOAD_CHOICES.into_iter()),
                mobile_data: (device == Device::Phone)
                    .then(|| switch("downloads", "mobileData", "Use mobile data", "Off: downloads wait for Wi-Fi")),
            }),
            devices: (!tv).then(|| self.devices_view(device, press)),
        }
    }
}

impl Settings {
    fn devices_view(&self, device: Device, press: &str) -> Devices {
        let this = match device {
            Device::Phone => "This phone",
            Device::Tv => "This TV",
            Device::Desktop => "This PC",
        };
        let mut tile = Devices {
            head: head("Devices", "Where this account is signed in."),
            loading: self.devices.is_none() && self.note.is_none(),
            note: self.note.clone(),
            ..Devices::default()
        };
        let Some((list, max)) = &self.devices else { return tile };
        let n = list.len();
        tile.count =
            Some(if n as i64 > *max { format!("Signed in  ·  {n}") } else { format!("Signed in  ·  {n} of {max}") });
        tile.rows = list
            .iter()
            .map(|d| DeviceRow {
                id: d.id.clone(),
                name: d.name.clone(),
                kind: d.kind,
                line: if d.current { this.into() } else { last_used(d.seen, self.now) },
                remove: (!d.current).then(|| Danger {
                    label: "Remove".into(),
                    confirm: format!("{press} again to sign it out"),
                    asking: self.asking.as_ref() == Some(&Asking::Device(d.id.clone())),
                }),
            })
            .collect();
        tile.others = (n > 1).then(|| Danger {
            label: "Sign out every other device".into(),
            confirm: format!("{press} again to sign them all out"),
            asking: self.asking == Some(Asking::Others),
        });
        tile
    }
}

fn head(title: &str, line: &str) -> Head {
    Head { title: title.into(), line: line.into() }
}

fn switch(group: &str, key: &str, name: &str, line: &str) -> Switch {
    Switch { group: group.into(), key: key.into(), name: name.into(), line: line.into() }
}

fn choice<'a>(group: &str, key: &str, name: &str, options: impl Iterator<Item = (&'a str, &'a str)>) -> Choice {
    Choice {
        group: group.into(),
        key: key.into(),
        name: name.into(),
        options: options.map(|(value, label)| Pick { value: value.into(), label: label.into() }).collect(),
    }
}

/// The devices and how many there may be, from /api/devices; four when it doesn't say.
fn parse(body: &str) -> (Vec<Signed>, i64) {
    let v: Value = serde_json::from_str(body).unwrap_or_default();
    let text = |d: &Value, k: &str| d.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let list = v
        .get("devices")
        .and_then(Value::as_array)
        .map(|all| {
            all.iter()
                .map(|d| Signed {
                    id: text(d, "id"),
                    name: text(d, "name"),
                    kind: match text(d, "kind").as_str() {
                        "tv" => DeviceKind::Tv,
                        "computer" | "desktop" => DeviceKind::Computer,
                        _ => DeviceKind::Phone,
                    },
                    seen: d.get("seen").and_then(Value::as_f64).unwrap_or_default(),
                    current: d.get("current").and_then(Value::as_bool).unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    (list, v.get("max").and_then(Value::as_i64).unwrap_or(4))
}

/// "Active now", "Used 12 min ago", "Used 2 h ago", "Used yesterday", "Used 3 days ago"; nothing when never seen.
pub fn last_used(seen: f64, now: f64) -> String {
    if seen <= 0. {
        return String::new();
    }
    let ago = (now - seen).max(0.);
    let minutes = (ago / 60.) as i64;
    match ago {
        a if a < ACTIVE_SECONDS => "Active now".into(),
        a if a < 3_600. => format!("Used {minutes} min ago"),
        a if a < 86_400. => format!("Used {} h ago", minutes / 60),
        a if a < 2. * 86_400. => "Used yesterday".into(),
        _ => format!("Used {} days ago", minutes / 1_440),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{response, test_context};

    const LIST: &str = r#"{"devices": [
        {"id": "me", "name": "Pixel", "kind": "phone", "seen": 9990, "current": true},
        {"id": "tv", "name": "Living room", "kind": "tv", "seen": 2800, "current": false},
        {"id": "pc", "name": "Desk", "kind": "desktop", "seen": 0, "current": false}
    ], "max": 4}"#;

    fn request(effects: &[Effect]) -> (u32, crate::screen::Request) {
        match effects {
            [Effect::Request { id, request }] => (*id, request.clone()),
            other => panic!("expected a request, got {other:?}"),
        }
    }

    fn listed(device: Device) -> (Settings, Context) {
        let mut cx = test_context(10_000.);
        cx.platform.device = device;
        let mut s = Settings::default();
        let (id, _) = request(&s.update(Event::Opened { plus: false }, &cx));
        s.answer(id, response(200, LIST, None), &cx);
        (s, cx)
    }

    #[test]
    fn devices_are_listed_with_this_one_first_named() {
        let (s, _) = listed(Device::Phone);
        let d = s.view().devices.expect("devices");
        assert_eq!(d.count.as_deref(), Some("Signed in  ·  3 of 4"));
        assert_eq!(
            d.rows.iter().map(|r| (r.kind, r.line.as_str())).collect::<Vec<_>>(),
            vec![(DeviceKind::Phone, "This phone"), (DeviceKind::Tv, "Used 2 h ago"), (DeviceKind::Computer, "")]
        );
        assert!(d.rows[0].remove.is_none());
        assert_eq!(d.others.map(|o| o.confirm), Some("Press again to sign them all out".into()));
        let (s, _) = listed(Device::Desktop);
        let d = s.view().devices.expect("devices");
        assert_eq!(d.rows[0].line, "This PC");
        assert_eq!(d.rows[1].remove.as_ref().map(|r| r.confirm.as_str()), Some("Click again to sign it out"));
    }

    #[test]
    fn removing_asks_once_more_then_lists_again() {
        let (mut s, cx) = listed(Device::Phone);
        assert!(s.update(Event::Remove { id: "tv".into() }, &cx).is_empty());
        assert_eq!(s.view().devices.and_then(|d| d.rows[1].remove.clone()).map(|r| r.asking), Some(true));
        let (id, r) = request(&s.update(Event::Remove { id: "tv".into() }, &cx));
        assert_eq!(r.url, "https://host.invalid/api/devices/remove");
        assert!(matches!(r.body, Some(crate::screen::Body::Text { ref text, .. }) if text == r#"{"id":"tv"}"#));
        let (_, again) = request(&s.answer(id, response(200, "{}", None), &cx));
        assert_eq!(again.url, "https://host.invalid/api/devices");
        assert!(s.update(Event::RemoveOthers, &cx).is_empty());
        let (_, r) = request(&s.update(Event::RemoveOthers, &cx));
        assert!(matches!(r.body, Some(crate::screen::Body::Text { ref text, .. }) if text == r#"{"others":true}"#));
    }

    #[test]
    fn a_failure_is_said_and_signed_out_goes_to_sign_in() {
        let (mut s, cx) = listed(Device::Phone);
        s.update(Event::RemoveOthers, &cx);
        let (id, _) = request(&s.update(Event::RemoveOthers, &cx));
        s.answer(id, response(500, r#"{"error": "Nope."}"#, None), &cx);
        assert_eq!(s.view().devices.and_then(|d| d.note), Some("Nope.".into()));
        let mut s = Settings::default();
        let (id, _) = request(&s.update(Event::Opened { plus: false }, &cx));
        assert_eq!(s.answer(id, response(401, "", None), &cx), vec![Effect::SignedOut]);
    }

    #[test]
    fn a_tv_has_no_profiles_devices_or_downloads() {
        let mut cx = test_context(1000.);
        cx.platform.device = Device::Tv;
        let mut s = Settings::default();
        assert!(s.update(Event::Opened { plus: true }, &cx).is_empty());
        let v = s.view();
        assert_eq!(v.single, vec![Part::Subtitles, Part::Playback, Part::Languages, Part::Version]);
        assert_eq!(v.left, vec![Part::Subtitles]);
        assert_eq!(v.right, vec![Part::Languages, Part::Playback, Part::Version]);
        assert!(v.devices.is_none() && v.downloads.is_none());
        assert!(v.playback.quality.options.iter().any(|o| o.label == "4K"));
    }

    #[test]
    fn only_a_phone_asks_about_mobile_data() {
        let (s, _) = listed(Device::Phone);
        assert!(s.view().downloads.and_then(|d| d.mobile_data).is_some());
        let (s, _) = listed(Device::Desktop);
        assert!(s.view().downloads.is_some_and(|d| d.mobile_data.is_none()));
    }

    #[test]
    fn saved_shows_for_a_moment_and_a_new_change_starts_it_again() {
        let cx = test_context(1000.);
        let mut s = Settings::default();
        let first = s.update(Event::Changed, &cx);
        let [Effect::Timer { id: first, ms: SAVED_MS }] = first[..] else { panic!("{first:?}") };
        assert_eq!(s.view().saved.as_deref(), Some("Saved"));
        let again = s.update(Event::Changed, &cx);
        assert_eq!(again[0], Effect::Cancel { id: first });
        let Effect::Timer { id: second, .. } = again[1] else { panic!() };
        assert!(s.answer(first, Reply::Fired, &cx).is_empty());
        assert!(s.view().saved.is_some());
        s.answer(second, Reply::Fired, &cx);
        assert_eq!(s.view().saved, None);
    }

    #[test]
    fn last_used_words() {
        let now = 1_000_000.;
        assert_eq!(last_used(now - 300., now), "Active now");
        assert_eq!(last_used(now - 1_500., now), "Used 25 min ago");
        assert_eq!(last_used(now - 7_300., now), "Used 2 h ago");
        assert_eq!(last_used(now - 100_000., now), "Used yesterday");
        assert_eq!(last_used(now - 3. * 86_400., now), "Used 3 days ago");
        assert_eq!(last_used(0., now), "");
    }
}
