//! What a download will weigh before it comes down: a title page's "~1.2 GB", and the size a transfer shows from its
//! start.

use crate::hls;
use super::probe::{self, Found};
use super::Io;
use crate::screen::{Context, DiskOp, Header, Reply};
use std::collections::HashMap;

/// Pieces weighed to tell what a playlist weighs: they differ a lot (a film's measured from 0.4 to 9 Mbit/s), so a
/// dozen missed the whole by up to 15%; 48 come within about 5%.
const PICKS: usize = 48;
/// Anime's: its pieces are short and cut at scene changes, so they differ far more (48 missed an episode by up to 31%,
/// measured on every piece of 12 episodes of two shows); 128 come within about 10%, 5% on most.
const MANY: usize = 128;
/// Pieces weighed at once: dulo turns a crowd of requests away ("Playback rate limit exceeded").
const AT_ONCE: usize = 8;

/// Bytes a second a round BANDWIDTH really came out at, by "title:figure": a show's copies stating the same one came
/// out alike, so dulo's are weighed once per title and figure.
pub type Rates = HashMap<String, f64>;

/// A weighing under way.
#[derive(Debug)]
pub struct Weight {
    seconds: f64,
    referer: Option<String>,
    /// Where a dulo figure's rate is kept once weighed.
    rate_key: Option<String>,
    /// What it falls back on when the pieces won't say.
    fallback: Option<f64>,
    picks: Vec<(f64, String)>,
    next: usize,
    answered: usize,
    sizes: Vec<(f64, u64)>,
}

/// How a weighing goes on: these requests, or its bytes (0 if it can't tell).
pub enum Next {
    Ask(Vec<(usize, Io)>),
    Bytes(u64),
}

impl Weight {
    /// What a level's download weighs (its pieces added up): a bit rate the source states as its pieces' average, over
    /// how long they run. ARY's AVERAGE-BANDWIDTH is that (measured to the byte on its pieces), and dulo's own copies
    /// give it as the BANDWIDTH (within 1%, the sound counted in). A round figure is a packager's target instead
    /// (dulo's 8940800, 9% over what came out; anime's 3000000 at 720p, 2.2 to 5 times over): then what its pieces weigh,
    /// at dulo once per title and figure, anime's each on its own and from more of them ([`MANY`]: one show's episodes
    /// stating the same figure came out from 0.6 to 0.85 Mbit/s). YouTube's files say their sizes.
    pub fn start(title_id: &str, found: &Found, rates: &Rates) -> (Weight, Next) {
        let media = hls::media(&found.playlist, &found.at);
        let seconds = media.length();
        let mut w = Weight {
            seconds,
            referer: found.referer.clone(),
            rate_key: None,
            fallback: None,
            picks: vec![],
            next: 0,
            answered: 0,
            sizes: vec![],
        };
        if let Some(f) = &found.file {
            return (w, Next::Bytes(f.size));
        }
        if let Some(y) = &found.youtube {
            let both = if y.video_size > 0 && y.sound_size > 0 { y.video_size + y.sound_size } else { 0 };
            return (w, Next::Bytes(both));
        }
        let stated = found.level.as_ref().map_or(0, hls::Level::rate);
        let average = found.level.as_ref().is_some_and(|l| l.average.is_some());
        if found.level.is_some() && (average || found.referer.is_none() && !stated.is_multiple_of(100)) {
            return (w, Next::Bytes((stated as f64 / 8. * seconds) as u64));
        }
        let count = if found.referer.is_some() {
            MANY
        } else {
            let key = format!("{title_id}:{stated}");
            if let Some(rate) = rates.get(&key) {
                return (w, Next::Bytes((rate * seconds) as u64));
            }
            w.rate_key = Some(key);
            w.fallback = Some(stated as f64 / 8.);
            PICKS
        };
        let all: Vec<(f64, String)> = media.pieces.into_iter().map(|p| (p.seconds, p.url)).collect();
        if all.len() < 3 {
            let bytes = w.give_up();
            return (w, Next::Bytes(bytes));
        }
        let mut picks: Vec<(f64, String)> =
            (0..count).map(|i| all[((((i as f64) + 0.5) * all.len() as f64 / count as f64) as usize).min(all.len() - 1)].clone()).collect();
        picks.dedup_by(|a, b| a.1 == b.1);
        w.picks = picks;
        let first = w.more();
        (w, Next::Ask(first))
    }

    fn more(&mut self) -> Vec<(usize, Io)> {
        let mut out = vec![];
        while self.next < self.picks.len() && self.next - self.answered < AT_ONCE {
            let (_, url) = &self.picks[self.next];
            let ask = probe::source_request(url, self.referer.as_deref(), vec![Header::new("Range", "bytes=0-0")]);
            out.push((self.next, Io::Ask(ask)));
            self.next += 1;
        }
        out
    }

