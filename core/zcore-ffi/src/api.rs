//! The zunflix server's API as Kotlin calls it (`zcore::api`): the requests, what each answer means and its words,
//! when a read is asked again, and the error reports' rules.

use std::sync::{Mutex, PoisonError};
use zcore::api::{self, Failure as ApiFailure, Kind as ApiFailureKind, Outcome as ApiOutcome, Report as ErrorReport};
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
