//! The app's updater: when to look for a newer build, when to offer it and how, the Wi-Fi rule for fetching it ahead,
//! and its words. The server decides which build each account gets (public, or a dev build for Dev-builds accounts);
//! the core compares build numbers. Each app fetches and installs the file itself (Android's installer, Windows'
//! bootstrapper) and reports how far it got.
//!
//! The cadences differ on purpose: a phone or TV looks again every 3 minutes and on coming back to the front; Windows
//! every 2 minutes, and as it opens it updates before going on.
//!
//! ```
//! use zcore::root::Root;
//! use zcore::screen::{Clock, Device, Effect, Platform, Reply};
//! use zcore::update::{Event, Updater};
//!
//! let platform = Platform { device: Device::Phone, model: "Pixel".into(), version_name: "2.0".into(),
//!                           version_code: 1250, os: "t".into(), stream: "s".into(), codecs: vec![], hdr: false };
//! let clock = Clock { now: 1_794_657_600., utc_offset: 0 };
//! let mut root = Root::new(platform, "https://host.invalid".into(), clock);
//! let updater = root.open(Updater::default());
//! let turn = root.update::<Updater>(updater, Event::Opened, clock);
//! let Some(Effect::Request { id, request }) = turn.orders.first().map(|o| o.effect.clone()) else { panic!() };
//! assert_eq!(request.url, "https://host.invalid/app/version.json");
//! let body = r#"{"versionCode": 1300, "versionName": "2.1", "apk": "/app/zunflix.apk"}"#;
//! root.answer(id, Reply::Response { status: 200, headers: vec![], body: body.into() }, clock);
//! let view = root.view::<Updater>(updater).unwrap_or_default();
//! assert_eq!(view.card.map(|c| c.line), Some("v2.1 is ready to install".into()));
//! ```

use crate::api::{self, Outcome};
use crate::screen::{AppEvent, Asks, Context, Device, Effect, Reply, Screen};
use crate::{absolute, Error};
use serde::Deserialize;

/// Where the Android app asks.
pub const ANDROID: &str = "/app/version.json";
/// Where the Windows app asks.
pub const WINDOWS: &str = "/app/windows.json";
/// Where the Mac app asks: never the Windows one, whose setup a Mac can't run.
pub const MAC: &str = "/app/mac.json";

/// How long a look may take: Windows waits on it as it opens.
pub const CHECK_TIMEOUT_MS: u32 = 4_000;
/// How often a phone or TV looks while the app is open.
pub const EVERY_MS_ANDROID: u32 = 3 * 60_000;
/// How often Windows looks while it's open.
pub const EVERY_MS_WINDOWS: u32 = 2 * 60_000;
/// A TV's panel waits this long after the app opens: over the opening it came in mid-animation.
pub const TV_SETTLE_MS: u32 = 3_000;

/// The bootstrapper's title, which Windows' installer carries on under.
pub const UPDATING: &str = "Updating";
pub const FAILED: &str = "Couldn't download the update.";

/// What the server offers, compared with the installed build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    /// Nothing newer than the installed build.
    UpToDate,
    /// A newer build: its number, its name ("2.1") and where to download it.
    Offer { code: i64, name: String, url: String },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    version_code: i64,
    #[serde(default)]
    version_name: String,
    apk: Option<String>,
    exe: Option<String>,
    pkg: Option<String>,
    dmg: Option<String>,
}

/// The decision itself, from the manifest's text and the installed build `current`.
///
/// # Errors
/// [`Error::BadAnswer`] when `manifest` isn't valid, or offers a newer build without a download link.
pub fn decide(manifest: &str, current: i64) -> Result<Update, Error> {
    let m: Manifest = serde_json::from_str(manifest)?;
    if m.version_code <= current {
        return Ok(Update::UpToDate);
    }
    let link = m.apk.or(m.exe).or(m.pkg).or(m.dmg).ok_or_else(|| Error::BadAnswer("a newer build without a download link".into()))?;
    Ok(Update::Offer { code: m.version_code, name: m.version_name, url: absolute(&link) })
}

