//! The zunflix server, as the website talks to it, and what's kept on this PC.
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// This build's version and build number. The Mac has its own (MAC_NAME, MAC_BUILD), released on its own
/// (tools/release.sh mac, app/mac.json), so a fix for one never ships to the other.
#[cfg(not(target_os = "macos"))]
pub const VERSION: &str = "1.0.0";
#[cfg(not(target_os = "macos"))]
pub const VERSION_CODE: i64 = 9000;
#[cfg(target_os = "macos")]
pub const VERSION: &str = MAC_NAME;
#[cfg(target_os = "macos")]
pub const VERSION_CODE: i64 = MAC_BUILD;
#[cfg(target_os = "macos")]
const MAC_NAME: &str = "1.0.0";
#[cfg(target_os = "macos")]
const MAC_BUILD: i64 = 4;
pub use crate::platform::{os_name, MODEL};

#[derive(Debug, Clone)]
pub struct ApiError {
    pub message: String,
    pub status: u16,
}
impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
pub type Res<T> = Result<T, ApiError>;

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

/// Where zunflix keeps its things on this computer (`install::data_dir`), or `ZUNFLIX_HOME`.
pub fn root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let p = std::env::var("ZUNFLIX_HOME").map(PathBuf::from).unwrap_or_else(|_| crate::platform::install::data_dir());
        let _ = std::fs::create_dir_all(&p);
        p
    })
    .clone()
}

/// Where what can be fetched again is kept (`install::cache_dir`), inside `ZUNFLIX_HOME` when that's set.
pub fn cache_root() -> PathBuf {
    match std::env::var("ZUNFLIX_HOME") {
        Ok(home) => PathBuf::from(home).join("Cache"),
        Err(_) => crate::platform::install::cache_dir(),
    }
}
pub fn downloads_dir() -> PathBuf {
    let p = root().join("Downloads");
    let _ = std::fs::create_dir_all(&p);
    p
}

static WAITING: Mutex<Option<std::collections::HashMap<PathBuf, String>>> = Mutex::new(None);
static WRITING: Mutex<()> = Mutex::new(());

/// A small file written off the window's thread half a second after it last changed: the latest text wins, one
/// write for a burst (the volume dragged, a film's spot every few seconds). `flush_files` writes any still waiting.
pub fn write_soon(path: PathBuf, text: String) {
    let first = {
        let mut waiting = WAITING.lock().unwrap();
        let waiting = waiting.get_or_insert_with(Default::default);
        let first = waiting.is_empty();
        waiting.insert(path, text);
        first
    };
    if first {
        crate::spawn(async {
            tokio::time::sleep(Duration::from_millis(zcore::cache::WRITE_SOON_MS)).await;
            let _ = tokio::task::spawn_blocking(flush_files).await;
        });
    }
}

/// Every file still waiting to be written, written now (and at the app's end).
pub fn flush_files() {
    let _writing = WRITING.lock().unwrap();
    let all = WAITING.lock().unwrap().take().unwrap_or_default();
    for (path, text) in all {
        let _ = atomically(&path, &text);
    }
}

/// A cache folder kept to `most` bytes: past it, the files changed longest ago go first (`zcore::cache::evict`).
pub fn trim_folder(dir: &std::path::Path, most: u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let files: Vec<(String, u64, f64)> = rd.flatten().filter_map(|e| {
        let m = e.metadata().ok()?;
        let at = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs_f64();
        Some((e.path().to_string_lossy().into_owned(), m.len(), at))
    }).collect();
    for path in zcore::cache::evict(&files, most) {
        let _ = std::fs::remove_file(path);
    }
}

