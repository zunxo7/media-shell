//! The zunflix server as every app calls it: the address, the headers that say who's asking, what each answer means
//! (a page, unchanged, signed out, the server's error, no connection), the words a viewer is shown for it, when a
//! read is asked again, and which failures become error reports. The apps only send the request and hand back the
//! answer (decision 0007).

use crate::screen::{cookies_allowed, header, Body, Device, Header, Method, Reply, Request};
use serde_json::{json, Value};

/// The zunflix server.
pub const BASE: &str = "https://host.invalid";
/// How long a call to the server may take, all of it.
pub const TIMEOUT_MS: u32 = 60_000;
/// How long a read waits before it's asked again (a server restart, the network waking up).
pub const RETRY_WAIT_MS: u32 = 1_500;
/// How many times a read is asked again before its failure is shown.
const RETRIES: u32 = 2;

pub const OFFLINE: &str = "You're offline. Your downloads still work.";
pub const UNREACHABLE: &str = "Couldn't reach zunflix. Try again in a moment.";
pub const SERVER_DOWN: &str = "zunflix couldn't load this just now. Try again in a moment.";

/// The headers every call to the server carries: the server answers a phone, a TV and a PC differently, and counts
/// each run of an app (`stream`) as one screen. `os` is the system and its version ("Android 15"), `version` the app's version name and `build` its build number.
pub fn identity(device: Device, model: &str, os: &str, version: &str, build: i64, stream: &str) -> Vec<Header> {
    let device = match device {
        Device::Phone => "phone",
        Device::Tv => "tv",
        Device::Desktop => "desktop",
    };
    vec![
        Header::new("X-Zunflix", "1"),
        Header::new("X-Zunflix-Stream", stream),
        Header::new("X-Zunflix-Device", device),
        Header::new("X-Zunflix-Model", &model_text(model)),
        Header::new("X-Zunflix-OS", os),
        Header::new("X-Zunflix-Version", version),
        Header::new("X-Zunflix-Build", &build.to_string()),
        // (the server refuses apps without it: those from before 2.0.0)
        Header::new("X-Zunflix-Core", "2"),
    ]
}

/// The headers the crash sender carries (it runs before the core does, and says "android": the server tells those
/// reports from the app's own calls by it).
pub fn report_headers() -> Vec<Header> {
    vec![Header::new("X-Zunflix", "1"), Header::new("X-Zunflix-Device", "android")]
}

/// A server path (or an address already whole) as a whole address.
pub fn url(base: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.into()
    } else if path.starts_with('/') {
        format!("{base}{path}")
    } else {
        format!("{base}/{path}")
    }
}

/// A server path or address from an answer as a whole address; nothing stays nothing.
pub fn address(base: &str, path: &str) -> String {
    if path.is_empty() {
        String::new()
    } else {
        url(base, path)
    }
}

/// The server path of a title's page.
pub fn title_path(id: &str) -> String {
    format!("/api/title/{id}")
}

/// What a title's page is kept under (`store`).
pub fn title_key(id: &str) -> String {
    format!("title-{id}")
}

fn call(base: &str, who: &[Header], method: Method, path: &str, body: Option<Body>) -> Request {
    let url = url(base, path);
    let mut headers = vec![Header::new("Accept", "application/json")];
    // (the sign-in and who's asking go to zunflix only, never to another site)
    let ours = cookies_allowed(base, &url);
    if ours {
        headers.extend(who.iter().cloned());
    }
    Request { method, url, headers, body, cookies: ours, timeout_ms: TIMEOUT_MS }
}

/// A read of `path`; `etag`, the kept copy's, so an unchanged page comes back as a 304.
pub fn get(base: &str, who: &[Header], path: &str, etag: Option<&str>) -> Request {
    let mut request = call(base, who, Method::Get, path, None);
    if let Some(etag) = etag {
        request.headers.push(Header::new("If-None-Match", etag));
    }
    request
}

/// An action: `json` sent to `path`.
pub fn post(base: &str, who: &[Header], path: &str, json: String) -> Request {
    call(base, who, Method::Post, path, Some(Body::Text { text: json, content_type: "application/json".into() }))
}

/// A file sent to `path` as it is (a stretch of a film's sound).
pub fn upload(base: &str, who: &[Header], path: &str, bytes: Vec<u8>, content_type: &str) -> Request {
    call(base, who, Method::Post, path, Some(Body::Bytes { bytes, content_type: content_type.into() }))
}

