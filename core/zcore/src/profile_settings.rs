//! The profile in use's own settings: its subtitles, languages and playback. They're kept with the profile on the
//! server (profiles.py), so they follow it between devices; they come with the account, change here at once and are
//! saved behind. Until the server has one, the device's own older setting is where it starts.
//!
//! One of these lives as long as the app: the session tells it the account ([`AppEvent::Account`]), and every part
//! of the app reads the profile's settings from its view.

use crate::api;
use crate::screen::{AppEvent, Asks, Context, Effect, Reply, Screen};
use crate::session::{Account, PROFILES};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

/// The device's own older settings, by the names it kept them under.
const DEVICE: [&str; 5] = ["captions", "ccSize", "audio", "quality", "downloadTier"];

/// A setting's new value.
#[derive(Clone, Debug, PartialEq)]
pub enum Setting {
    Flag { on: bool },
    Text { text: String },
    Number { number: f64 },
}

/// The profile in use's settings, each with its fallback worked out.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// The profile in use ("" before one's known): its own pages and downloads are kept under it.
    pub profile: String,
    pub captions_on: bool,
    /// "small", "medium" or "large".
    pub caption_size: String,
    /// "white", "yellow", "cyan", "green" or "magenta".
    pub caption_color: String,
    /// How dark the box behind captions is, 0 to 1.
    pub caption_background: f32,
    /// Anime: "sub" (Japanese, subtitled) or "dub" (English).
    pub anime_audio: String,
    /// Dramas from abroad: "original" (their own language) or "dub" (English).
    pub dramas_audio: String,
    pub autoplay_next: bool,
    /// "auto", or a height ("1080p", "2160p HDR"), capped by `quality::most`.
    pub quality: String,
    /// Downloads: "standard", "high" or "best" (`quality::download`).
    pub download_quality: String,
    /// Goes up whenever any of them changes.
    pub changes: u32,
}

impl Default for Settings {
    fn default() -> Self {
        ProfileSettings::default().view()
    }
}

/// What happened.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The app opened: the device's older settings are read.
    Started,
    /// One setting of the profile in use changed (`group` and `key` as the server names them).
    Set { group: String, key: String, value: Setting },
}

/// A change saved behind, put back over a newer account until the server has it.
#[derive(Debug)]
struct Saving {
    id: u32,
    profile: String,
    group: String,
    key: String,
    value: Value,
}

/// The settings. See the module's docs.
#[derive(Debug, Default)]
pub struct ProfileSettings {
    /// Each profile's settings as the server gave them, with the changes made here.
    all: HashMap<String, Value>,
    current: String,
    device: HashMap<String, String>,
    saving: Vec<Saving>,
    changes: u32,
    asks: Asks<Option<String>>,
    /// The download settings last told to the app (quality, anime's sound).
    told: Option<(String, String)>,
}

/// The value `value` as the server keeps it under `group`/`key`, when it's one the server allows (profiles.py
/// SETTINGS_CHOICES).
fn allowed(group: &str, key: &str, value: &Setting) -> Option<Value> {
    let one_of = |text: &str, choices: &[&str]| choices.contains(&text).then(|| json!(text));
    match ((group, key), value) {
        (("subtitles", "on") | ("playback", "autoplayNext"), Setting::Flag { on }) => Some(json!(on)),
        (("subtitles", "background"), Setting::Number { number }) if number.is_finite() => {
            Some(json!((number.clamp(0., 1.) * 100.).round() / 100.))
        }
        (("subtitles", "size"), Setting::Text { text }) => one_of(text, &["small", "medium", "large"]),
        (("subtitles", "style"), Setting::Text { text }) => one_of(text, &["classic", "yellow", "outline", "box"]),
        (("subtitles", "color"), Setting::Text { text }) => {
            one_of(text, &["white", "yellow", "cyan", "green", "magenta"])
        }
        (("languages", "anime"), Setting::Text { text }) => one_of(text, &["sub", "dub"]),
        (("languages", "dramas"), Setting::Text { text }) => one_of(text, &["original", "dub"]),
        (("playback", "quality"), Setting::Text { text }) => quality(text).then(|| json!(text)),
        (("playback", "downloadQuality"), Setting::Text { text }) => one_of(text, &["standard", "high", "best"]),
        _ => None,
    }
}

