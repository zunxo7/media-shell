//! Whole flows through the queue, against a pretend device: its files, the sites it reaches, its timers.

use super::*;
use crate::screen::{test_context, AppEvent, DiskEntry, DiskReply, Header};
use std::collections::BTreeMap;

/// A device for the queue: files (their text), the web (address -> status and body), and the effects not yet run.
struct Device_ {
    q: Queue,
    cx: Context,
    files: BTreeMap<String, String>,
    web: HashMap<String, (u16, String)>,
    asked: Vec<String>,
    /// The ranges files were asked in, in order.
    ranges: Vec<String>,
    /// A range that answers this status instead.
    failing: HashMap<String, u16>,
    timers: Vec<(u32, u32)>,
    prefs: HashMap<String, String>,
    free: u64,
}

impl Device_ {
    fn new(device: Device) -> Self {
        let mut cx = test_context(1_800_000_000.);
        cx.platform.device = device;
        Self {
            q: Queue::default(),
            cx,
            files: BTreeMap::new(),
            web: HashMap::new(),
            asked: vec![],
            ranges: vec![],
            failing: HashMap::new(),
            timers: vec![],
            prefs: HashMap::new(),
            free: 50_000_000_000,
        }
    }

    fn site(&mut self, url: &str, status: u16, body: &str) {
        self.web.insert(url.into(), (status, body.into()));
    }

    fn send(&mut self, e: Event) {
        let out = self.q.update(e, &self.cx);
        self.run(out);
    }

    fn tick(&mut self, seconds: f64) {
        self.cx.clock.now += seconds;
    }

    /// Every effect carried out, and every answer given, until only timers are left.
    fn run(&mut self, effects: Vec<Effect>) {
        let mut todo: std::collections::VecDeque<Effect> = effects.into();
        // (the disk answers before the network does)
        while let Some(e) = todo.iter().position(|e| matches!(e, Effect::Disk { .. })).and_then(|i| todo.remove(i)).or_else(|| todo.pop_front()) {
            let reply = match e {
                Effect::Request { id, request } => {
                    self.asked.push(request.url.clone());
                    let (status, body) = self.web.get(&request.url).cloned().unwrap_or((404, String::new()));
                    let range = request.headers.iter().any(|h| h.name == "Range" && h.value == "bytes=0-0");
                    let headers = if range { vec![Header::new("Content-Range", &format!("bytes 0-0/{}", body.len()))] } else { vec![] };
                    Some((id, Reply::Response { status: if range && status == 200 { 206 } else { status }, headers, body }))
                }
                Effect::Fetch { id, request, file } => {
                    self.asked.push(request.url.clone());
                    let (mut status, mut body) = self.web.get(&request.url).cloned().unwrap_or((404, String::new()));
                    if let Some(range) = request.headers.iter().find(|h| h.name == "Range").map(|h| h.value.clone()) {
                        self.ranges.push(range.clone());
                        let (from, to) = range.trim_start_matches("bytes=").split_once('-').unwrap_or(("0", ""));
                        let from: usize = from.parse().unwrap_or(0);
                        let to: usize = to.parse::<usize>().map_or(body.len(), |t| (t + 1).min(body.len()));
                        if let Some(fail) = self.failing.get(&range) {
                            status = *fail;
                        } else if (200..300).contains(&status) && from >= body.len() {
                            status = 416;
                        } else if (200..300).contains(&status) {
                            body = body[from..to].to_string();
                            status = 206;
                        }
                    }
                    let ok = (200..300).contains(&status);
                    if ok {
                        self.files.insert(file, body.clone());
                    }
                    Some((id, Reply::Saved { status, bytes: if ok { body.len() as u64 } else { 0 } }))
                }
                Effect::Disk { id, op } => Some((id, Reply::Disk { disk: self.disk(op) })),
                Effect::ReadPref { id, key } => Some((id, Reply::Pref { value: self.prefs.get(&key).cloned() })),
                Effect::WritePref { key, value } => {
                    match value {
                        Some(v) => self.prefs.insert(key, v),
                        None => self.prefs.remove(&key),
                    };
                    None
                }
                Effect::Timer { id, ms } => {
                    self.timers.push((id, ms));
                    None
                }
                Effect::Cancel { id } => {
                    self.timers.retain(|t| t.0 != id);
                    None
                }
                _ => None,
            };
            if let Some((id, reply)) = reply {
                todo.extend(self.q.answer(id, reply, &self.cx));
            }
        }
    }