/// Why a call to the server failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The device has no connection.
    Offline,
    /// Online, but the server couldn't be reached (a timeout, a restart).
    Unreachable,
    /// The server no longer knows this device (removed from the account, signed out elsewhere): the app goes to
    /// sign-in.
    SignedOut,
    /// The server answered with an error (its HTTP status).
    Answered { status: u16 },
}

/// A failed call: why, and the words a viewer is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub kind: Kind,
    pub words: String,
}

impl Failure {
    /// The HTTP status, 0 when the server wasn't reached.
    pub fn status(&self) -> u16 {
        match self.kind {
            Kind::Offline | Kind::Unreachable => 0,
            Kind::SignedOut => 401,
            Kind::Answered { status } => status,
        }
    }

    /// How it counts for the error reports (`stream::worth_reporting`).
    pub fn reported_as(&self) -> crate::stream::Failure {
        match self.status() {
            0 => crate::stream::Failure::Offline,
            status => crate::stream::Failure::Answered(status),
        }
    }
}

/// What the server's answer means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The page or the action's answer, and its ETag when it sent one.
    Body { body: String, etag: Option<String> },
    /// A 304: the kept copy is still right.
    Unchanged,
    Failed { failure: Failure },
}

/// The meaning of the answer to a call to the server; `online`, whether the device has a connection.
pub fn outcome(reply: Reply, online: bool) -> Outcome {
    match reply {
        Reply::Response { status: 304, .. } => Outcome::Unchanged,
        Reply::Response { status: 200..=299, headers, body } => {
            Outcome::Body { etag: header(&headers, "ETag").map(String::from), body }
        }
        Reply::Response { status, body, .. } => Outcome::Failed { failure: answered(status, &body) },
        Reply::Unreachable { offline, .. } if offline || !online => {
            Outcome::Failed { failure: Failure { kind: Kind::Offline, words: OFFLINE.into() } }
        }
        _ => Outcome::Failed { failure: Failure { kind: Kind::Unreachable, words: UNREACHABLE.into() } },
    }
}

/// The server's error, in its own words when it gave some.
fn answered(status: u16, body: &str) -> Failure {
    let said = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("error")?.as_str().filter(|s| !s.trim().is_empty()).map(String::from));
    let words = said.unwrap_or_else(|| {
        if status >= 500 {
            SERVER_DOWN.into()
        } else {
            format!("Something went wrong ({status}).")
        }
    });
    Failure { kind: if status == 401 { Kind::SignedOut } else { Kind::Answered { status } }, words }
}

/// How long to wait before asking again after the `attempt`th failure (from 0), or None to show it: only reads are
/// asked again (an action might have happened), only when the server wasn't reached, and never offline.
pub fn retry_wait(method: Method, failure: &Failure, attempt: u32, online: bool) -> Option<u32> {
    (method == Method::Get && failure.kind == Kind::Unreachable && online && attempt < RETRIES).then_some(RETRY_WAIT_MS)
}

/// A failure that isn't the server's (another site, a file), in words a viewer can use: offline says so, anything
/// else its own words when it has some.
pub fn words(offline: bool, message: &str) -> String {
    if offline {
        OFFLINE.into()
    } else if message.trim().is_empty() {
        UNREACHABLE.into()
    } else {
        message.trim().into()
    }
}

/// Whether a failure that isn't the server's answer means the device is offline: an answer never does; no answer says
/// so itself (`unreached`, when it came that way); any other failure does when the device has no network or the
/// server's name couldn't be looked up.
pub fn offline_after(answered: bool, unreached: Option<bool>, online: bool, host_unknown: bool) -> bool {
    if answered {
        false
    } else if let Some(offline) = unreached {
        offline
    } else {
        !online || host_unknown
    }
}

// ---------- error reports ----------

/// How long the same error from the same place isn't sent again.
const REPORT_QUIET_MS: f64 = 60_000.;
/// The most of a stack a report carries.
const STACK_MOST: usize = 12_000;

/// The error reports sent lately, so one cause looping doesn't flood the server.
#[derive(Debug, Default)]
pub struct Reports {
    sent: Vec<(String, f64)>,
}

impl Reports {
    /// Whether an error of `kind` at `place` is sent now (`now_ms`): not if the same was sent within a minute.
    pub fn due(&mut self, place: &str, kind: &str, now_ms: f64) -> bool {
        self.sent.retain(|(_, at)| now_ms - at < REPORT_QUIET_MS);
        let key = format!("{place}|{kind}");
        if self.sent.iter().any(|(k, _)| *k == key) {
            return false;
        }
        self.sent.push((key, now_ms));
        true
    }
}