/// "auto", or a height of three or four digits with "p", and " HDR" after it or not.
fn quality(text: &str) -> bool {
    if text == "auto" {
        return true;
    }
    let height = text.strip_suffix(" HDR").unwrap_or(text);
    let Some(digits) = height.strip_suffix('p') else { return false };
    (3..=4).contains(&digits.len()) && !digits.starts_with('0') && digits.bytes().all(|b| b.is_ascii_digit())
}

impl ProfileSettings {
    fn value(&self, group: &str, key: &str) -> Option<&Value> {
        self.all.get(&self.current)?.get(group)?.get(key)
    }

    /// A choice the server allows, else `fallback`.
    fn text(&self, group: &str, key: &str, fallback: impl FnOnce() -> String) -> String {
        self.value(group, key)
            .and_then(Value::as_str)
            .filter(|t| allowed(group, key, &Setting::Text { text: (*t).to_string() }).is_some())
            .map_or_else(fallback, String::from)
    }

    fn flag(&self, group: &str, key: &str, fallback: impl FnOnce() -> bool) -> bool {
        self.value(group, key).and_then(Value::as_bool).unwrap_or_else(fallback)
    }

    fn device(&self, key: &str) -> Option<&str> {
        self.device.get(key).map(String::as_str).filter(|v| !v.is_empty())
    }

    /// `value` put in for `profile`.
    fn put(&mut self, profile: &str, group: &str, key: &str, value: Value) {
        let entry = self.all.entry(profile.to_string()).or_insert_with(|| json!({}));
        if !entry.is_object() {
            *entry = json!({});
        }
        if !entry.get(group).is_some_and(Value::is_object) {
            entry[group] = json!({});
        }
        entry[group][key] = value;
    }
}

impl Screen for ProfileSettings {
    type Event = Event;
    type View = Settings;

    fn update(&mut self, event: Event, cx: &Context) -> Vec<Effect> {
        match event {
            Event::Started => DEVICE
                .iter()
                .map(|key| Effect::ReadPref { id: self.asks.ask(Some((*key).to_string())), key: (*key).to_string() })
                .collect(),
            Event::Set { group, key, value } => {
                let Some(value) = allowed(&group, &key, &value) else { return vec![] };
                let profile = self.current.clone();
                self.put(&profile, &group, &key, value.clone());
                self.changes += 1;
                let mut out = self.tell();
                if profile.is_empty() {
                    return out;
                }
                let mut body = Map::new();
                body.insert(group.clone(), json!({ key.as_str(): value.clone() }));
                let path = format!("{PROFILES}/{}/settings", crate::sections::encode(&profile));
                let id = self.asks.ask(None);
                // (a later change of the same setting replaces this one's place)
                self.saving.retain(|s| !(s.profile == profile && s.group == group && s.key == key));
                self.saving.push(Saving { id, profile, group, key, value });
                let request = api::post(&cx.base, &cx.who(), &path, Value::Object(body).to_string());
                out.push(Effect::Request { id, request });
                out
            }
        }
    }

    fn answer(&mut self, id: u32, reply: Reply, _cx: &Context) -> Vec<Effect> {
        match self.asks.take(id) {
            Some(Some(key)) => {
                if let Reply::Pref { value: Some(value) } = reply {
                    self.device.insert(key, value);
                    self.changes += 1;
                }
            }
            // (saved, or not: either way the change stays here until the account says otherwise)
            Some(None) => self.saving.retain(|s| s.id != id),
            None => {}
        }
        vec![]
    }

    fn app(&mut self, event: &AppEvent, _cx: &Context) -> Vec<Effect> {
        if let AppEvent::Account { account } = event {
            self.take(account.as_ref());
            return self.tell();
        }
        vec![]
    }

