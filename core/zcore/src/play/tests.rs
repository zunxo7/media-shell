//! The player's flows, events in and effects and views out, on fake answers and a fake clock.

use super::*;
use crate::screen::{response, test_context, Body};

const FILM: &str = r#"{"id": "cinema-movie-tt1234567", "title": "Dune", "source": "cinema", "type": "movie",
    "imdb": "tt1234567", "year": "2021", "minutes": 155, "cover": "c.jpg"}"#;
const SHOW: &str = r#"{"id": "s", "title": "Show", "source": "cinema", "type": "series", "imdb": "tt7654321",
    "episodes": [{"id": "e1", "n": 1, "season": 1}, {"id": "e2", "n": 2, "season": 1}]}"#;
const MASTER: &str = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=6000000,RESOLUTION=3840x1600,FRAME-RATE=24\n4k.m3u8\n\
    #EXT-X-STREAM-INF:BANDWIDTH=3000000,RESOLUTION=1920x800\n1080.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=900000,RESOLUTION=1280x536\n720.m3u8\n";

const NO_COPIES: &str = r#"{"copies": []}"#;
const ONE: &str = r#"{"copies": [{"id": "fhd", "quality": "1080p"}]}"#;

/// The link answer for `id`'s copy: an HLS playlist, so its levels are read.
fn linked(id: u32) -> String {
    serde_json::json!({"url": master(id)}).to_string()
}

fn master(id: u32) -> String {
    format!("https://cdn.vq/films/{id}/master.m3u8")
}

struct Run {
    play: Play,
    now: f64,
    out: Vec<Effect>,
    /// Everything asked for so far: answers may come in any order.
    sent: Vec<Effect>,
}

fn settings() -> Settings {
    Settings { profile: "p1".into(), ..Settings::default() }
}

fn open(title: &str, episode: Option<&str>) -> Run {
    open_on(title, episode, true, settings())
}

/// A player opened on an engine that adapts by itself (ExoPlayer) or not (mpv), with these settings.
fn open_on(title: &str, episode: Option<&str>, adaptive: bool, settings: Settings) -> Run {
    let mut run = Run { play: Play::default(), now: 1_794_657_600., out: vec![], sent: vec![] };
    run.event(Event::Open {
        title: title.into(),
        episode: episode.map(String::from),
        local: false,
        settings,
        adaptive,
        screen: 16. / 9.,
    });
    run
}

impl Run {
    fn cx(&self) -> Context {
        test_context(self.now)
    }

    fn event(&mut self, event: Event) {
        let cx = self.cx();
        self.out = self.play.update(event, &cx);
        self.sent.extend(self.out.clone());
    }

    fn reply(&mut self, id: u32, reply: Reply) {
        let cx = self.cx();
        self.out = self.play.answer(id, reply, &cx);
        self.sent.extend(self.out.clone());
    }

    /// The effect asking for `path` (a page, or a request whose address has it), answered with `body`.
    fn answer(&mut self, path: &str, body: &str) {
        let id = self.id(path);
        let reply = match self.sent.iter().find(|e| matches!(e, Effect::Page { id: i, .. } if *i == id)) {
            Some(_) => Reply::Page { page: PageReply { body: Some(body.into()), at: self.now, failure: None, stale: false } },
            None => response(200, body, None),
        };
        self.reply(id, reply);
    }

    fn page(&mut self, id: u32, body: &str) {
        self.reply(id, Reply::Page { page: PageReply { body: Some(body.into()), at: self.now, failure: None, stale: false } });
    }

    fn id(&self, path: &str) -> u32 {
        self.out
            .iter()
            .chain(self.sent.iter().rev())
            .find_map(|e| match e {
                Effect::Page { id, path: p, .. } if p.contains(path) => Some(*id),
                Effect::Request { id, request } if request.url.contains(path) => Some(*id),
                _ => None,
            })
            .unwrap_or_else(|| panic!("nothing asked for {path}: {:?}", self.out))
    }