    fn give_up(&self) -> u64 {
        self.fallback.map_or(0, |rate| (rate * self.seconds) as u64)
    }

    /// The answer to pick `i`.
    pub fn answer(&mut self, i: usize, reply: Reply, rates: &mut Rates) -> Next {
        self.answered += 1;
        if let (Some(whole), Some((secs, _))) = (probe::whole_size(&reply), self.picks.get(i)) {
            if *secs > 0. {
                self.sizes.push((*secs, whole));
            }
        }
        if self.answered < self.picks.len() {
            return Next::Ask(self.more());
        }
        if self.sizes.len() < 3 || self.sizes.len() < self.picks.len() / 2 {
            return Next::Bytes(if self.referer.is_some() { 0 } else { self.give_up() });
        }
        let rate = self.sizes.iter().map(|s| s.1 as f64).sum::<f64>() / self.sizes.iter().map(|s| s.0).sum::<f64>();
        if let Some(key) = &self.rate_key {
            rates.insert(key.clone(), rate);
        }
        Next::Bytes((rate * self.seconds) as u64)
    }
}

/// Episodes weighed one by one for a show's Download all at most; past that, the rest weigh what these average.
pub const WEIGHED: usize = 48;
/// Anime's at most: each is asked of the server first and many of its pieces are weighed (its BANDWIDTH says nothing).
/// A show's episodes differ less than dulo's copies (8% and 13% apart in two shows measured), so these, spread through
/// them, stand for the rest within a few %.
pub const ANIME_WEIGHED: usize = 8;
/// Episodes weighed at once (dulo turns a crowd of requests away).
pub const EPISODES_AT_ONCE: usize = 6;
/// How long a weighed size is kept: dulo now and then swaps the copy it serves.
pub const SIZE_KEPT_MS: f64 = 7. * 24. * 3_600_000.;
/// Where weighed sizes are kept, in the app's files folder: "copy:height" -> [bytes, when (ms)].
pub const SIZES: &str = "sizes.json";

/// Which of `n` episodes are weighed for a show (every one up to the most, else that many spread through them).
pub fn picks(n: usize, anime: bool) -> Vec<usize> {
    let most = if anime { ANIME_WEIGHED } else { WEIGHED };
    if n <= most {
        (0..n).collect()
    } else {
        (0..most).map(|i| i * n / most).collect()
    }
}

/// What all `n` weigh from what those weighed came to; None when none could be.
pub fn whole(weighed: &[u64], n: usize) -> Option<u64> {
    (!weighed.is_empty()).then(|| weighed.iter().sum::<u64>() * n as u64 / weighed.len() as u64)
}

// ---------- a series' chapters ----------

/// Chapters weighed for a series' size at most, spread through it. One chapter said little (the middle one missed Pick
/// Me Up's 2.7 GB by 32%: its chapters run from 4.6 to 19 MB, and drift over a series); 24 came within about 3% (6% at
/// worst) of every page of three series added up.
pub const CHAPTERS_WEIGHED: usize = 24;
/// Chapters weighed at once.
pub const CHAPTERS_AT_ONCE: usize = 4;

/// Which of `n` chapters are weighed for a series (every one up to the most, else that many spread through them).
pub fn chapter_picks(n: usize) -> Vec<usize> {
    if n <= CHAPTERS_WEIGHED {
        (0..n).collect()
    } else {
        (0..CHAPTERS_WEIGHED).map(|i| (((i as f64 + 0.5) * n as f64 / CHAPTERS_WEIGHED as f64) as usize).min(n - 1)).collect()
    }
}

/// What all `n` chapters weigh from the `picked` weighed for it: the average of those that could be, for the rest;
/// None when fewer than 3 (or than were picked) could be.
pub fn chapters_whole(weighed: &[u64], picked: usize, n: usize) -> Option<u64> {
    (!weighed.is_empty() && weighed.len() >= picked.min(3)).then(|| weighed.iter().sum::<u64>() * n as u64 / weighed.len() as u64)
}

/// What a chapter's weighing asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChapterStep {
    /// Its folder on the device.
    Files,
    /// Its pages' list, from the server.
    Pages,
    /// A page's size, from Asura's CDN.
    Page,
}

/// A chapter being weighed: by its files when it's on the device, else its pages' sizes asked (each its first byte
/// only: the answer says the whole page's size, and a HEAD isn't served from Asura's cache), nothing downloaded.
#[derive(Debug, Default)]
pub struct ChapterWeight {
    path: String,
    left: usize,
    sum: u64,
    missed: bool,
}

