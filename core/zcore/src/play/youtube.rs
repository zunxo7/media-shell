//! The channels' dramas, found by the device itself: YouTube answers a home connection with no account, so the video
//! comes straight from YouTube and none of it through the server. One visitor id and one player request, as the
//! Vision Pro's client asks: the HLS list with every size for playing, files that stay fetchable past their first few
//! MB for downloads, the caption tracks and the storyboards. This depends on YouTube leaving that client alone; if it
//! stops working, playing goes through the server.

use crate::model::{b, i, objects, s, t};
use crate::quality::pick_level;
use crate::screen::{Body, Header, Method, Request};
use serde_json::{json, Value};

/// What YouTube's files, captions and every video stream are asked for with.
pub const AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; rv:128.0) Gecko/20100101 Firefox/128.0";
const CLIENT_AGENT: &str = "com.google.ios.youtube/1.02 (RealityDevice14,1; U; CPU visionOS 25_6_0 like Mac OS X; US)";
/// Seconds a lookup is kept: its links last some hours.
pub const KEEP: f64 = 3600.;
/// How long a lookup may take before Play goes through the server instead.
pub const DIRECT_WAIT_MS: u32 = 3_000;
const TIMEOUT_MS: u32 = 10_000;

fn client(visitor: Option<&str>) -> Value {
    let mut client = json!({"clientName": "VISIONOS", "clientVersion": "1.02", "clientScreen": "WATCH", "platform": "MOBILE",
        "deviceMake": "Apple", "deviceModel": "RealityDevice14,1", "osName": "visionOS", "osVersion": "25.6.0.23O471",
        "hl": "en", "gl": "US", "utcOffsetMinutes": 0});
    if let Some(v) = visitor {
        client["visitorData"] = json!(v);
    }
    json!({"client": client, "request": {"internalExperimentFlags": [], "useSsl": true}, "user": {"lockedSafetyMode": false}})
}

fn ask(url: &str, body: Value, visitor: Option<&str>) -> Request {
    let mut headers = vec![
        Header::new("User-Agent", CLIENT_AGENT),
        Header::new("X-Goog-Api-Format-Version", "2"),
        Header::new("X-YouTube-Client-Name", "101"),
        Header::new("X-YouTube-Client-Version", "1.02"),
    ];
    if let Some(v) = visitor {
        headers.push(Header::new("X-Goog-Visitor-Id", v));
    }
    let body = Some(Body::Text { text: body.to_string(), content_type: "application/json".into() });
    Request { method: Method::Post, url: url.into(), headers, body, cookies: false, timeout_ms: TIMEOUT_MS }
}

/// The first ask: a visitor id.
pub fn visitor_request() -> Request {
    ask("https://www.youtube.com/youtubei/v1/visitor_id?prettyPrint=false", json!({"context": client(None)}), None)
}

/// The visitor id in its answer.
pub fn visitor(answer: &str) -> Option<String> {
    let v: Value = serde_json::from_str(answer).ok()?;
    s(&v["responseContext"], "visitorData")
}

/// The second: the video `id`'s streams, as `visitor` (`now_ms` makes its playback nonce).
pub fn player_request(id: &str, visitor: &str, now_ms: f64) -> Request {
    let cpn = format!("{:016x}", (now_ms as u64) ^ 0x5_DEEC_E66D);
    let body = json!({"context": client(Some(visitor)), "videoId": id, "cpn": cpn, "contentCheckOk": true, "racyCheckOk": true});
    ask("https://www.youtube.com/youtubei/v1/player?prettyPrint=false", body, Some(visitor))
}

/// One picture or sound file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stream {
    pub url: String,
    pub width: i64,
    pub height: i64,
    pub bitrate: i64,
    pub size: i64,
}

/// A caption track: its language, address, and whether it's YouTube's automatic one.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub language: String,
    pub url: String,
    pub automatic: bool,
}

/// A storyboard: a grid of little pictures of the video, `interval` seconds apart.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Board {
    pub template: String,
    pub w: i64,
    pub h: i64,
    pub count: i64,
    pub cols: i64,
    pub rows: i64,
    pub interval: f64,
    pub name: String,
}

/// An episode's streams.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    /// The HLS list, None when YouTube sent none.
    pub hls: Option<String>,
    pub video: Vec<Stream>,
    pub audio: Vec<Stream>,
    pub captions: Vec<Track>,
    pub boards: Vec<Board>,
}