/// What one error report says, as `/api/errors` takes it. `place` ("where") is how the server groups reports into
/// issues, so the apps never rename one.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    /// "android" or "windows".
    pub app: String,
    /// "error", "crash", "anr".
    pub kind: String,
    /// The exception's type ("IOException", "Panic").
    pub kind_type: String,
    pub message: String,
    pub stack: String,
    pub place: String,
    pub version: String,
    pub os: String,
    pub device: String,
    /// What the app was doing (a title, an episode), as named values.
    pub context: Vec<Header>,
}

impl Report {
    /// The report of the last run's panic, as a file wrote it ("panicked at src/player.rs:12:5:", the message on the next
    /// line). Release builds are stripped, so the backtrace has no names: the panic's file and its message without
    /// digits (they change run to run) identify it.
    pub fn crash(text: &str, app: &str, version: &str, os: &str, device: &str) -> Report {
        let (head, rest) = text.split_once('\n').unwrap_or((text, ""));
        let file = head.trim_start_matches("panicked at ").split(':').next().unwrap_or("");
        let message = rest.lines().next().unwrap_or("").trim();
        let gist: String = message.chars().filter(|c| !c.is_ascii_digit()).take(60).collect();
        Report {
            app: app.into(),
            kind: "crash".into(),
            kind_type: "Panic".into(),
            message: message.into(),
            stack: text.into(),
            place: format!("{file}: {gist}"),
            version: version.into(),
            os: os.into(),
            device: device.into(),
            context: vec![],
        }
    }
}

/// The report as `/api/errors` takes it.
pub fn report_body(report: &Report) -> String {
    let stack: String = report.stack.chars().take(STACK_MOST).collect();
    let mut body = json!({
        "app": report.app, "kind": report.kind, "type": report.kind_type, "message": report.message, "stack": stack,
        "where": report.place, "version": report.version, "os": report.os, "device": report.device,
    });
    if !report.context.is_empty() {
        let context: serde_json::Map<String, Value> =
            report.context.iter().map(|h| (h.name.clone(), Value::String(h.value.clone()))).collect();
        body["context"] = Value::Object(context);
    }
    body.to_string()
}

/// How the apps' connections are set up, the same on every app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transport {
    /// How long to wait for a connection.
    pub connect_ms: u32,
    /// How long a connection may be silent mid-answer.
    pub read_ms: u32,
    /// The same for pictures, which are only worth a short wait.
    pub image_read_ms: u32,
    /// Connections a file download may use at once, one each (over one shared connection a source's single stream
    /// is as fast as it goes, about 0.4 MB/s; separate ones add up).
    pub file_connections: u32,
    /// How long an idle connection is kept for the next request (seconds).
    pub pool_idle_secs: u32,
}

/// How a window without a picture library fetches and decodes pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PictureRules {
    /// How long a card's picture may take, whole (a manhwa page's size is given [`PictureRules::page_wait_ms`]).
    pub wait_ms: u32,
    /// How long a manhwa page's picture may take: it runs to a few MB.
    pub page_wait_ms: u32,
    /// Pictures fetched at once.
    pub fetches: usize,
    /// Pictures that failed are asked for again after this many seconds.
    pub retry_secs: u64,
    /// How many failed pictures are remembered before the old ones are let go.
    pub failures_most: usize,
}

/// The pictures' settings for a window that fetches them itself.
pub const fn picture_rules() -> PictureRules {
    PictureRules { wait_ms: 8_000, page_wait_ms: 60_000, fetches: 16, retry_secs: 30, failures_most: 256 }
}

/// How many pictures are decoded at once on a machine with `cores`: it leaves the rest to drawing.
pub fn picture_decodes(cores: usize) -> usize {
    cores.clamp(2, 4)
}

/// The apps' connection settings.
pub const fn transport() -> Transport {
    Transport { connect_ms: 15_000, read_ms: 60_000, image_read_ms: 30_000, file_connections: 48, pool_idle_secs: 300 }
}

/// The most of a device's model the server is told.
const MODEL_MOST: usize = 40;
/// The most of an ANR's stack a report carries.
const ANR_STACK_MOST: usize = 8_000;

/// The app's version as the reports and the server's device list show it: "2.0.3 (9000)".
pub fn version_text(version: &str, build: i64) -> String {
    format!("{version} ({build})")
}

