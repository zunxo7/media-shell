//! How a screen lives in the core (decision 0008): the app sends it events, draws its view model, and carries out the
//! effects it asks for, answering each one that wants an answer. The Elm architecture as Crux lays it out, on our own
//! UniFFI bridge.
//!
//! A screen is a [`Screen`], owned by the app's one [`crate::root::Root`]. Every call returns the effects to run now;
//! after any call the app draws [`Screen::view`] again. Effects that want an answer carry an `id`, and the answer comes
//! back through [`Screen::answer`].
//!
//! [`Source`] is a server page as a screen gets it, by the cache rules ([`crate::cache`]): kept on the device, shown
//! at once, asked for again when it's due, and offline better than nothing.

use crate::api::{self, Kind};
pub use crate::store::{Need, PageReply};

/// Something the core asks the app to do. Every bit of I/O the core needs is one of these.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// An HTTP request. Answered with [`Reply::Response`] or [`Reply::Unreachable`].
    Request { id: u32, request: Request },
    /// An HTTP request whose body is written to `file` (relative to the app's files folder) as it comes, but only for
    /// a 2xx. Answered with [`Reply::Saved`] or [`Reply::Unreachable`].
    Fetch { id: u32, request: Request, file: String },
    /// Answered with [`Reply::Fired`] once `ms` have passed.
    Timer { id: u32, ms: u32 },
    /// Stop a pending timer or request: its answer, if one still comes, is ignored.
    Cancel { id: u32 },
    /// Read one of the app's own settings. Answered with [`Reply::Pref`].
    ReadPref { id: u32, key: String },
    /// Set one of the app's own settings; None removes it. No answer.
    WritePref { key: String, value: Option<String> },
    /// A server page from the app's store ([`crate::store`]), as `need` says. The root answers it itself with
    /// [`Reply::Page`]: the app never sees it.
    Page { id: u32, path: String, key: String, need: Need },
    /// Read a page kept on the device. Answered with [`Reply::Kept`] or [`Reply::Missing`].
    Load { id: u32, key: String },
    /// Keep a page on the device (replacing any under `key`, its time now). No answer.
    Store { key: String, body: String, etag: Option<String> },
    /// Play a title from where it was left: `episode` the one to play, None for the title's own pick.
    Play { show: String, episode: Option<String> },
    /// What Play would start is read ahead (its source looked up, its spot found), so pressing it starts at once.
    Warm { show: String, episode: Option<String> },
    /// Show another page.
    Open { route: Route },
    /// The server no longer knows this device: the app forgets the sign-in and goes to sign-in.
    SignedOut,
    /// This profile is in use now: the app's settings and pages are this profile's from here.
    Profile { id: String },
    /// Signed out: everything kept of the account (its sign-in, pages, settings) is forgotten.
    Forget,
    /// Something every screen should hear of (the profiles changed). The root tells them itself: the app never sees it.
    Tell { event: AppEvent },
    /// Something worth knowing about went wrong: the app reports it as it reports its own errors.
    Report { place: String, message: String },
    /// Something only the player's page can do: its engine, a profile setting, closing ([`crate::play::Command`]).
    Player { order: crate::play::Command },
    /// Something a phone asked of this TV, for its pages or its player ([`crate::on_tv`]).
    Tv { order: crate::on_tv::Order },
    /// Something done to the app's files ([`DiskOp`]). Answered with [`Reply::Disk`].
    Disk { id: u32, op: DiskOp },
}

/// The app's answer to an effect.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    /// What the server said, whatever its status.
    Response { status: u16, headers: Vec<Header>, body: String },
    /// A [`Effect::Fetch`]'s answer: its status, and how much was written.
    Saved { status: u16, bytes: u64 },
    /// No answer at all: whether the device is offline, and the transport's own words for why.
    Unreachable { offline: bool, message: String },
    /// A timer's time is up.
    Fired,
    /// A setting's value; None when it isn't set.
    Pref { value: Option<String> },
    /// The kept page: its body, its ETag, and when it came (seconds since 1970).
    Kept { body: String, etag: Option<String>, at: f64 },
    /// Nothing kept under that key.
    Missing,
    /// A page from the store.
    Page { page: PageReply },
    /// A [`Effect::Disk`]'s answer.
    Disk { disk: DiskReply },
}