/// The streams in the player request's answer; why not, in words, when it has none to play.
///
/// # Errors
/// The words for a video YouTube won't play, or an answer with nothing playable.
pub fn info(answer: &str) -> Result<Info, String> {
    let r: Value = serde_json::from_str(answer).map_err(|_| "YouTube's answer couldn't be read".to_string())?;
    if r["playabilityStatus"]["status"].as_str() != Some("OK") {
        return Err("YouTube wouldn't play this one".into());
    }
    let streaming = &r["streamingData"];
    let formats = objects(streaming, "adaptiveFormats");
    let streams = |mime: &str| -> Vec<Stream> {
        formats
            .iter()
            .filter(|f| t(f, "mimeType").starts_with(mime))
            .filter_map(|f| {
                Some(Stream {
                    url: s(f, "url")?,
                    width: i(f, "width").unwrap_or(0),
                    height: i(f, "height").unwrap_or(0),
                    bitrate: i(f, "averageBitrate").or_else(|| i(f, "bitrate")).unwrap_or(0),
                    size: i(f, "contentLength").unwrap_or(0),
                })
            })
            .collect()
    };
    let found = Info {
        hls: s(streaming, "hlsManifestUrl"),
        video: streams("video/mp4"),
        audio: streams("audio/mp4"),
        captions: objects(&r["captions"]["playerCaptionsTracklistRenderer"], "captionTracks")
            .iter()
            .map(|c| Track { language: t(c, "languageCode"), url: t(c, "baseUrl"), automatic: t(c, "kind") == "asr" })
            .collect(),
        boards: boards(r["storyboards"]["playerStoryboardSpecRenderer"]["spec"].as_str().unwrap_or("")),
    };
    if found.hls.is_none() && (found.video.is_empty() || found.audio.is_empty()) {
        return Err("YouTube returned no playable streams".into());
    }
    Ok(found)
}

/// "url|w#h#count#cols#rows#interval#name#sig|w#h#..." with $L for the level and $N for the name.
fn boards(spec: &str) -> Vec<Board> {
    let mut parts = spec.split('|');
    let Some(base) = parts.next() else { return Vec::new() };
    parts
        .enumerate()
        .filter_map(|(level, p)| {
            let f: Vec<&str> = p.split('#').collect();
            if f.len() < 8 {
                return None;
            }
            let n = |i: usize| f[i].parse::<i64>().unwrap_or(0);
            let mut template = base.replace("$L", &level.to_string()).replace("$N", f[6]);
            template += &format!("{}sigh={}", if template.contains('?') { "&" } else { "?" }, f[7]);
            Some(Board {
                template,
                w: n(0),
                h: n(1),
                count: n(2),
                cols: n(3),
                rows: n(4),
                interval: n(5) as f64 / 1000.,
                name: f[6].to_string(),
            })
        })
        .filter(|b| b.w > 0 && b.cols > 0 && b.rows > 0)
        .collect()
}

/// The storyboard a seek bar's pictures come from: the biggest up to 320 wide, else the smallest.
pub fn board(info: &Info) -> Option<Board> {
    let usable = || info.boards.iter().filter(|b| b.interval > 0. && b.count > 0 && b.h > 0);
    usable().filter(|b| b.w <= 320).max_by_key(|b| b.w).or_else(|| usable().min_by_key(|b| b.w)).cloned()
}

/// Where the English subtitles are as WebVTT: Some(None) when it has none (never the automatic ones).
pub fn captions(info: &Info) -> Option<String> {
    let track = info.captions.iter().find(|c| c.language.starts_with("en") && !c.automatic)?;
    let (path, query) = track.url.split_once('?').unwrap_or((&track.url, ""));
    let kept: Vec<&str> = query.split('&').filter(|p| !p.is_empty() && !p.starts_with("fmt=")).collect();
    let sep = if kept.is_empty() { "" } else { "&" };
    Some(format!("{path}?{}{sep}fmt=vtt", kept.join("&")))
}

/// What YouTube's subtitles file is asked for with.
pub fn captions_request(url: &str) -> Request {
    Request {
        method: Method::Get,
        url: url.into(),
        headers: vec![Header::new("User-Agent", AGENT)],
        body: None,
        cookies: false,
        timeout_ms: TIMEOUT_MS,
    }
}

