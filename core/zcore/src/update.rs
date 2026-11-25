//! The app's updater: when to look for a newer build, when to offer it and how, whether it is forced, how it comes
//! down, and its words. The server decides which build each account gets (public, or a dev build for Dev-builds
//! accounts) and the oldest build still allowed (`min`); the core compares build numbers.
//!
//! The core owns the download too, as plain effects every app already does: the file comes in 512 KiB stretches
//! (`Effect::Fetch` with a `Range`, each added to a `.dl` file beside it, so a stop picks up where it was and every
//! stretch is a progress report), a stretch that fails is tried again, and the finished file's SHA-256 is checked
//! against the one the server published. Only a verified file reaches the app's own step: handing it to Android's
//! installer, running Windows' setup ([`View::install`], [`View::installs`]).
//!
//! Every app looks again every 15 minutes while open (each look is a signed-in request the server answers itself); a
//! phone or TV also looks on coming back to the front, and Windows and the Mac as they open. As it opens, Windows installs a forced update before going on; an optional one is offered on its
//! card (Update, or put away), as one found later is.
//!
//! A build older than the server's `min` is forced: its card has no Later and can't be put away.
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
use crate::screen::{AppEvent, Asks, Context, Device, DiskOp, Effect, Header, Method, Reply, Request, Screen};
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
pub const EVERY_MS_ANDROID: u32 = 15 * 60_000;
/// How often Windows and the Mac look while open.
pub const EVERY_MS_WINDOWS: u32 = 15 * 60_000;
/// A TV's panel waits this long after the app opens: over the opening it came in mid-animation.
pub const TV_SETTLE_MS: u32 = 3_000;

/// The bootstrapper's title, which Windows' installer carries on under.
pub const UPDATING: &str = "Updating";
pub const FAILED: &str = "Couldn't download the update.";
pub const INSTALL_FAILED: &str = "Couldn't install the update.";
pub const DAMAGED: &str = "The update didn't download properly. Try again.";
pub const UNSIGNED: &str = "This update isn't signed by zunflix, so it wasn't installed.";

/// zunflix's update-signing key (Ed25519). Its private half never leaves the release machine (`secrets/`, used by
/// `tools/release.sh`), so a Windows setup is installed only if zunflix itself released it: not the server alone,
/// were it ever broken into. The build number is signed with it, so an older build can't be passed off as newer.
const RELEASE_KEY: [u8; 32] = [
    0xfa, 0x64, 0x86, 0xf3, 0x2a, 0x3a, 0x1e, 0x44, 0xec, 0xbf, 0x82, 0x1e, 0xd9, 0x26, 0xe4, 0x08, 0x1a, 0x94, 0xdb, 0x4a, 0x52,
    0x0f, 0xa5, 0x34, 0x56, 0xb9, 0xc1, 0xba, 0x6d, 0x74, 0x3c, 0xf2,
];

/// What a release signs: the platform, the build and the file's SHA-256 (lowercase hex).
pub fn signed_text(platform: &str, code: i64, sha256: &str) -> String {
    format!("zunflix-update {platform} {code} {}", sha256.to_ascii_lowercase())
}

/// Whether `sig` (128 hex digits) is the release key's signature of this build of this file.
fn signed_by(key: &[u8; 32], platform: &str, code: i64, sha256: &str, sig: &str) -> bool {
    let sig = sig.trim();
    if sig.len() != 128 || !sig.is_ascii() {
        return false;
    }
    let mut bytes = [0u8; 64];
    for (i, b) in bytes.iter_mut().enumerate() {
        match u8::from_str_radix(&sig[i * 2..i * 2 + 2], 16) {
            Ok(v) => *b = v,
            Err(_) => return false,
        }
    }
    let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(key) else { return false };
    key.verify_strict(signed_text(platform, code, sha256).as_bytes(), &ed25519_dalek::Signature::from_bytes(&bytes)).is_ok()
}

#[cfg(not(test))]
fn release_key() -> [u8; 32] {
    RELEASE_KEY
}

/// (tests sign with a key of their own: the release key's private half isn't in the repo)
#[cfg(test)]
fn release_key() -> [u8; 32] {
    ed25519_dalek::SigningKey::from_bytes(&tests::TEST_SEED).verifying_key().to_bytes()
}
/// A forced update's card title.
pub const REQUIRED: &str = "Update required";

/// Where an update is kept, under the app's files: `update-<build>.<apk|exe|pkg|dmg>` (whole), `….dl` (so far).
pub const FOLDER: &str = "update";
/// How much comes down at a time: also how often it reports how far it's come.
pub const CHUNK: u64 = 512 * 1024;
/// How long one stretch may take.
const CHUNK_TIMEOUT_MS: u32 = 60_000;
/// How long to wait before each new try of a stretch that failed (so it tries three times, then gives up).
const RETRY_MS: [u32; 3] = [2_000, 5_000, 10_000];
/// How many times a file whose checksum is wrong is fetched.
const ATTEMPTS: u8 = 2;
/// What Windows' updater left in the app's folder before the core fetched updates.
const LEGACY: [&str; 2] = ["zunflix-update.exe", "zunflix-update.part"];

/// What the server offers, compared with the installed build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    /// Nothing newer than the installed build.
    UpToDate,
    /// A newer build: its number, its name ("2.1"), where to download it, its SHA-256 and size (empty and 0 when the
    /// server doesn't say), and whether the installed build is below the server's minimum.
    Offer { code: i64, name: String, url: String, sha256: String, size: u64, forced: bool, archive: Option<Archive>, sig: String },
}

/// The app itself as a zip, which a Mac puts in place quietly (no installer window, no password). The `url`, `sha256` and
/// `size` of an [`Update::Offer`] stay the installer package's, for what can't do that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archive {
    pub url: String,
    pub sha256: String,
    pub size: u64,
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
    /// The oldest build still allowed to run: a build below it must update.
    #[serde(default)]
    min: i64,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    sha256: String,
    /// The Mac app as a zip (`mac.json` only), with its own size and checksum.
    zip: Option<String>,
    #[serde(default)]
    zip_size: u64,
    #[serde(default)]
    zip_sha256: String,
    /// The release key's signature of the Windows setup (signed_text), hex.
    #[serde(default)]
    sig: String,
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
    Ok(Update::Offer {
        code: m.version_code,
        name: m.version_name,
        url: absolute(&link),
        sha256: m.sha256.trim().to_lowercase(),
        size: m.size,
        forced: m.min > current,
        archive: m.zip.map(|zip| Archive { url: absolute(&zip), sha256: m.zip_sha256.trim().to_lowercase(), size: m.zip_size }),
        sig: m.sig,
    })
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
    /// The card put away (✕, a flick, Later, Back): not shown again this run unless a newer build comes out. A forced
    /// update can't be put away: this does nothing.
    Later,
    /// The app's own step (Android's installer, Windows' setup, a Mac's swap of the app) couldn't be done. A Mac that
    /// was to put the zip in place then gets the installer package instead.
    Failed,
}