/// Something done to the app's files. A path is under the app's files folder, except `downloads/…` (the folder
/// downloads are kept in) and `manhwa/…` (the manhwa chapters' folders), wherever each app keeps those; [`Effect::Fetch`]'s
/// `file` is read the same way.
#[derive(Clone, Debug, PartialEq)]
pub enum DiskOp {
    /// A file's text: `text`, None when there's no such file.
    Read { path: String },
    /// A file's text written whole (by way of a file beside it, so it's never seen half written).
    Write { path: String, text: String },
    /// `to` cut to `at` bytes, then `from` added to its end and removed: `bytes` is `to`'s length after.
    Append { from: String, to: String, at: u64 },
    /// A file or folder moved to `to` (over what's there).
    Rename { from: String, to: String },
    /// Files or folders gone, with everything in them.
    Remove { paths: Vec<String> },
    /// The files in a folder with their sizes, as `entries` (none when there's no such folder).
    Listing { path: String },
    /// Each file's size, in order, as `entries` (0 for one that isn't there).
    Sizes { paths: Vec<String> },
    /// Whether a file is a picture the app can show: `ok`.
    Picture { path: String },
    /// The bytes free where downloads are kept: `bytes`.
    Free,
    /// A file's SHA-256, as lowercase hex in `text` (None when it can't be read).
    Hash { path: String },
}

/// What a [`DiskOp`] came to; `ok` false (with the system's `message`) when it couldn't be done.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiskReply {
    pub ok: bool,
    pub message: String,
    pub text: Option<String>,
    pub entries: Vec<DiskEntry>,
    pub bytes: u64,
}

/// A file a [`DiskOp`] answers about.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiskEntry {
    pub name: String,
    pub bytes: u64,
}

/// An HTTP request as the app's client sends it.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub method: Method,
    /// The whole address.
    pub url: String,
    pub headers: Vec<Header>,
    pub body: Option<Body>,
    /// Sent with the app's sign-in (its cookie jar); false sends it with none and keeps none it gets back.
    pub cookies: bool,
    /// How long the whole request may take.
    pub timeout_ms: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub value: String,
}

impl Header {
    pub fn new(name: &str, value: &str) -> Self {
        Self { name: name.into(), value: value.into() }
    }
}

/// A request's body, with its content type.
#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    Text { text: String, content_type: String },
    Bytes { bytes: Vec<u8>, content_type: String },
}

/// A page the core can ask the app to show.
#[derive(Clone, Debug, PartialEq)]
pub enum Route {
    /// A title's page, by its id.
    Title { id: String },
    /// A section's page on its own (a TV's, a wide window's tab), by its key: "dramas", "shows", "movies", "anime".
    Section { key: String },
    /// Search through everything.
    Search,
    History,
    Settings,
    Downloads,
    /// Playing on the account's TV.
    YourTv,
    /// Manhwa's shelf as a page of its own (the Windows app's).
    Manhwa,
    /// A page the app still draws by itself: its name, and what it's opened on (a manhwa series' path, a chapter's).
    Other { name: String, arg: String },
}

/// Whether a request to `url` may carry the sign-in: only when it goes to the zunflix server at `base` (its scheme and
/// host, as "https://host.invalid"). Anything that doesn't plainly start with that origin (another host, a look-alike,
/// a port or case spelled differently) goes without.
pub fn cookies_allowed(base: &str, url: &str) -> bool {
    let base = base.trim_end_matches('/');
    match url.strip_prefix(base) {
        Some(rest) => !base.is_empty() && (rest.is_empty() || rest.starts_with(['/', '?', '#'])),
        None => false,
    }
}

/// The value of a response's header `name`, whatever its case.
pub fn header<'a>(headers: &'a [Header], name: &str) -> Option<&'a str> {
    headers.iter().find(|h| h.name.eq_ignore_ascii_case(name)).map(|h| h.value.as_str())
}

/// What kind of device the app runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    Phone,
    Tv,
    Desktop,
}

