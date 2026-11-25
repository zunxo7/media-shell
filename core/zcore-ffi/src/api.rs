//! The zunflix server's API as Kotlin calls it (`zcore::api`): the requests, what each answer means and its words,
//! when a read is asked again, and the error reports' rules.

use std::sync::{Mutex, PoisonError};
use zcore::api::{self, ExitKind, ExitPlan, PastExit, Transport, Failure as ApiFailure, Kind as ApiFailureKind, Outcome as ApiOutcome, Report as ErrorReport};
use zcore::screen::{Device as DeviceKind, Header as HttpHeader, Method as HttpMethod, Reply, Request as HttpRequest};

#[uniffi::remote(Enum)]
pub enum ApiFailureKind {
    Offline,
    Unreachable,
    SignedOut,
    Answered { status: u16 },
}

#[uniffi::remote(Record)]
pub struct ApiFailure {
    pub kind: ApiFailureKind,
    pub words: String,
}

#[uniffi::remote(Enum)]
pub enum ApiOutcome {
    Body { body: String, etag: Option<String> },
    Unchanged,
    Failed { failure: ApiFailure },
}

#[uniffi::remote(Record)]
pub struct Transport {
    pub connect_ms: u32,
    pub read_ms: u32,
    pub image_read_ms: u32,
    pub file_connections: u32,
    pub pool_idle_secs: u32,
}

#[uniffi::remote(Enum)]
pub enum ExitKind {
    Anr,
    NativeCrash,
}

#[uniffi::remote(Record)]
pub struct PastExit {
    pub at: i64,
    pub kind: Option<ExitKind>,
}

#[uniffi::remote(Record)]
pub struct ExitPlan {
    pub seen: i64,
    pub send: Vec<u32>,
}

#[uniffi::remote(Record)]
pub struct ErrorReport {
    pub app: String,
    pub kind: String,
    pub kind_type: String,
    pub message: String,
    pub stack: String,
    pub place: String,
    pub version: String,
    pub os: String,
    pub device: String,
    pub context: Vec<HttpHeader>,
}

/// The zunflix server (`zcore::api::BASE`).
#[uniffi::export]
pub fn api_base() -> String {
    api::BASE.into()
}

/// A server path or address from an answer as a whole address under `base`; nothing stays nothing (`zcore::api::address`).
#[uniffi::export]
pub fn api_address(base: String, path: String) -> String {
    api::address(&base, &path)
}

/// The headers every call to the server carries (`zcore::api::identity`).
#[uniffi::export]
pub fn api_identity(device: DeviceKind, model: String, os: String, version: String, build: i64, stream: String) -> Vec<HttpHeader> {
    api::identity(device, &model, &os, &version, build, &stream)
}

/// A read of a server path (`zcore::api::get`).
#[uniffi::export]
pub fn api_get(base: String, who: Vec<HttpHeader>, path: String, etag: Option<String>) -> HttpRequest {
    api::get(&base, &who, &path, etag.as_deref())
}

/// An action: JSON sent to a server path (`zcore::api::post`).
#[uniffi::export]
pub fn api_post(base: String, who: Vec<HttpHeader>, path: String, json: String) -> HttpRequest {
    api::post(&base, &who, &path, json)
}

/// What the server's answer means (`zcore::api::outcome`).
#[uniffi::export]
pub fn api_outcome(reply: Reply, online: bool) -> ApiOutcome {
    api::outcome(reply, online)
}

/// How long to wait before asking again, or None to show the failure (`zcore::api::retry_wait`).
#[uniffi::export]
pub fn api_retry_wait(method: HttpMethod, failure: ApiFailure, attempt: u32, online: bool) -> Option<u32> {
    api::retry_wait(method, &failure, attempt, online)
}

/// A failure that isn't the server's, in words a viewer can use (`zcore::api::words`).
#[uniffi::export]
pub fn api_words(offline: bool, message: String) -> String {
    api::words(offline, &message)
}