/// How far a download has come, as the update shows it: "Downloading  ·  6.2 of 14.9 MB", then "Opening installer";
/// "Connecting" until its size is known.
pub fn progress_words(done: u64, total: u64) -> String {
    if total == 0 {
        "Connecting".into()
    } else if done >= total {
        "Opening installer".into()
    } else {
        format!("Downloading  ·  {:.1} of {:.1} MB", done as f64 / 1e6, total as f64 / 1e6)
    }
}

/// What happened to the update.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The app has started: the first look, and the looks after it.
    Opened,
    /// Something started or stopped playing: while anything plays, nothing is looked for, shown or installed.
    Playing { playing: bool },
    /// Update pressed (the card, the TV rail, Settings' version card).
    Update,
    /// The card put away (✕, a flick, Later, Back): not shown again this run unless a newer build comes out.
    Later,
    /// How much of the file is in, of how much (0 until known).
    Progress { done: u64, total: u64 },
    /// All of it is in (or was already, from before).
    Fetched,
    /// Fetching or opening it failed; what came is kept to carry on from.
    Failed,
}

/// A newer build, as the app fetches it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Build {
    pub code: i64,
    pub url: String,
}

/// The update card: a phone's toast, a TV's panel, Windows' card.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    /// "Update available", or "Updating zunflix" once it's coming down.
    pub title: String,
    /// "v2.1".
    pub version: String,
    /// "v2.1 is ready to install", how far it's come, or why it failed.
    pub line: String,
    pub button: String,
    pub later: String,
    /// Coming down after Update: the bar shows instead of the buttons.
    pub updating: bool,
    /// How far, 0 to 1, and as "42%".
    pub share: f32,
    pub percent: String,
}

/// The update under way in Windows' bootstrapper.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bootstrap {
    pub title: String,
    pub line: String,
    pub share: f32,
}

/// Settings' version card: the app's name, its version and build, and Up to date or Update.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VersionCard {
    pub name: String,
    pub line: String,
    /// "Up to date", shown when there's no offer.
    pub up_to_date: String,
    pub offer: Option<VersionOffer>,
}

/// A newer build on the version card: its button, and the version it names ("v2.1").
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VersionOffer {
    pub button: String,
    pub version: String,
}

/// Exactly what the app draws of the update, and what it's to do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub card: Option<Card>,
    /// The sidebar's entry (a TV's rail, a wide phone's) once the card is put away: "Update to v2.1", with its dot.
    pub rail: Option<String>,
    pub version: VersionCard,
    /// Windows' bootstrapper while it fetches the update.
    pub bootstrap: Option<Bootstrap>,
    /// Why the last try failed, until the next.
    pub note: Option<String>,
    /// The app is to be fetching this build now (ahead on Wi-Fi, or asked for); it reports [`Event::Progress`], then
    /// [`Event::Fetched`] or [`Event::Failed`].
    pub fetch: Option<Build>,
    /// The build the installer is for; `installs` goes up each time it's to open (on Windows that fetches it too).
    pub install: Option<Build>,
    pub installs: u32,
    /// Windows: the install is the one it opens with, so failing, it carries on opening.
    pub booting: bool,
    /// Windows: still deciding whether to update before opening; the splash waits.
    pub opening: bool,
    /// A look found this build the newest: what an earlier update left behind can go.
    pub up_to_date: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ask {
    /// A look with the account's sign-in (a Dev-builds account gets the dev build).
    Look,
    /// The same without it, when that failed (signed out).
    Public,
    Again,
    Settle,
}

