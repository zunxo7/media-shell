//! What the app knows: titles, saved spots and the signed-in profile; the pages kept on this PC for the core's store.
use crate::net::{self, Res};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

// What the server sends about titles: the shared core's models, read by its rules (zcore::model).
pub use zcore::model::{Episode, Item, Title};

// ---------- title pages and the anime list, kept on this PC so they open at once ----------
fn pages_dir() -> PathBuf {
    let p = net::cache_root().join("pages");
    let _ = std::fs::create_dir_all(&p);
    p
}
fn page_file(key: &str) -> PathBuf {
    let clean: String = key.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    pages_dir().join(format!("{clean}.json"))
}
/// A page kept on this PC, however old: its body, its ETag (kept beside it), and when it came.
fn page_kept(key: &str) -> Option<zcore::cache::Kept> {
    let f = page_file(key);
    let at = std::fs::metadata(&f).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64();
    let body = std::fs::read_to_string(&f).ok()?;
    Some(zcore::cache::Kept { body, etag: std::fs::read_to_string(f.with_extension("etag")).ok(), at })
}
fn page_write_tagged(key: &str, text: &str, etag: Option<&str>) {
    let f = page_file(key);
    let _ = net::atomically(&f, text);
    match etag {
        Some(tag) => { let _ = std::fs::write(f.with_extension("etag"), tag); }
        None => { let _ = std::fs::remove_file(f.with_extension("etag")); }
    }
    net::trim_folder(&pages_dir(), zcore::cache::DISK_CAP);
}

/// A server page from the core's one store (`zcore::store`): read from this PC and asked of the server once however
/// many want it, by the cache rules (a stale copy at once, with a newer one told as changed).
pub(crate) async fn page_get(key: &str, path: &str) -> Res<String> {
    crate::core_screen::page(path, key, zcore::store::Need::Any).await
}

/// A kept page as the core's screens read it (core_screen): its body, ETag and when it came.
pub(crate) fn kept(key: &str) -> zcore::screen::Reply {
    // (Home was kept beside the pages, as home.json, before the core's store kept it with them)
    let old = net::root().join("home.json");
    if key == zcore::home::KEY && old.exists() && !page_file(key).exists() {
        let _ = std::fs::rename(&old, page_file(key));
    }
    match page_kept(key) {
        Some(k) => zcore::screen::Reply::Kept { body: k.body, etag: k.etag, at: k.at },
        None => zcore::screen::Reply::Missing,
    }
}

/// A page a core screen keeps.
pub(crate) fn keep(key: &str, body: &str, etag: Option<&str>) {
    page_write_tagged(key, body, etag);
}

/// A title, from the store.
pub async fn title(id: &str) -> Res<Arc<Title>> {
    let t = page_get(&format!("title-{id}"), &format!("/api/title/{id}")).await?;
    let o: Value = serde_json::from_str(&t).unwrap_or(json!({}));
    Ok(Arc::new(zcore::model::title(id, &o)))
}

// ---------- saved spots ----------
// Where each film or episode was left, read and kept by the core's rules (zcore::progress).
pub use zcore::progress::Saved;
// (shared, not copied: a page reading it on every frame takes a reference, not the whole map)
static PROGRESS: Mutex<Option<Arc<zcore::progress::Spots>>> = Mutex::new(None);

async fn progress_page(need: zcore::store::Need) -> Res<Arc<HashMap<String, Saved>>> {
    let body = crate::core_screen::page(zcore::progress::PATH, &zcore::progress::key(&crate::profile_settings::current_id()), need).await?;
    let mut known = PROGRESS.lock().unwrap();
    let map = Arc::new(zcore::progress::merge(known.as_deref().unwrap_or(&HashMap::new()), zcore::progress::spots(&body)));
    *known = Some(Arc::clone(&map));
    Ok(map)
}
/// The spots straight from the server (the player's last one just went in).
pub async fn progress_fresh() {
    let _ = progress_page(zcore::store::Need::Server).await;
}

/// A spot just saved here, shown before the server's page has it.
pub fn progress_remember(s: Saved) {
    zcore::progress::remember(Arc::make_mut(PROGRESS.lock().unwrap().get_or_insert_with(Default::default)), s);
}
/// Another profile, or signed out: the spots held let go.
pub(crate) fn progress_forget() {
    *PROGRESS.lock().unwrap() = None;
}

pub fn forget_all() {
    crate::core_screen::forget_pages();
    let _ = std::fs::remove_dir_all(pages_dir());
    let _ = std::fs::remove_file(net::root().join("home.json"));
    progress_forget();
}

// ---------- who's signed in ----------
#[derive(Clone, Debug)]
pub struct Me {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
    /// Its place among the account's profiles: picks its colour when it has no picture.
    pub index: usize,
}
static PROFILES: Mutex<Vec<Me>> = Mutex::new(Vec::new());
static PLUS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The account has 2K and 4K switched on (the owner's admin switch): without it, nothing above 1080p plays.
pub fn plus() -> bool {
    PLUS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Every profile on the account, as the last check found them (Who's watching).
pub fn profiles() -> Vec<Me> {
    PROFILES.lock().unwrap().clone()
}

/// A profile as the pages draw it.
pub fn me_of(p: &zcore::session::Profile) -> Me {
    Me { id: p.id.clone(), name: p.name.clone(), avatar: p.avatar.clone(), index: p.index as usize }
}

/// The account as the core's session has it: its profiles and the one in use.
pub fn account(a: &zcore::session::Account) -> Option<Me> {
    PLUS.store(a.plus, std::sync::atomic::Ordering::Relaxed);
    *PROFILES.lock().unwrap() = a.profiles.iter().map(me_of).collect();
    a.me().map(me_of)
}
