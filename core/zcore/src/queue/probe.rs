//! Finding where a download comes from: a film's or an episode's file (Real-Debrid's, else Easynews's, as the server
//! picks it), the anime source's copy, an ARY stream or YouTube's files, and the level a download of it takes. Fresh
//! each round of a transfer (links run out); also what a title page's size is weighed from.

use super::hls;
use crate::hls::{written, Level};
use super::{Fail, Io};
use crate::screen::{header, Body, Context, Header, Method, Reply, Request};
use serde_json::{json, Value};

/// What a source with no Referer of its own is asked with: a browser's.
pub const BROWSER: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
/// What YouTube's files and captions are asked with.
pub const YOUTUBE_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; rv:128.0) Gecko/20100101 Firefox/128.0";
/// ARY's streams want their own site as the Referer.
pub const ARY_REFERER: &str = "https://aryplus.tv/";
/// How long a request to a source may take.
pub const SOURCE_TIMEOUT_MS: u32 = 60_000;
/// Where a film's or an episode's file is asked for.
pub const CINEMA_PATH: &str = "/api/downloads/cinema";
/// YouTube's player requests.
const YOUTUBE_TIMEOUT_MS: u32 = 10_000;
const YOUTUBE_CLIENT: &str = "com.google.ios.youtube/1.02 (RealityDevice14,1; U; CPU visionOS 25_6_0 like Mac OS X; US)";

/// A request to a source: as a browser (no `referer`), or with the Referer the anime and ARY sources want.
/// Never with the sign-in.
pub fn source_request(url: &str, referer: Option<&str>, extra: Vec<Header>) -> Request {
    let mut headers = vec![Header::new("Accept", "*/*")];
    match referer {
        None => headers.push(Header::new("User-Agent", BROWSER)),
        Some(r) => headers.extend([
            Header::new("Referer", r),
            Header::new("Origin", r.trim_end_matches('/')),
            Header::new("User-Agent", "Mozilla/5.0"),
        ]),
    }
    headers.extend(extra);
    Request { method: Method::Get, url: url.into(), headers, body: None, cookies: false, timeout_ms: SOURCE_TIMEOUT_MS }
}

/// A call to the zunflix server.
pub fn server_post(cx: &Context, path: &str, body: &Value) -> Request {
    crate::api::post(&cx.base, &cx.who(), path, body.to_string())
}

/// What the server knows a film (`episode` None) or an episode by.
pub fn ids(imdb: &str, season: Option<i64>, episode: Option<i64>) -> Value {
    match episode {
        None => json!({"type": "movie", "imdb": imdb, "season": null, "episode": null}),
        Some(n) => json!({"type": "series", "imdb": imdb, "season": season, "episode": n}),
    }
}

/// Where a download's copy comes from, read from what's kept with it ([`crate::downloads::copy_of`]).
#[derive(Clone, Debug, PartialEq)]
pub enum Copy {
    /// Movies & TV: one file of what `ids` names (saved as "dulo|...", as downloads always were).
    Cinema { ids: Value },
    Anime { episode: String, show: String, audio: String },
    Ary { stream: String },
    YouTube { id: String },
}

impl Copy {
    /// None for a copy it doesn't know (its record damaged).
    pub fn read(copy: &str, imdb: Option<&str>, season: Option<i64>, episode: Option<i64>) -> Option<Copy> {
        let mut parts = copy.split('|');
        match parts.next()? {
            "dulo" => Some(Copy::Cinema { ids: ids(imdb?, season, episode) }),
            "anime" => {
                let (e, s, a) = (parts.next()?, parts.next()?, parts.next().unwrap_or("sub"));
                Some(Copy::Anime { episode: e.into(), show: s.into(), audio: a.into() })
            }
            "ary" => Some(Copy::Ary { stream: copy.strip_prefix("ary|")?.into() }),
            "yt" => Some(Copy::YouTube { id: parts.next()?.into() }),
            _ => None,
        }
    }
}

/// YouTube's two files for a download, and the picture's size (to name its level).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Files {
    pub video: String,
    pub video_size: u64,
    pub sound: String,
    pub sound_size: u64,
    pub width: i64,
    pub height: i64,
    /// The English captions' address, as WebVTT (never the automatic ones).
    pub captions: Option<String>,
}

/// A film's or an episode's file, as the server gives it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct File {
    pub url: String,
    pub size: u64,
    /// "2160p", "1080p"...
    pub quality: String,
}

