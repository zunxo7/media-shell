//! The zunflix server, as the website talks to it, and what's kept on this PC.
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// This build's version and build number. The Mac has its own (MAC_NAME, MAC_BUILD), released on its own
/// (tools/release.sh mac, app/mac.json), so a fix for one never ships to the other.
#[cfg(not(target_os = "macos"))]
pub const VERSION: &str = "2.0.3";
#[cfg(not(target_os = "macos"))]
pub const VERSION_CODE: i64 = 9000;
#[cfg(target_os = "macos")]
pub const VERSION: &str = MAC_NAME;
#[cfg(target_os = "macos")]
pub const VERSION_CODE: i64 = MAC_BUILD;
#[cfg(target_os = "macos")]
const MAC_NAME: &str = "1.0.0";
#[cfg(target_os = "macos")]
const MAC_BUILD: i64 = 1;
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
            tokio::time::sleep(Duration::from_millis(500)).await;
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
fn same_origin(url: &str) -> bool {
    let (Ok(url), Ok(base)) = (reqwest::Url::parse(url), reqwest::Url::parse(base())) else { return false };
    url.origin() == base.origin()
}

// ---------- the sign-in ----------
#[derive(Clone)]
struct Cookie {
    n: String,
    v: String,
    e: i64,
    p: String,
    d: String,
    h: bool,
    s: bool,
    o: bool,
    keep: bool,
}

static JAR: Mutex<Vec<Cookie>> = Mutex::new(Vec::new());

fn jar_file() -> PathBuf {
    root().join("cookies.json")
}
fn load_jar() {
    let Ok(text) = std::fs::read_to_string(jar_file()) else { return };
    let Ok(Value::Array(list)) = serde_json::from_str::<Value>(&text) else { return };
    let mut jar = JAR.lock().unwrap();
    for o in list {
        jar.push(Cookie {
            n: o.t("n"), v: o.t("v"), e: o["e"].as_i64().unwrap_or(0), p: o.t("p"), d: o.t("d"), h: o["h"].as_bool().unwrap_or(true), s: o["s"].as_bool().unwrap_or(false), o: o["o"].as_bool().unwrap_or(false), keep: true,
        });
    }
}
fn save_jar(jar: &[Cookie]) {
    let list: Vec<Value> = jar.iter().filter(|c| c.keep).map(|c| json!({"n": c.n, "v": c.v, "e": c.e, "p": c.p, "d": c.d, "h": c.h, "s": c.s, "o": c.o})).collect();
    let _ = atomically(&jar_file(), &Value::Array(list).to_string());
}

/// "Wed, 21 Oct 2026 07:28:00 GMT" to milliseconds.
fn http_date(s: &str) -> Option<i64> {
    let p: Vec<&str> = s.split([' ', '-', ':', ',']).filter(|x| !x.is_empty()).collect();
    if p.len() < 7 {
        return None;
    }
    let day: i64 = p[1].parse().ok()?;
    let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"].iter().position(|m| p[2].starts_with(m))? as i64 + 1;
    let mut year: i64 = p[3].parse().ok()?;
    if year < 100 {
        year += 2000;
    }
    let (h, m, sec): (i64, i64, i64) = (p[4].parse().ok()?, p[5].parse().ok()?, p[6].parse().ok()?);
    let (y, mo) = if month <= 2 { (year - 1, month + 9) } else { (year, month - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * mo + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(((era * 146097 + doe - 719468) * 86400 + h * 3600 + m * 60 + sec) * 1000)
}

fn take_cookies(url: &str, headers: &reqwest::header::HeaderMap) {
    let Ok(url) = reqwest::Url::parse(url) else { return };
    let Some(host) = url.host_str() else { return };
    let mut jar = JAR.lock().unwrap();
    let mut changed = false;
    for raw in headers.get_all(reqwest::header::SET_COOKIE) {
        let Ok(line) = raw.to_str() else { continue };
        let mut parts = line.split(';');
        let Some((n, v)) = parts.next().and_then(|p| p.split_once('=')) else { continue };
        let mut c = Cookie { n: n.trim().into(), v: v.trim().into(), e: i64::MAX, p: "/".into(), d: host.into(), h: true, s: false, o: false, keep: false };
        for a in parts {
            let (k, val) = a.split_once('=').unwrap_or((a, ""));
            match k.trim().to_ascii_lowercase().as_str() {
                "max-age" => {
                    if let Ok(secs) = val.trim().parse::<i64>() {
                        c.e = now_ms().saturating_add(secs.saturating_mul(1000));
                        c.keep = true;
                    }
                }
                "expires" if !c.keep => {
                    if let Some(t) = http_date(val.trim()) {
                        c.e = t;
                        c.keep = true;
                    }
                }
                "path" => c.p = val.trim().into(),
                "domain" => {
                    c.d = val.trim().trim_start_matches('.').into();
                    c.h = false;
                }
                "secure" => c.s = true,
                "httponly" => c.o = true,
                _ => {}
            }
        }
        if c.d != host && !host.strip_suffix(&c.d).is_some_and(|prefix| prefix.ends_with('.')) {
            continue;
        }
        jar.retain(|x| x.n != c.n || x.d != c.d || x.p != c.p);
        if c.e > now_ms() {
            jar.push(c);
        }
        changed = true;
    }
    if changed {
        save_jar(&jar);
    }
}

fn cookie_header(url: &str) -> Option<String> {
    let url = reqwest::Url::parse(url).ok()?;
    let host = url.host_str()?;
    let secure = url.scheme() == "https";
    let path = url.path();
    let now = now_ms();
    let jar = JAR.lock().unwrap();
    let all: Vec<String> = jar
        .iter()
        .filter(|c| c.e > now && (!c.s || secure) && (path == c.p || path.strip_prefix(&c.p).is_some_and(|rest| c.p.ends_with('/') || rest.starts_with('/'))) && (host == c.d || (!c.h && host.ends_with(&format!(".{}", c.d)))))
        .map(|c| format!("{}={}", c.n, c.v))
        .collect();
    if all.is_empty() {
        None
    } else {
        Some(all.join("; "))
    }
}

/// The sign-in, for mpv: it asks for what the server relays itself, and has no cookie jar of its own. Never sent anywhere else.
pub fn cookie_for(url: &str) -> Option<String> {
    if !same_origin(url) {
        return None;
    }
    cookie_header(url)
}

// ---------- asking ----------
fn client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        load_jar();
        reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).read_timeout(Duration::from_secs(60)).pool_idle_timeout(Duration::from_secs(90)).build().unwrap()
    })
}