    /// The timers due, fired.
    fn fire(&mut self) {
        let timers = std::mem::take(&mut self.timers);
        for (id, ms) in timers {
            self.tick(f64::from(ms) / 1000.);
            let out = self.q.answer(id, Reply::Fired, &self.cx);
            self.run(out);
        }
    }

    fn disk(&mut self, op: DiskOp) -> DiskReply {
        let ok = DiskReply { ok: true, ..Default::default() };
        match op {
            DiskOp::Read { path } => DiskReply { text: self.files.get(&path).cloned(), ..ok },
            DiskOp::Write { path, text } => {
                self.files.insert(path, text);
                ok
            }
            DiskOp::Append { from, to, at } => {
                let Some(add) = self.files.remove(&from) else { return DiskReply::default() };
                let mut was = self.files.get(&to).cloned().unwrap_or_default();
                was.truncate(at as usize);
                was.push_str(&add);
                let bytes = was.len() as u64;
                self.files.insert(to, was);
                DiskReply { bytes, ..ok }
            }
            DiskOp::Rename { from, to } => match self.files.remove(&from) {
                Some(t) => {
                    self.files.insert(to, t);
                    ok
                }
                None => DiskReply { message: "not found".into(), ..Default::default() },
            },
            DiskOp::Remove { paths } => {
                for p in paths {
                    self.files.retain(|k, _| *k != p && !k.starts_with(&format!("{p}/")));
                }
                ok
            }
            DiskOp::Listing { path } => {
                let entries = self
                    .files
                    .iter()
                    .filter_map(|(k, v)| k.strip_prefix(&format!("{path}/")).filter(|rest| !rest.contains('/')).map(|n| DiskEntry { name: n.into(), bytes: v.len() as u64 }))
                    .collect();
                DiskReply { entries, ..ok }
            }
            DiskOp::Sizes { paths } => DiskReply {
                entries: paths.iter().map(|p| DiskEntry { name: p.clone(), bytes: self.files.get(p).map_or(0, |t| t.len() as u64) }).collect(),
                ..ok
            },
            DiskOp::Picture { path } => DiskReply { ok: self.files.get(&path).is_some_and(|t| t.starts_with("IMG")), ..Default::default() },
            DiskOp::Free => DiskReply { bytes: self.free, ..ok },
            DiskOp::Hash { .. } => DiskReply::default(),
        }
    }

    fn start(&mut self, profile: &str) {
        self.q.app(&AppEvent::DownloadSettings { quality: "high".into(), anime_audio: "sub".into() }, &self.cx);
        self.account(profile);
        self.send(Event::Start);
    }

    /// The core tells the queue the account, with `profile` in use.
    fn account(&mut self, profile: &str) {
        let account = crate::session::Account { profiles: vec![], current: Some(profile.into()), plus: false, max_profiles: 0 };
        self.q.app(&AppEvent::Account { account: Some(account) }, &self.cx);
    }

    fn view(&self) -> View {
        self.q.view()
    }

    fn item(&self, key: &str) -> Item {
        self.view().items.into_iter().find(|i| i.key == key).unwrap_or_default()
    }
}

fn film() -> WantedTitle {
    WantedTitle { id: "cinema-movie-tt9".into(), name: "Film".into(), source: "cinema".into(), imdb: Some("tt9".into()), minutes: Some(100), art: Some("https://i/f.jpg".into()), ..Default::default() }
}

/// The film's file, as the server hands it out: 36 bytes, four stretches of 10.
const FILE: &str = "0123456789abcdefghijABCDEFGHIJ-+-+-+";