/// How a chapter's weighing goes on.
#[derive(Debug, PartialEq)]
pub enum ChapterNext {
    Ask(Vec<(ChapterStep, Io)>),
    /// More answers to come.
    Wait,
    /// What it weighs (None: it can't tell); `keep`: weighed from its pages, which never change, so kept for good.
    Bytes { bytes: Option<u64>, keep: bool },
}

impl ChapterWeight {
    pub fn start(path: &str) -> (Self, Vec<(ChapterStep, Io)>) {
        let look = Io::Disk(DiskOp::Listing { path: super::transfer::chapter_dir(path) });
        (ChapterWeight { path: path.into(), ..Default::default() }, vec![(ChapterStep::Files, look)])
    }

    pub fn answer(&mut self, step: ChapterStep, reply: Reply, cx: &Context) -> ChapterNext {
        match step {
            ChapterStep::Files => {
                let entries = match reply {
                    Reply::Disk { disk } => disk.entries,
                    _ => vec![],
                };
                if entries.iter().any(|e| e.name == crate::downloads::COMPLETE && e.bytes > 0) {
                    return ChapterNext::Bytes { bytes: Some(entries.iter().map(|e| e.bytes).sum()), keep: false };
                }
                let ask = crate::api::get(&cx.base, &cx.who(), &crate::manhwa::pages_path(&self.path), None);
                ChapterNext::Ask(vec![(ChapterStep::Pages, Io::Ask(ask))])
            }
            ChapterStep::Pages => {
                let pages = match crate::api::outcome(reply, cx.online) {
                    crate::api::Outcome::Body { body, .. } => crate::manhwa::parse_pages(&body).unwrap_or_default(),
                    _ => vec![],
                };
                if pages.is_empty() {
                    return ChapterNext::Bytes { bytes: None, keep: false };
                }
                self.left = pages.len();
                let range = || vec![Header::new("Range", "bytes=0-0")];
                ChapterNext::Ask(pages.iter().map(|url| (ChapterStep::Page, Io::Ask(crate::manhwa::page_request(url, range())))).collect())
            }
            ChapterStep::Page => {
                match probe::whole_size(&reply) {
                    Some(n) => self.sum += n,
                    // (a page that won't say makes the chapter unknown: counting it as nothing undercounted the whole)
                    None => self.missed = true,
                }
                self.left = self.left.saturating_sub(1);
                match (self.left, self.missed) {
                    (0, false) => ChapterNext::Bytes { bytes: Some(self.sum), keep: true },
                    (0, true) => ChapterNext::Bytes { bytes: None, keep: false },
                    _ => ChapterNext::Wait,
                }
            }
        }
    }
}