    fn request(&self, path: &str) -> Request {
        self.out
            .iter()
            .find_map(|e| match e {
                Effect::Request { request, .. } if request.url.contains(path) => Some(request.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no request for {path}: {:?}", self.out))
    }

    fn commands(&self) -> Vec<Command> {
        self.out.iter().filter_map(|e| if let Effect::Player { order } = e { Some(order.clone()) } else { None }).collect()
    }

    fn load(&self) -> (Media, f64, Option<Rung>) {
        self.commands()
            .into_iter()
            .find_map(|c| if let Command::Load { media, start, level, .. } = c { Some((media, start, level)) } else { None })
            .unwrap_or_else(|| panic!("no Load: {:?}", self.out))
    }

    fn timer(&self) -> Option<u32> {
        self.out.iter().find_map(|e| if let Effect::Timer { id, .. } = e { Some(*id) } else { None })
    }

    fn status(&mut self, at: f64, total: f64, buffering: bool) {
        self.event(Event::Status { status: Status { at, total, buffered: at + 30., paused: false, buffering, throughput: 0. } });
    }

    /// A film from the title page through Real-Debrid to its Load, with `spot` saved.
    fn through_rd(&mut self, spot: &str) {
        self.answer("/api/title/", FILM);
        assert!(self.commands().iter().any(|c| matches!(c, Command::FindLocal { offline: false, .. })));
        self.answer("/api/progress", spot);
        self.event(Event::Local { title: "cinema-movie-tt1234567".into(), episode: None, copy: None, offline: None });
        self.answer("/api/cinema/debrid", ONE);
        self.answer("/api/cinema/debrid/link", &linked(1));
    }

    /// The master playlist's levels, read after Load.
    fn levels(&mut self) {
        self.answer("master.m3u8", MASTER);
    }
}

#[test]
fn a_spot_in_the_last_ninety_seconds_starts_again() {
    let mut r = open("cinema-movie-tt1234567", None);
    r.through_rd(r#"{"cinema-movie-tt1234567": {"t": 9250, "d": 9300}}"#);
    assert_eq!(r.load().1, 0.);
}

#[test]
fn a_stream_resolved_ahead_is_what_play_plays() {
    let mut r = Run { play: Play::default(), now: 1_794_657_600., out: vec![], sent: vec![] };
    r.event(Event::Warm { title: "cinema-movie-tt1234567".into(), episode: None, settings: settings() });
    let progress = r.id("/api/progress");
    r.answer("/api/title/", FILM);
    r.page(progress, "{}");
    r.event(Event::Local { title: "cinema-movie-tt1234567".into(), episode: None, copy: None, offline: None });
    r.answer("/api/cinema/debrid", ONE);
    r.answer("/api/cinema/debrid/link", &linked(1));
    assert!(r.commands().iter().any(|c| matches!(c, Command::Preload { start, .. } if *start == 0.)));
    r.event(Event::Open { title: "cinema-movie-tt1234567".into(), episode: None, local: false, settings: settings(),
                          adaptive: true, screen: 1.7 });
    r.answer("/api/title/", FILM);
    let progress = r.id("/api/progress");
    r.page(progress, "{}");
    r.event(Event::Local { title: "cinema-movie-tt1234567".into(), episode: None, copy: None, offline: None });
    assert_eq!(r.load().0.url, master(1));
}

const COPIES: &str = r#"{"copies": [{"id": "uhd", "quality": "2160p", "hdr": true}, {"id": "fhd", "quality": "1080p"}]}"#;

#[test]
fn a_film_plays_its_debrid_copy_for_the_profile_s_quality() {
    let mut r = open("cinema-movie-tt1234567", None);
    r.answer("/api/title/", FILM);
    r.answer("/api/progress", r#"{"cinema-movie-tt1234567": {"t": 600, "d": 9300}}"#);
    r.event(Event::Local { title: "cinema-movie-tt1234567".into(), episode: None, copy: None, offline: None });
    let asked = r.request("/api/cinema/debrid");
    assert!(format!("{:?}", asked.body).contains("tt1234567"), "{:?}", asked.body);
    assert!(!r.out.iter().any(|e| matches!(e, Effect::Request { request, .. } if request.url.contains("vidqueen"))));
    r.answer("/api/cinema/debrid", COPIES);
    // (Auto: the 1080p copy)
    assert!(format!("{:?}", r.request("/api/cinema/debrid/link").body).contains("fhd"));
    r.answer("/api/cinema/debrid/link", r#"{"url": "https://dl.rd/fhd.mkv"}"#);
    let (media, start, _) = r.load();
    assert_eq!((media.url.as_str(), media.hls, start), ("https://dl.rd/fhd.mkv", false, 600.));
    // (a plain remote file may be read ahead by an app that does so; a track with no name is "Track 2")
    assert!(media.relay);
    assert_eq!((sound_label(None, None, 2).as_str(), refused(403), refused(410), refused(404)), ("Track 2", true, true, false));
    // (Quality lists its copies; 4K switches to that copy from the same spot)
    r.status(700., 9300., false);
    let view = r.play.view();
    assert_eq!(view.settings[0].choices.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["4K", "1080p"]);
    r.event(Event::Pick { panel: Panel::Settings, group: 0, choice: 0 });
    assert!(format!("{:?}", r.request("/api/cinema/debrid/link").body).contains("uhd"));
    r.answer("/api/cinema/debrid/link", r#"{"url": "https://dl.rd/uhd.mkv"}"#);
    assert_eq!(r.load().0.url, "https://dl.rd/uhd.mkv");
    assert_eq!(r.load().1, 700.);
}

#[test]
fn an_episode_plays_its_own_debrid_copy() {
    let mut r = open("s", Some("e2"));
    r.answer("/api/title/", SHOW);
    r.answer("/api/progress", "{}");
    r.event(Event::Local { title: "s".into(), episode: Some("e2".into()), copy: None, offline: None });
    let asked = format!("{:?}", r.request("/api/cinema/debrid").body);
    assert!(asked.contains(r#"\"season\":1"#) && asked.contains(r#"\"episode\":2"#), "{asked}");
    r.answer("/api/cinema/debrid", COPIES);
    r.answer("/api/cinema/debrid/link", r#"{"url": "https://dl.rd/s01e02.mkv"}"#);
    assert_eq!(r.load().0.url, "https://dl.rd/s01e02.mkv");
}

#[test]
fn too_many_requests_pauses_unless_its_the_last_save_and_saves_go_in_order() {
    let mut r = open("cinema-movie-tt1234567", None);
    r.through_rd("{}");
    r.status(100., 9300., false);
    r.now += 11.;
    r.status(111., 9300., false);
    let first = r.id("/api/progress");
    r.now += 11.;
    r.status(122., 9300., false);
    // (one save at a time: the next waits for it)
    assert!(r.out.iter().all(|e| !matches!(e, Effect::Request { request, .. } if request.url.ends_with("/api/progress"))));
    r.reply(first, response(429, r#"{"error": "Playing on another screen."}"#, None));
    assert!(r.commands().contains(&Command::Pause { paused: true }));
    assert_eq!(r.play.view().fault.as_deref(), Some("Playing on another screen."));
    let second = r.id("/api/progress");
    r.reply(second, response(200, "{}", None));
    r.event(Event::Close);
    let last = r.id("/api/progress");
    assert!(matches!(&r.request("/api/progress").body, Some(Body::Text { text, .. }) if text.contains("\"ended\":true")));
    r.reply(last, response(429, "{}", None));
    assert_eq!(r.commands(), vec![Command::Watched]);
    assert!(r.out.contains(&Effect::Tell { event: crate::screen::AppEvent::Watched }));
}

#[test]
fn offline_the_spot_is_kept_here_and_nothing_is_sent() {
    let mut r = open("cinema-movie-tt1234567", None);
    r.through_rd("{}");
    r.status(100., 9300., false);
    let mut cx = r.cx();
    cx.online = false;
    r.now += 11.;
    cx.clock.now = r.now;
    let out = r.play.update(Event::Status { status: Status { at: 111., total: 9300., buffered: 140., paused: false, buffering: false, throughput: 0. } }, &cx);
    assert!(out.iter().all(|e| !matches!(e, Effect::Request { .. })));
    assert_eq!(r.play.spots.get("cinema-movie-tt1234567").map(|s| s.0), Some(111.));
}

#[test]
fn quality_picks_a_level_and_the_profile_keeps_it() {
    let mut r = open("cinema-movie-tt1234567", None);
    r.through_rd("{}");
    r.levels();
    r.status(10., 9300., false);
    r.event(Event::Pick { panel: Panel::Settings, group: 0, choice: 2 });
    let commands = r.commands();
    assert!(commands.contains(&Command::Setting { group: "playback".into(), key: "quality".into(), value: crate::profile_settings::Setting::Text { text: "720p".into() } }));
    assert!(commands.iter().any(|c| matches!(c, Command::Level { level: Some(Rung { height: 720, .. }), .. })));
    assert!(r.play.view().settings[0].choices[2].on);
}

#[test]
fn an_engine_that_cant_adapt_steps_down_after_two_stalls() {
    let mut r = open("cinema-movie-tt1234567", None);
    if let Some(p) = r.play.p.as_mut() {
        p.adaptive = false;
    }
    r.through_rd("{}");
    r.levels();
    assert!(r.commands().iter().any(|c| matches!(c, Command::Level { level: Some(Rung { height: 1080, .. }), .. })));
    r.status(10., 9300., false);
    r.status(11., 9300., true);
    r.status(12., 9300., false);
    r.status(13., 9300., true);
    assert_eq!(r.load().2.map(|l| l.height), Some(720));
}

#[test]
fn next_episode_counts_down_and_plays_the_next() {
    let mut r = open("s", Some("e1"));
    r.answer("/api/title/", SHOW);
    r.answer("/api/progress", "{}");
    r.event(Event::Local { title: "s".into(), episode: Some("e1".into()), copy: None, offline: None });
    r.answer("/api/cinema/debrid", ONE);
    r.answer("/api/cinema/debrid/link", &linked(3));
    r.answer("master.m3u8", MASTER);
    r.status(1000., 1400., false);
    r.status(1381., 1400., false);
    let pill = r.play.view().pill.unwrap_or_else(|| panic!("no pill"));
    assert!(pill.next && pill.label == "Next episode · E2");
    r.event(Event::Ended);
    // (the end with autoplay on: the next episode, from its own spot)
    assert!(r.commands().iter().any(|c| matches!(c, Command::FindLocal { episode: Some(e), .. } if e == "e2")));
    assert_eq!(r.play.view().place, "S1 E2");
    r.answer("/api/progress", "{}");
    assert_eq!(r.play.view().episode_id.as_deref(), Some("e2"));
}

#[test]
fn a_show_played_with_no_episode_picks_up_where_it_was_left() {
    let mut r = open("s", None);
    r.answer("/api/title/", SHOW);
    // (e2 half watched: Play carries on there, by the title page's rule)
    let spots = r#"{"e2": {"key": "e2", "show": "s", "title": "Show", "ep": 2, "t": 600, "d": 1400, "updated": 1794657000}}"#;
    r.answer("/api/progress", spots);
    assert!(r.commands().iter().any(|c| matches!(c, Command::FindLocal { episode: Some(e), .. } if e == "e2")));

    let mut r = open("s", None);
    r.answer("/api/title/", SHOW);
    r.answer("/api/progress", "{}");
    assert!(r.commands().iter().any(|c| matches!(c, Command::FindLocal { episode: Some(e), .. } if e == "e1")));
}

#[test]
fn a_download_plays_from_the_device_with_its_own_captions() {
    let mut r = open("cinema-movie-tt1234567", None);
    r.answer("/api/title/", FILM);
    r.answer("/api/progress", "{}");
    let copy = LocalCopy {
        url: "file:///d/v.mp4".into(),
        captions: Some("WEBVTT\n\n00:00:01.000 --> 00:00:05.000\nHello\n".into()),
        length: 9300.,
        spot: 300.,
        ..LocalCopy::default()
    };
    r.event(Event::Local { title: "cinema-movie-tt1234567".into(), episode: None, copy: Some(copy), offline: None });
    let (media, start, _) = r.load();
    assert_eq!((media.url.as_str(), start), ("file:///d/v.mp4", 300.));
    r.status(2., 9300., false);
    assert_eq!(r.play.view().caption, "Hello");
    assert!(r.play.view().subtitles[0].choices[1].on);
}

fn film_download() -> crate::queue::Item {
    crate::queue::Item {
        key: "k".into(), title_id: "cinema-movie-tt1234567".into(), title: "Dune".into(), source: "cinema".into(), length: 9300.,
        ..Default::default()
    }
}

#[test]
fn offline_with_no_title_page_downloads_say_what_it_is() {
    let mut r = open("cinema-movie-tt1234567", None);
    let id = r.id("/api/title/");
    let away = api::Failure { kind: Kind::Offline, words: api::OFFLINE.into() };
    r.reply(id, Reply::Page { page: PageReply { body: None, at: 0., failure: Some(away), stale: false } });
    assert!(r.commands().iter().any(|c| matches!(c, Command::FindLocal { offline: true, .. })));
    r.event(Event::Local { title: "cinema-movie-tt1234567".into(), episode: None, copy: None, offline: Some(vec![film_download()]) });
    assert_eq!(r.play.view().name, "Dune");
    assert!(r.commands().iter().any(|c| matches!(c, Command::FindLocal { offline: false, .. })));
}

#[test]
fn youtube_through_the_server_when_youtube_is_slow() {
    let show = r#"{"id": "yt-show", "title": "Drama", "source": "youtube", "type": "series",
                   "episodes": [{"id": "yt-abc", "n": 1}]}"#;
    let mut r = open("yt-show", Some("yt-abc"));
    r.answer("/api/title/", show);
    r.answer("/api/progress", "{}");
    r.event(Event::Local { title: "yt-show".into(), episode: Some("yt-abc".into()), copy: None, offline: None });
    assert!(r.request("visitor_id").body.is_some());
    let wait = r.timer().unwrap_or_else(|| panic!("no wait"));
    r.reply(wait, Reply::Fired);
    r.answer("/api/youtube/play/abc", r#"{"stream": "/api/youtube/hls/abc.m3u8"}"#);
    assert_eq!(r.load().0.url, "https://host.invalid/api/youtube/hls/abc.m3u8");
}

#[test]
fn controls_hide_after_a_while_untouched_but_not_while_paused() {
    let mut r = open("cinema-movie-tt1234567", None);
    let hide = r.timer().unwrap_or_else(|| panic!("no hide"));
    r.through_rd("{}");
    r.status(10., 9300., false);
    r.now += 4.;
    r.reply(hide, Reply::Fired);
    assert!(!r.play.view().controls);
    r.event(Event::Poke);
    assert!(r.play.view().controls);
    let hide = r.timer().unwrap_or_else(|| panic!("no hide"));
    r.event(Event::Toggle);
    r.now += 4.;
    r.reply(hide, Reply::Fired);
    assert!(r.play.view().controls);
}

#[test]
fn a_film_real_debrid_hasnt_got_isnt_available() {
    let mut r = open("cinema-movie-tt1234567", None);
    r.answer("/api/title/", FILM);
    r.answer("/api/progress", "{}");
    r.event(Event::Local { title: "cinema-movie-tt1234567".into(), episode: None, copy: None, offline: None });
    r.answer("/api/cinema/debrid", NO_COPIES);
    assert_eq!(r.play.view().fault.as_deref(), Some(NOT_AVAILABLE));
}

#[test]
fn a_failing_copy_gets_a_fresh_link_twice_then_says_so() {
    let mut r = open("cinema-movie-tt1234567", None);
    r.through_rd("{}");
    r.status(100., 9300., false);
    for n in 2..4 {
        r.event(Event::Failed { failure: Failure::Refused, message: "403".into() });
        r.answer("/api/cinema/debrid/link", &linked(n));
        let (media, start, _) = r.load();
        assert_eq!((media.url, start), (master(n), 100.));
    }
    r.event(Event::Failed { failure: Failure::Refused, message: "403".into() });
    assert_eq!(r.play.view().fault.as_deref(), Some(FAILED));
    assert!(r.out.iter().any(|e| matches!(e, Effect::Report { place, .. } if place == "player.play")));
}

#[test]
fn anime_in_japanese_asks_the_player_for_japanese_sound_first() {
    const ANIME: &str = r#"{"id": "anime-show", "title": "Show", "source": "anime", "type": "series",
        "episodes": [{"id": "anime-ep1", "n": 1, "season": 1}]}"#;
    let mut r = open("anime-show", Some("anime-ep1"));
    r.answer("/api/title/", ANIME);
    r.answer("/api/progress", "{}");
    r.event(Event::Local { title: "anime-show".into(), episode: Some("anime-ep1".into()), copy: None, offline: None });
    r.answer("/api/anime/play/", r#"{"stream": "/api/anime/fallback/ep1", "audio": "sub", "available": ["sub", "dub"]}"#);
    assert_eq!(r.load().0.languages, vec!["ja".to_string(), "en".to_string()]);
}

fn engine(r: &mut Run, at: f64, ahead: f64, mbit: f64) {
    r.event(Event::Status { status: Status { at, total: 9300., buffered: at + ahead, paused: false, buffering: false, throughput: mbit * 1e6 } });
}

fn loaded(r: &Run) -> Option<(String, Option<Rung>)> {
    r.commands().into_iter().find_map(|c| if let Command::Load { media, first, .. } = c { Some((media.url, first)) } else { None })
}

#[test]
fn an_engine_that_adapts_starts_auto_at_1080p_then_is_let_go() {
    let mut r = open("cinema-movie-tt1234567", None);
    r.through_rd("{}");
    r.levels();
    engine(&mut r, 1., 4., 0.);
    assert!(!r.commands().contains(&Command::Adapt));
    engine(&mut r, 2., 12., 0.);
    assert!(r.commands().contains(&Command::Adapt));
    engine(&mut r, 3., 14., 0.);
    assert!(!r.commands().contains(&Command::Adapt));
}

#[test]
fn mpv_opens_just_auto_s_level_and_moves_up_once_the_bandwidth_and_buffer_allow() {
    let mut r = open_on("cinema-movie-tt1234567", None, false, Settings { plus: true, ..settings() });
    r.through_rd("{}");
    r.levels();
    assert!(r.commands().iter().any(|c| matches!(c, Command::Level { level: Some(Rung { height: 1080, .. }), .. })), "{:?}", r.out);
    // (fast enough for 4K, but not yet trusted, rested or buffered: it stays)
    for t in 1..=4 {
        r.now += 1.;
        engine(&mut r, f64::from(t), 5., 40.);
        assert_eq!(loaded(&r), None);
    }
    let mut up = None;
    for t in 5..=12 {
        r.now += 1.;
        engine(&mut r, f64::from(t), 12., 40.);
        up = up.or_else(|| r.commands().into_iter().find_map(|c| if let Command::Load { level, start, .. } = c { Some((level, start)) } else { None }));
    }
    let (level, start) = up.unwrap_or_else(|| panic!("never moved up"));
    assert_eq!(level.map(|l| l.height), Some(2160));
    assert!(start >= 10., "the spot kept: {start}");
}

#[test]
fn mpv_moves_down_when_the_bandwidth_drops_with_little_buffered() {
    let mut r = open_on("cinema-movie-tt1234567", None, false, settings());
    r.through_rd("{}");
    r.levels();
    let mut down = None;
    for t in 1..=12 {
        r.now += 1.;
        engine(&mut r, f64::from(t), 6., 1.);
        down = down.or_else(|| r.commands().into_iter().find_map(|c| if let Command::Load { level, .. } = c { level } else { None }));
    }
    assert_eq!(down.map(|l| l.height), Some(720));
}

#[test]
fn a_seek_holds_its_spot_until_the_engine_gets_there() {
    // (the engine still says the old time for a moment after a seek: the bar and the words jumped back to it)
    let mut r = open("cinema-movie-tt1234567", None);
    r.through_rd("{}");
    r.status(600., 9300., false);
    r.event(Event::Seek { to: 2400. });
    r.now += 0.3;
    r.status(601., 9300., true);
    assert!((r.play.view().time - 2400.).abs() < 1., "{}", r.play.view().time);
    assert!(r.play.view().buffering, "the spinner while it loads the spot");
    r.now += 0.5;
    r.status(2400.4, 9300., false);
    r.now += 1.;
    r.status(2401.4, 9300., false);
    assert!((r.play.view().time - 2401.4).abs() < 0.01, "{}", r.play.view().time);
    assert!(!r.play.view().buffering);
}

#[test]
fn a_report_says_what_and_where() {
    let mut r = open("cinema-movie-tt1234567", None);
    r.through_rd("{}");
    r.status(42., 9300., false);
    r.event(Event::Panel { panel: Some(Panel::Report) });
    r.event(Event::Reason { index: 1 });
    assert!(r.play.view().report.ready);
    r.event(Event::Send { note: " no words ".into() });
    let Some(Body::Text { text, .. }) = r.request("/api/report").body else { panic!("no body") };
    let sent: Value = serde_json::from_str(&text).unwrap_or_default();
    assert_eq!((sent["reason"].as_str(), sent["note"].as_str()), (Some("subtitles"), Some("no words")));
    assert_eq!(sent["title"], "Dune (cinema-movie-tt1234567)");
    r.answer("/api/report", "{}");
    assert_eq!(r.play.view().report.answer.as_deref(), Some(REPORTED));
}

#[test]
fn referers_go_to_the_host_and_folder_only() {
    let r = referer("https://mega.x/anime/1/2/master.m3u8", "https://mega.x/").unwrap_or_else(|| panic!("none"));
    assert_eq!(r.folder.as_deref(), Some("/anime/1/2"));
    assert_eq!(referer_for(&r, "https://mega.x/p.ts").as_deref(), Some("https://mega.x/"));
    assert_eq!(referer_for(&r, "https://cdn.y/anime/1/2/p.ts").as_deref(), Some("https://mega.x/"));
    assert_eq!(referer_for(&r, "https://cdn.y/other/p.ts"), None);
    assert_eq!(speed_name(1.5, false), "1.5×");
    assert_eq!(speed_name(1., true), "1×");
}

#[test]
fn jumps_run_on_adding_up_until_the_burst_ends() {
    let mut r = open("cinema-movie-tt1234567", None);
    r.through_rd("{}");
    r.status(100., 9300., false);
    r.event(Event::Jump { ahead: true, held: false });
    assert_eq!(r.play.view().burst, Some(Burst { ahead: true, seconds: 10, count: 1 }));
    assert!(r.commands().iter().any(|c| matches!(c, Command::Seek { to } if (*to - 110.).abs() < 0.01)));
    let first = r.timer().unwrap_or_else(|| panic!("no burst timer"));
    r.event(Event::Jump { ahead: true, held: true });
    assert_eq!(r.play.view().burst, Some(Burst { ahead: true, seconds: 40, count: 2 }));
    let last = r.timer().unwrap_or_else(|| panic!("no burst timer"));
    // (the first timer is stale: the run went on)
    r.reply(first, Reply::Fired);
    assert!(r.play.view().burst.is_some());
    r.reply(last, Reply::Fired);
    assert_eq!(r.play.view().burst, None);
    // (the other way starts its own run)
    r.event(Event::Jump { ahead: false, held: false });
    r.event(Event::Jump { ahead: true, held: false });
    assert_eq!(r.play.view().burst, Some(Burst { ahead: true, seconds: 10, count: 1 }));
}

#[test]
fn an_episode_the_title_hasnt_got_plays_where_the_show_was_left() {
    let mut r = open("s", Some("nope"));
    r.answer("/api/title/", SHOW);
    r.answer("/api/progress", "{}");
    assert!(r.commands().iter().any(|c| matches!(c, Command::FindLocal { episode: Some(e), .. } if e == "e1")));
}

#[test]
fn engine_errors_as_the_core_weighs_them() {
    use crate::play::{failure_of_exo, failure_of_mpv, Failure};
    assert_eq!(failure_of_exo(2001, false), Failure::Network);
    assert_eq!(failure_of_exo(2001, true), Failure::Refused);
    assert_eq!(failure_of_exo(4003, false), Failure::Unplayable);
    assert_eq!(failure_of_exo(1000, false), Failure::Other);
    assert_eq!(failure_of_mpv(-13), Failure::Network);
    assert_eq!(failure_of_mpv(-17), Failure::Unplayable);
    assert_eq!(failure_of_mpv(-1), Failure::Other);
}

#[test]
fn sound_tracks_are_named_group_and_track() {
    use crate::play::{parse_sound_id, sound_id};
    assert_eq!(sound_id(2, 1), "2:1");
    assert_eq!(parse_sound_id("2:1"), Some((2, 1)));
    assert_eq!((parse_sound_id("2"), parse_sound_id("a:1"), parse_sound_id("2:1:3")), (None, None, None));
}

#[test]
fn a_phones_remote_lists_the_choices_and_picks_by_their_words() {
    let mut r = open("cinema-movie-tt1234567", None);
    r.through_rd("{}");
    r.levels();
    r.status(10., 9300., false);
    let view = r.play.view();
    assert_eq!(view.tv.title, "Dune");
    let quality = &view.tv.groups[0];
    assert_eq!(quality.title, view.settings[0].title);
    let want = &view.settings[0].choices[2];
    let words = want.badge.as_ref().map_or(want.label.clone(), |b| format!("{} {b}", want.label));
    assert_eq!(quality.choices[2].label, words);
    r.event(Event::PickNamed { group: quality.title.clone(), choice: words });
    assert!(r.play.view().settings[0].choices[2].on);
    r.event(Event::PickNamed { group: "Nothing".into(), choice: "x".into() });
    assert!(r.commands().is_empty());
}

#[test]
fn a_sound_track_is_named_by_its_title_then_its_language_then_its_place() {
    assert_eq!(sound_label(Some("Director's cut"), Some("en"), 1), "Director's cut");
    assert_eq!(sound_label(Some(" "), Some("jpn"), 1), "Japanese");
    assert_eq!(sound_label(None, Some("pt-BR"), 1), "Portuguese");
    assert_eq!(sound_label(None, Some("xx"), 1), "xx");
    assert_eq!(sound_label(None, Some(""), 3), "Track 3");
    assert_eq!(engine_missing("no dll"), "Couldn't load libmpv. no dll");
}

#[test]
fn a_burst_says_which_way_and_how_far() {
    let mut r = open("cinema-movie-tt1234567", None);
    r.event(Event::Jump { ahead: true, held: false });
    assert_eq!(r.play.view().burst_label, "+10");
    r.event(Event::Jump { ahead: false, held: false });
    assert_eq!(r.play.view().burst_label, "\u{2212}10");
}

#[test]
fn the_pointer_moving_is_told_at_most_every_quarter_second() {
    let mut r = open("cinema-movie-tt1234567", None);
    r.event(Event::Poke);
    assert!(r.out.is_empty());
    let first = r.play.p.as_ref().map(|p| p.poked);
    r.now += 0.1;
    r.event(Event::Poke);
    assert_eq!(r.play.p.as_ref().map(|p| p.poked), first);
    r.now += 0.2;
    r.event(Event::Poke);
    assert_ne!(r.play.p.as_ref().map(|p| p.poked), first);
}

#[test]
fn the_picture_fit_is_the_cores_to_keep() {
    let pref = |r: &Run, key: &str| {
        r.sent.iter().find_map(|e| match e {
            Effect::ReadPref { id, key: k } if k == key => Some(*id),
            _ => None,
        })
    };
    // (nothing kept: this test device is not a desktop, so it fills)
    let mut r = open("cinema-movie-tt1234567", None);
    let id = pref(&r, "fit").unwrap_or_else(|| panic!("no read"));
    r.reply(id, Reply::Pref { value: None });
    assert!(r.play.view().fill);
    assert!(r.commands().contains(&Command::Fit { fill: true }));
    // (kept as "fit": the engine is told, and a pick is written back)
    let mut r = open("cinema-movie-tt1234567", None);
    let id = pref(&r, "fit").unwrap_or_else(|| panic!("no read"));
    r.reply(id, Reply::Pref { value: Some("fit".into()) });
    assert!(!r.play.view().fill);
    assert!(r.commands().contains(&Command::Fit { fill: false }));
}

#[test]
fn the_brightness_is_kept_for_the_next_play() {
    let mut r = open("cinema-movie-tt1234567", None);
    let id = r.sent.iter().find_map(|e| match e {
        Effect::ReadPref { id, key } if key == "playerLight" => Some(*id),
        _ => None,
    });
    r.reply(id.unwrap_or_default(), Reply::Pref { value: Some("0.4".into()) });
    assert_eq!(r.play.view().light, Some(0.4));
    r.event(Event::Light { level: 0.7 });
    assert!(r.out.contains(&Effect::WritePref { key: "playerLight".into(), value: Some("0.7".into()) }));
    assert_eq!(r.play.view().light, Some(0.7));
}