/// The updater. See the module's docs.
#[derive(Debug, Default)]
pub struct Updater {
    asks: Asks<Ask>,
    started: bool,
    looking: bool,
    offer: Option<(Build, String)>,
    up_to_date: bool,
    settled: bool,
    playing: bool,
    /// The card put away this run.
    dismissed: bool,
    /// Update pressed, and not yet handed to the installer.
    updating: bool,
    /// Fetching was asked for (Update pressed): it goes on even on mobile data.
    asked: bool,
    /// All of the offered build is in.
    ready: bool,
    fetch_failed: bool,
    progress: (u64, u64),
    note: Option<String>,
    installs: u32,
    opening: bool,
    booting: bool,
    /// Fetching as it opened failed: it isn't tried again as it opens this run.
    skip_opening: bool,
    desktop: bool,
    version: VersionCard,
    metered: bool,
}

impl Updater {
    fn look(&mut self, ask: Ask, cx: &Context) -> Effect {
        let path = match cx.platform.device {
            Device::Desktop if cx.platform.os.starts_with("macOS") => MAC,
            Device::Desktop => WINDOWS,
            _ => ANDROID,
        };
        let mut request = api::get(&cx.base, &cx.who(), path, None);
        request.timeout_ms = CHECK_TIMEOUT_MS;
        if ask == Ask::Public {
            request.cookies = false;
        }
        self.looking = true;
        Effect::Request { id: self.asks.ask(ask), request }
    }

    /// A look now, unless one is under way, something plays, or the update is already coming.
    fn check(&mut self, cx: &Context) -> Vec<Effect> {
        if self.looking || self.playing || self.updating {
            return vec![];
        }
        vec![self.look(Ask::Look, cx)]
    }

    fn again(&mut self, cx: &Context) -> Effect {
        let ms = if cx.platform.device == Device::Desktop { EVERY_MS_WINDOWS } else { EVERY_MS_ANDROID };
        Effect::Timer { id: self.asks.ask(Ask::Again), ms }
    }

    /// What a look found.
    fn found(&mut self, found: Option<Update>) {
        self.looking = false;
        match found {
            Some(Update::Offer { code, name, url }) => {
                self.up_to_date = false;
                self.fetch_failed = false;
                if self.offer.as_ref().is_some_and(|(b, _)| b.code == code) {
                    self.opening = false;
                    return;
                }
                // (a newer build than the one put away: shown again)
                self.offer = Some((Build { code, url }, name));
                (self.dismissed, self.asked, self.ready, self.progress) = (false, false, false, (0, 0));
                if self.opening && !self.skip_opening {
                    self.booting = true;
                    self.updating = true;
                    self.installs += 1;
                    return;
                }
            }
            Some(Update::UpToDate) => {
                self.offer = None;
                self.up_to_date = true;
            }
            None => {}
        }
        self.opening = false;
    }

    /// The installer, never over something playing.
    fn install(&mut self) {
        if self.playing || self.offer.is_none() {
            return;
        }
        self.installs += 1;
        self.note = None;
        if !self.desktop {
            // (a phone's card goes and stays away once Android's installer takes over)
            self.updating = false;
            self.dismissed = true;
        }
    }
}

impl Screen for Updater {
    type Event = Event;
    type View = View;