/// A newer build, as the app installs it once it's down and checked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Build {
    pub code: i64,
    pub url: String,
    /// Where the whole file is (as `DiskOp` paths are read).
    pub file: String,
}

/// The update card: a phone's toast, a TV's panel, Windows' card.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    /// "Update available", "Update required", or "Updating zunflix" once it's coming down.
    pub title: String,
    /// "v2.1".
    pub version: String,
    /// "v2.1 is ready to install", how far it's come, or why it failed.
    pub line: String,
    pub button: String,
    /// "Later"; empty when the update is forced.
    pub later: String,
    /// The installed build is below the server's minimum: no Later, and the card can't be put away.
    pub forced: bool,
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
    /// Takes the app off this PC (Windows').
    pub uninstall: String,
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
    /// The build the installer is for, down and checked; `installs` goes up each time the app is to run its own step
    /// (Android's installer, Windows' setup) on `file`.
    pub install: Option<Build>,
    pub installs: u32,
    /// Windows: the install is the one it opens with, so failing, it carries on opening.
    pub booting: bool,
    /// Windows: still deciding whether to update before opening; the splash waits.
    pub opening: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ask {
    /// A look with the account's sign-in (a Dev-builds account gets the dev build).
    Look,
    /// The same without it, when that failed (signed out).
    Public,
    Again,
    Settle,
    /// The files kept so far; the build the answer is about, for each step of the download.
    Listing(i64),
    Sizes(i64),
    Chunk(i64),
    Joined(i64),
    Named(i64),
    Hashed(i64),
    Cleared(i64),
    Retry(i64),
    /// An answer nothing waits on (tidying).
    Ignore,
}

/// What the server says of the offered file.
#[derive(Debug, Default, Clone)]
struct Pack {
    sha256: String,
    size: u64,
    /// Windows: the release key's signature, which must match before the setup runs (a Mac's update is checked
    /// against Apple's signature of zunflix's team instead, and Android's against the app's own signing key).
    signed: Option<String>,
}