/// An Android system as the server lists it: "Android 15 (API 35)".
pub fn android_os(release: &str, sdk: i64) -> String {
    format!("Android {release} (API {sdk})")
}

/// A device's model as sent in the identity headers: cut to what the server keeps.
fn model_text(model: &str) -> String {
    model.chars().take(MODEL_MOST).collect()
}

/// A device as the error reports name it: "Google Pixel 8", and "(TV)" on a TV.
pub fn report_device(manufacturer: &str, model: &str, tv: bool) -> String {
    format!("{manufacturer} {model}{}", if tv { " (TV)" } else { "" })
}

/// How an earlier run of the app ended, when it wasn't a plain exit or a crash the apps catch themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitKind {
    /// The app froze (an ANR).
    Anr,
    /// A crash in native code, which no in-process handler can catch.
    NativeCrash,
}

/// The main thread's stack from an ANR's thread dump: where it was stuck.
pub fn anr_stack(dump: &str) -> String {
    let Some(start) = dump.find("\"main\"") else { return dump.chars().take(ANR_STACK_MOST).collect() };
    let end = dump[start..].find("

").filter(|e| *e > 0).map_or(dump.len(), |e| start + e);
    dump[start..end].chars().take(ANR_STACK_MOST).collect()
}

/// The report of an earlier run's end. Always at the place "exit", one issue group on the server.
/// How many of the system's kept past exits are looked at.
pub const EXITS_LOOKED_AT: u32 = 5;

/// One of the system's kept past exits: when it was (ms since 1970) and, when it was one worth a report, which.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PastExit {
    pub at: i64,
    pub kind: Option<ExitKind>,
}

/// Which past exits to report, and where to remember having looked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitPlan {
    /// The newest exit seen so far (kept, so each is sent once).
    pub seen: i64,
    /// The positions in the list of the exits to report: those after `seen` that were worth it.
    pub send: Vec<u32>,
}

/// The past exits (newest first or not) as reports: each one after the last looked at, and only a freeze or a native
/// crash (the rest are normal exits, and JVM crashes come through the crash sender).
pub fn exits_plan(seen: i64, exits: &[PastExit]) -> ExitPlan {
    let newest = exits.iter().map(|e| e.at).max().map_or(seen, |at| at.max(seen));
    let send = exits.iter().enumerate().filter(|(_, e)| e.at > seen && e.kind.is_some()).map(|(i, _)| i as u32).collect();
    ExitPlan { seen: newest, send }
}

pub fn exit_report(kind: ExitKind, description: &str, dump: &str, app: &str, version: &str, os: &str, device: &str) -> Report {
    let (kind_name, kind_type, stack) = match kind {
        ExitKind::Anr => ("anr", "ANR", anr_stack(dump)),
        ExitKind::NativeCrash => ("crash", "NativeCrash", String::new()),
    };
    Report {
        app: app.into(),
        kind: kind_name.into(),
        kind_type: kind_type.into(),
        message: description.into(),
        stack,
        place: "exit".into(),
        version: version.into(),
        os: os.into(),
        device: device.into(),
        context: vec![],
    }
}

/// While the app is offline, a shell with no OS word of the network asks the server for this (a small file) every
/// [`PROBE_EVERY_MS`]; an answer means it's back.
pub const PROBE_PATH: &str = "/app/version.json";
/// How often the offline probe asks.
pub const PROBE_EVERY_MS: u32 = 8_000;