    fn update(&mut self, event: Event, cx: &Context) -> Vec<Effect> {
        match event {
            Event::Opened => {
                self.desktop = cx.platform.device == Device::Desktop;
                self.metered = cx.metered;
                self.version = version_card(cx);
                if self.started {
                    // (Windows opening again after an update that failed: on at once, no second try)
                    self.opening = false;
                    return vec![];
                }
                self.started = true;
                self.opening = self.desktop;
                let mut out = vec![self.look(Ask::Look, cx), self.again(cx)];
                if cx.platform.device == Device::Tv {
                    out.push(Effect::Timer { id: self.asks.ask(Ask::Settle), ms: TV_SETTLE_MS });
                } else {
                    self.settled = true;
                }
                out
            }
            Event::Playing { playing } => {
                self.playing = playing;
                if !playing && self.updating && self.ready {
                    self.install();
                }
                vec![]
            }
            Event::Update => {
                if self.offer.is_none() || self.playing || (self.desktop && self.updating) {
                    return vec![];
                }
                self.dismissed = false;
                self.note = None;
                self.fetch_failed = false;
                self.asked = true;
                self.updating = true;
                if self.desktop || self.ready {
                    self.install();
                }
                vec![]
            }
            Event::Later => {
                self.dismissed = true;
                if !self.desktop {
                    self.updating = false;
                }
                vec![]
            }
            Event::Progress { done, total } => {
                self.progress = (done, total);
                vec![]
            }
            Event::Fetched => {
                self.ready = true;
                if self.updating {
                    self.install();
                }
                vec![]
            }
            Event::Failed => {
                self.updating = false;
                self.fetch_failed = true;
                self.dismissed = false;
                self.note = Some(FAILED.into());
                if self.booting {
                    self.booting = false;
                    self.skip_opening = true;
                    self.opening = false;
                }
                vec![]
            }
        }
    }

    fn answer(&mut self, id: u32, reply: Reply, cx: &Context) -> Vec<Effect> {
        let Some(ask) = self.asks.take(id) else { return vec![] };
        match ask {
            Ask::Settle => {
                self.settled = true;
                vec![]
            }
            Ask::Again => {
                let mut out = self.check(cx);
                out.push(self.again(cx));
                out
            }
            Ask::Look | Ask::Public => match api::outcome(reply, cx.online) {
                Outcome::Body { body, .. } => {
                    self.found(decide(&body, cx.platform.version_code).ok());
                    vec![]
                }
                Outcome::Failed { .. } if ask == Ask::Look => vec![self.look(Ask::Public, cx)],
                _ => {
                    self.found(None);
                    vec![]
                }
            },
        }
    }

    fn view(&self) -> View {
        let offer = self.offer.as_ref();
        let named = offer.map(|(_, name)| format!("v{name}"));
        let (done, total) = self.progress;
        let share = if total > 0 { (done as f32 / total as f32).clamp(0., 1.) } else { 0. };
        let shows = offer.is_some() && !self.playing && self.settled && (!self.dismissed || self.updating);
        let card = (shows && !(self.desktop && self.updating)).then(|| {
            let version = named.clone().unwrap_or_default();
            let line = match (&self.note, self.updating) {
                (_, true) => progress_words(done, total),
                (Some(note), false) => note.clone(),
                (None, false) => format!("{version} is ready to install"),
            };
            Card {
                title: if self.updating { "Updating zunflix" } else { "Update available" }.into(),
                version,
                line,
                button: "Update".into(),
                later: "Later".into(),
                updating: self.updating,
                share,
                percent: format!("{}%", (share * 100.) as u32),
            }
        });
        let rail = (!self.desktop && self.dismissed && !self.updating)
            .then_some(named.as_ref())
            .flatten()
            .map(|v| format!("Update to {v}"));
        let bootstrap = (self.desktop && self.updating).then(|| Bootstrap {
            title: UPDATING.into(),
            line: progress_words(done, total),
            share,
        });
        let want = !self.desktop && !self.ready && !self.fetch_failed && (self.asked || !self.metered);
        View {
            card,
            rail,
            version: VersionCard {
                offer: named.map(|version| VersionOffer { button: "Update".into(), version }),
                ..self.version.clone()
            },
            bootstrap,
            note: self.note.clone(),
            fetch: offer.filter(|_| want).map(|(b, _)| b.clone()),
            install: offer.map(|(b, _)| b.clone()),
            installs: self.installs,
            booting: self.booting,
            opening: self.opening,
            up_to_date: self.up_to_date,
        }
    }

    fn app(&mut self, event: &AppEvent, cx: &Context) -> Vec<Effect> {
        match event {
            AppEvent::Lifecycle { foreground: true } if self.started => self.check(cx),
            AppEvent::Connectivity { metered, .. } => {
                self.metered = *metered;
                vec![]
            }
            _ => vec![],
        }
    }
}