fn real_debrid(d: &mut Device_) {
    d.site("https://host.invalid/api/downloads/cinema", 200, &json!({"url": "https://rd.x/d/1/Film.mkv", "size": FILE.len(), "quality": "1080p"}).to_string());
    d.site("https://rd.x/d/1/Film.mkv", 200, FILE);
    d.site("https://host.invalid/api/cinema/subtitles", 200, r#"{"subtitles":[{"vtt":"WEBVTT\n\nhello"}]}"#);
}

#[test]
fn a_film_comes_down_from_its_file_in_stretches_at_once() {
    for device in [Device::Phone, Device::Desktop] {
        let mut d = Device_::new(device);
        real_debrid(&mut d);
        d.start("p1");
        d.send(Event::Run { on: true });
        d.send(Event::Add { title: film(), episodes: vec![] });
        let key = "cinema-movie-tt9";
        let item = d.item(key);
        assert_eq!((item.state, item.label.as_str(), item.name.as_str()), (State::Done, "1080p", "Film"), "{}", item.line);
        // (four stretches asked at once, the last open-ended; joined in order into one file)
        assert_eq!(d.ranges, ["bytes=0-9", "bytes=10-19", "bytes=20-29", "bytes=30-"]);
        assert_eq!(d.files.get("downloads/cinema-movie-tt9.video").map(String::as_str), Some(FILE));
        assert!(!d.files.keys().any(|k| k.contains(".got/") || k.ends_with(".part")));
        assert_eq!(d.files.get("downloads/cinema-movie-tt9.vtt").map(String::as_str), Some("WEBVTT\n\nhello"));
        let local = item.local.expect("a copy to play");
        assert!(!local.hls && local.sound.is_none());
        assert_eq!(local.file, "downloads/cinema-movie-tt9.video");
        let told = d.view().told;
        assert_eq!((told[0].title.as_str(), told[0].line.as_str()), ("Film", "Ready to watch"));
    }
}

#[test]
fn a_dropped_stretch_is_tried_again_and_carries_on_from_whats_down() {
    let mut d = Device_::new(Device::Desktop);
    real_debrid(&mut d);
    d.failing.insert("bytes=20-29".into(), 503);
    d.start("p1");
    d.send(Event::Run { on: true });
    d.send(Event::Add { title: film(), episodes: vec![] });
    let key = "cinema-movie-tt9";
    assert_eq!(d.item(key).state, State::Running);
    assert_eq!(d.timers.len(), 1);
    // (the first two stretches kept)
    assert_eq!(d.files.get("downloads/cinema-movie-tt9.part").map(String::as_str), Some(&FILE[..20]));
    d.failing.clear();
    d.ranges.clear();
    d.fire();
    assert_eq!(d.item(key).state, State::Done);
    // (round two asked the server again, then only for what it didn't have)
    assert_eq!(d.ranges, ["bytes=20-29", "bytes=30-"]);
    assert_eq!(d.files.get("downloads/cinema-movie-tt9.video").map(String::as_str), Some(FILE));
}

#[test]
fn six_drops_in_a_row_fail_it_and_losing_the_connection_only_holds_it() {
    let mut d = Device_::new(Device::Phone);
    real_debrid(&mut d);
    d.site("https://rd.x/d/1/Film.mkv", 503, "");
    d.start("p1");
    d.send(Event::Run { on: true });
    d.send(Event::Add { title: film(), episodes: vec![] });
    // (a round that got further starts the count again)
    for _ in 0..20 {
        d.fire();
    }
    let item = d.item("cinema-movie-tt9");
    assert_eq!(item.state, State::Failed);
    assert_eq!(item.line, rules::DROPPED);
    d.send(Event::Act { key: "cinema-movie-tt9".into(), act: Act::Retry });
    assert_eq!(d.item("cinema-movie-tt9").state, State::Running);
    // (no connection it may use: it waits for one, in those words)
    d.cx.metered = true;
    let out = d.q.app(&AppEvent::Connectivity { online: true, metered: true }, &d.cx);
    d.run(out);
    let item = d.item("cinema-movie-tt9");
    assert_eq!((item.state, item.line.as_str()), (State::Queued, "Waiting for Wi-Fi"));
    assert!(!d.view().busy && d.view().pending);
    d.send(Event::MobileData { on: true });
    assert_eq!(d.prefs.get(MOBILE_DATA).map(String::as_str), Some("true"));
    assert_eq!(d.item("cinema-movie-tt9").state, State::Running);
}

#[test]
fn a_waiting_chapter_says_the_same_on_downloads_and_to_the_screens_that_show_it() {
    // (mobile data, Wi-Fi only: "Waiting for Wi-Fi" everywhere, a chapter's line as a film's)
    let mut d = Device_::new(Device::Phone);
    d.cx.metered = true;
    d.start("p1");
    d.send(Event::Run { on: true });
    let path = "/comics/a/chapter/3";
    d.send(Event::AddChapters { series: series(), chapters: vec![WantedChapter { path: path.into(), label: "Chapter 3".into(), thumb: None }] });
    let item = d.item(path);
    assert_eq!((item.state, item.line.as_str()), (State::Queued, "Waiting for Wi-Fi"));
    let held = d.q.snapshot();
    assert_eq!(held.get(path).map(|h| (h.state, h.line.as_str())), Some((State::Queued, "Waiting for Wi-Fi")));
    // (the PC has no mobile data to hold off)
    let mut pc = Device_::new(Device::Desktop);
    pc.cx.metered = true;
    pc.start("p1");
    pc.send(Event::Run { on: true });
    pc.send(Event::AddChapters { series: series(), chapters: vec![WantedChapter { path: path.into(), label: "Chapter 3".into(), thumb: None }] });
    assert_eq!(pc.q.snapshot().get(path).map(|h| h.line.clone()), Some(pc.item(path).line));
}

#[test]
fn offline_spots_are_kept_in_the_file_they_always_were() {
    let mut d = Device_::new(Device::Desktop);
    real_debrid(&mut d);
    // (an older app's file is read as it is)
    d.files.insert("offline.json".into(), r#"{"cinema-movie-tt9": 12.5, "gone": 3}"#.into());
    d.start("p1");
    d.send(Event::Run { on: true });
    d.send(Event::Add { title: film(), episodes: vec![] });
    assert_eq!(d.item("cinema-movie-tt9").local.map(|l| l.spot), Some(12.5));
    d.send(Event::Spot { key: "cinema-movie-tt9".into(), at: 30. });
    assert_eq!(d.item("cinema-movie-tt9").local.map(|l| l.spot), Some(30.));
    let kept: Value = serde_json::from_str(&d.files["offline.json"]).expect("an object");
    assert_eq!(kept, json!({"cinema-movie-tt9": 30.0, "gone": 3.0}));
}

#[test]
fn a_full_device_says_so() {
    let mut d = Device_::new(Device::Phone);
    real_debrid(&mut d);
    d.free = 30;
    d.start("p1");
    d.send(Event::Run { on: true });
    d.send(Event::Add { title: film(), episodes: vec![] });
    let item = d.item("cinema-movie-tt9");
    assert_eq!((item.state, item.line.as_str()), (State::Failed, rules::FULL));
}

#[test]
fn a_film_real_debrid_hasnt_got_cant_be_downloaded() {
    let mut d = Device_::new(Device::Phone);
    d.site("https://host.invalid/api/downloads/cinema", 404, r#"{"error":"This one can't be downloaded."}"#);
    d.start("p1");
    d.send(Event::Run { on: true });
    d.send(Event::Add { title: film(), episodes: vec![] });
    assert_eq!(d.item("cinema-movie-tt9").line, rules::CANT);
}

#[test]
fn anime_comes_down_joined_with_its_captions() {
    let mut d = Device_::new(Device::Phone);
    d.site("https://host.invalid/api/downloads/anime", 200, r#"{"master":"https://a.x/m.m3u8","referer":"https://mp.x/","subtitles":"https://a.x/s.vtt"}"#);
    d.site("https://a.x/m.m3u8", 200, "#EXTM3U\n#EXTINF:10,\np0.ts\n#EXTINF:10,\np1.ts\n#EXTINF:10,\np2.ts");
    for p in ["p0", "p1", "p2"] {
        d.site(&format!("https://a.x/{p}.ts"), 200, &format!("[{p}]"));
    }
    d.site("https://a.x/s.vtt", 200, "WEBVTT anime");
    d.start("p1");
    d.send(Event::Run { on: true });
    let show = WantedTitle { id: "anime-9".into(), name: "Anime".into(), source: "anime".into(), ..Default::default() };
    let ep = WantedEpisode { id: "anime-12".into(), number: Some(12), ..Default::default() };
    d.send(Event::Add { title: show, episodes: vec![ep] });
    let item = d.item("anime-12");
    assert_eq!(item.state, State::Done, "{}", item.line);
    assert_eq!(item.name, "Episode 12");
    assert_eq!(d.files.get("downloads/anime-12.video").map(String::as_str), Some("[p0][p1][p2]"));
    assert_eq!(d.files.get("downloads/anime-12.vtt").map(String::as_str), Some("WEBVTT anime"));
}

#[test]
fn youtube_comes_down_in_stretches() {
    let mut d = Device_::new(Device::Phone);
    d.site("https://www.youtube.com/youtubei/v1/visitor_id?prettyPrint=false", 200, r#"{"responseContext":{"visitorData":"V"}}"#);
    let player = json!({"playabilityStatus": {"status": "OK"}, "streamingData": {"adaptiveFormats": [
        {"mimeType": "video/mp4", "url": "https://v/pic", "height": 720, "width": 1280, "contentLength": "10"},
        {"mimeType": "audio/mp4", "url": "https://v/snd", "bitrate": 1, "contentLength": "4"}]}});
    d.site("https://www.youtube.com/youtubei/v1/player?prettyPrint=false", 200, &player.to_string());
    d.site("https://v/pic", 206, "0123456789");
    d.site("https://v/snd", 206, "abcd");
    d.start("p1");
    d.send(Event::Run { on: true });
    let drama = WantedTitle { id: "yt-show".into(), name: "Drama".into(), source: "youtube".into(), ..Default::default() };
    d.send(Event::Add { title: drama, episodes: vec![WantedEpisode { id: "yt-abc".into(), number: Some(1), ..Default::default() }] });
    let item = d.item("yt-abc");
    assert_eq!((item.state, item.label.as_str()), (State::Done, "720p"), "{}", item.line);
    assert_eq!(d.files.get("downloads/yt-abc.video").map(String::as_str), Some("0123456789"));
    assert_eq!(d.files.get("downloads/yt-abc.sound").map(String::as_str), Some("abcd"));
}

fn series() -> WantedSeries {
    WantedSeries { path: "/comics/a".into(), title: "Alpha".into(), cover: "https://c/s.jpg".into(), art: "https://c/l.jpg".into() }
}

#[test]
fn a_chapter_comes_down_page_by_page_into_its_folder() {
    let mut d = Device_::new(Device::Phone);
    let path = "/comics/a/chapter/3";
    let pages_url = format!("https://host.invalid{}", crate::manhwa::pages_path(path));
    d.site(&pages_url, 200, r#"{"images":["https://cdn.asurascans.com/asura-images/chapters/a/1.webp","https://cdn.asurascans.com/asura-images/chapters/a/2.webp"]}"#);
    d.site("https://cdn.asurascans.com/asura-images/chapters/a/1.webp", 200, "IMG1");
    d.site("https://cdn.asurascans.com/asura-images/chapters/a/2.webp", 200, "IMG2");
    d.start("p1");
    d.send(Event::Run { on: true });
    d.send(Event::AddChapters { series: series(), chapters: vec![WantedChapter { path: path.into(), label: "Chapter 3".into(), thumb: None }] });
    let item = d.item(path);
    assert_eq!(item.state, State::Done, "{}", item.line);
    let dir = transfer::chapter_dir(path);
    assert_eq!(d.files.get(&format!("{dir}/1.img")).map(String::as_str), Some("IMG2"));
    assert_eq!(d.files.get(&format!("{dir}/{}", rules::COMPLETE)).map(String::as_str), Some("2"));
    assert!(d.files.contains_key(&format!("{dir}/{}", rules::KEPT)));
    assert_eq!(item.line, "8 MB".replace("8 MB", &rules::size_words(8)));
    let told = d.view().told;
    assert_eq!((told[0].title.as_str(), told[0].line.as_str()), ("Alpha", "Chapter 3 · Ready to read"));
    // (on Downloads: the series' name over its chapters)
    let page = d.view().page;
    assert_eq!(page.sections.len(), 1);
    assert_eq!(page.sections[0].heading.as_deref(), Some("Alpha"));
    assert_eq!(page.sections[0].entries.iter().map(|e| (e.kind, e.open)).collect::<Vec<_>>(), [(EntryKind::Chapter, Open::Read)]);
    // (a page that isn't a picture is never kept)
    let mut d2 = Device_::new(Device::Phone);
    d2.web = d.web.clone();
    d2.site("https://cdn.asurascans.com/asura-images/chapters/a/2.webp", 200, "<html>");
    d2.start("p1");
    d2.send(Event::Run { on: true });
    d2.send(Event::AddChapters { series: series(), chapters: vec![WantedChapter { path: path.into(), label: "Chapter 3".into(), thumb: None }] });
    for _ in 0..20 {
        d2.fire();
    }
    assert_eq!(d2.item(path).line, rules::PAGE_MISSING);
}

#[test]
fn a_series_is_weighed_from_its_chapters_pages_and_kept_for_good() {
    let mut d = Device_::new(Device::Phone);
    let chapters: Vec<String> = (1..=4).map(|n| format!("/comics/a/chapter/{n}")).collect();
    for (n, c) in chapters.iter().enumerate().take(3) {
        let page = format!("https://cdn.asurascans.com/asura-images/chapters/a/{n}.webp");
        d.site(&format!("https://host.invalid{}", crate::manhwa::pages_path(c)), 200, &format!(r#"{{"images":["{page}"]}}"#));
        d.site(&page, 200, &"x".repeat(100 * (n + 1)));
    }
    // (one on the phone already, weighed by its files)
    let dir = transfer::chapter_dir(&chapters[3]);
    d.files.insert(format!("{dir}/0.img"), "y".repeat(599));
    d.files.insert(format!("{dir}/{}", rules::COMPLETE), "1".into());
    d.start("p1");
    d.send(Event::WeighChapters { series: "/comics/a".into(), chapters: chapters.clone() });
    let size = |d: &Device_| d.view().sizes.iter().find(|s| s.title == "/comics/a").map(|s| s.bytes);
    assert_eq!(size(&d), Some(Some(1200)));
    let kept = d.files.get(weigh::SIZES).cloned().unwrap_or_default();
    assert!(kept.contains("/comics/a/chapter/1") && !kept.contains("/comics/a/chapter/4"), "{kept}");
    // (a chapter's pages never change: weighed once, even weeks on)
    d.web.clear();
    d.tick(60. * 24. * 3600.);
    d.asked.clear();
    d.send(Event::WeighChapters { series: "/comics/a".into(), chapters });
    assert_eq!(size(&d), Some(Some(1200)));
    assert!(d.asked.is_empty(), "{:?}", d.asked);
}

#[test]
fn whats_kept_is_read_back_and_androids_chapters_join_the_list() {
    let mut d = Device_::new(Device::Phone);
    let list = json!([
        {"key":"done","titleId":"t","title":"Done","copy":"anime|1|2|sub","label":"High","quality":"1080p","size":100,"state":"DONE"},
        {"key":"gone","titleId":"t","title":"Gone","copy":"anime|1|2|sub","label":"720p","quality":"720p","size":100,"state":"DONE"},
        {"key":"half","titleId":"t","title":"Half","copy":"anime|1|2|sub","label":"720p","quality":"720p","size":100,"state":"RUNNING"}]);
    d.files.insert(row::LIST.into(), list.to_string());
    d.files.insert("downloads/done.video".into(), "x".repeat(100));
    d.files.insert("downloads/half.part".into(), "x".repeat(40));
    // (Android's chapters from before: one waiting in its queue, one kept on purpose, one only read)
    d.files.insert("manhwa/queue-abc.json".into(), r#"[{"series":"/comics/a","title":"Alpha","cover":"c","chapter":"/comics/a/chapter/4","label":"Chapter 4","state":"QUEUED","note":""}]"#.into());
    d.files.insert("manhwa/catalog.json".into(), r#"[{"path":"/comics/a","title":"Alpha","cover":"https://c/l.jpg","thumb":"https://c/s.jpg"}]"#.into());
    let series_list = format!("manhwa/{}.json", rules::chapter_folder("/comics/a"));
    d.files.insert(series_list, r#"[{"path":"/comics/a/chapter/1","label":"Chapter 1"},{"path":"/comics/a/chapter/2","label":"Chapter 2"}]"#.into());
    for (chapter, kept) in [("/comics/a/chapter/1", true), ("/comics/a/chapter/2", false)] {
        let dir = transfer::chapter_dir(chapter);
        d.files.insert(format!("{dir}/{}", rules::COMPLETE), "1".into());
        d.files.insert(format!("{dir}/0.img"), "IMG".into());
        if kept {
            d.files.insert(format!("{dir}/{}", rules::KEPT), "1".into());
        }
    }
    d.start("p1");
    let keys: Vec<String> = d.view().items.iter().map(|i| i.key.clone()).collect();
    assert_eq!(keys, ["done", "half", "/comics/a/chapter/1", "/comics/a/chapter/4"]);
    let half = d.item("half");
    assert_eq!((half.state, half.done), (State::Queued, 40));
    assert_eq!(d.item("done").label, "1080p");
    assert_eq!(d.item("/comics/a/chapter/1").state, State::Done);
    assert!(!d.files.contains_key("manhwa/queue-abc.json"));
    assert!(d.files.contains_key(CHAPTERS_MOVED));
    // (and the list kept with them)
    assert_eq!(row::read_list(d.files.get(row::LIST).map_or("", String::as_str)).len(), 4);
    // (what was asked before it was all read waits for it)
    let mut d = Device_::new(Device::Desktop);
    d.files.insert(row::LIST.into(), "[]".into());
    let out = d.q.update(Event::Start, &d.cx);
    let early = d.q.update(Event::Delete { key: "x".into() }, &d.cx);
    assert!(early.is_empty());
    d.run(out);
    assert!(d.view().page.empty.is_some());
}

#[test]
fn each_profile_its_own_and_deleting_asks_first() {
    let mut d = Device_::new(Device::Phone);
    real_debrid(&mut d);
    d.start("p1");
    d.send(Event::Run { on: true });
    d.send(Event::Add { title: film(), episodes: vec![] });
    d.account("p2");
    assert!(d.view().items.is_empty());
    // (saved already by another profile: now this one's too, nothing fetched again)
    let asked = d.asked.len();
    d.send(Event::Add { title: film(), episodes: vec![] });
    assert_eq!(d.asked.len(), asked);
    assert_eq!(d.view().items.len(), 1);
    d.send(Event::Delete { key: "cinema-movie-tt9".into() });
    assert!(d.view().page.sections[0].entries[0].asking && d.view().page.asking);
    d.send(Event::Delete { key: "cinema-movie-tt9".into() });
    assert!(d.view().items.is_empty());
    // (still p1's, files and all)
    d.account("p1");
    assert_eq!(d.view().items.len(), 1);
    d.send(Event::Delete { key: "cinema-movie-tt9".into() });
    d.send(Event::Dismiss);
    d.send(Event::Delete { key: "cinema-movie-tt9".into() });
    assert_eq!(d.view().items.len(), 1);
    d.send(Event::Delete { key: "cinema-movie-tt9".into() });
    assert!(d.view().items.is_empty());
    d.fire();
    assert!(!d.files.keys().any(|k| k.starts_with("downloads/")));
}

#[test]
fn a_show_on_downloads_is_one_entry_that_opens_on_its_episodes() {
    let mut d = Device_::new(Device::Phone);
    d.start("p1");
    let show = WantedTitle { id: "cinema-series-tt1".into(), name: "Show".into(), source: "cinema".into(), imdb: Some("tt1".into()), art: Some("art".into()), ..Default::default() };
    let ep = |n: i64| WantedEpisode { id: format!("e{n}"), season: Some(1), number: Some(n), still: Some(format!("s{n}")), ..Default::default() };
    d.send(Event::Add { title: show, episodes: vec![ep(2), ep(1)] });
    let page = d.view().page;
    assert_eq!(page.sections.len(), 1);
    assert_eq!(page.sections[0].entries.len(), 1);
    let e = &page.sections[0].entries[0];
    assert_eq!((e.kind, e.key.as_str(), e.badge.as_str(), e.picture.as_deref()), (EntryKind::Show, "show:cinema-series-tt1", "2 episodes", Some("art")));
    assert_eq!((e.open, e.show.as_deref()), (Open::Show, Some("cinema-series-tt1")));
    assert_eq!(e.line, "0 of 2 episodes
0 MB of 0 MB");
    d.send(Event::Show { title: Some("cinema-series-tt1".into()) });
    let page = d.view().page;
    assert_eq!(page.heading, "Show");
    assert_eq!(page.sections[0].entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["S1 E1", "S1 E2"]);
    d.send(Event::Show { title: None });
    // (what's searched for is taken as it's trimmed)
    d.send(Event::Search { query: "  zzz ".into() });
    assert_eq!(d.view().page.empty.map(|e| e.title), Some("Nothing found".into()));
    let note = d.view().note.expect("a note");
    assert_eq!((note.title.as_str(), note.detail.as_str()), ("Downloading 2 videos", "0 MB of 0 MB"));
    // (the whole show deleted at once)
    d.send(Event::Search { query: String::new() });
    d.send(Event::Delete { key: "show:cinema-series-tt1".into() });
    d.send(Event::Delete { key: "show:cinema-series-tt1".into() });
    assert!(d.view().items.is_empty());
}

#[test]
fn a_title_page_is_told_what_it_will_weigh() {
    let mut d = Device_::new(Device::Phone);
    real_debrid(&mut d);
    d.start("p1");
    d.send(Event::Weigh { title: film(), episodes: vec![] });
    // (the file's size, as the server says it)
    assert_eq!(d.view().sizes, [Weighed { title: "cinema-movie-tt9".into(), bytes: Some(FILE.len() as u64) }]);
    // (kept: the next time it's said at once, with nothing asked)
    let asked = d.asked.len();
    d.send(Event::Weigh { title: film(), episodes: vec![] });
    assert_eq!(d.asked.len(), asked);
    assert_eq!(d.view().sizes[0].bytes, Some(FILE.len() as u64));
    let drama = WantedTitle { id: "y".into(), source: "youtube".into(), ..Default::default() };
    d.send(Event::Weigh { title: drama, episodes: vec![] });
    assert_eq!(d.view().sizes[1].bytes, None);
    assert!(!offered(Device::Tv, &film(), None, "sub"));
    assert!(offered(Device::Phone, &film(), None, "sub"));
}

fn video(episode: Option<&str>, here: bool) -> Item {
    Item {
        key: format!("k-{}", episode.unwrap_or("film")),
        title_id: "t".into(),
        episode_id: episode.map(String::from),
        title: "Show".into(),
        source: "cinema".into(),
        picture: Some("https://i/p.jpg".into()),
        episode: episode.map(|e| e.len() as i64),
        season: Some(1),
        length: 3000.,
        local: here.then(LocalCopy::default),
        ..Default::default()
    }
}

#[test]
fn offline_a_title_is_what_downloads_have_of_it() {
    let items = [video(Some("e1"), true), video(Some("e22"), false), video(Some("e3"), true), Item { title_id: "other".into(), ..video(None, true) }];
    // (a show: only the episodes that are here, in order)
    let show = offline_title(&items, "t").expect("a title");
    assert_eq!((show.kind.as_str(), show.film, show.minutes), ("series", false, None));
    assert_eq!(show.episodes.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["e1", "e3"]);
    assert_eq!(show.episodes[0].length, 3000.);
    // (a film: its length in minutes)
    let film = offline_title(&items, "other").expect("a film");
    assert_eq!((film.kind.as_str(), film.film, film.minutes, film.episodes.len()), ("movie", true, Some(50), 0));
    assert!(offline_title(&items, "none").is_none());
}

#[test]
fn only_a_finished_copy_plays() {
    let items = [video(Some("e1"), false), video(Some("e1"), true), video(Some("e2"), false), Item { title_id: "film".into(), ..video(None, true) }];
    assert!(playable(&items, "t", Some("e1")).is_some());
    assert!(playable(&items, "t", Some("e2")).is_none() && playable(&items, "t", Some("e9")).is_none());
    // (a film is asked for with no episode, or its own id as one)
    assert!(playable(&items, "film", None).is_some() && playable(&items, "film", Some("film")).is_some());
}

#[test]
fn the_app_is_told_when_the_job_is_running_and_asks_about_notifications_once() {
    let mut d = Device_::new(Device::Phone);
    d.start("p1");
    assert!(!d.view().running);
    d.send(Event::Run { on: true });
    assert!(d.view().running);
    assert!(d.view().ask_notes);
    d.send(Event::NotesAsked);
    assert!(!d.view().ask_notes);
    assert_eq!(d.prefs.get(ASKED_NOTES).map(String::as_str), Some("true"));
    // (kept: not asked again when the app starts next)
    let mut d2 = Device_::new(Device::Phone);
    d2.prefs.insert(ASKED_NOTES.into(), "true".into());
    d2.start("p1");
    assert!(!d2.view().ask_notes);
}
