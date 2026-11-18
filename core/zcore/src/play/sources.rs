//! Where a stream is asked for, and what each answer means: Real-Debrid copies, the anime server,
//! the stream a title page names, and the subtitles for each. The player and downloads ask the same way; the apps
//! only send the requests.

use crate::model::{b, d, objects, s, Episode, Title};
use crate::player::Segment;
use crate::screen::{Header, Method, Request};
use serde_json::{json, Value};

/// What anything with a browser's manners is asked with.
pub const BROWSER: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
/// How long a read straight from a source may take.
const SOURCE_MS: u32 = 60_000;

/// What the server knows a film or an episode by. A title with no media type goes by its kind.
pub fn ids(title: &Title, ep: Option<&Episode>) -> Value {
    let kind = title.media_type.clone().unwrap_or_else(|| if title.kind == "movie" { "movie" } else { "series" }.into());
    json!({"type": kind, "imdb": title.imdb, "season": ep.and_then(|e| e.season), "episode": ep.and_then(|e| e.n)})
}

/// An id without its source's prefix ("yt-abc" is "abc").
pub fn bare<'a>(id: &'a str, prefix: &str) -> &'a str {
    id.strip_prefix(prefix).unwrap_or(id)
}

/// A query value, percent-encoded.
pub fn enc(value: &str) -> String {
    value
        .bytes()
        .map(|c| match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (c as char).to_string(),
            _ => format!("%{c:02X}"),
        })
        .collect()
}

/// A read straight from a source (not the zunflix server), with no sign-in: captions files, playlists.
pub fn direct(url: &str, agent: &str) -> Request {
    Request {
        method: Method::Get,
        url: url.into(),
        headers: vec![Header::new("User-Agent", agent), Header::new("Accept", "*/*")],
        body: None,
        cookies: false,
        timeout_ms: SOURCE_MS,
    }
}

// ---------- anime ----------

/// The anime server's stream for an episode, in `audio` ("sub" or "dub").
pub fn anime_path(title: &Title, ep: &Episode, audio: &str) -> String {
    let show = bare(ep.season_id.as_deref().unwrap_or(&title.id), "anime-");
    format!("/api/anime/play/{}?audio={}&show={}", enc(bare(&ep.id, "anime-")), enc(audio), enc(show))
}

/// An anime episode's stream as the server found it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Anime {
    /// Straight from the source when the server says where (none of it through the server), else the relay.
    pub url: String,
    pub referer: Option<String>,
    /// The relay, when `url` is the source's own.
    pub fallback: Option<String>,
    pub captions: Option<String>,
    pub audio: Option<String>,
    pub audios: Vec<String>,
    pub skip: Vec<Segment>,
}

/// The anime server's answer; None when it has no stream. `base` makes its own addresses whole.
pub fn anime(answer: &str, base: &str) -> Option<Anime> {
    let v: Value = serde_json::from_str(answer).ok()?;
    let relay = s(&v, "stream").map(|u| crate::api::url(base, &u));
    let master = s(&v["direct"], "master");
    let (url, referer, fallback) = match master {
        Some(m) => (m, s(&v["direct"], "referer"), relay),
        None => (relay?, None, None),
    };
    Some(Anime {
        url,
        referer,
        fallback,
        captions: objects(&v, "subtitles").first().and_then(|o| s(o, "src")).map(|u| crate::api::url(base, &u)),
        audio: s(&v, "audio"),
        audios: v["available"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
        // (no end, or 0: it runs to the end of the video)
        skip: objects(&v, "skip")
            .into_iter()
            .filter_map(|o| Some(Segment { kind: s(o, "kind")?, start: d(o, "start"), end: o["end"].as_f64().filter(|e| *e > 0.) }))
            .collect(),
    })
}

/// Through the server: a drama episode's stream, and its HLS list when the direct one fails.
pub fn youtube_play_path(id: &str) -> String {
    format!("/api/youtube/play/{}", enc(id))
}

pub fn youtube_relay_path(id: &str) -> String {
    format!("/api/youtube/hls/{}/master.m3u8", enc(id))
}

pub fn youtube_captions_path(id: &str) -> String {
    format!("/api/youtube/captions/{}", enc(id))
}

pub fn youtube_skip_path(id: &str) -> String {
    format!("/api/youtube/skip/{}", enc(id))
}

/// The stream named in an answer (`{"stream": …}`).
pub fn stream(answer: &str) -> Option<String> {
    serde_json::from_str::<Value>(answer).ok().and_then(|v| s(&v, "stream"))
}

/// The captions the server finds for a stream a title page names.
pub fn ary_captions_path(stream: &str) -> String {
    format!("/api/ary/captions?stream={}", enc(stream))
}

/// The first subtitles file's words in an answer (`{"subtitles": [{"vtt": …}]}`).
pub fn first_vtt(answer: &str) -> Option<String> {
    let v: Value = serde_json::from_str(answer).ok()?;
    objects(&v, "subtitles").first().and_then(|o| s(o, "vtt"))
}

// ---------- Movies & TV subtitles ----------

pub const SUBTITLES_PATH: &str = "/api/cinema/subtitles";

/// What's asked for a copy's subtitles: `ids` with the copy's `length`, quick or the full sync.
pub fn subtitles_body(ids: &Value, quick: bool) -> String {
    let mut body = ids.clone();
    body["quick"] = json!(quick);
    body.to_string()
}

/// A subtitles answer: the file's words, and whether the server's still lining them up.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Subtitles {
    pub vtt: Option<String>,
    pub syncing: bool,
}