/// Sizes an older Android app kept, as `(key, "bytes:when")`, added to the sizes file's `kept` text (those it has
/// already stay as they are).
pub fn moved_sizes(kept: Option<&str>, old: &[(String, String)]) -> String {
    let mut all = kept.and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default();
    for (key, value) in old {
        let Some((bytes, at)) = value.split_once(':') else { continue };
        let (Ok(bytes), Ok(at)) = (bytes.parse::<u64>(), at.parse::<i64>()) else { continue };
        all.entry(key.clone()).or_insert(serde_json::json!([bytes, at]));
    }
    serde_json::Value::Object(all).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hls::Level;
    use crate::screen::{response, Header};

    fn found(rate: u64, average: bool, referer: Option<&str>, pieces: usize) -> Found {
        let mut text = String::from("#EXTM3U\n");
        for i in 0..pieces {
            text.push_str(&format!("#EXTINF:10,\n{i}.ts\n"));
        }
        Found {
            at: "https://s.x/l/list.m3u8".into(),
            playlist: text,
            referer: referer.map(String::from),
            level: Some(Level { bandwidth: rate, average: average.then_some(rate), ..Default::default() }),
            ..Default::default()
        }
    }

    fn piece(bytes: u64) -> Reply {
        Reply::Response { status: 206, headers: vec![Header::new("Content-Range", &format!("bytes 0-0/{bytes}"))], body: "x".into() }
    }

    #[test]
    fn a_stated_average_is_taken_at_its_word() {
        let mut rates = Rates::new();
        let (_, next) = Weight::start("t", &found(8_000_000, true, Some("https://r/"), 6), &rates);
        assert!(matches!(next, Next::Bytes(60_000_000)));
        // (dulo's odd figure is its own pieces' average)
        let (_, next) = Weight::start("t", &found(8_000_123, false, None, 6), &rates);
        assert!(matches!(next, Next::Bytes(60_000_922)));
        rates.insert("t:9000000".into(), 1000.);
        let (_, next) = Weight::start("t", &found(9_000_000, false, None, 6), &rates);
        assert!(matches!(next, Next::Bytes(60_000)));
    }

    #[test]
    fn a_round_figure_is_weighed_from_its_pieces() {
        let mut rates = Rates::new();
        let (mut w, next) = Weight::start("t", &found(9_000_000, false, None, 10), &rates);
        let Next::Ask(first) = next else { panic!() };
        // (every piece of ten, eight at a time)
        assert_eq!(first.len(), 8);
        let mut out = None;
        for i in 0..10 {
            out = Some(w.answer(i, piece(20_000), &mut rates));
        }
        assert!(matches!(out, Some(Next::Bytes(200_000))));
        assert_eq!(rates.get("t:9000000"), Some(&2000.));
        // (pieces that won't say: the stated figure)
        let (mut w, _) = Weight::start("u", &found(8_000_000, false, None, 4), &rates);
        let mut out = None;
        for i in 0..4 {
            out = Some(w.answer(i, response(404, "", None), &mut rates));
        }
        assert!(matches!(out, Some(Next::Bytes(40_000_000))));
        assert_eq!(picks(100, true).len(), ANIME_WEIGHED);
        assert_eq!(picks(3, false), vec![0, 1, 2]);
        assert_eq!(whole(&[10, 20], 4), Some(60));
        assert_eq!(whole(&[], 4), None);
    }

    #[test]
    fn a_series_is_weighed_from_its_chapters() {
        assert_eq!(chapter_picks(5), vec![0, 1, 2, 3, 4]);
        let spread = chapter_picks(240);
        assert_eq!((spread.len(), spread[0], spread[23]), (24, 5, 235));
        assert_eq!(chapters_whole(&[10, 20, 30], 24, 100), Some(2000));
        assert_eq!(chapters_whole(&[10, 20], 24, 100), None);
        assert_eq!(chapters_whole(&[10], 1, 4), Some(40));
        assert_eq!(chapters_whole(&[], 0, 4), None);
    }

    #[test]
    fn a_chapter_by_its_pages() {
        let cx = crate::screen::test_context(0.);
        let (mut w, first) = ChapterWeight::start("/series/x/chapter/1");
        assert!(matches!(first[0], (ChapterStep::Files, Io::Disk(DiskOp::Listing { .. }))));
        let ChapterNext::Ask(pages) = w.answer(ChapterStep::Files, Reply::Disk { disk: Default::default() }, &cx) else { panic!() };
        assert_eq!(pages[0].0, ChapterStep::Pages);
        let body = r#"{"images": ["https://cdn.asurascans.com/asura-images/chapters/a/1.webp", "https://cdn.asurascans.com/asura-images/chapters/a/2.webp"]}"#;
        let ChapterNext::Ask(asks) = w.answer(ChapterStep::Pages, response(200, body, None), &cx) else { panic!() };
        assert_eq!(asks.len(), 2);
        let Io::Ask(r) = &asks[0].1 else { panic!() };
        assert!(r.headers.iter().any(|h| h.name == "Range") && r.headers.iter().any(|h| h.name == "Referer"));
        assert_eq!(w.answer(ChapterStep::Page, piece(300), &cx), ChapterNext::Wait);
        assert_eq!(w.answer(ChapterStep::Page, piece(700), &cx), ChapterNext::Bytes { bytes: Some(1000), keep: true });
        // (on the device: its files)
        let (mut w, _) = ChapterWeight::start("/series/x/chapter/2");
        let entry = |name: &str, bytes| crate::screen::DiskEntry { name: name.into(), bytes };
        let disk = crate::screen::DiskReply { ok: true, entries: vec![entry("0.img", 500u64), entry("complete", 1)], ..Default::default() };
        assert_eq!(w.answer(ChapterStep::Files, Reply::Disk { disk }, &cx), ChapterNext::Bytes { bytes: Some(501), keep: false });
    }

    #[test]
    fn an_older_apps_sizes_are_read() {
        let old = vec![("/series/x/chapter/1".to_string(), "1234:1700000000000".to_string()), ("bad".into(), "x".into())];
        let text = moved_sizes(Some(r#"{"dulo|a:1080": [5, 6]}"#), &old);
        let v: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(v["/series/x/chapter/1"], serde_json::json!([1234, 1700000000000i64]));
        assert_eq!(v["dulo|a:1080"], serde_json::json!([5, 6]));
        assert!(v.get("bad").is_none());
    }
}