/// Written whole or not at all: a crash halfway leaves the last good copy.
pub fn atomically(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("{}tmp", path.extension().and_then(|e| e.to_str()).map(|e| format!("{e}.")).unwrap_or_default()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

pub fn base() -> &'static str {
    static BASE: OnceLock<String> = OnceLock::new();
    BASE.get_or_init(|| std::env::var("ZUNFLIX_BASE").unwrap_or_else(|_| zcore::api::BASE.into()).trim_end_matches('/').to_string())
}
// ---------- the sign-in ----------
// Which cookies are taken, sent and kept is the core's (`zcore::cookies`); this holds the jar and keeps its text.
static JAR: Mutex<Option<zcore::cookies::Jar>> = Mutex::new(None);

fn jar_file() -> PathBuf {
    root().join("cookies.json")
}
fn with_jar<T>(f: impl FnOnce(&mut zcore::cookies::Jar) -> T) -> T {
    let mut jar = JAR.lock().unwrap();
    f(jar.get_or_insert_with(|| zcore::cookies::Jar::load(&std::fs::read_to_string(jar_file()).unwrap_or_default())))
}
fn take_cookies(url: &str, headers: &reqwest::header::HeaderMap) {
    let lines: Vec<String> = headers.get_all(reqwest::header::SET_COOKIE).iter().filter_map(|v| v.to_str().ok().map(String::from)).collect();
    if lines.is_empty() {
        return;
    }
    with_jar(|jar| {
        if jar.take(url, &lines, now_ms()) {
            let _ = atomically(&jar_file(), &jar.save());
        }
    });
}

fn cookie_header(url: &str) -> Option<String> {
    with_jar(|jar| jar.header(url, now_ms()))
}

/// The sign-in, for mpv: it asks for what the server relays itself, and has no cookie jar of its own. Never sent anywhere else.
pub fn cookie_for(url: &str) -> Option<String> {
    if !zcore::screen::cookies_allowed(base(), url) {
        return None;
    }
    cookie_header(url)
}

// ---------- asking ----------
fn client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        let wire = zcore::api::transport();
        reqwest::Client::builder().connect_timeout(Duration::from_millis(wire.connect_ms.into())).read_timeout(Duration::from_millis(wire.read_ms.into())).pool_idle_timeout(Duration::from_secs(wire.pool_idle_secs.into())).build().unwrap()
    })
}

pub fn clear_session() {
    with_jar(|jar| {
        jar.clear();
        let _ = atomically(&jar_file(), &jar.save());
    });
}

pub fn plain() -> &'static reqwest::Client {
    client()
}

pub fn stream_id() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    S.get_or_init(|| format!("{:x}{:x}", now_ms(), std::process::id()))
}

/// Who's asking, as every call to the server says (`zcore::api::identity`).
fn who() -> &'static [zcore::screen::Header] {
    static WHO: OnceLock<Vec<zcore::screen::Header>> = OnceLock::new();
    WHO.get_or_init(|| zcore::api::identity(zcore::screen::Device::Desktop, MODEL, os_name(), VERSION, VERSION_CODE, stream_id()))
}

fn failed(failure: &zcore::api::Failure) -> ApiError {
    if failure.kind == zcore::api::Kind::SignedOut {
        crate::ui(|app| app.signed_out());
    }
    ApiError { message: failure.words.clone(), status: failure.status() }
}

/// The server's answer as text, by the core's rules: what counts as failing, its words, and which failed reads are
/// asked again.
pub async fn raw(path: &str) -> Res<String> {
    let request = zcore::api::get(base(), who(), path, None);
    let mut attempt = 0;
    loop {
        match zcore::api::outcome(send(request.clone()).await, true) {
            zcore::api::Outcome::Body { body, .. } => return Ok(body),
            zcore::api::Outcome::Unchanged => return Ok(String::new()),
            zcore::api::Outcome::Failed { failure } => {
                let Some(wait) = zcore::api::retry_wait(request.method, &failure, attempt, true) else {
                    return Err(failed(&failure));
                };
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(wait.into())).await;
            }
        }
    }
}

/// Whether zunflix could be reached the last time it was tried; the core is told when that changes (it says so on
/// every screen, and Home asks again once it's back).
pub fn reached(online: bool) {
    static ONLINE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
    if ONLINE.swap(online, std::sync::atomic::Ordering::Relaxed) != online {
        crate::core_screen::app(zcore::screen::AppEvent::Connectivity { online, metered: false });
        if !online {
            // (back without the viewer pressing Retry: a small request every few seconds until one gets through, which
            // `send` reports as reached)
            crate::spawn(async {
                while !ONLINE.load(std::sync::atomic::Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(zcore::api::PROBE_EVERY_MS.into())).await;
                    let _ = send(zcore::api::get(base(), who(), zcore::api::PROBE_PATH, None)).await;
                }
            });
        }
    }
}