/// Settings' version card for this app, before any look.
fn version_card(cx: &Context) -> VersionCard {
    let p = &cx.platform;
    let name = match p.device {
        Device::Phone => "zunflix for Android",
        Device::Tv => "zunflix for TV",
        Device::Desktop => "zunflix for Windows",
    };
    VersionCard {
        name: name.into(),
        line: format!("Version {}", p.version_name),
        up_to_date: "Up to date".into(),
        offer: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{response, test_context, Request};

    const OFFER: &str = r#"{"versionCode": 1300, "versionName": "2.1", "apk": "/app/zunflix.apk"}"#;

    fn context(device: Device) -> Context {
        let mut cx = test_context(1000.);
        cx.platform.device = device;
        cx.platform.version_code = 1250;
        cx
    }

    fn request(effects: &[Effect]) -> (u32, Request) {
        effects
            .iter()
            .find_map(|e| match e {
                Effect::Request { id, request } => Some((*id, request.clone())),
                _ => None,
            })
            .expect("a request")
    }

    fn timers(effects: &[Effect]) -> Vec<(u32, u32)> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Timer { id, ms } => Some((*id, *ms)),
                _ => None,
            })
            .collect()
    }

    /// An updater opened and told of `manifest`.
    fn offered(device: Device, metered: bool) -> (Updater, Context) {
        let mut cx = context(device);
        cx.metered = metered;
        let mut u = Updater::default();
        let (id, _) = request(&u.update(Event::Opened, &cx));
        u.answer(id, response(200, OFFER, None), &cx);
        (u, cx)
    }

    #[test]
    fn newer_build_is_offered_with_a_full_link() {
        let found = decide(r#"{"versionCode": 1300, "versionName": "2.1", "apk": "/app/dev/zunflix.apk?key=k"}"#, 1250);
        assert_eq!(
            found,
            Ok(Update::Offer {
                code: 1300,
                name: "2.1".into(),
                url: "https://host.invalid/app/dev/zunflix.apk?key=k".into()
            })
        );
    }

    #[test]
    fn same_or_older_build_is_up_to_date() {
        assert_eq!(decide(r#"{"versionCode": 1250, "apk": "/a"}"#, 1250), Ok(Update::UpToDate));
        assert_eq!(decide(r#"{"versionCode": 1100, "apk": "/a"}"#, 1250), Ok(Update::UpToDate));
    }

    #[test]
    fn windows_manifest_uses_exe() {
        let found =
            decide(r#"{"versionCode": 1300, "versionName": "2.0", "exe": "/app/ZunFlix-Setup.exe", "size": 1}"#, 13);
        assert!(matches!(found, Ok(Update::Offer { url, .. }) if url == "https://host.invalid/app/ZunFlix-Setup.exe"));
    }

    #[test]
    fn mac_manifest_uses_its_pkg_else_dmg() {
        let found = decide(r#"{"versionCode": 2, "versionName": "1.0.1", "pkg": "/app/zunflix.pkg", "dmg": "/app/zunflix.dmg", "size": 1}"#, 1);
        assert!(matches!(found, Ok(Update::Offer { url, .. }) if url == "https://host.invalid/app/zunflix.pkg"));
        let found = decide(r#"{"versionCode": 2, "versionName": "1.0.1", "dmg": "/app/zunflix.dmg", "size": 1}"#, 1);
        assert!(matches!(found, Ok(Update::Offer { url, .. }) if url == "https://host.invalid/app/zunflix.dmg"));
    }

    #[test]
    fn broken_answers_are_errors() {
        assert!(matches!(decide("<html>", 1), Err(Error::BadAnswer(_))));
        assert!(matches!(decide(r#"{"versionCode": 9}"#, 1), Err(Error::BadAnswer(_))));
    }

    #[test]
    fn each_app_asks_its_own_manifest_with_its_sign_in_and_its_own_cadence() {
        let cx = context(Device::Phone);
        let mut u = Updater::default();
        let out = u.update(Event::Opened, &cx);
        let (_, r) = request(&out);
        assert_eq!(r.url, "https://host.invalid/app/version.json");
        assert!(r.cookies);
        assert_eq!(r.timeout_ms, CHECK_TIMEOUT_MS);
        assert_eq!(timers(&out).iter().map(|t| t.1).collect::<Vec<_>>(), vec![EVERY_MS_ANDROID]);
        let cx = context(Device::Desktop);
        let out = Updater::default().update(Event::Opened, &cx);
        assert_eq!(request(&out).1.url, "https://host.invalid/app/windows.json");
        assert_eq!(timers(&out).iter().map(|t| t.1).collect::<Vec<_>>(), vec![EVERY_MS_WINDOWS]);
    }

    #[test]
    fn a_failed_look_asks_again_without_the_sign_in() {
        let cx = context(Device::Phone);
        let mut u = Updater::default();
        let (id, _) = request(&u.update(Event::Opened, &cx));
        let (id, r) = request(&u.answer(id, response(401, "", None), &cx));
        assert!(!r.cookies);
        assert!(u.answer(id, response(200, OFFER, None), &cx).is_empty());
        assert!(u.view().card.is_some());
    }

    #[test]
    fn on_wifi_it_comes_down_ahead_and_update_installs_at_once() {
        let (mut u, cx) = offered(Device::Phone, false);
        let v = u.view();
        assert_eq!(v.fetch.as_ref().map(|b| b.code), Some(1300));
        assert_eq!(v.version.offer.as_ref().map(|o| o.version.as_str()), Some("v2.1"));
        u.update(Event::Progress { done: 6_200_000, total: 14_900_000 }, &cx);
        u.update(Event::Fetched, &cx);
        assert_eq!(u.view().fetch, None);
        u.update(Event::Update, &cx);
        let v = u.view();
        assert_eq!(v.installs, 1);
        // (the card goes once Android's installer takes over; the sidebar keeps a way back to it)
        assert_eq!(v.card, None);
        assert_eq!(v.rail.as_deref(), Some("Update to v2.1"));
    }

    #[test]
    fn on_mobile_data_it_waits_for_update_then_shows_how_far() {
        let (mut u, cx) = offered(Device::Phone, true);
        assert_eq!(u.view().fetch, None);
        u.update(Event::Update, &cx);
        assert!(u.view().fetch.is_some());
        assert_eq!(u.view().card.map(|c| c.line), Some("Connecting".into()));
        u.update(Event::Progress { done: 6_200_000, total: 14_900_000 }, &cx);
        let card = u.view().card.expect("the card");
        assert_eq!(card.line, "Downloading  ·  6.2 of 14.9 MB");
        assert_eq!(card.title, "Updating zunflix");
        assert_eq!(card.percent, "41%");
        u.update(Event::Fetched, &cx);
        assert_eq!(u.view().installs, 1);
    }

    #[test]
    fn a_failure_shows_and_isnt_fetched_again_until_asked() {
        let (mut u, cx) = offered(Device::Phone, false);
        u.update(Event::Failed, &cx);
        let v = u.view();
        assert_eq!(v.fetch, None);
        assert_eq!(v.card.map(|c| c.line), Some(FAILED.into()));
        u.update(Event::Update, &cx);
        assert!(u.view().fetch.is_some());
    }

    #[test]
    fn nothing_shows_or_installs_while_something_plays() {
        let (mut u, cx) = offered(Device::Phone, false);
        u.update(Event::Fetched, &cx);
        u.update(Event::Playing { playing: true }, &cx);
        assert_eq!(u.view().card, None);
        u.update(Event::Update, &cx);
        assert_eq!(u.view().installs, 0);
        assert!(u.app(&AppEvent::Lifecycle { foreground: true }, &cx).is_empty());
        u.update(Event::Playing { playing: false }, &cx);
        assert!(u.view().card.is_some());
    }

    #[test]
    fn a_tv_waits_to_settle_and_later_moves_it_to_the_rail() {
        let cx = context(Device::Tv);
        let mut u = Updater::default();
        let out = u.update(Event::Opened, &cx);
        let (look, _) = request(&out);
        let settle = timers(&out).into_iter().find(|t| t.1 == TV_SETTLE_MS).expect("settle").0;
        u.answer(look, response(200, OFFER, None), &cx);
        assert_eq!(u.view().card, None);
        u.answer(settle, Reply::Fired, &cx);
        assert!(u.view().card.is_some());
        u.update(Event::Later, &cx);
        let v = u.view();
        assert_eq!(v.card, None);
        assert_eq!(v.rail.as_deref(), Some("Update to v2.1"));
        u.update(Event::Update, &cx);
        assert!(u.view().card.is_some_and(|c| c.updating));
    }

    #[test]
    fn windows_updates_as_it_opens_and_carries_on_if_that_fails() {
        let cx = context(Device::Desktop);
        let mut u = Updater::default();
        let (id, _) = request(&u.update(Event::Opened, &cx));
        assert!(u.view().opening);
        let manifest = r#"{"versionCode": 1300, "versionName": "2.1", "exe": "/app/ZunFlix-Setup.exe"}"#;
        u.answer(id, response(200, manifest, None), &cx);
        let v = u.view();
        assert!(v.opening && v.booting);
        assert_eq!(v.installs, 1);
        assert_eq!(v.bootstrap.map(|b| b.title), Some(UPDATING.into()));
        assert_eq!(v.card, None);
        u.update(Event::Failed, &cx);
        let v = u.view();
        assert!(!v.opening && !v.booting);
        assert_eq!(v.note.as_deref(), Some(FAILED));
        assert!(u.update(Event::Opened, &cx).is_empty());
        assert!(!u.view().opening);
    }

    #[test]
    fn windows_opens_on_when_up_to_date_or_unreachable() {
        let cx = context(Device::Desktop);
        let mut u = Updater::default();
        let (id, _) = request(&u.update(Event::Opened, &cx));
        u.answer(id, response(200, r#"{"versionCode": 1250, "exe": "/a"}"#, None), &cx);
        let v = u.view();
        assert!(!v.opening && v.up_to_date);
        assert_eq!(v.version.name, "zunflix for Windows");
        assert_eq!(v.version.line, "Version 1");
        let mut u = Updater::default();
        let (id, _) = request(&u.update(Event::Opened, &cx));
        let (id, _) = request(&u.answer(id, Reply::Unreachable { offline: true, message: String::new() }, &cx));
        u.answer(id, Reply::Unreachable { offline: true, message: String::new() }, &cx);
        assert!(!u.view().opening);
    }

    #[test]
    fn a_later_look_offers_a_build_found_while_open() {
        let cx = context(Device::Desktop);
        let mut u = Updater::default();
        let out = u.update(Event::Opened, &cx);
        let (id, _) = request(&out);
        let (again, _) = timers(&out)[0];
        u.answer(id, response(200, r#"{"versionCode": 1250, "exe": "/a"}"#, None), &cx);
        let out = u.answer(again, Reply::Fired, &cx);
        let (id, _) = request(&out);
        assert_eq!(timers(&out).len(), 1);
        u.answer(id, response(200, r#"{"versionCode": 1300, "versionName": "2.1", "exe": "/a"}"#, None), &cx);
        let v = u.view();
        assert_eq!(v.installs, 0);
        assert_eq!(v.card.map(|c| c.version), Some("v2.1".into()));
        u.update(Event::Update, &cx);
        let v = u.view();
        assert_eq!(v.installs, 1);
        assert!(v.card.is_none() && v.bootstrap.is_some());
    }
}