/// What was found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    /// The master playlist's address (YouTube: none).
    pub master: String,
    /// The anime and ARY sources' Referer; None for dulo.
    pub referer: Option<String>,
    /// The level taken (None: the master is a level of its own).
    pub level: Option<Level>,
    /// The level's playlist: its address and text.
    pub at: String,
    pub playlist: String,
    /// The anime source's captions.
    pub subtitles: Option<String>,
    pub youtube: Option<Files>,
    /// Movies & TV: the one file.
    pub file: Option<File>,
}

/// Things worth keeping between lookups (none now).
#[derive(Debug, Default)]
pub struct Known {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Cinema,
    Master,
    Level,
    Anime,
    Visitor,
    Player,
}

/// A lookup under way. Its answers come in through [`Probe::answer`]; it ends with Some(what was found), None (the
/// source hasn't got it), or a failure.
#[derive(Debug)]
pub struct Probe {
    copy: Copy,
    height: i64,
    /// Asks for the sound's list too (a transfer; a size needs only the picture's).
    with_sound: bool,
    found: Found,
    visitor: String,
}

pub type Ended = Option<Result<Option<Found>, Fail>>;

fn ok_body(reply: &Reply) -> Option<&str> {
    match reply {
        Reply::Response { status: 200..=299, body, .. } => Some(body),
        _ => None,
    }
}

impl Probe {
    pub fn new(copy: Copy, height: i64, with_sound: bool) -> Self {
        Self { copy, height, with_sound, found: Found::default(), visitor: String::new() }
    }

    pub fn start(&mut self, cx: &Context, _known: &Known) -> Vec<(Step, Io)> {
        match &self.copy {
            Copy::Cinema { ids } => {
                // (a size alone needs no link: none is made)
                let mut body = ids.clone();
                body["height"] = json!(self.height);
                body["codecs"] = json!(cx.platform.codecs);
                body["link"] = json!(self.with_sound);
                vec![(Step::Cinema, Io::Ask(server_post(cx, CINEMA_PATH, &body)))]
            }
            Copy::Anime { episode, show, audio } => {
                let body = json!({"episode": episode, "show": show, "audio": audio});
                vec![(Step::Anime, Io::Ask(server_post(cx, "/api/downloads/anime", &body)))]
            }
            Copy::Ary { stream } => {
                self.found.master = crate::api::url(&cx.base, stream);
                self.found.referer = Some(ARY_REFERER.into());
                vec![(Step::Master, Io::Ask(source_request(&self.found.master, Some(ARY_REFERER), vec![])))]
            }
            Copy::YouTube { .. } => {
                vec![(Step::Visitor, Io::Ask(youtube_ask("visitor_id", &json!({"context": youtube_context(None)}), None)))]
            }
        }
    }