/// Facts about the device and the app, given once at the start.
#[derive(Clone, Debug, PartialEq)]
pub struct Platform {
    pub device: Device,
    pub model: String,
    /// The system and its version, as the server lists devices ("Android 15", "Windows 11 26200").
    pub os: String,
    pub version_name: String,
    pub version_code: i64,
    /// This run of the app, as the server counts screens.
    pub stream: String,
    /// What it can play, as the server is told.
    pub codecs: Vec<String>,
    pub hdr: bool,
}

/// The time an event happened.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Clock {
    /// Seconds since 1970.
    pub now: f64,
    /// The device's zone, seconds east of UTC.
    pub utc_offset: i32,
}

/// Something that happened to the whole app, told to every live screen.
#[derive(Clone, Debug, PartialEq)]
pub enum AppEvent {
    /// The app came to the front, or went behind (minimised, another app on top).
    Lifecycle { foreground: bool },
    /// The network came or went, or changed cost.
    Connectivity { online: bool, metered: bool },
    /// A newer copy of the server page kept under `key` arrived in the store.
    PageChanged { key: String },
    /// A profile was added, renamed, pictured or deleted.
    ProfilesChanged,
    /// A profile was deleted.
    ProfileGone { id: String },
    /// The session's account as it is now (None: signed out), the profile in use with it.
    Account { account: Option<crate::session::Account> },
    /// The player closed: what shows how far things are watched asks again.
    Watched,
    /// The player closed over the page of `show`, the last thing played `episode`: that page shows it.
    Played { show: String, episode: Option<String> },
    /// The profile's download settings as they are now (told when they change, and when the account comes), so a
    /// download added next uses them.
    DownloadSettings { quality: String, anime_audio: String },
    /// The server is under maintenance (its 503 with `X-Zunflix-Maintenance`): the owner's message.
    Maintenance { message: String },
}

/// What a screen knows of the app when it's called: the device, the server, the time, and how the app is now.
#[derive(Clone, Debug, PartialEq)]
pub struct Context {
    pub platform: Platform,
    /// The zunflix server, as "https://host.invalid".
    pub base: String,
    pub clock: Clock,
    pub foreground: bool,
    pub online: bool,
    pub metered: bool,
}

impl Context {
    /// The headers that tell the server who's asking ([`api::identity`]).
    pub fn who(&self) -> Vec<Header> {
        api::identity(self.platform.device, &self.platform.model, &self.platform.os, &self.platform.version_name, self.platform.version_code, &self.platform.stream)
    }
}

/// Why a page couldn't be shown, in the app's words.
#[derive(Clone, Debug, PartialEq)]
pub struct Trouble {
    pub message: String,
    pub offline: bool,
    /// Asking again right now (Try again shows a spinner).
    pub busy: bool,
}

/// Nothing to show: the icon, a title and a line.
#[derive(Clone, Debug, PartialEq)]
pub struct Empty {
    pub glyph: Glyph,
    pub title: String,
    pub line: String,
}

/// An icon a view model names; each app draws its own copy of the Lucide icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    History,
    Search,
    Download,
    House,
    Clapperboard,
    Tv,
    Film,
    Sparkles,
    BookOpen,
}

/// A screen in the core.
pub trait Screen {
    type Event;
    type View;

    /// Something happened (a tap, typing, the page opened): the model changes, and these effects are to be run.
    fn update(&mut self, event: Self::Event, cx: &Context) -> Vec<Effect>;

    /// The answer to the effect `id`; one the screen no longer waits for is ignored.
    fn answer(&mut self, id: u32, reply: Reply, cx: &Context) -> Vec<Effect>;

    /// Exactly what to draw.
    fn view(&self) -> Self::View;

    /// Something happened to the whole app; `cx` already says how it is now.
    fn app(&mut self, _event: &AppEvent, _cx: &Context) -> Vec<Effect> {
        vec![]
    }

    /// Its timers keep running while the app is in the background (downloads coming down with the screen off).
    fn behind(&self) -> bool {
        false
    }

    /// This profile's downloads (or what they weigh) changed, or the screen has just opened: a screen that shows
    /// their controls (a title page, a manhwa series) goes by this. The root tells it; the apps never do.
    fn downloads(&mut self, _now: &crate::queue::Snapshot, _cx: &Context) -> Vec<Effect> {
        vec![]
    }