pub fn clear_session() {
    let mut jar = JAR.lock().unwrap();
    jar.clear();
    save_jar(&jar);
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

/// A request as the core built it, sent as it is: with this PC's sign-in only when it says so.
pub async fn send(request: zcore::screen::Request) -> zcore::screen::Reply {
    let response = match call(request).await {
        Ok(response) => response,
        Err(e) => return unreachable(&e),
    };
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
        reqwest::Client::builder().http1_only().pool_max_idle_per_host(48).connect_timeout(Duration::from_secs(15)).read_timeout(Duration::from_secs(60)).build().unwrap()
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
    let report = zcore::api::Report {
        app: if cfg!(target_os = "macos") { "mac" } else { "windows" }.into(),
        kind: kind.into(),
        kind_type: kind_type.into(),
        message: message.into(),
        stack: stack.into(),
        place: place.into(),
        version: format!("{VERSION} ({VERSION_CODE})"),
        os: crate::platform::os_name().into(),
        device: MODEL.into(),
        context: vec![],
    };
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
    // Release builds are stripped, so the backtrace has no names: the panic's file and message identify it.
    // The text starts "panicked at src/player.rs:12:5:" with the message on the next line.
    let (head, rest) = text.split_once('\n').unwrap_or((&text, ""));
    let file = head.trim_start_matches("panicked at ").split(':').next().unwrap_or("");
    let message = rest.lines().next().unwrap_or("").trim();
    let gist: String = message.chars().filter(|c| !c.is_ascii_digit()).take(60).collect();
    report_error("crash", "Panic", message, &text, &format!("{file}: {gist}"));
}

// ---------- reading the server's JSON ----------
pub trait Json {
    fn s(&self, key: &str) -> Option<String>;
    fn t(&self, key: &str) -> String;
    fn i(&self, key: &str) -> Option<i64>;
    fn d(&self, key: &str) -> f64;
}
impl Json for Value {
    fn s(&self, key: &str) -> Option<String> {
        match self.get(key)? {
            Value::Null => None,
            Value::String(s) if s.is_empty() => None,
            Value::String(s) => Some(s.clone()),
            v => Some(v.to_string()),
        }
    }
    fn t(&self, key: &str) -> String {
        self.s(key).unwrap_or_default()
    }
    fn i(&self, key: &str) -> Option<i64> {
        match self.get(key)? {
            Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
            Value::String(s) => s.parse().ok(),
            _ => None,
        }
    }
    fn d(&self, key: &str) -> f64 {
        self.get(key).and_then(|v| v.as_f64()).unwrap_or(0.)
    }
}

// ---------- addresses ----------
pub fn absolute(url: &str) -> String {
    if url.is_empty() {
        String::new()
    } else if url.starts_with("http") {
        url.to_string()
    } else if url.starts_with('/') {
        format!("{}{url}", base())
    } else {
        format!("{}/{url}", base())
    }
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
pub fn pref_s(key: &str, default: &str) -> String {
    with_prefs(|p| p.s(key).unwrap_or_else(|| default.to_string()))
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