    /// The answer to its `step`: what to ask next, and how it ended once it has.
    pub fn answer(&mut self, step: Step, reply: Reply, cx: &Context, _known: &mut Known) -> (Vec<(Step, Io)>, Ended) {
        let dropped = || (vec![], Some(Err(Fail::Dropped(crate::downloads::DROPPED.into()))));
        match step {
            Step::Cinema => match crate::api::outcome(reply, cx.online) {
                crate::api::Outcome::Body { body, .. } => {
                    let v: Value = serde_json::from_str(&body).unwrap_or_default();
                    let file = File {
                        url: v["url"].as_str().unwrap_or_default().into(),
                        size: v["size"].as_u64().unwrap_or(0),
                        quality: v["quality"].as_str().unwrap_or_default().into(),
                    };
                    if self.with_sound && file.url.is_empty() || !self.with_sound && file.size == 0 {
                        return (vec![], Some(Ok(None)));
                    }
                    self.found.file = Some(file);
                    (vec![], Some(Ok(Some(self.found.clone()))))
                }
                // (neither has it)
                crate::api::Outcome::Failed { failure } if failure.status() == 404 => (vec![], Some(Ok(None))),
                crate::api::Outcome::Failed { failure } if (400..500).contains(&failure.status()) => (vec![], Some(Err(Fail::Cant(failure.words)))),
                _ => dropped(),
            },
            Step::Anime => match crate::api::outcome(reply, cx.online) {
                crate::api::Outcome::Body { body, .. } => {
                    let v: Value = serde_json::from_str(&body).unwrap_or_default();
                    let (master, referer) = (v["master"].as_str().unwrap_or_default(), v["referer"].as_str().unwrap_or_default());
                    if master.is_empty() {
                        return (vec![], Some(Err(Fail::Cant(crate::downloads::CANT.into()))));
                    }
                    self.found.master = crate::api::url(&cx.base, master);
                    self.found.referer = Some(referer.into());
                    self.found.subtitles = v["subtitles"].as_str().filter(|s| !s.is_empty()).map(|s| crate::api::url(&cx.base, s));
                    (vec![(Step::Master, Io::Ask(source_request(&self.found.master, Some(referer), vec![])))], None)
                }
                crate::api::Outcome::Failed { failure } if failure.status() != 0 => (vec![], Some(Err(Fail::Cant(failure.words)))),
                _ => dropped(),
            },
            Step::Master => {
                let Some(body) = ok_body(&reply) else { return dropped() };
                let Some(level) = hls::pick(hls::levels(body, &self.found.master), self.height) else {
                    if self.found.referer.is_none() {
                        return dropped();
                    }
                    // (a master with no levels is its own)
                    self.found.at = self.found.master.clone();
                    self.found.playlist = body.into();
                    return (vec![], Some(Ok(Some(self.found.clone()))));
                };
                let at = level.url.clone();
                self.found.level = Some(level);
                self.found.at = at.clone();
                (vec![(Step::Level, Io::Ask(source_request(&at, self.found.referer.as_deref(), vec![])))], None)
            }
            Step::Level => {
                let Some(body) = ok_body(&reply) else { return dropped() };
                if written(body).pieces.is_empty() {
                    return dropped();
                }
                self.found.playlist = body.into();
                (vec![], Some(Ok(Some(self.found.clone()))))
            }
            Step::Visitor => {
                let Some(body) = ok_body(&reply) else { return dropped() };
                let v: Value = serde_json::from_str(body).unwrap_or_default();
                let Some(visitor) = v["responseContext"]["visitorData"].as_str() else { return dropped() };
                self.visitor = visitor.into();
                let Copy::YouTube { id } = &self.copy else { return dropped() };
                let cpn = format!("{:016x}", (cx.clock.now * 1000.) as u64 ^ 0x5DEE_CE66D);
                let body = json!({"context": youtube_context(Some(visitor)), "videoId": id, "cpn": cpn, "contentCheckOk": true, "racyCheckOk": true});
                (vec![(Step::Player, Io::Ask(youtube_ask("player", &body, Some(visitor))))], None)
            }
            Step::Player => {
                let Some(body) = ok_body(&reply) else { return dropped() };
                match youtube_files(body, self.height) {
                    Some(files) => {
                        self.found.youtube = Some(files);
                        (vec![], Some(Ok(Some(self.found.clone()))))
                    }
                    None => dropped(),
                }
            }
        }
    }
}

fn youtube_context(visitor: Option<&str>) -> Value {
    let mut client = json!({"clientName": "VISIONOS", "clientVersion": "1.02", "clientScreen": "WATCH", "platform": "MOBILE",
        "deviceMake": "Apple", "deviceModel": "RealityDevice14,1", "osName": "visionOS", "osVersion": "25.6.0.23O471",
        "hl": "en", "gl": "US", "utcOffsetMinutes": 0});
    if let Some(v) = visitor {
        client["visitorData"] = json!(v);
    }
    json!({"client": client, "request": {"internalExperimentFlags": [], "useSsl": true}, "user": {"lockedSafetyMode": false}})
}

/// A call to YouTube's player API, as the Vision Pro's client makes it: it answers a home connection with no account,
/// with files that stay fetchable past their first few MB.
fn youtube_ask(what: &str, body: &Value, visitor: Option<&str>) -> Request {
    let mut headers = vec![
        Header::new("User-Agent", YOUTUBE_CLIENT),
        Header::new("X-Goog-Api-Format-Version", "2"),
        Header::new("X-YouTube-Client-Name", "101"),
        Header::new("X-YouTube-Client-Version", "1.02"),
    ];
    if let Some(v) = visitor {
        headers.push(Header::new("X-Goog-Visitor-Id", v));
    }
    Request {
        method: Method::Post,
        url: format!("https://www.youtube.com/youtubei/v1/{what}?prettyPrint=false"),
        headers,
        body: Some(Body::Text { text: body.to_string(), content_type: "application/json".into() }),
        cookies: false,
        timeout_ms: YOUTUBE_TIMEOUT_MS,
    }
}