/// A download's two files: the tallest picture at or under `height` (nothing that low: the smallest) and the best
/// sound; with their size together (0 when not known) and the picture's size.
#[derive(Clone, Debug, PartialEq)]
pub struct Files {
    pub video: String,
    pub sound: String,
    pub size: i64,
    pub width: i64,
    pub height: i64,
}

/// The files a download of `height` fetches.
pub fn files(info: &Info, height: i64) -> Option<Files> {
    let pictures: Vec<&Stream> = info.video.iter().filter(|v| v.height > 0).collect();
    let heights: Vec<i64> = pictures.iter().map(|v| v.height).collect();
    let video = pictures[pick_level(&heights, height)?];
    let sound = info.audio.iter().max_by_key(|a| a.bitrate)?;
    let size = if video.size > 0 && sound.size > 0 { video.size + sound.size } else { 0 };
    Some(Files { video: video.url.clone(), sound: sound.url.clone(), size, width: video.width, height: video.height })
}

/// The server's skip segments for a drama's episode: found, or still being listened for (`pending`).
pub fn skips(answer: &str) -> (Vec<crate::player::Segment>, bool) {
    let Ok(v) = serde_json::from_str::<Value>(answer) else { return (Vec::new(), false) };
    let found = objects(&v, "skip")
        .into_iter()
        .filter_map(|o| {
            Some(crate::player::Segment { kind: s(o, "kind")?, start: o["start"].as_f64()?, end: o["end"].as_f64() })
        })
        .collect();
    (found, b(&v, "pending"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSWER: &str = r#"{"playabilityStatus": {"status": "OK"},
        "streamingData": {"hlsManifestUrl": "https://m/hls.m3u8", "adaptiveFormats": [
            {"mimeType": "video/mp4; codecs=avc1", "url": "v1080", "width": 1920, "height": 1080, "contentLength": "100"},
            {"mimeType": "video/mp4", "url": "v720", "height": 720, "contentLength": "50"},
            {"mimeType": "audio/mp4", "url": "a1", "averageBitrate": 128000, "contentLength": "10"},
            {"mimeType": "audio/mp4", "url": "a2", "bitrate": 64000},
            {"mimeType": "video/webm", "url": "w"}]},
        "captions": {"playerCaptionsTracklistRenderer": {"captionTracks": [
            {"languageCode": "en", "baseUrl": "https://c/t?v=1&kind=asr&fmt=srv3", "kind": "asr"},
            {"languageCode": "en-GB", "baseUrl": "https://c/t?v=1&fmt=srv3"}]}},
        "storyboards": {"playerStoryboardSpecRenderer": {"spec": "https://i/$L/$N.jpg|48#27#100#10#10#2000#default#s0|160#90#100#5#5#5000#M$M#s1"}}}"#;

    #[test]
    fn an_episodes_streams() {
        let found = info(ANSWER).unwrap_or_default();
        assert_eq!(found.hls.as_deref(), Some("https://m/hls.m3u8"));
        assert_eq!(found.video.len(), 2);
        assert_eq!(captions(&found).as_deref(), Some("https://c/t?v=1&fmt=vtt"));
        let files = files(&found, 900).unwrap_or_else(|| panic!("no files"));
        assert_eq!((files.video.as_str(), files.sound.as_str(), files.size), ("v720", "a1", 60));
        let b = board(&found).unwrap_or_default();
        assert_eq!((b.w, b.interval, b.template.as_str()), (160, 5., "https://i/1/M$M.jpg?sigh=s1"));
        assert!(info(r#"{"playabilityStatus": {"status": "LOGIN_REQUIRED"}}"#).is_err());
        assert_eq!(visitor(r#"{"responseContext": {"visitorData": "vd"}}"#).as_deref(), Some("vd"));
    }

    #[test]
    fn asks_go_without_the_sign_in() {
        let r = player_request("abc", "vd", 1.);
        assert!(!r.cookies);
        assert_eq!(crate::screen::header(&r.headers, "X-Goog-Visitor-Id"), Some("vd"));
        assert!(matches!(r.body, Some(Body::Text { ref text, .. }) if text.contains("\"videoId\":\"abc\"")));
        assert_eq!(skips(r#"{"skip": [{"kind": "intro", "start": 1, "end": 9}], "pending": false}"#).0.len(), 1);
        assert!(skips(r#"{"skip": [], "pending": true}"#).1);
    }
}