/// A request as the core built it, sent as it is: with this PC's sign-in only when it says so.
pub async fn send(request: zcore::screen::Request) -> zcore::screen::Reply {
    // (only zunflix's own tell whether it can be reached: a source's being down is not being offline)
    let ours = request.url.starts_with(base());
    let response = match call(request).await {
        Ok(response) => response,
        Err(e) => {
            if ours && e.is_connect() {
                reached(false);
            }
            return unreachable(&e);
        }
    };
    if ours {
        reached(true);
    }
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| Some(zcore::screen::Header { name: name.as_str().into(), value: value.to_str().ok()?.into() }))
        .collect();
    match response.text().await {
        Ok(body) => zcore::screen::Reply::Response { status, headers, body },
        Err(e) => unreachable(&e),
    }
}

/// As [`send`], the body written to `file` (under this app's folder) as it comes, and only for a 2xx.
pub async fn fetch(request: zcore::screen::Request, file: &str) -> zcore::screen::Reply {
    use tokio::io::AsyncWriteExt;
    let mut response = match call_with(request, true).await {
        Ok(response) => response,
        Err(e) => return unreachable(&e),
    };
    let status = response.status().as_u16();
    if !response.status().is_success() {
        return zcore::screen::Reply::Saved { status, bytes: 0 };
    }
    let to = crate::disk::path(file);
    // (written beside it first, so a fetch cut short never leaves half a file under the name)
    let part = PathBuf::from(format!("{}.part", to.display()));
    let failed = |message: String| zcore::screen::Reply::Unreachable { offline: false, message };
    if let Some(dir) = to.parent() {
        let _ = tokio::fs::create_dir_all(dir).await;
    }
    let mut out = match tokio::fs::File::create(&part).await {
        Ok(out) => out,
        Err(e) => return failed(e.to_string()),
    };
    let mut bytes = 0u64;
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if let Err(e) = out.write_all(&chunk).await {
                    return failed(e.to_string());
                }
                bytes += chunk.len() as u64;
            }
            Ok(None) => break,
            Err(e) => return unreachable(&e),
        }
    }
    if let Err(e) = out.flush().await {
        return failed(e.to_string());
    }
    drop(out);
    match tokio::fs::rename(&part, &to).await {
        Ok(()) => zcore::screen::Reply::Saved { status, bytes },
        Err(e) => failed(e.to_string()),
    }
}

async fn call(r: zcore::screen::Request) -> reqwest::Result<reqwest::Response> {
    call_with(r, false).await
}

/// What file fetches go through: HTTP/1.1, a connection each (over HTTP/2 a download's pieces shared one connection,
/// and one is as fast as a source lets a single stream go, ~0.4 MB/s; separate ones add up).
fn files_client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        let wire = zcore::api::transport();
        reqwest::Client::builder().http1_only().pool_max_idle_per_host(wire.file_connections as usize).connect_timeout(Duration::from_millis(wire.connect_ms.into())).read_timeout(Duration::from_millis(wire.read_ms.into())).build().unwrap()
    })
}

async fn call_with(r: zcore::screen::Request, file: bool) -> reqwest::Result<reqwest::Response> {
    use zcore::screen::{Body, Method};
    let method = match r.method {
        Method::Get => reqwest::Method::GET,
        Method::Post => reqwest::Method::POST,
        Method::Put => reqwest::Method::PUT,
        Method::Delete => reqwest::Method::DELETE,
    };
    let pipe = if file && !r.cookies { files_client() } else { client() };
    let mut b = pipe.request(method, &r.url).timeout(Duration::from_millis(r.timeout_ms.into()));
    for h in &r.headers {
        b = b.header(&h.name, &h.value);
    }
    if r.cookies {
        if let Some(cookie) = cookie_header(&r.url) {
            b = b.header("Cookie", cookie);
        }
    }
    b = match r.body {
        Some(Body::Text { text, content_type }) => b.header("Content-Type", content_type).body(text),
        Some(Body::Bytes { bytes, content_type }) => b.header("Content-Type", content_type).body(bytes),
        None => b,
    };
    let response = b.send().await?;
    if r.cookies {
        take_cookies(&r.url, response.headers());
    }
    Ok(response)
}

/// No answer: offline when nothing could be connected to.
fn unreachable(e: &reqwest::Error) -> zcore::screen::Reply {
    zcore::screen::Reply::Unreachable { offline: e.is_connect(), message: e.to_string() }
}