/// Whether a failure means the device is offline (`zcore::api::offline_after`).
#[uniffi::export]
pub fn api_offline_after(answered: bool, unreached: Option<bool>, online: bool, host_unknown: bool) -> bool {
    api::offline_after(answered, unreached, online, host_unknown)
}

/// How many of the system's past exits are looked at (`zcore::api::EXITS_LOOKED_AT`).
#[uniffi::export]
pub fn api_exits_looked_at() -> u32 {
    api::EXITS_LOOKED_AT
}

/// Which past exits to report (`zcore::api::exits_plan`).
#[uniffi::export]
pub fn api_exits_plan(seen: i64, exits: Vec<PastExit>) -> ExitPlan {
    api::exits_plan(seen, &exits)
}

/// Where error reports go (`zcore::api::REPORTS_PATH`).
#[uniffi::export]
pub fn api_reports_path() -> String {
    api::REPORTS_PATH.into()
}

/// An error report as `/api/errors` takes it (`zcore::api::report_body`).
#[uniffi::export]
pub fn api_report_body(report: ErrorReport) -> String {
    api::report_body(&report)
}

/// The headers the crash sender carries (`zcore::api::report_headers`).
#[uniffi::export]
pub fn api_report_headers() -> Vec<HttpHeader> {
    api::report_headers()
}

/// How connections are set up (`zcore::api::transport`).
#[uniffi::export]
pub fn api_transport() -> Transport {
    api::transport()
}

/// The app's version as reports and the server show it (`zcore::api::version_text`).
#[uniffi::export]
pub fn api_version_text(version: String, build: i64) -> String {
    api::version_text(&version, build)
}

/// An Android system as the server lists it (`zcore::api::android_os`).
#[uniffi::export]
pub fn api_android_os(release: String, sdk: i64) -> String {
    api::android_os(&release, sdk)
}

/// A device as the error reports name it (`zcore::api::report_device`).
#[uniffi::export]
pub fn api_report_device(manufacturer: String, model: String, tv: bool) -> String {
    api::report_device(&manufacturer, &model, tv)
}

/// The report of an earlier run's end (`zcore::api::exit_report`).
#[uniffi::export]
#[allow(clippy::too_many_arguments)]
pub fn api_exit_report(kind: ExitKind, description: String, dump: String, app: String, version: String, os: String, device: String) -> ErrorReport {
    api::exit_report(kind, &description, &dump, &app, &version, &os, &device)
}

/// The error reports sent lately (`zcore::api::Reports`).
#[derive(Debug, Default, uniffi::Object)]
pub struct ErrorReports(Mutex<api::Reports>);

#[uniffi::export]
impl ErrorReports {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether an error of `kind` at `place` is sent now: not if the same was sent within a minute.
    pub fn due(&self, place: String, kind: String, now_ms: f64) -> bool {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).due(&place, &kind, now_ms)
    }
}

/// The sign-in's cookies (`zcore::cookies::Jar`): the app keeps what `take` and `clear` give back.
#[derive(Debug, Default, uniffi::Object)]
pub struct SignInCookies(Mutex<zcore::cookies::Jar>);

#[uniffi::export]
impl SignInCookies {
    /// The jar a saved text describes (None: an empty one).
    #[uniffi::constructor]
    pub fn new(saved: Option<String>) -> Self {
        Self(Mutex::new(saved.map(|s| zcore::cookies::Jar::load(&s)).unwrap_or_default()))
    }

    /// Takes an answer's `Set-Cookie` lines; the text to keep when the jar changed.
    pub fn take(&self, url: String, set_cookies: Vec<String>, now_ms: i64) -> Option<String> {
        let mut jar = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        jar.take(&url, &set_cookies, now_ms).then(|| jar.save())
    }

    /// The `Cookie` header a request to `url` carries.
    pub fn header(&self, url: String, now_ms: i64) -> Option<String> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).header(&url, now_ms)
    }
}