/// A download's two files from YouTube's player answer: the tallest picture at or under `height` (nothing that low:
/// the smallest), and the best sound. None when it won't play or has neither.
pub fn youtube_files(body: &str, height: i64) -> Option<Files> {
    let r: Value = serde_json::from_str(body).ok()?;
    if r["playabilityStatus"]["status"].as_str() != Some("OK") {
        return None;
    }
    let number = |f: &Value, k: &str| f[k].as_i64().or_else(|| f[k].as_str().and_then(|s| s.parse().ok())).unwrap_or(0);
    let formats = r["streamingData"]["adaptiveFormats"].as_array().cloned().unwrap_or_default();
    let of = |mime: &str| -> Vec<&Value> {
        formats.iter().filter(|f| f["mimeType"].as_str().is_some_and(|m| m.starts_with(mime)) && f["url"].is_string()).collect()
    };
    let pictures: Vec<&Value> = of("video/mp4").into_iter().filter(|f| number(f, "height") > 0).collect();
    let heights: Vec<i64> = pictures.iter().map(|f| number(f, "height")).collect();
    let video = pictures.get(crate::quality::pick_level(&heights, height)?)?;
    let sound = of("audio/mp4").into_iter().max_by_key(|f| if number(f, "averageBitrate") > 0 { number(f, "averageBitrate") } else { number(f, "bitrate") })?;
    let tracks = r["captions"]["playerCaptionsTracklistRenderer"]["captionTracks"].as_array().cloned().unwrap_or_default();
    let captions = tracks
        .iter()
        .find(|t| t["languageCode"].as_str().is_some_and(|l| l.starts_with("en")) && t["kind"].as_str() != Some("asr"))
        .and_then(|t| t["baseUrl"].as_str())
        .and_then(|u| url::Url::parse(u).ok())
        .map(|mut u| {
            let kept: Vec<(String, String)> = u.query_pairs().filter(|(k, _)| k != "fmt").map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
            u.query_pairs_mut().clear().extend_pairs(kept).append_pair("fmt", "vtt");
            u.to_string()
        });
    Some(Files {
        video: video["url"].as_str()?.into(),
        video_size: number(video, "contentLength").max(0) as u64,
        sound: sound["url"].as_str()?.into(),
        sound_size: number(sound, "contentLength").max(0) as u64,
        width: number(video, "width"),
        height: number(video, "height"),
        captions,
    })
}