    fn view(&self) -> Settings {
        let size = || {
            match self.device("ccSize") {
                Some("s") => "small",
                Some("l") => "large",
                _ => "medium",
            }
            .to_string()
        };
        let style = self.text("subtitles", "style", || "classic".into());
        Settings {
            profile: self.current.clone(),
            captions_on: self.flag("subtitles", "on", || self.device("captions") != Some("false")),
            caption_size: self.text("subtitles", "size", size),
            caption_color: self
                .text("subtitles", "color", || if style == "yellow" { "yellow" } else { "white" }.into()),
            caption_background: self
                .value("subtitles", "background")
                .and_then(Value::as_f64)
                .map_or(0.6, |v| v.clamp(0., 1.)) as f32,
            anime_audio: self
                .text("languages", "anime", || self.device("audio").filter(|a| *a == "dub").unwrap_or("sub").into()),
            dramas_audio: self.text("languages", "dramas", || "original".into()),
            autoplay_next: self.flag("playback", "autoplayNext", || true),
            quality: self
                .text("playback", "quality", || self.device("quality").filter(|q| quality(q)).unwrap_or("auto").into()),
            download_quality: self.text("playback", "downloadQuality", || {
                self.device("downloadTier")
                    .filter(|d| ["standard", "high", "best"].contains(d))
                    .unwrap_or(crate::quality::DOWNLOAD_DEFAULT)
                    .into()
            }),
            changes: self.changes,
        }
    }
}

impl ProfileSettings {
    /// The download settings, told to the app when they're not what it was last told.
    fn tell(&mut self) -> Vec<Effect> {
        let view = self.view();
        let now = (view.download_quality, view.anime_audio);
        if self.told.as_ref() == Some(&now) {
            return vec![];
        }
        self.told = Some(now.clone());
        vec![Effect::Tell { event: AppEvent::DownloadSettings { quality: now.0, anime_audio: now.1 } }]
    }