    /// What the screen wants the downloads queue told (a press on a download control, a size to weigh): the root takes
    /// it after each call and tells the queue.
    fn to_queue(&mut self) -> Vec<crate::queue::Event> {
        vec![]
    }
}

/// The effects a screen waits on, by id, with what each was for.
#[derive(Debug)]
pub struct Asks<T> {
    next: u32,
    open: Vec<(u32, T)>,
}

impl<T> Default for Asks<T> {
    fn default() -> Self {
        Self { next: 1, open: Vec::new() }
    }
}

impl<T> Asks<T> {
    /// A new id for an effect asked `for_`.
    pub fn ask(&mut self, for_: T) -> u32 {
        let id = self.next;
        self.next = self.next.wrapping_add(1).max(1);
        self.open.push((id, for_));
        id
    }

    /// What `id` was asked for, if it's still awaited (and no longer after this).
    pub fn take(&mut self, id: u32) -> Option<T> {
        let at = self.open.iter().position(|(i, _)| *i == id)?;
        Some(self.open.swap_remove(at).1)
    }

    /// Stop waiting for everything asked so far: their answers will be ignored. Ids keep counting, so a late answer
    /// can't be mistaken for a new one.
    pub fn forget(&mut self) {
        self.open.clear();
    }
}

/// A server page as a screen shows it, from the app's [`crate::store`]: kept on the device, shown at once, asked for
/// again when it's due, and offline better than nothing. A newer copy arriving later is told as
/// [`AppEvent::PageChanged`], and [`Source::changed`] takes it.
#[derive(Debug)]
pub struct Source {
    path: String,
    key: String,
    asks: Asks<()>,
    /// The body to show, None until there's one.
    pub shown: Option<String>,
    /// Waiting for the device or the server.
    pub busy: bool,
    /// Why the last ask failed, until one succeeds.
    pub failed: Option<(String, bool)>,
}

impl Source {
    /// The page at server `path`, kept on the device under `key`.
    pub fn new(path: &str, key: &str) -> Self {
        Self { path: path.into(), key: key.into(), asks: Asks::default(), shown: None, busy: false, failed: None }
    }

    /// Opened: the page as the store has it, or as soon as it does.
    pub fn open(&mut self, _cx: &Context) -> Vec<Effect> {
        self.asks.forget();
        self.busy = true;
        self.failed = None;
        vec![self.want(Need::Any)]
    }

    /// Ask the server now, whatever the rules say (Try again, or something changed it). A failure stays shown, busy,
    /// until the answer comes.
    pub fn refresh(&mut self, _cx: &Context) -> Vec<Effect> {
        self.busy = true;
        vec![self.want(Need::Server)]
    }

    /// A page changed in the store: this one's newer copy is asked for (the store has it, so it comes at once).
    pub fn changed(&mut self, key: &str) -> Vec<Effect> {
        if key == self.key {
            vec![self.want(Need::Any)]
        } else {
            vec![]
        }
    }

    fn want(&mut self, need: Need) -> Effect {
        Effect::Page { id: self.asks.ask(()), path: self.path.clone(), key: self.key.clone(), need }
    }

    /// The answer to one of its effects: None when `id` isn't one of them; else the effects that follow (the shown
    /// body may have changed).
    pub fn answer(&mut self, id: u32, reply: Reply) -> Option<Vec<Effect>> {
        self.asks.take(id)?;
        if let Reply::Page { page } = reply {
            self.busy = false;
            if page.body.is_some() {
                self.shown = page.body;
            }
            self.failed = page.failure.map(|f| (f.words, f.kind == Kind::Offline));
        }
        Some(vec![])
    }

    /// Nothing to show and the last ask failed: why, for the page's Try again.
    pub fn trouble(&self) -> Option<Trouble> {
        match (&self.shown, &self.failed) {
            (None, Some((message, offline))) => {
                Some(Trouble { message: message.clone(), offline: *offline, busy: self.busy })
            }
            _ => None,
        }
    }
}