/// Sends a crash or an error to the server's collector (server/errors.py), at most once a minute per place.
/// Never fails the caller: reporting is best effort.
pub fn report_error(kind: &'static str, kind_type: &str, message: &str, stack: &str, place: &str) {
    static SENT: Mutex<Option<zcore::api::Reports>> = Mutex::new(None);
    let due = SENT.lock().map(|mut sent| sent.get_or_insert_with(Default::default).due(place, kind_type, now_ms() as f64));
    if !due.unwrap_or(true) {
        return;
    }
    report_sent(zcore::api::Report {
        app: app_name().into(),
        kind: kind.into(),
        kind_type: kind_type.into(),
        message: message.into(),
        stack: stack.into(),
        place: place.into(),
        version: version_text(),
        os: crate::platform::os_name().into(),
        device: MODEL.into(),
        context: vec![],
    });
}

fn app_name() -> &'static str {
    if cfg!(target_os = "macos") { "mac" } else { "windows" }
}

fn version_text() -> String {
    format!("{VERSION} ({VERSION_CODE})")
}

fn report_sent(report: zcore::api::Report) {
    let request = zcore::api::post(base(), who(), zcore::api::REPORTS_PATH, zcore::api::report_body(&report));
    crate::spawn(async move {
        let _ = send(request).await;
    });
}

/// The last run's panic (main.rs writes crash.txt), sent once and then removed.
pub fn send_last_crash() {
    let path = root().join("crash.txt");
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let _ = std::fs::remove_file(&path);
    report_sent(zcore::api::Report::crash(&text, app_name(), &version_text(), crate::platform::os_name(), MODEL));
}

// ---------- reading the server's JSON ----------
pub trait Json {
    fn i(&self, key: &str) -> Option<i64>;
}
impl Json for Value {
    fn i(&self, key: &str) -> Option<i64> {
        match self.get(key)? {
            Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
            Value::String(s) => s.parse().ok(),
            _ => None,
        }
    }
}

// ---------- addresses ----------
pub fn absolute(url: &str) -> String {
    zcore::api::address(base(), url)
}
/// A picture's whole address (`zcore::pictures::url`).
pub fn img(url: &str) -> String {
    zcore::pictures::url(base(), url)
}
/// A picture at a card's size (`zcore::pictures::card`).
pub fn card_img(url: &str, size: &str) -> String {
    zcore::pictures::card(base(), url, size)
}

// ---------- settings kept on this device ----------
static PREFS: Mutex<Option<Value>> = Mutex::new(None);
fn prefs_file() -> PathBuf {
    root().join("prefs.json")
}
fn with_prefs<T>(f: impl FnOnce(&mut Value) -> T) -> T {
    let mut p = PREFS.lock().unwrap();
    let v = p.get_or_insert_with(|| std::fs::read_to_string(prefs_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({})));
    f(v)
}
pub fn pref_b(key: &str, default: bool) -> bool {
    with_prefs(|p| p.get(key).and_then(|v| v.as_bool()).unwrap_or(default))
}
pub fn pref_i(key: &str, default: i64) -> i64 {
    with_prefs(|p| p.i(key).unwrap_or(default))
}
/// A setting as the core reads it: its value as text, whatever type it's kept as.
pub fn pref_get(key: &str) -> Option<String> {
    with_prefs(|p| match p.get(key)? {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        v => Some(v.to_string()),
    })
}

/// A setting the core writes (None removes it), kept as the type it already has so the app's own reads of it go on
/// working; a new one, or one whose value doesn't fit that type, is kept as text.
pub fn pref_put(key: &str, value: Option<String>) {
    with_prefs(|p| {
        let Some(all) = p.as_object_mut() else { return };
        match value {
            None => {
                all.remove(key);
            }
            Some(text) => {
                let typed = match all.get(key) {
                    Some(Value::Bool(_)) => text.parse::<bool>().ok().map(Value::Bool),
                    Some(Value::Number(_)) => text.parse::<serde_json::Number>().ok().map(Value::Number),
                    _ => None,
                };
                all.insert(key.into(), typed.unwrap_or(Value::String(text)));
            }
        }
        write_soon(prefs_file(), p.to_string());
    })
}

pub fn pref_set(key: &str, value: Value) {
    with_prefs(|p| {
        p[key] = value;
        write_soon(prefs_file(), p.to_string());
    })
}