    /// The account as the session has it now: every profile's settings, the one in use, and the changes still being
    /// saved put back over them.
    fn take(&mut self, account: Option<&Account>) {
        self.all = account
            .map(|a| {
                a.profiles
                    .iter()
                    .filter_map(|p| Some((p.id.clone(), serde_json::from_str(p.settings.as_deref()?).ok()?)))
                    .collect()
            })
            .unwrap_or_default();
        self.current = account.and_then(|a| a.current.clone()).unwrap_or_default();
        if account.is_none() {
            self.saving.clear();
        }
        let saving: Vec<_> =
            self.saving.iter().map(|s| (s.profile.clone(), s.group.clone(), s.key.clone(), s.value.clone())).collect();
        for (profile, group, key, value) in saving {
            self.put(&profile, &group, &key, value);
        }
        self.changes += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{response, test_context, Body};
    use crate::session::Profile;

    fn account(settings: &str) -> Account {
        let profile = |id: &str, settings: Option<&str>| Profile {
            id: id.into(),
            name: id.into(),
            avatar: None,
            index: 0,
            settings: settings.map(String::from),
        };
        Account {
            profiles: vec![profile("p1", Some(settings)), profile("p2", None)],
            current: Some("p1".into()),
            plus: false,
            max_profiles: 5,
        }
    }

    fn told(s: &mut ProfileSettings, account: Option<Account>) {
        s.app(&AppEvent::Account { account }, &test_context(0.));
    }

    #[test]
    fn the_profiles_own_settings_over_the_devices_older_ones() {
        let cx = test_context(0.);
        let mut s = ProfileSettings::default();
        let asked: Vec<(u32, String)> = s
            .update(Event::Started, &cx)
            .into_iter()
            .filter_map(|e| match e {
                Effect::ReadPref { id, key } => Some((id, key)),
                _ => None,
            })
            .collect();
        assert_eq!(asked.len(), DEVICE.len());
        for (id, key) in asked {
            let value = match key.as_str() {
                "captions" => "false",
                "ccSize" => "l",
                "audio" => "dub",
                "quality" => "720p",
                _ => "nonsense",
            };
            s.answer(id, Reply::Pref { value: Some(value.into()) }, &cx);
        }
        let v = s.view();
        assert_eq!((v.captions_on, v.caption_size.as_str(), v.anime_audio.as_str()), (false, "large", "dub"));
        assert_eq!((v.quality.as_str(), v.download_quality.as_str()), ("720p", crate::quality::DOWNLOAD_DEFAULT));
        told(
            &mut s,
            Some(account(r#"{"subtitles": {"on": true, "style": "yellow", "size": "huge", "background": 3}}"#)),
        );
        let v = s.view();
        assert_eq!(v.profile, "p1");
        assert_eq!((v.captions_on, v.caption_size.as_str(), v.caption_color.as_str()), (true, "large", "yellow"));
        assert_eq!((v.caption_background, v.dramas_audio.as_str(), v.autoplay_next), (1., "original", true));
    }

    #[test]
    fn a_change_shows_at_once_and_is_saved_behind() {
        let cx = test_context(0.);
        let mut s = ProfileSettings::default();
        told(&mut s, Some(account("{}")));
        let before = s.view().changes;
        let set = |group: &str, key: &str, value: Setting| Event::Set { group: group.into(), key: key.into(), value };
        assert!(s.update(set("subtitles", "size", Setting::Text { text: "huge".into() }), &cx).is_empty());
        assert!(s.update(set("playback", "autoplayNext", Setting::Text { text: "yes".into() }), &cx).is_empty());
        let effects = s.update(set("playback", "quality", Setting::Text { text: "2160p HDR".into() }), &cx);
        let [Effect::Request { id, request }] = effects.as_slice() else { panic!("expected a save, got {effects:?}") };
        assert!(request.url.ends_with("/api/profiles/p1/settings"));
        let Some(Body::Text { text, .. }) = &request.body else { panic!("no body") };
        assert_eq!(serde_json::from_str::<Value>(text).ok(), Some(json!({"playback": {"quality": "2160p HDR"}})));
        assert_eq!((s.view().quality.as_str(), s.view().changes > before), ("2160p HDR", true));
        // (the account read again before the server had it: the change stays)
        told(&mut s, Some(account("{}")));
        assert_eq!(s.view().quality, "2160p HDR");
        s.answer(*id, response(200, "{}", None), &cx);
        told(&mut s, Some(account(r#"{"playback": {"quality": "1080p"}}"#)));
        assert_eq!(s.view().quality, "1080p");
    }

    #[test]
    fn the_download_settings_are_told_once_and_again_when_they_change() {
        let cx = test_context(0.);
        let mut s = ProfileSettings::default();
        let tells = |effects: &[Effect]| effects.iter().filter(|e| matches!(e, Effect::Tell { .. })).count();
        assert_eq!(tells(&s.app(&AppEvent::Account { account: Some(account("{}")) }, &cx)), 1);
        assert_eq!(tells(&s.app(&AppEvent::Account { account: Some(account("{}")) }, &cx)), 0);
        let set = Event::Set { group: "playback".into(), key: "downloadQuality".into(), value: Setting::Text { text: "best".into() } };
        let effects = s.update(set, &cx);
        let Some(Effect::Tell { event: AppEvent::DownloadSettings { quality, .. } }) = effects.first() else { panic!("{effects:?}") };
        assert_eq!(quality, "best");
    }

    #[test]
    fn signed_out_nothing_is_saved_and_numbers_are_rounded() {
        let cx = test_context(0.);
        let mut s = ProfileSettings::default();
        told(&mut s, None);
        let effects = s.update(
            Event::Set {
                group: "subtitles".into(),
                key: "background".into(),
                value: Setting::Number { number: 0.456 },
            },
            &cx,
        );
        assert!(effects.is_empty());
        assert_eq!((s.view().profile.as_str(), s.view().caption_background), ("", 0.46));
        assert!(
            quality("auto")
                && quality("1080p")
                && quality("2160p HDR")
                && !quality("080p")
                && !quality("10800p")
                && !quality("hd")
        );
    }
}