/// A context for tests: a phone talking to host.invalid at `now`, in UTC.
#[cfg(test)]
pub(crate) fn test_context(now: f64) -> Context {
    Context {
        platform: Platform {
            device: Device::Phone,
            model: "test".into(),
            os: "test".into(),
            version_name: "1".into(),
            version_code: 1,
            stream: "s".into(),
            codecs: vec!["h264".into()],
            hdr: false,
        },
        base: "https://host.invalid".into(),
        clock: Clock { now, utc_offset: 0 },
        foreground: true,
        online: true,
        metered: false,
    }
}

/// A server answer for tests: `status` with `body`, and its ETag when given.
#[cfg(test)]
pub(crate) fn response(status: u16, body: &str, etag: Option<&str>) -> Reply {
    let headers = etag.map(|e| vec![Header::new("etag", e)]).unwrap_or_default();
    Reply::Response { status, headers, body: body.into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = "/api/progress"; // (fresh a minute, usable a day)

    fn page_id(effects: &[Effect]) -> (u32, bool) {
        match effects {
            [Effect::Page { id, need, .. }] => (*id, *need == Need::Server),
            other => panic!("expected a Page, got {other:?}"),
        }
    }

    fn page(body: Option<&str>, failure: Option<api::Failure>) -> Reply {
        Reply::Page { page: PageReply { body: body.map(String::from), at: 1., failure, stale: false } }
    }

    #[test]
    fn a_page_from_the_store_is_shown() {
        let cx = test_context(1000.);
        let mut s = Source::new(PATH, "k");
        let (id, fresh) = page_id(&s.open(&cx));
        assert!(!fresh && s.busy);
        assert_eq!(s.answer(id, page(Some("old"), None)), Some(vec![]));
        assert_eq!(s.shown.as_deref(), Some("old"));
        assert!(!s.busy);
        assert_eq!(s.trouble(), None);
    }

    #[test]
    fn nothing_and_a_failure_is_trouble_and_late_answers_are_ignored() {
        let cx = test_context(1000.);
        let mut s = Source::new(PATH, "k");
        let (first, _) = page_id(&s.open(&cx));
        let away = api::Failure { kind: Kind::Offline, words: api::OFFLINE.into() };
        s.answer(first, page(None, Some(away)));
        assert_eq!(s.trouble(), Some(Trouble { message: api::OFFLINE.into(), offline: true, busy: false }));
        let (again, fresh) = page_id(&s.refresh(&cx));
        assert!(fresh);
        assert_eq!(s.trouble().map(|t| t.busy), Some(true));
        assert_eq!(s.answer(first, page(Some("late"), None)), None);
        s.answer(again, page(Some("new"), None));
        assert_eq!(s.shown.as_deref(), Some("new"));
    }

    #[test]
    fn a_kept_copy_beats_an_error() {
        let cx = test_context(1000.);
        let mut s = Source::new(PATH, "k");
        let (id, _) = page_id(&s.open(&cx));
        let away = api::Failure { kind: Kind::Unreachable, words: api::UNREACHABLE.into() };
        s.answer(id, page(Some("old"), Some(away)));
        assert_eq!(s.shown.as_deref(), Some("old"));
        assert_eq!(s.trouble(), None);
    }

    #[test]
    fn only_its_own_page_changing_asks_again() {
        let mut s = Source::new(PATH, "k");
        assert!(s.changed("home").is_empty());
        assert!(!page_id(&s.changed("k")).1);
    }

    #[test]
    fn cookies_go_only_to_the_zunflix_server() {
        let base = "https://host.invalid";
        for url in ["https://host.invalid", "https://host.invalid/api/progress", "https://host.invalid?x=1"] {
            assert!(cookies_allowed(base, url), "{url}");
        }
        for url in [
            "https://host.invalid.evil.com/api",
            "https://host.invalid@evil.com/",
            "https://evil.com/https://host.invalid/",
            "http://host.invalid/api",
            "https://cdn.host.invalid/x",
            "https://image.tmdb.org/t/p/w342/a.jpg",
            "",
        ] {
            assert!(!cookies_allowed(base, url), "{url}");
            if !url.is_empty() {
                assert!(!api::get(base, &[], url, None).cookies, "{url}");
            }
        }
        assert!(!cookies_allowed("", "https://evil.com"));
    }
}
