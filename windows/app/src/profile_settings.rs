//! This profile's own settings, as the core keeps them (`zcore::profile_settings`): its subtitles, languages and
//! playback. Read here from any thread; changed through the core, which shows the change at once and saves it behind.
use crate::core_screen::Lasting;
use serde_json::Value;
use std::sync::{Mutex, OnceLock};
use zcore::profile_settings::{Event, ProfileSettings, Setting, Settings};

static SCREEN: OnceLock<Lasting<ProfileSettings>> = OnceLock::new();
/// The core's latest view of them.
static NOW: Mutex<Option<Settings>> = Mutex::new(None);

/// Opened on the core as the app starts, before the session, so it hears of the account from the first.
pub fn start() {
    SCREEN.get_or_init(|| Lasting::new(shown)).send(Event::Started);
}

fn shown(settings: Settings) {
    *NOW.lock().unwrap() = Some(settings);
    crate::ui(|a| a.kit.dirty = true);
}

fn read<T>(f: impl FnOnce(&Settings) -> T) -> T {
    let now = NOW.lock().unwrap();
    match now.as_ref() {
        Some(s) => f(s),
        None => f(&Settings::default()),
    }
}

/// The profile watching now ("" before one's picked).
pub fn current_id() -> String {
    read(|s| s.profile.clone())
}

/// Captions shown.
pub fn cc_on() -> bool {
    read(|s| s.captions_on)
}
/// "small" | "medium" | "large".
pub fn cc_size() -> String {
    read(|s| s.caption_size.clone())
}
/// The captions' colour: white, yellow, cyan, green or magenta.
pub fn cc_color() -> String {
    read(|s| s.caption_color.clone())
}
/// How dark the box behind captions is, 0 to 1.
pub fn cc_background() -> f32 {
    read(|s| s.caption_background)
}
/// Dramas from abroad (Korean, Turkish...): "original" (their own language, English subtitles) | "dub" (English).
pub fn dramas_audio() -> String {
    read(|s| s.dramas_audio.clone())
}
/// Anime's audio: "sub" (Japanese, subtitled) | "dub" (English).
pub fn anime_audio() -> String {
    read(|s| s.anime_audio.clone())
}
/// The next episode plays by itself at the end.
pub fn autoplay_next() -> bool {
    read(|s| s.autoplay_next)
}
/// "auto", or a height ("1080p"; "2160p HDR" from the player's Quality), capped by `zcore::quality::most`.
pub fn quality() -> String {
    read(|s| s.quality.clone())
}
/// Downloads: "standard" (720p) | "high" (1080p) | "best" (the tallest the account may have): `zcore::quality`.
pub fn download_tier() -> String {
    read(|s| s.download_quality.clone())
}

/// Changes one setting of the profile in use (the core keeps only values the server allows).
pub fn set(group: &str, key: &str, v: Value) {
    let value = match v {
        Value::Bool(on) => Setting::Flag { on },
        Value::String(text) => Setting::Text { text },
        Value::Number(n) => Setting::Number { number: n.as_f64().unwrap_or_default() },
        _ => return,
    };
    put(group, key, value);
}

/// Changes one setting of the profile in use to a value the core already typed (the player's own picks).
pub fn put(group: &str, key: &str, value: Setting) {
    if let Some(screen) = SCREEN.get() {
        screen.send(Event::Set { group: group.into(), key: key.into(), value });
    }
}