/// Where error reports go.
pub const REPORTS_PATH: &str = "/api/errors";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crash_sender_and_pictures_are_set_here() {
        let names: Vec<_> = report_headers().into_iter().map(|h| (h.name, h.value)).collect();
        assert_eq!(names, [("X-Zunflix".into(), "1".into()), ("X-Zunflix-Device".into(), "android".into())]);
        assert_eq!((picture_decodes(1), picture_decodes(8)), (2, 4));
        assert_eq!(transport().pool_idle_secs, 300);
    }

    #[test]
    fn past_exits_are_reported_once_and_only_freezes_and_native_crashes() {
        let exit = |at, kind| PastExit { at, kind };
        let exits = [exit(30, None), exit(20, Some(ExitKind::Anr)), exit(10, Some(ExitKind::NativeCrash))];
        assert_eq!(exits_plan(0, &exits), ExitPlan { seen: 30, send: vec![1, 2] });
        assert_eq!(exits_plan(10, &exits), ExitPlan { seen: 30, send: vec![1] });
        assert_eq!(exits_plan(50, &exits), ExitPlan { seen: 50, send: vec![] });
        assert_eq!(exits_plan(7, &[]), ExitPlan { seen: 7, send: vec![] });
    }

    #[test]
    fn what_counts_as_offline() {
        assert!(!offline_after(true, None, false, true));
        assert!(offline_after(false, Some(true), true, false));
        assert!(!offline_after(false, Some(false), false, true));
        assert!(offline_after(false, None, false, false));
        assert!(offline_after(false, None, true, true));
        assert!(!offline_after(false, None, true, false));
    }

    #[test]
    fn texts_for_the_server_and_the_reports() {
        assert_eq!(version_text("2.0.3", 9000), "2.0.3 (9000)");
        assert_eq!(android_os("15", 35), "Android 15 (API 35)");
        assert_eq!(report_device("Google", "Pixel 8", false), "Google Pixel 8");
        assert_eq!(report_device("Nvidia", "Shield", true), "Nvidia Shield (TV)");
        assert_eq!(model_text(&"x".repeat(60)).len(), 40);
    }

    #[test]
    fn an_anr_reports_the_main_threads_stack() {
        let dump = "\"Binder:1\" prio=5
  at a

\"main\" prio=5
  at b
  at c

\"other\"
  at d";
        assert_eq!(anr_stack(dump), "\"main\" prio=5
  at b
  at c");
        assert_eq!(anr_stack("no main here"), "no main here");
        let report = exit_report(ExitKind::Anr, "why", dump, "android", "1 (2)", "Android 15", "dev");
        assert_eq!((report.kind.as_str(), report.kind_type.as_str(), report.place.as_str()), ("anr", "ANR", "exit"));
        let native = exit_report(ExitKind::NativeCrash, "sig", "", "android", "1 (2)", "os", "dev");
        assert_eq!((native.kind.as_str(), native.kind_type.as_str(), native.stack.as_str()), ("crash", "NativeCrash", ""));
    }

    fn who() -> Vec<Header> {
        identity(Device::Tv, "Shield", "Android 15", "2.0.2", 14, "s1")
    }

    fn reply(status: u16, body: &str) -> Reply {
        Reply::Response { status, headers: vec![Header::new("etag", "e2")], body: body.into() }
    }

    #[test]
    fn a_crash_is_grouped_by_its_file_and_message() {
        let text = "panicked at src/player.rs:12:5:
called unwrap on 42 items
stack";
        let r = Report::crash(text, "windows", "2.0 (3)", "Windows 11", "PC");
        assert_eq!((r.kind.as_str(), r.kind_type.as_str(), r.message.as_str()), ("crash", "Panic", "called unwrap on 42 items"));
        assert_eq!(r.place, "src/player.rs: called unwrap on  items");
        assert_eq!(r.stack, text);
    }

    #[test]
    fn addresses_and_title_pages() {
        let b = "https://host.invalid";
        assert_eq!(address(b, ""), "");
        assert_eq!(address(b, "/a.png"), "https://host.invalid/a.png");
        assert_eq!(address(b, "a.png"), "https://host.invalid/a.png");
        assert_eq!(address(b, "https://x.y/a"), "https://x.y/a");
        assert_eq!((title_path("t1").as_str(), title_key("t1").as_str()), ("/api/title/t1", "title-t1"));
    }

    #[test]
    fn calls_to_zunflix_say_who_asks_and_carry_the_sign_in() {
        let r = get(BASE, &who(), "/api/home", Some("e1"));
        assert_eq!(r.url, "https://host.invalid/api/home");
        assert!(r.cookies);
        assert_eq!(header(&r.headers, "x-zunflix-device"), Some("tv"));
        assert_eq!(header(&r.headers, "X-Zunflix-Stream"), Some("s1"));
        assert_eq!(header(&r.headers, "X-Zunflix-Version"), Some("2.0.2"));
        assert_eq!(header(&r.headers, "X-Zunflix-Build"), Some("14"));
        assert_eq!(header(&r.headers, "X-Zunflix-OS"), Some("Android 15"));
        assert_eq!(header(&r.headers, "If-None-Match"), Some("e1"));
        assert_eq!(r.timeout_ms, TIMEOUT_MS);
        let p = post(BASE, &who(), "api/x", "{}".into());
        assert_eq!((p.method, p.url.as_str()), (Method::Post, "https://host.invalid/api/x"));
    }

    #[test]
    fn other_sites_never_get_the_sign_in_or_who_asks() {
        for to in ["https://host.invalid.evil.com/x", "https://cdn.example/x", "http://host.invalid/x"] {
            let r = get(BASE, &who(), to, None);
            assert!(!r.cookies, "{to}");
            assert_eq!(header(&r.headers, "X-Zunflix-Stream"), None, "{to}");
        }
    }

    #[test]
    fn answers_mean_what_the_server_says() {
        assert_eq!(outcome(reply(200, "ok"), true), Outcome::Body { body: "ok".into(), etag: Some("e2".into()) });
        assert_eq!(outcome(reply(304, ""), true), Outcome::Unchanged);
        let gone = Failure { kind: Kind::SignedOut, words: "Sign in again.".into() };
        assert_eq!(outcome(reply(401, r#"{"error":"Sign in again."}"#), true), Outcome::Failed { failure: gone });
        let full = Failure { kind: Kind::Answered { status: 429 }, words: "Something went wrong (429).".into() };
        assert_eq!(outcome(reply(429, "<html>"), true), Outcome::Failed { failure: full });
        let down = Failure { kind: Kind::Answered { status: 502 }, words: SERVER_DOWN.into() };
        assert_eq!(outcome(reply(502, ""), true), Outcome::Failed { failure: down });
    }

    #[test]
    fn no_answer_is_offline_or_unreachable_in_plain_words() {
        let cut = |offline| Reply::Unreachable { offline, message: "Failed to connect to host.invalid/1.2.3.4".into() };
        let offline = Failure { kind: Kind::Offline, words: OFFLINE.into() };
        assert_eq!(outcome(cut(true), true), Outcome::Failed { failure: offline.clone() });
        assert_eq!(outcome(cut(false), false), Outcome::Failed { failure: offline.clone() });
        let away = Failure { kind: Kind::Unreachable, words: UNREACHABLE.into() };
        assert_eq!(outcome(cut(false), true), Outcome::Failed { failure: away.clone() });
        assert_eq!((offline.status(), away.status()), (0, 0));
        assert_eq!(away.reported_as(), crate::stream::Failure::Offline);
    }

    #[test]
    fn only_reads_are_asked_again_twice_and_never_offline() {
        let away = Failure { kind: Kind::Unreachable, words: String::new() };
        assert_eq!(retry_wait(Method::Get, &away, 0, true), Some(RETRY_WAIT_MS));
        assert_eq!(retry_wait(Method::Get, &away, 1, true), Some(RETRY_WAIT_MS));
        assert_eq!(retry_wait(Method::Get, &away, 2, true), None);
        assert_eq!(retry_wait(Method::Post, &away, 0, true), None);
        assert_eq!(retry_wait(Method::Get, &away, 0, false), None);
        let answered = Failure { kind: Kind::Answered { status: 500 }, words: String::new() };
        assert_eq!(retry_wait(Method::Get, &answered, 0, true), None);
    }

    #[test]
    fn other_failures_in_their_own_words() {
        assert_eq!(words(true, "dulo answered 403"), OFFLINE);
        assert_eq!(words(false, "dulo answered 403"), "dulo answered 403");
        assert_eq!(words(false, "  "), UNREACHABLE);
    }

    #[test]
    fn the_same_error_is_sent_once_a_minute() {
        let mut reports = Reports::default();
        assert!(reports.due("home", "IOException", 0.));
        assert!(!reports.due("home", "IOException", 59_000.));
        assert!(reports.due("home", "ApiError", 59_000.));
        assert!(reports.due("home", "IOException", 60_000.));
    }

    #[test]
    fn a_report_keeps_its_place_and_a_short_stack() {
        let report = Report {
            app: "windows".into(),
            kind: "error".into(),
            kind_type: "Panic".into(),
            message: "m".into(),
            stack: "x".repeat(20_000),
            place: "player: load".into(),
            version: "2.0 (13)".into(),
            os: "Windows".into(),
            device: "App on Windows".into(),
            context: vec![Header::new("title", "t1")],
        };
        let body: Value = serde_json::from_str(&report_body(&report)).unwrap_or_default();
        assert_eq!(body["where"], "player: load");
        assert_eq!(body["type"], "Panic");
        assert_eq!(body["stack"].as_str().map(str::len), Some(STACK_MOST));
        assert_eq!(body["context"]["title"], "t1");
        let bare = Report { context: vec![], ..report };
        let body: Value = serde_json::from_str(&report_body(&bare)).unwrap_or_default();
        assert!(body.get("context").is_none());
    }
}