/// The updater. See the module's docs.
#[derive(Debug, Default)]
pub struct Updater {
    asks: Asks<Ask>,
    started: bool,
    looking: bool,
    offer: Option<(Build, String)>,
    pack: Pack,
    /// A Mac: the installer package (its link and what the server says of it), kept for when the zip can't be put in
    /// place. None once it's in use, and for every other device.
    spare: Option<(String, Pack)>,
    /// The build whose zip couldn't be put in place, so its package is the one fetched.
    packaged: Option<i64>,
    /// The installed build is below the server's minimum.
    forced: bool,
    up_to_date: bool,
    settled: bool,
    playing: bool,
    /// The card put away this run.
    dismissed: bool,
    /// Update pressed, and not yet handed to the installer.
    updating: bool,
    /// Fetching was asked for (Update pressed): it goes on even on mobile data.
    asked: bool,
    /// All of the offered build is in, and checked.
    ready: bool,
    /// The download is under way (or about to be).
    getting: bool,
    /// How much of the file is in the `.dl` file.
    have: u64,
    /// How much the stretch asked for last brought.
    landed: u64,
    /// Tries of this stretch that failed in a row.
    tries: usize,
    /// Fetches of the file that failed its checksum.
    attempts: u8,
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

/// The file an offered build is kept in: named for its build and the kind of file its link is.
fn file_for(code: i64, url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or_default();
    let kind = path
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, kind)| kind)
        .filter(|kind| !kind.is_empty() && kind.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or("bin");
    format!("{FOLDER}/update-{code}.{kind}")
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

    fn disk(&mut self, ask: Ask, op: DiskOp) -> Effect {
        Effect::Disk { id: self.asks.ask(ask), op }
    }

    fn removing(&mut self, paths: Vec<String>) -> Effect {
        self.disk(Ask::Ignore, DiskOp::Remove { paths })
    }

    /// What a look found.
    fn found(&mut self, found: Option<Update>, cx: &Context) -> Vec<Effect> {
        self.looking = false;
        match found {
            Some(Update::Offer { code, name, url, sha256, size, forced, archive, sig }) => {
                // (a Mac takes the zip: the app is swapped in quietly. The package is for when that can't be done)
                let mac = cx.platform.device == Device::Desktop && cx.platform.os.starts_with("macOS");
                let windows = cx.platform.device == Device::Desktop && !mac;
                let package = (url, Pack { sha256, size, signed: windows.then_some(sig) });
                let ((url, pack), spare) = match archive {
                    Some(a) if mac && self.packaged != Some(code) => ((a.url, Pack { sha256: a.sha256, size: a.size, signed: None }), Some(package)),
                    _ => (package, None),
                };
                self.spare = spare;
                self.up_to_date = false;
                self.fetch_failed = false;
                self.forced = forced;
                if let Some((build, _)) = self.offer.as_mut().filter(|(b, _)| b.code == code) {
                    // (the same build: its link is renewed, as an old one stops working)
                    build.url = url;
                    self.opening = false;
                    return self.download();
                }
                // (a newer build than the one put away: shown again, and its download starts over)
                let file = file_for(code, &url);
                self.offer = Some((Build { code, url, file }, name));
                self.pack = pack;
                (self.dismissed, self.ready, self.progress, self.have) = (false, false, (0, 0), 0);
                self.asked = self.updating;
                self.getting = false;
                self.attempts = 0;
                // (a forced update installs as the PC opens; an optional one waits on its card)
                if self.opening && !self.skip_opening && forced {
                    self.booting = true;
                    self.updating = true;
                    return self.download();
                }
            }
            Some(Update::UpToDate) => {
                self.offer = None;
                self.forced = false;
                self.getting = false;
                if !self.up_to_date {
                    self.up_to_date = true;
                    self.opening = false;
                    // (this build the newest: what an earlier update left behind can go)
                    let mut paths = vec![FOLDER.to_string()];
                    paths.extend(LEGACY.map(String::from));
                    return vec![self.removing(paths)];
                }
            }
            None => {}
        }
        self.opening = false;
        self.download()
    }

    /// The installer, never over something playing.
    fn install(&mut self) {
        if self.playing || self.offer.is_none() {
            return;
        }
        self.installs += 1;
        self.note = None;
        if !self.desktop {
            // (a phone's card goes and stays away once Android's installer takes over; a forced one stays, as the
            // update has to be made)
            self.updating = false;
            self.dismissed = !self.forced;
        }
    }

    // ---------- the download ----------

    fn code(&self) -> Option<i64> {
        self.offer.as_ref().map(|(b, _)| b.code)
    }

    /// Whether the file is to come down now: on Wi-Fi ahead of time on a phone or TV, anywhere once Update is pressed.
    fn may_download(&self) -> bool {
        self.offer.is_some() && !self.ready && !self.fetch_failed && (self.asked || self.updating || (!self.desktop && !self.metered))
    }

    /// The paths of the offered build's whole file, its `.dl` so far and the stretch in flight.
    fn paths(&self) -> (String, String, String) {
        let file = self.offer.as_ref().map(|(b, _)| b.file.clone()).unwrap_or_default();
        let chunk = format!("{}.chunk", file.rsplit_once('.').map_or(file.as_str(), |(stem, _)| stem));
        (file.clone(), format!("{file}.dl"), chunk)
    }

    /// Starts the download if it's wanted and isn't under way: what's kept is looked at first.
    fn download(&mut self) -> Vec<Effect> {
        let Some(code) = self.code() else { return vec![] };
        if self.getting || !self.may_download() {
            return vec![];
        }
        self.getting = true;
        self.tries = 0;
        vec![self.disk(Ask::Listing(code), DiskOp::Listing { path: FOLDER.into() })]
    }

    /// The next stretch, from where the file has got to.
    fn chunk(&mut self) -> Vec<Effect> {
        let Some((build, _)) = &self.offer else { return vec![] };
        if !self.may_download() {
            // (on mobile data and not asked for: it waits for Wi-Fi, or for Update)
            self.getting = false;
            return vec![];
        }
        let (code, url) = (build.code, build.url.clone());
        let range = format!("bytes={}-{}", self.have, self.have + CHUNK - 1);
        let request = Request {
            method: Method::Get,
            url,
            headers: vec![Header::new("Range", &range), Header::new("Accept-Encoding", "identity")],
            body: None,
            cookies: false,
            timeout_ms: CHUNK_TIMEOUT_MS,
        };
        let file = self.paths().2;
        vec![Effect::Fetch { id: self.asks.ask(Ask::Chunk(code)), request, file }]
    }

    /// A stretch failed: tried again after a wait, up to three times.
    fn retry(&mut self, code: i64) -> Vec<Effect> {
        let Some(ms) = RETRY_MS.get(self.tries).copied() else {
            self.fail(FAILED);
            return vec![];
        };
        self.tries += 1;
        vec![Effect::Timer { id: self.asks.ask(Ask::Retry(code)), ms }]
    }

    /// All the bytes are in: the `.dl` file takes its name.
    fn keep(&mut self, code: i64) -> Vec<Effect> {
        let (file, dl, _) = self.paths();
        vec![self.disk(Ask::Named(code), DiskOp::Rename { from: dl, to: file })]
    }

    /// The whole file is there: checked against the server's SHA-256. One published without a checksum is never
    /// installed: an unchecked file isn't run.
    fn verify(&mut self, code: i64) -> Vec<Effect> {
        if self.pack.sha256.is_empty() {
            self.fail(FAILED);
            return vec![];
        }
        let file = self.paths().0;
        vec![self.disk(Ask::Hashed(code), DiskOp::Hash { path: file })]
    }

    /// Down and checked: it goes to the installer when Update was pressed.
    fn ready_now(&mut self) -> Vec<Effect> {
        self.ready = true;
        self.getting = false;
        self.progress = (self.have, self.pack.size.max(self.have));
        if self.updating {
            self.install();
        }
        vec![]
    }

    /// The download (or the app's step after it) failed: shown, and not tried again until Update is pressed.
    fn fail(&mut self, words: &str) {
        self.getting = false;
        self.updating = false;
        self.fetch_failed = true;
        self.dismissed = false;
        self.note = Some(words.into());
        if self.booting {
            self.booting = false;
            self.skip_opening = true;
            self.opening = false;
        }
    }

    /// An answer to a step of the download.
    fn downloaded(&mut self, ask: Ask, reply: &Reply) -> Vec<Effect> {
        let (Ask::Listing(code) | Ask::Sizes(code) | Ask::Chunk(code) | Ask::Joined(code) | Ask::Named(code) | Ask::Hashed(code) | Ask::Cleared(code) | Ask::Retry(code)) = ask else {
            return vec![];
        };
        // (answers for a build since replaced, or a download since stopped, are let go)
        if !self.getting || self.code() != Some(code) {
            return vec![];
        }
        let disk = match reply {
            Reply::Disk { disk } => Some(disk),
            _ => None,
        };
        let size = self.pack.size;
        let (file, dl, chunk) = self.paths();
        match ask {
            Ask::Listing(_) => {
                let keep = format!("update-{code}.");
                let mut stale: Vec<String> = disk
                    .map(|d| d.entries.iter().filter(|e| !e.name.starts_with(&keep)).map(|e| format!("{FOLDER}/{}", e.name)).collect())
                    .unwrap_or_default();
                stale.extend(LEGACY.map(String::from));
                vec![self.removing(stale), self.disk(Ask::Sizes(code), DiskOp::Sizes { paths: vec![file, dl] })]
            }
            Ask::Sizes(_) => {
                let sizes: Vec<u64> = disk.map(|d| d.entries.iter().map(|e| e.bytes).collect()).unwrap_or_default();
                let (whole, part) = (sizes.first().copied().unwrap_or(0), sizes.get(1).copied().unwrap_or(0));
                if whole > 0 {
                    self.have = whole;
                    return self.verify(code);
                }
                if size > 0 && part > size {
                    // (more than the server has: not this file's)
                    self.have = 0;
                    return vec![self.disk(Ask::Cleared(code), DiskOp::Remove { paths: vec![dl] })];
                }
                self.have = part;
                self.progress = (part, size);
                if size > 0 && part == size {
                    return self.keep(code);
                }
                self.chunk()
            }
            Ask::Chunk(_) => match reply {
                Reply::Saved { status: 206, bytes } if *bytes > 0 => {
                    self.tries = 0;
                    self.landed = *bytes;
                    vec![self.disk(Ask::Joined(code), DiskOp::Append { from: chunk, to: dl, at: self.have })]
                }
                Reply::Saved { status: 200, bytes } if *bytes > 0 => {
                    // (the server sent it all, whatever was asked: that is the file)
                    self.have = *bytes;
                    self.progress = (*bytes, size.max(*bytes));
                    vec![self.disk(Ask::Named(code), DiskOp::Rename { from: chunk, to: file })]
                }
                // (asked from past its end: it was all there already)
                Reply::Saved { status: 416, .. } if self.have > 0 => self.keep(code),
                _ => self.retry(code),
            },
            Ask::Joined(_) => {
                let Some(d) = disk.filter(|d| d.ok) else {
                    self.fail(FAILED);
                    return vec![];
                };
                self.have = d.bytes;
                self.progress = (self.have, size);
                if (size > 0 && self.have >= size) || self.landed < CHUNK {
                    self.keep(code)
                } else {
                    self.chunk()
                }
            }
            Ask::Named(_) => {
                if disk.is_some_and(|d| d.ok) {
                    self.progress = (self.have, size.max(self.have));
                    self.verify(code)
                } else {
                    self.fail(FAILED);
                    vec![]
                }
            }
            Ask::Hashed(_) => match disk.and_then(|d| d.text.as_deref()) {
                Some(hash) if hash.eq_ignore_ascii_case(&self.pack.sha256) => {
                    match self.pack.signed.clone() {
                        Some(sig) if !signed_by(&release_key(), "windows", code, hash, &sig) => {
                            // (not released by zunflix: never run, and not fetched again)
                            self.fail(UNSIGNED);
                            vec![self.removing(vec![file, dl])]
                        }
                        _ => self.ready_now(),
                    }
                }
                Some(_) => {
                    // (a damaged file goes, and it's fetched again from the start, once)
                    self.attempts += 1;
                    self.have = 0;
                    self.progress = (0, size);
                    let paths = vec![file, dl];
                    if self.attempts < ATTEMPTS {
                        vec![self.disk(Ask::Cleared(code), DiskOp::Remove { paths })]
                    } else {
                        self.fail(DAMAGED);
                        vec![self.removing(paths)]
                    }
                }
                None => {
                    self.fail(FAILED);
                    vec![]
                }
            },
            Ask::Cleared(_) | Ask::Retry(_) => self.chunk(),
            _ => vec![],
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
                if self.ready {
                    self.install();
                    return vec![];
                }
                self.download()
            }
            Event::Later => {
                if self.forced {
                    return vec![];
                }
                self.dismissed = true;
                if !self.desktop {
                    self.updating = false;
                }
                vec![]
            }
            Event::Failed => {
                if let (Some((url, pack)), Some((build, _))) = (self.spare.take(), self.offer.as_mut()) {
                    // (the zip couldn't be put in place: the installer package is the other way, fetched now)
                    self.packaged = Some(build.code);
                    build.file = file_for(build.code, &url);
                    build.url = url;
                    self.pack = pack;
                    (self.ready, self.getting, self.progress, self.have, self.attempts) = (false, false, (0, 0), 0, 0);
                    return self.download();
                }
                self.fail(INSTALL_FAILED);
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
                Outcome::Body { body, .. } => self.found(decide(&body, cx.platform.version_code).ok(), cx),
                Outcome::Failed { .. } if ask == Ask::Look => vec![self.look(Ask::Public, cx)],
                _ => self.found(None, cx),
            },
            Ask::Ignore => vec![],
            step => self.downloaded(step, &reply),
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
                (None, false) if self.forced => format!("{version} is needed to keep using zunflix"),
                (None, false) => format!("{version} is ready to install"),
            };
            let title = match (self.updating, self.forced) {
                (true, _) => "Updating zunflix",
                (false, true) => REQUIRED,
                (false, false) => "Update available",
            };
            Card {
                title: title.into(),
                version,
                line,
                button: "Update".into(),
                later: if self.forced { String::new() } else { "Later".into() },
                forced: self.forced,
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
        View {
            card,
            rail,
            version: VersionCard {
                offer: named.map(|version| VersionOffer { button: "Update".into(), version }),
                ..self.version.clone()
            },
            bootstrap,
            note: self.note.clone(),
            install: offer.filter(|_| self.ready).map(|(b, _)| b.clone()),
            installs: self.installs,
            booting: self.booting,
            opening: self.opening,
        }
    }

    fn app(&mut self, event: &AppEvent, cx: &Context) -> Vec<Effect> {
        match event {
            AppEvent::Lifecycle { foreground: true } if self.started => self.check(cx),
            AppEvent::Connectivity { metered, .. } => {
                self.metered = *metered;
                // (back on Wi-Fi: what waited for it comes down)
                self.download()
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
        uninstall: "Uninstall".into(),
        offer: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{response, test_context, DiskEntry, DiskReply};
    use sha2::{Digest, Sha256};
    use std::collections::{BTreeMap, VecDeque};

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

    /// What the server has: a file of `len` bytes.
    fn build(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 7 % 251) as u8).collect()
    }

    fn hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    pub(super) const TEST_SEED: [u8; 32] = [7; 32];

    /// A release's signature of build `code` of `body`, as tools/release.sh makes it (with the tests' key).
    fn sign(code: i64, body: &[u8], key: &[u8; 32]) -> String {
        use ed25519_dalek::Signer;
        let text = signed_text("windows", code, &hex(body));
        ed25519_dalek::SigningKey::from_bytes(key).sign(text.as_bytes()).to_bytes().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// (signed, as every release is: only a Windows PC checks it)
    fn manifest(body: &[u8], extra: &str) -> String {
        format!(
            r#"{{"versionCode": 1300, "versionName": "2.1", "apk": "/app/zunflix.apk?e=1&k=2", "size": {}, "sha256": "{}", "sig": "{}"{extra}}}"#,
            body.len(),
            hex(body),
            sign(1300, body, &TEST_SEED)
        )
    }

    /// The setup's manifest: the same, with an exe.
    fn windows_manifest(body: &[u8]) -> String {
        manifest(body, "").replace("/app/zunflix.apk", "/app/ZunFlix-Setup.exe")
    }

    /// The device's side of the core's effects: files in memory, and a server that serves `body` by byte ranges.
    #[derive(Default)]
    struct Phone {
        files: BTreeMap<String, Vec<u8>>,
        body: Vec<u8>,
        /// Stretches that fail before one comes.
        fail: usize,
        /// Ranges asked for, in order.
        ranges: Vec<String>,
        timers: Vec<u32>,
        /// Answer 200 with the whole body, whatever the range.
        ignores_ranges: bool,
    }

    impl Phone {
        fn serving(body: Vec<u8>) -> Self {
            Self { body, ..Default::default() }
        }

        /// Carries out `effects` and everything they lead to.
        fn run(&mut self, u: &mut Updater, cx: &Context, effects: Vec<Effect>) {
            let mut todo: VecDeque<Effect> = effects.into();
            while let Some(effect) = todo.pop_front() {
                todo.extend(self.step(u, cx, effect));
            }
        }

        /// Carries out one effect; what the updater asked next.
        fn step(&mut self, u: &mut Updater, cx: &Context, effect: Effect) -> Vec<Effect> {
            let (id, reply) = match effect {
                Effect::Disk { id, op } => (id, Reply::Disk { disk: self.disk(op) }),
                Effect::Fetch { id, request, file } => (id, self.fetch(&request, &file)),
                Effect::Timer { id, ms } => {
                    self.timers.push(ms);
                    (id, Reply::Fired)
                }
                other => panic!("unexpected {other:?}"),
            };
            u.answer(id, reply, cx)
        }

        fn fetch(&mut self, request: &Request, file: &str) -> Reply {
            let range = request.headers.iter().find(|h| h.name == "Range").map(|h| h.value.clone()).unwrap_or_default();
            self.ranges.push(range.clone());
            if self.fail > 0 {
                self.fail -= 1;
                return Reply::Unreachable { offline: false, message: "cut".into() };
            }
            if self.ignores_ranges {
                self.files.insert(file.into(), self.body.clone());
                return Reply::Saved { status: 200, bytes: self.body.len() as u64 };
            }
            let (from, to) = range.trim_start_matches("bytes=").split_once('-').expect("a range");
            let (from, to): (usize, usize) = (from.parse().expect("from"), to.parse().expect("to"));
            if from >= self.body.len() {
                return Reply::Saved { status: 416, bytes: 0 };
            }
            let part = self.body[from..self.body.len().min(to + 1)].to_vec();
            let bytes = part.len() as u64;
            self.files.insert(file.into(), part);
            Reply::Saved { status: 206, bytes }
        }

        fn disk(&mut self, op: DiskOp) -> DiskReply {
            let ok = DiskReply { ok: true, ..Default::default() };
            match op {
                DiskOp::Listing { path } => DiskReply {
                    entries: self
                        .files
                        .iter()
                        .filter_map(|(k, v)| k.strip_prefix(&format!("{path}/")).map(|n| DiskEntry { name: n.into(), bytes: v.len() as u64 }))
                        .collect(),
                    ..ok
                },
                DiskOp::Sizes { paths } => DiskReply {
                    entries: paths.into_iter().map(|p| DiskEntry { bytes: self.files.get(&p).map_or(0, |f| f.len() as u64), name: p }).collect(),
                    ..ok
                },
                DiskOp::Remove { paths } => {
                    self.files.retain(|k, _| !paths.iter().any(|p| k == p || k.starts_with(&format!("{p}/"))));
                    ok
                }
                DiskOp::Append { from, to, at } => {
                    let add = self.files.remove(&from).unwrap_or_default();
                    let out = self.files.entry(to).or_default();
                    out.truncate(at as usize);
                    out.extend(add);
                    DiskReply { bytes: out.len() as u64, ..ok }
                }
                DiskOp::Rename { from, to } => match self.files.remove(&from) {
                    Some(f) => {
                        self.files.insert(to, f);
                        ok
                    }
                    None => DiskReply { ok: false, message: "missing".into(), ..Default::default() },
                },
                DiskOp::Hash { path } => match self.files.get(&path) {
                    Some(f) => DiskReply { text: Some(hex(f)), ..ok },
                    None => DiskReply { ok: false, ..Default::default() },
                },
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    /// An updater opened and told of `manifest`; what it asked for next.
    fn offered_with(device: Device, metered: bool, manifest: &str) -> (Updater, Context, Vec<Effect>) {
        let mut cx = context(device);
        cx.metered = metered;
        let mut u = Updater::default();
        let (id, _) = request(&u.update(Event::Opened, &cx));
        let out = u.answer(id, response(200, manifest, None), &cx);
        (u, cx, out)
    }

    #[test]
    fn newer_build_is_offered_with_a_full_link() {
        let found = decide(r#"{"versionCode": 1300, "versionName": "2.1", "apk": "/app/dev/zunflix.apk?key=k"}"#, 1250);
        assert_eq!(
            found,
            Ok(Update::Offer {
                code: 1300,
                name: "2.1".into(),
                url: "https://host.invalid/app/dev/zunflix.apk?key=k".into(),
                sha256: String::new(),
                size: 0,
                forced: false,
                archive: None,
                sig: String::new(),
            })
        );
    }

    #[test]
    fn the_manifest_says_the_size_the_checksum_and_the_oldest_build_allowed() {
        let m = r#"{"versionCode": 1300, "apk": "/a", "size": 99, "sha256": "ABCD", "min": 1251}"#;
        assert_eq!(
            decide(m, 1250),
            Ok(Update::Offer { code: 1300, name: String::new(), url: "https://host.invalid/a".into(), sha256: "abcd".into(), size: 99, forced: true, archive: None, sig: String::new() })
        );
        // (at the minimum, or no minimum, it's optional)
        assert!(matches!(decide(m, 1251), Ok(Update::Offer { forced: false, .. })));
        assert!(matches!(decide(r#"{"versionCode": 1300, "apk": "/a", "min": 0}"#, 1250), Ok(Update::Offer { forced: false, .. })));
        assert!(matches!(decide(r#"{"versionCode": 1300, "apk": "/a"}"#, 1250), Ok(Update::Offer { forced: false, .. })));
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
        assert!(matches!(found, Ok(Update::Offer { url, size: 1, .. }) if url == "https://host.invalid/app/ZunFlix-Setup.exe"));
    }

    #[test]
    fn mac_manifest_uses_its_pkg_else_dmg() {
        let found = decide(r#"{"versionCode": 2, "versionName": "1.0.1", "pkg": "/app/zunflix.pkg", "dmg": "/app/zunflix.dmg", "size": 1}"#, 1);
        assert!(matches!(found, Ok(Update::Offer { url, .. }) if url == "https://host.invalid/app/zunflix.pkg"));
        let found = decide(r#"{"versionCode": 2, "versionName": "1.0.1", "dmg": "/app/zunflix.dmg", "size": 1}"#, 1);
        assert!(matches!(found, Ok(Update::Offer { url, .. }) if url == "https://host.invalid/app/zunflix.dmg"));
    }

    fn mac() -> Context {
        let mut cx = context(Device::Desktop);
        cx.platform.os = "macOS 15.1".into();
        cx
    }

    /// The Mac's manifest: the package, and the app's zip beside it.
    fn mac_manifest(zip: &[u8], pkg: &[u8]) -> String {
        format!(
            r#"{{"versionCode": 1300, "versionName": "2.1", "pkg": "/app/zunflix.pkg", "size": {}, "sha256": "{}", "zip": "/app/zunflix-app.zip", "zipSize": {}, "zipSha256": "{}", "sig": "{}"}}"#,
            pkg.len(),
            hex(pkg),
            zip.len(),
            hex(zip),
            sign(1300, pkg, &TEST_SEED)
        )
    }

    #[test]
    fn the_manifest_may_carry_the_apps_zip_beside_the_package() {
        let found = decide(&mac_manifest(b"zip", b"pkg"), 1250);
        let Ok(Update::Offer { url, archive: Some(a), .. }) = found else { panic!("{found:?}") };
        assert_eq!(url, "https://host.invalid/app/zunflix.pkg");
        assert_eq!((a.url.as_str(), a.size), ("https://host.invalid/app/zunflix-app.zip", 3));
        assert_eq!(a.sha256, hex(b"zip"));
    }

    #[test]
    fn a_mac_fetches_the_zip_when_there_is_one_and_the_package_when_not() {
        let (zip, pkg) = (build(300_000), build(200_000));
        let cx = mac();
        let mut u = Updater::default();
        let (id, _) = request(&u.update(Event::Opened, &cx));
        u.answer(id, response(200, &mac_manifest(&zip, &pkg), None), &cx);
        let out = u.update(Event::Update, &cx);   // (an optional update: Update pressed on its card)
        Phone::serving(zip).run(&mut u, &cx, out);
        let v = u.view();
        assert_eq!((v.installs, v.install.map(|b| b.file)), (1, Some("update/update-1300.zip".into())));
        // (no zip in the manifest: the package, as older Mac builds do)
        let mut u = Updater::default();
        let (id, _) = request(&u.update(Event::Opened, &cx));
        let body = build(5);
        let m = format!(r#"{{"versionCode": 1300, "versionName": "2.1", "pkg": "/app/zunflix.pkg", "size": 5, "sha256": "{}"}}"#, hex(&body));
        u.answer(id, response(200, &m, None), &cx);
        let out = u.update(Event::Update, &cx);
        let mut phone = Phone::serving(body);
        phone.run(&mut u, &cx, out);
        assert_eq!(u.view().install.map(|b| b.file), Some("update/update-1300.pkg".into()));
    }

    #[test]
    fn the_release_key_is_a_real_key() {
        assert!(ed25519_dalek::VerifyingKey::from_bytes(&RELEASE_KEY).is_ok());
    }

    /// Windows runs a setup only when the release key signed that very build of that very file.
    fn windows_installs(sig: &str, code_signed: i64) -> bool {
        let cx = context(Device::Desktop);
        let mut u = Updater::default();
        let (id, _) = request(&u.update(Event::Opened, &cx));
        let body = build(5);
        let sig = if sig.is_empty() { sign(code_signed, &body, &TEST_SEED) } else { sig.to_string() };
        let m = format!(
            r#"{{"versionCode": 1300, "versionName": "2.1", "exe": "/app/ZunFlix-Setup.exe", "size": 5, "sha256": "{}", "sig": "{sig}"}}"#,
            hex(&body)
        );
        u.answer(id, response(200, &m, None), &cx);
        let out = u.update(Event::Update, &cx);
        Phone::serving(body).run(&mut u, &cx, out);
        let v = u.view();
        assert!(v.installs > 0 || v.note.as_deref() == Some(UNSIGNED), "{:?}", v.note);
        v.installs > 0
    }

    #[test]
    fn windows_installs_only_what_the_release_key_signed() {
        assert!(windows_installs("", 1300));
        // (signed by anyone else, for another build (an old one passed off as new), or not at all: never run)
        let stranger = sign(1300, &build(5), &[9; 32]);
        assert!(!windows_installs(&stranger, 1300));
        assert!(!windows_installs("", 1200));
        assert!(!windows_installs(&"0".repeat(128), 1300));
        assert!(!windows_installs("not hex", 1300));
    }

    #[test]
    fn a_mac_or_phone_needs_no_release_signature() {
        let body = build(5);
        let m = format!(r#"{{"versionCode": 1300, "apk": "/app/zunflix.apk", "size": 5, "sha256": "{}"}}"#, hex(&body));
        let cx = context(Device::Phone);
        let mut u = Updater::default();
        let (id, _) = request(&u.update(Event::Opened, &cx));
        u.answer(id, response(200, &m, None), &cx);
        let out = u.update(Event::Update, &cx);
        Phone::serving(body).run(&mut u, &cx, out);
        assert!(u.view().note.as_deref() != Some(UNSIGNED));
    }

    #[test]
    fn a_file_published_without_a_checksum_is_never_installed() {
        let cx = context(Device::Desktop);
        let mut u = Updater::default();
        let (id, _) = request(&u.update(Event::Opened, &cx));
        let body = build(5);
        let m = r#"{"versionCode": 1300, "versionName": "2.1", "exe": "/app/ZunFlix-Setup.exe", "size": 5, "sha256": ""}"#;
        u.answer(id, response(200, m, None), &cx);
        let out = u.update(Event::Update, &cx);
        Phone::serving(body).run(&mut u, &cx, out);
        let v = u.view();
        assert_eq!(v.installs, 0);
        assert!(v.install.is_none());
    }

    #[test]
    fn windows_ignores_a_zip() {
        let (zip, pkg) = (build(10), build(20));
        let cx = context(Device::Desktop);
        let mut u = Updater::default();
        let (id, _) = request(&u.update(Event::Opened, &cx));
        u.answer(id, response(200, &mac_manifest(&zip, &pkg), None), &cx);
        let out = u.update(Event::Update, &cx);   // (an optional update: Update pressed on its card)
        Phone::serving(pkg).run(&mut u, &cx, out);
        assert_eq!(u.view().install.map(|b| b.file), Some("update/update-1300.pkg".into()));
    }

    #[test]
    fn a_zip_that_cant_be_put_in_place_gives_way_to_the_package_once() {
        let (zip, pkg) = (build(300_000), build(200_000));
        let cx = mac();
        let mut u = Updater::default();
        let (id, _) = request(&u.update(Event::Opened, &cx));
        u.answer(id, response(200, &mac_manifest(&zip, &pkg), None), &cx);
        let out = u.update(Event::Update, &cx);   // (an optional update: Update pressed on its card)
        let mut phone = Phone::serving(zip);
        phone.run(&mut u, &cx, out);
        assert_eq!(u.view().installs, 1);
        // (the Mac's swap failed: the package comes down, and is the next thing to install)
        let out = u.update(Event::Failed, &cx);
        assert!(u.view().note.is_none() && u.view().install.is_none() && u.view().bootstrap.is_some());
        phone.body = pkg;
        phone.run(&mut u, &cx, out);
        let v = u.view();
        assert_eq!((v.installs, v.install.map(|b| b.file)), (2, Some("update/update-1300.pkg".into())));
        // (and failing too, it says so)
        assert!(u.update(Event::Failed, &cx).is_empty());
        assert_eq!(u.view().note.as_deref(), Some(INSTALL_FAILED));
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
        u.answer(id, response(200, OFFER, None), &cx);
        assert!(u.view().card.is_some());
    }

    #[test]
    fn on_wifi_it_comes_down_ahead_in_stretches_and_update_installs_at_once() {
        let body = build(1_300_000);
        let (mut u, cx, out) = offered_with(Device::Phone, false, &manifest(&body, ""));
        assert_eq!(u.view().version.offer.as_ref().map(|o| o.version.as_str()), Some("v2.1"));
        // (nothing to install until it's down and checked)
        assert_eq!(u.view().install, None);
        let mut phone = Phone::serving(body.clone());
        phone.run(&mut u, &cx, out);
        // (512 KiB at a time: three stretches, from the bytes in)
        assert_eq!(phone.ranges, vec!["bytes=0-524287", "bytes=524288-1048575", "bytes=1048576-1572863"]);
        let file = "update/update-1300.apk";
        assert_eq!(phone.files.get(file), Some(&body));
        assert!(!phone.files.keys().any(|k| k.ends_with(".dl") || k.ends_with(".chunk")));
        let v = u.view();
        assert_eq!(v.install.as_ref().map(|b| (b.code, b.file.as_str())), Some((1300, file)));
        assert_eq!(v.installs, 0);
        u.update(Event::Update, &cx);
        let v = u.view();
        assert_eq!(v.installs, 1);
        // (the card goes once Android's installer takes over; the sidebar keeps a way back to it)
        assert_eq!(v.card, None);
        assert_eq!(v.rail.as_deref(), Some("Update to v2.1"));
    }

    #[test]
    fn on_mobile_data_it_waits_for_update_then_shows_how_far_after_each_stretch() {
        let body = build(1_300_000);
        let (mut u, cx, out) = offered_with(Device::Phone, true, &manifest(&body, ""));
        assert!(out.is_empty());
        let mut phone = Phone::serving(body);
        let mut todo: VecDeque<Effect> = u.update(Event::Update, &cx).into();
        let card = u.view().card.expect("the card");
        assert_eq!((card.updating, card.title.as_str(), card.line.as_str()), (true, "Updating zunflix", "Connecting"));
        let mut lines = vec![];
        while let Some(effect) = todo.pop_front() {
            // (each stretch is told once it's added to the file)
            let added = matches!(effect, Effect::Disk { op: DiskOp::Append { .. }, .. });
            todo.extend(phone.step(&mut u, &cx, effect));
            if added {
                lines.push(u.view().card.map(|c| (c.line, c.percent)));
            }
        }
        assert_eq!(lines[0], Some(("Downloading  ·  0.5 of 1.3 MB".into(), "40%".into())));
        assert_eq!(u.view().installs, 1);
    }

    #[test]
    fn a_stopped_download_carries_on_from_the_bytes_kept() {
        let body = build(1_300_000);
        let (mut u, cx, out) = offered_with(Device::Phone, false, &manifest(&body, ""));
        let mut phone = Phone::serving(body.clone());
        phone.files.insert("update/update-1300.apk.dl".into(), body[..600_000].to_vec());
        phone.run(&mut u, &cx, out);
        assert_eq!(phone.ranges[0], "bytes=600000-1124287");
        assert_eq!(phone.files.get("update/update-1300.apk"), Some(&body));
        assert!(u.view().install.is_some());
    }

    #[test]
    fn a_server_that_ignores_ranges_still_gives_the_whole_file() {
        let body = build(900_000);
        let (mut u, cx, out) = offered_with(Device::Phone, false, &manifest(&body, ""));
        let mut phone = Phone::serving(body.clone());
        phone.ignores_ranges = true;
        phone.run(&mut u, &cx, out);
        assert_eq!(phone.files.get("update/update-1300.apk"), Some(&body));
        assert!(u.view().install.is_some());
    }

    #[test]
    fn without_a_size_it_ends_at_the_short_stretch() {
        let body = build(600_000);
        let m = format!(r#"{{"versionCode": 1300, "versionName": "2.1", "apk": "/app/zunflix.apk", "sha256": "{}"}}"#, hex(&body));
        let (mut u, cx, out) = offered_with(Device::Phone, false, &m);
        let mut phone = Phone::serving(body);
        phone.run(&mut u, &cx, out);
        assert_eq!(phone.ranges.len(), 2);
        assert!(u.view().install.is_some());
    }

    #[test]
    fn a_stretch_that_fails_is_tried_again_then_given_up() {
        let body = build(300_000);
        let (mut u, cx, out) = offered_with(Device::Phone, false, &manifest(&body, ""));
        let mut phone = Phone::serving(body.clone());
        phone.fail = 3;
        phone.run(&mut u, &cx, out);
        assert_eq!(phone.timers, RETRY_MS.to_vec());
        assert!(u.view().install.is_some());
        // (a fourth failure in a row is the end of it, shown, and not tried again by itself)
        let (mut u, cx, out) = offered_with(Device::Phone, false, &manifest(&body, ""));
        let mut phone = Phone::serving(body);
        phone.fail = 4;
        phone.run(&mut u, &cx, out);
        let v = u.view();
        assert_eq!(v.install, None);
        assert_eq!(v.card.map(|c| c.line), Some(FAILED.into()));
        assert_eq!(phone.ranges.len(), 4);
        // (Update asks again, from what's kept)
        let out = u.update(Event::Update, &cx);
        phone.run(&mut u, &cx, out);
        assert!(u.view().install.is_some());
    }

    #[test]
    fn a_wrong_checksum_deletes_the_file_and_fetches_again_once_then_says_so() {
        let body = build(300_000);
        let wrong = manifest(&body, "").replace(&hex(&body), &hex(b"other"));
        let (mut u, cx, out) = offered_with(Device::Phone, false, &wrong);
        let mut phone = Phone::serving(body);
        phone.run(&mut u, &cx, out);
        assert_eq!(phone.ranges.len(), 2);
        assert!(phone.files.is_empty(), "{:?}", phone.files.keys());
        let v = u.view();
        assert_eq!(v.install, None);
        assert_eq!(v.card.map(|c| c.line), Some(DAMAGED.into()));
    }

    #[test]
    fn a_damaged_file_kept_from_before_is_replaced() {
        let body = build(300_000);
        let (mut u, cx, out) = offered_with(Device::Phone, false, &manifest(&body, ""));
        let mut phone = Phone::serving(body.clone());
        phone.files.insert("update/update-1300.apk".into(), vec![9; 300_000]);
        phone.run(&mut u, &cx, out);
        assert_eq!(phone.files.get("update/update-1300.apk"), Some(&body));
        assert!(u.view().install.is_some());
    }

    #[test]
    fn what_an_older_build_left_is_cleared_when_this_one_downloads() {
        let body = build(300_000);
        let (mut u, cx, out) = offered_with(Device::Phone, false, &manifest(&body, ""));
        let mut phone = Phone::serving(body);
        phone.files.insert("update/update-1290.apk".into(), vec![1; 10]);
        phone.files.insert("zunflix-update.exe".into(), vec![1; 10]);
        phone.run(&mut u, &cx, out);
        assert!(!phone.files.contains_key("update/update-1290.apk"));
        assert!(!phone.files.contains_key("zunflix-update.exe"));
    }

    #[test]
    fn up_to_date_clears_the_updates_folder_once() {
        let cx = context(Device::Phone);
        let mut u = Updater::default();
        let (id, _) = request(&u.update(Event::Opened, &cx));
        let out = u.answer(id, response(200, r#"{"versionCode": 1250, "apk": "/a"}"#, None), &cx);
        assert!(matches!(&out[..], [Effect::Disk { op: DiskOp::Remove { paths }, .. }] if paths[0] == FOLDER));
    }

    #[test]
    fn mobile_data_pauses_what_wifi_started_and_wifi_resumes_it() {
        let body = build(1_300_000);
        let (mut u, mut cx, out) = offered_with(Device::Phone, false, &manifest(&body, ""));
        let mut phone = Phone::serving(body.clone());
        let mut todo: VecDeque<Effect> = out.into();
        // (the first stretch goes down; then the connection turns metered)
        while phone.ranges.is_empty() {
            let effect = todo.pop_front().expect("effect");
            todo.extend(phone.step(&mut u, &cx, effect));
        }
        cx.metered = true;
        assert!(u.app(&AppEvent::Connectivity { online: true, metered: true }, &cx).is_empty());
        while let Some(effect) = todo.pop_front() {
            todo.extend(phone.step(&mut u, &cx, effect));
        }
        // (the stretch in flight landed; no more were asked)
        assert_eq!(phone.ranges.len(), 1);
        assert_eq!(u.view().install, None);
        cx.metered = false;
        let out = u.app(&AppEvent::Connectivity { online: true, metered: false }, &cx);
        phone.run(&mut u, &cx, out);
        assert_eq!(phone.files.get("update/update-1300.apk"), Some(&body));
        assert_eq!(phone.ranges[1], "bytes=524288-1048575");
    }

    #[test]
    fn nothing_shows_or_installs_while_something_plays() {
        let body = build(100);
        let (mut u, cx, out) = offered_with(Device::Phone, false, &manifest(&body, ""));
        Phone::serving(body).run(&mut u, &cx, out);
        u.update(Event::Playing { playing: true }, &cx);
        assert_eq!(u.view().card, None);
        u.update(Event::Update, &cx);
        assert_eq!(u.view().installs, 0);
        assert!(u.app(&AppEvent::Lifecycle { foreground: true }, &cx).is_empty());
        u.update(Event::Playing { playing: false }, &cx);
        assert!(u.view().card.is_some());
        assert_eq!(u.view().installs, 0);
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
        assert!(u.view().card.is_some_and(|c| !c.forced && c.later == "Later"));
        u.update(Event::Later, &cx);
        let v = u.view();
        assert_eq!(v.card, None);
        assert_eq!(v.rail.as_deref(), Some("Update to v2.1"));
        u.update(Event::Update, &cx);
        assert!(u.view().card.is_some_and(|c| c.updating));
    }

    #[test]
    fn a_forced_update_has_no_later_and_cant_be_put_away() {
        let manifest = r#"{"versionCode": 1300, "versionName": "2.1", "apk": "/app/zunflix.apk", "min": 1300}"#;
        for device in [Device::Phone, Device::Tv] {
            let mut cx = context(device);
            cx.metered = true;
            let mut u = Updater::default();
            let out = u.update(Event::Opened, &cx);
            let (look, _) = request(&out);
            if device == Device::Tv {
                let settle = timers(&out).into_iter().find(|t| t.1 == TV_SETTLE_MS).expect("settle").0;
                u.answer(settle, Reply::Fired, &cx);
            }
            u.answer(look, response(200, manifest, None), &cx);
            let card = u.view().card.expect("the card");
            assert!(card.forced && card.later.is_empty(), "{device:?}");
            assert_eq!(card.title, REQUIRED);
            assert_eq!(card.line, "v2.1 is needed to keep using zunflix");
            // (Later, a flick, Back: none of them puts it away)
            u.update(Event::Later, &cx);
            let v = u.view();
            assert!(v.card.is_some() && v.rail.is_none(), "{device:?}");
        }
    }

    #[test]
    fn a_forced_card_comes_back_if_the_installer_is_left() {
        let body = build(100);
        let m = manifest(&body, r#", "min": 1300"#);
        let (mut u, cx, out) = offered_with(Device::Phone, false, &m);
        Phone::serving(body).run(&mut u, &cx, out);
        u.update(Event::Update, &cx);
        let v = u.view();
        assert_eq!(v.installs, 1);
        assert!(v.card.is_some_and(|c| c.forced && !c.updating));
        assert_eq!(v.rail, None);
    }

    #[test]
    fn windows_updates_as_it_opens_and_carries_on_if_that_fails() {
        let cx = context(Device::Desktop);
        let mut u = Updater::default();
        let (id, _) = request(&u.update(Event::Opened, &cx));
        assert!(u.view().opening);
        let body = build(300_000);
        let forced = manifest(&body, r#", "min": 1300"#).replace("/app/zunflix.apk", "/app/ZunFlix-Setup.exe");
        let out = u.answer(id, response(200, &forced, None), &cx);
        let v = u.view();
        assert!(v.opening && v.booting);
        assert_eq!(v.installs, 0);
        assert_eq!(v.bootstrap.map(|b| b.title), Some(UPDATING.into()));
        assert_eq!(v.card, None);
        // (it fetches the setup, then running it is the app's)
        Phone::serving(body).run(&mut u, &cx, out);
        let v = u.view();
        assert_eq!(v.installs, 1);
        assert_eq!(v.install.map(|b| b.file), Some("update/update-1300.exe".into()));
        // (running it failed)
        u.update(Event::Failed, &cx);
        let v = u.view();
        assert!(!v.opening && !v.booting);
        assert_eq!(v.note.as_deref(), Some(INSTALL_FAILED));
        assert!(u.update(Event::Opened, &cx).is_empty());
        assert!(!u.view().opening);
    }

    #[test]
    fn windows_carries_on_opening_if_the_download_fails() {
        let cx = context(Device::Desktop);
        let mut u = Updater::default();
        let (id, _) = request(&u.update(Event::Opened, &cx));
        let body = build(300_000);
        let forced = manifest(&body, r#", "min": 1300"#).replace("/app/zunflix.apk", "/app/ZunFlix-Setup.exe");
        let out = u.answer(id, response(200, &forced, None), &cx);
        let mut phone = Phone::serving(body);
        phone.fail = 4;
        phone.run(&mut u, &cx, out);
        let v = u.view();
        assert!(!v.opening && !v.booting && v.bootstrap.is_none());
        assert_eq!(v.note.as_deref(), Some(FAILED));
    }

    #[test]
    fn windows_offers_an_optional_update_on_its_card_as_it_opens() {
        let cx = context(Device::Desktop);
        let mut u = Updater::default();
        let (id, _) = request(&u.update(Event::Opened, &cx));
        let body = build(100);
        // (nothing comes down, the app opens, and the card offers it: Update, or put it away)
        assert!(u.answer(id, response(200, &windows_manifest(&body), None), &cx).is_empty());
        let v = u.view();
        assert!(!v.opening && !v.booting && v.bootstrap.is_none());
        assert!(v.card.is_some_and(|c| !c.forced && c.version == "v2.1"));
        u.update(Event::Later, &cx);
        assert!(u.view().card.is_none());
    }

    #[test]
    fn windows_opens_on_when_up_to_date_or_unreachable() {
        let cx = context(Device::Desktop);
        let mut u = Updater::default();
        let (id, _) = request(&u.update(Event::Opened, &cx));
        u.answer(id, response(200, r#"{"versionCode": 1250, "exe": "/a"}"#, None), &cx);
        let v = u.view();
        assert!(!v.opening);
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
        let body = build(100);
        // (on a PC nothing comes down until Update)
        assert!(u.answer(id, response(200, &windows_manifest(&body), None), &cx).is_empty());
        let v = u.view();
        assert_eq!(v.installs, 0);
        assert_eq!(v.card.map(|c| c.version), Some("v2.1".into()));
        let out = u.update(Event::Update, &cx);
        let v = u.view();
        assert!(v.card.is_none() && v.bootstrap.is_some());
        Phone::serving(body).run(&mut u, &cx, out);
        assert_eq!(u.view().installs, 1);
    }

    #[test]
    fn a_pc_forced_update_mid_session_keeps_its_card_without_later() {
        let cx = context(Device::Desktop);
        let mut u = Updater::default();
        let out = u.update(Event::Opened, &cx);
        let (id, _) = request(&out);
        let (again, _) = timers(&out)[0];
        u.answer(id, response(200, r#"{"versionCode": 1250, "exe": "/a"}"#, None), &cx);
        let (id, _) = request(&u.answer(again, Reply::Fired, &cx));
        let forced = r#"{"versionCode": 1300, "versionName": "2.1", "exe": "/a", "min": 1300}"#;
        assert!(u.answer(id, response(200, forced, None), &cx).is_empty());
        u.update(Event::Later, &cx);
        let card = u.view().card.expect("the card");
        assert!(card.forced && card.later.is_empty());
    }
}