/// The whole size a ranged answer says ("bytes 0-0/12345"), or a whole answer's length.
pub fn whole_size(reply: &Reply) -> Option<u64> {
    let Reply::Response { status, headers, body } = reply else { return None };
    let ranged = header(headers, "Content-Range").and_then(|r| r.rsplit_once('/')).and_then(|(_, n)| n.trim().parse().ok());
    match (status, ranged) {
        (_, Some(n)) => Some(n),
        (200, None) => header(headers, "Content-Length").and_then(|n| n.trim().parse().ok()).or(Some(body.len() as u64)),
        _ => None,
    }
    .filter(|n| *n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{response, test_context};

    fn asked(io: &[(Step, Io)]) -> (Step, &Request) {
        match io {
            [(step, Io::Ask(r))] => (*step, r),
            other => panic!("expected one request, got {other:?}"),
        }
    }

    const LEVEL: &str = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4,\n0.m4s\n#EXTINF:4,\n1.m4s\n#EXT-X-ENDLIST";

    #[test]
    fn a_film_s_file_as_the_server_picks_it() {
        let cx = test_context(1000.);
        let mut known = Known::default();
        let copy = Copy::read("dulo|cinema-series-tt1|e", Some("tt1"), Some(1), Some(2)).expect("a copy");
        let mut p = Probe::new(copy, 1080, true);
        let io = p.start(&cx, &known);
        let (step, r) = asked(&io);
        assert_eq!((step, r.url.as_str(), r.cookies), (Step::Cinema, "https://host.invalid/api/downloads/cinema", true));
        let Some(crate::screen::Body::Text { text, .. }) = &r.body else { panic!() };
        let sent: Value = serde_json::from_str(text).unwrap_or_default();
        assert_eq!((sent["imdb"].as_str(), sent["season"].as_i64(), sent["episode"].as_i64(), sent["height"].as_i64(), sent["link"].as_bool()),
                   (Some("tt1"), Some(1), Some(2), Some(1080), Some(true)));
        let (_, end) = p.answer(Step::Cinema, response(200, r#"{"url":"https://rd.x/d/1/f.mkv","size":900,"quality":"1080p"}"#, None), &cx, &mut known);
        let found = end.and_then(|e| e.ok()).flatten().expect("found");
        assert_eq!(found.file, Some(File { url: "https://rd.x/d/1/f.mkv".into(), size: 900, quality: "1080p".into() }));
        // (neither has it)
        let mut p = Probe::new(Copy::read("dulo|x|", Some("tt1"), None, None).expect("a copy"), 1080, true);
        p.start(&cx, &known);
        let (_, end) = p.answer(Step::Cinema, response(404, r#"{"error":"Not available."}"#, None), &cx, &mut known);
        assert_eq!(end, Some(Ok(None)));
    }

    #[test]
    fn the_anime_source_says_where() {
        let cx = test_context(1000.);
        let mut known = Known::default();
        let mut p = Probe::new(Copy::read("anime|12|9|dub", None, None, None).expect("a copy"), 720, true);
        let io = p.start(&cx, &known);
        let Some(crate::screen::Body::Text { text, .. }) = &asked(&io).1.body else { panic!() };
        assert_eq!(serde_json::from_str::<Value>(text).ok(), Some(json!({"episode": "12", "show": "9", "audio": "dub"})));
        let (io, _) = p.answer(Step::Anime, response(200, r#"{"master":"https://a.x/m.m3u8","referer":"https://mp.x/","subtitles":"https://a.x/s.vtt"}"#, None), &cx, &mut known);
        let r = asked(&io).1;
        assert_eq!(header(&r.headers, "Referer"), Some("https://mp.x/"));
        assert_eq!(header(&r.headers, "Origin"), Some("https://mp.x"));
        // (no levels: the master is the list)
        let (_, end) = p.answer(Step::Master, response(200, LEVEL, None), &cx, &mut known);
        let found = end.and_then(|e| e.ok()).flatten().expect("found");
        assert_eq!((found.at.as_str(), found.subtitles.as_deref()), ("https://a.x/m.m3u8", Some("https://a.x/s.vtt")));
        // (the server's no is the viewer's)
        let mut p = Probe::new(Copy::read("anime|1|2|sub", None, None, None).expect("a copy"), 720, true);
        p.start(&cx, &known);
        let (_, end) = p.answer(Step::Anime, response(404, r#"{"error":"This title isn't available."}"#, None), &cx, &mut known);
        assert_eq!(end, Some(Err(Fail::Cant("This title isn't available.".into()))));
    }

    #[test]
    fn youtubes_files_picked() {
        let body = json!({"playabilityStatus": {"status": "OK"}, "streamingData": {"adaptiveFormats": [
            {"mimeType": "video/mp4; codecs=x", "url": "https://v/1080", "height": 1080, "width": 1920, "contentLength": "900"},
            {"mimeType": "video/mp4; codecs=x", "url": "https://v/720", "height": 720, "width": 1280, "contentLength": "500"},
            {"mimeType": "audio/mp4", "url": "https://a/lo", "bitrate": 64000, "contentLength": "10"},
            {"mimeType": "audio/mp4", "url": "https://a/hi", "bitrate": 128000, "contentLength": "20"}]},
            "captions": {"playerCaptionsTracklistRenderer": {"captionTracks": [
                {"languageCode": "en", "kind": "asr", "baseUrl": "https://c/auto"},
                {"languageCode": "en-GB", "baseUrl": "https://c/t?lang=en&fmt=srv3"}]}}}).to_string();
        let f = youtube_files(&body, 720).expect("files");
        assert_eq!((f.video.as_str(), f.video_size, f.sound.as_str(), f.sound_size), ("https://v/720", 500, "https://a/hi", 20));
        assert_eq!(f.captions.as_deref(), Some("https://c/t?lang=en&fmt=vtt"));
        assert_eq!(youtube_files(r#"{"playabilityStatus":{"status":"ERROR"}}"#, 720), None);
        let ranged = Reply::Response { status: 206, headers: vec![Header::new("content-range", "bytes 0-0/4321")], body: "x".into() };
        assert_eq!(whole_size(&ranged), Some(4321));
        assert_eq!(whole_size(&response(404, "", None)), None);
    }
}