pub fn subtitles(answer: &str) -> Subtitles {
    let Ok(v) = serde_json::from_str::<Value>(answer) else { return Subtitles::default() };
    Subtitles { vtt: first_vtt(answer), syncing: b(&v, "syncing") }
}

/// A film's year as a number.
pub fn year(title: &Title) -> Option<i64> {
    crate::model::year_number(title.year.as_deref())
}

// ---------- Real-Debrid ----------

pub const DEBRID_PATH: &str = "/api/cinema/debrid";
pub const DEBRID_LINK_PATH: &str = "/api/cinema/debrid/link";

/// What a film's (or an episode's) copies are asked for with.
pub fn debrid_body(title: &Title, ep: Option<&Episode>, codecs: &[String]) -> String {
    json!({"imdb": title.imdb, "title": title.title, "year": year(title), "minutes": title.minutes, "codecs": codecs,
           "season": ep.and_then(|e| e.season), "episode": ep.and_then(|e| e.n)})
        .to_string()
}

/// A Real-Debrid copy of a film or an episode: the server's id for it, its quality ("2160p", "1080p"…) and whether it's HDR.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebridCopy {
    pub id: String,
    pub quality: String,
    pub hdr: bool,
}

/// A title's Real-Debrid copies, best first, as the server sends them (only what the account may play).
pub fn debrid_copies(answer: &str) -> Vec<DebridCopy> {
    let Ok(v) = serde_json::from_str::<Value>(answer) else { return Vec::new() };
    objects(&v, "copies")
        .into_iter()
        .filter_map(|c| Some(DebridCopy { id: s(c, "id")?, quality: s(c, "quality")?, hdr: b(c, "hdr") }))
        .filter(|c| crate::quality::height(&c.quality) > 0)
        .collect()
}

/// The copy for a `quality` setting.
pub fn choose_copy<'a>(copies: &'a [DebridCopy], setting: &str) -> Option<&'a DebridCopy> {
    let facts: Vec<crate::quality::Copy> =
        copies.iter().map(|c| crate::quality::Copy { height: crate::quality::height(&c.quality), hdr: c.hdr }).collect();
    crate::quality::choose_easynews(&facts, setting).map(|i| &copies[i])
}

pub fn debrid_link_body(id: &str) -> String {
    json!({ "id": id }).to_string()
}

/// The direct link in an answer.
pub fn link(answer: &str) -> Option<String> {
    serde_json::from_str::<Value>(answer).ok().and_then(|v| s(&v, "url"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://host.invalid";

    fn film() -> Title {
        Title { id: "cinema-movie-tt1".into(), title: "Dune".into(), source: "cinema".into(), kind: "movie".into(),
                imdb: Some("tt1".into()), year: Some("2021-10".into()), minutes: Some(155), ..Title::default() }
    }

    #[test]
    fn debrid_copies_and_choice() {
        let copies = debrid_copies(r#"{"copies": [{"id": "1", "quality": "2160p", "hdr": true}, {"id": "2", "quality": "1080p"}, {"id": "3", "quality": "x"}]}"#);
        assert_eq!(copies.len(), 2);
        assert_eq!(choose_copy(&copies, "auto").map(|c| c.id.as_str()), Some("2"));
        assert_eq!(choose_copy(&copies, "2160p HDR").map(|c| c.id.as_str()), Some("1"));
        assert_eq!(link(r#"{"url": "https://rd/f.mkv"}"#).as_deref(), Some("https://rd/f.mkv"));
    }

    #[test]
    fn ids_knows_the_kind() {
        assert_eq!(ids(&film(), None)["type"], "movie");
        assert!(!direct("https://d/m", BROWSER).cookies);
    }

    #[test]
    fn anime_straight_from_the_source_with_the_relay_behind() {
        let a = anime(
            r#"{"stream": "/api/anime/hls/1.m3u8", "direct": {"master": "https://mega/m.m3u8", "referer": "https://mega/"},
                "subtitles": [{"src": "/s.vtt"}], "audio": "sub", "available": ["sub", "dub"],
                "skip": [{"kind": "intro", "start": 30, "end": 120}, {"kind": "credits", "start": 1300, "end": 0}]}"#,
            BASE,
        )
        .unwrap_or_default();
        assert_eq!(a.url, "https://mega/m.m3u8");
        assert_eq!(a.fallback.as_deref(), Some("https://host.invalid/api/anime/hls/1.m3u8"));
        assert_eq!(a.captions.as_deref(), Some("https://host.invalid/s.vtt"));
        assert_eq!(a.skip[1].end, None);
        assert_eq!(anime(r#"{"stream": "/r.m3u8"}"#, BASE).map(|a| a.fallback), Some(None));
        assert_eq!(anime("{}", BASE), None);
        let ep = Episode { id: "anime-e 1".into(), season_id: Some("anime-s".into()), ..Episode::default() };
        assert_eq!(anime_path(&film(), &ep, "sub"), "/api/anime/play/e%201?audio=sub&show=s");
    }

    #[test]
    fn cinema_subtitles() {
        let subs = subtitles(r#"{"subtitles": [{"vtt": "WEBVTT"}], "syncing": true}"#);
        assert_eq!(subs, Subtitles { vtt: Some("WEBVTT".into()), syncing: true });
    }
}
