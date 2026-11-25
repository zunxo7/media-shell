//! One download's transfer, as steps: where it comes from ([`Probe`]), what it will weigh ([`Weight`]), what's down
//! already, then its pieces (or stretches) fetched to files, as many at once as its source allows, the captions kept
//! with it, and its files put in place. A video comes down in one of two forms:
//! - **joined** (anime and ARY): the pieces added end to end into one file, where it got to kept beside it;
//! - **stretches**: YouTube's picture and sound, two files, each asked 8 MB at a time (YouTube slows one long request
//!   to about real time); a film's or an episode's one file, [`FILE_AT_ONCE`] stretches of [`FILE_STRETCH`] at once.
//!
//! A chapter's pages come down into its folder, each checked to be a picture and renamed whole.

use super::probe::{self, Copy, Found, Known, Probe};
use super::row::Row;
use super::weigh::{Next, Rates, Weight};
use super::{file, Fail, Io};
use crate::downloads::{self as rules, Kind};
use crate::screen::{Context, DiskOp, DiskReply, Header, Reply, Request};
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};

/// Pieces on their way at once for one download: each is slow alone, together they fill the line.
pub const PIECES_AT_ONCE: usize = 24;
/// A part turned away for asking too much (429) waits this long and asks again, twice as long each time, up to
/// [`BUSY_WAIT_MOST_MS`]; past that the round has failed.
const BUSY_WAIT_MS: u32 = 2000;
const BUSY_WAIT_MOST_MS: u32 = 32_000;
/// YouTube's files are asked this much at a time, one at a time.
pub const STRETCH: u64 = 8_000_000;
/// A film's or an episode's file: this much a stretch, this many at once (one connection is held to less than the
/// line; four fill it).
#[cfg(not(test))]
pub const FILE_STRETCH: u64 = 16_000_000;
#[cfg(test)]
pub const FILE_STRETCH: u64 = 10;
pub const FILE_AT_ONCE: usize = 4;
/// A chapter's pages on their way at once (as the reader asks for them).
pub const PAGES_AT_ONCE: usize = 4;
/// How often a transfer's progress is shown and its room checked, in seconds.
const TICK: f64 = 0.7;
/// How often the room left is asked while a download comes down, in seconds.
const ROOM_EVERY: f64 = 5.;
/// How long captions may take before the download goes on without them.
const CAPTIONS_MS: u32 = 20_000;
/// A chapter's page answered with one of these needs a sign-in.
const SIGN_IN: &str = "This chapter needs a sign-in.";

/// What one of a job's effects was for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tag {
    Probe(probe::Step),
    Weigh(usize),
    Have(usize),
    Room,
    Part(usize),
    Again(usize),
    Append(usize),
    Mark,
    Captions(u8),
    Finish,
    Pages,
    Check(usize),
    Keep(usize),
    Bytes,
    /// An answer nothing waits on (a playlist written beside the pieces).
    Quiet,
}

/// How a video comes down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    Joined,
    Stretches,
}

/// How a copy comes down: anime's and ARY's pieces joined; YouTube's files and Movies & TV's ("dulo|...") in stretches.
pub fn form(copy: &str, _device: crate::screen::Device) -> Form {
    if copy.starts_with("yt|") || copy.starts_with("dulo|") {
        Form::Stretches
    } else {
        Form::Joined
    }
}

/// The tallest level a download of `copy` takes, by its saved `quality`, never over what the account may have of its
/// source ("dulo|..." is Movies & TV's).
pub fn target(quality: &str, copy: &str, plus: bool) -> i64 {
    let source = if copy.starts_with("dulo|") { "cinema" } else { "" };
    crate::quality::download_most(quality, crate::quality::most(source, plus))
}

/// What a job hands back after each step.
#[derive(Debug, Default)]
pub struct Step {
    pub io: Vec<(Tag, Io)>,
    pub end: Option<End>,
}

#[derive(Debug, PartialEq)]
pub enum End {
    /// Down: its bytes all told.
    Done { total: u64 },
    Failed(Fail),
}

impl Step {
    fn io(io: Vec<(Tag, Io)>) -> Self {
        Self { io, end: None }
    }
    fn fail(fail: Fail) -> Self {
        Self { io: vec![], end: Some(End::Failed(fail)) }
    }
}

/// Things every job shares.
#[derive(Debug, Default)]
pub struct Shared {
    pub known: Known,
    pub rates: Rates,
    /// The bytes free where downloads are kept, as last asked.
    pub free: Option<u64>,
}

// ---------- a video ----------

#[derive(Debug)]
struct Part {
    url: String,
    range: Option<(u64, u64)>,
    /// Fetched into.
    file: String,
    track: usize,
    n: usize,
}

#[derive(Debug, Default)]
struct Track {
    /// Joined: the file its parts are added to, and where it got to is kept.
    into: Option<String>,
    mark: Option<String>,
    count: usize,
    joined: usize,
    bytes: u64,
    /// Joined: the file's length on the device as the transfer starts.
    on_disk: u64,
    /// Stretches: its whole size is known, so a stretch can be asked for where it got to.
    sized: bool,
    /// Nothing more to come (a stretch past the end).
    ended: bool,
    landed: BTreeMap<usize, (String, u64)>,
    appending: bool,
}

impl Track {
    fn finished(&self) -> bool {
        self.ended || self.into.is_none() || self.joined >= self.count
    }
}

#[derive(Debug)]
pub struct Plan {
    found: Found,
    form: Form,
    estimate: u64,
    /// Stretches: how much each asks for.
    stretch: u64,
    parts: Vec<Part>,
    tracks: Vec<Track>,
    next: usize,
    out: usize,
    waits: BTreeMap<usize, u32>,
    /// Answers to wait for before the transfer starts.
    have_left: usize,
    finish: VecDeque<(DiskOp, bool)>,
    vtt: Option<String>,
}

#[derive(Debug)]
enum Phase {
    Probe(Probe),
    Weigh(Found, Weight),
    Have(Plan),
    Room(Plan),
    Run(Plan),
    Captions(Plan),
    Finish(Plan),
}

/// A video coming down.
#[derive(Debug)]
pub struct VideoJob {
    pub key: String,
    form: Form,
    phase: Option<Phase>,
    shown: f64,
    roomed: f64,
}

impl VideoJob {
    pub fn new(row: &Row, cx: &Context) -> Self {
        Self { key: row.key.clone(), form: form(&row.copy, cx.platform.device), phase: None, shown: 0., roomed: 0. }
    }

    /// Pieces on their way.
    pub fn out(&self) -> usize {
        match &self.phase {
            Some(Phase::Run(plan)) => plan.out,
            _ => 0,
        }
    }

    /// A round starts: where it comes from, asked afresh.
    pub fn start(&mut self, row: &Row, plus: bool, cx: &Context, shared: &Shared) -> Step {
        let Some(copy) = Copy::read(&row.copy, row.imdb.as_deref(), row.season, row.episode) else {
            // (a copy it doesn't know: nothing to fetch it from)
            return Step::fail(Fail::Cant(rules::CANT.into()));
        };
        let mut p = Probe::new(copy, target(&row.quality, &row.copy, plus), true);
        let io = p.start(cx, &shared.known).into_iter().map(|(s, io)| (Tag::Probe(s), io)).collect();
        self.phase = Some(Phase::Probe(p));
        Step::io(io)
    }

    /// Pieces it may ask for now: up to `room` more.
    pub fn pump(&mut self, room: usize) -> Vec<(Tag, Io)> {
        let Some(Phase::Run(plan)) = &mut self.phase else { return vec![] };
        let mut io = vec![];
        let at_once = match plan.form {
            Form::Stretches if plan.found.file.is_some() => FILE_AT_ONCE,
            Form::Stretches => 1,
            _ => PIECES_AT_ONCE,
        };
        let mut room = room.min(at_once.saturating_sub(plan.out));
        while room > 0 && plan.next < plan.parts.len() {
            let i = plan.next;
            plan.next += 1;
            let part = &plan.parts[i];
            let track = &plan.tracks[part.track];
            if track.ended {
                continue;
            }
            io.push((Tag::Part(i), fetch(plan, i)));
            plan.out += 1;
            room -= 1;
        }
        io
    }

    /// The answer to one of its effects. `row` is its download's, changed as it goes.
    pub fn answer(&mut self, tag: Tag, reply: Reply, row: &mut Row, cx: &Context, shared: &mut Shared) -> Step {
        let Some(phase) = self.phase.take() else { return Step::default() };
        let (phase, step) = self.go(phase, tag, reply, row, cx, shared);
        self.phase = phase;
        step
    }

    fn go(&mut self, phase: Phase, tag: Tag, reply: Reply, row: &mut Row, cx: &Context, shared: &mut Shared) -> (Option<Phase>, Step) {
        match (phase, tag) {
            (Phase::Probe(mut p), Tag::Probe(step)) => {
                let (io, end) = p.answer(step, reply, cx, &mut shared.known);
                match end {
                    None => (Some(Phase::Probe(p)), Step::io(io.into_iter().map(|(s, io)| (Tag::Probe(s), io)).collect())),
                    Some(Err(fail)) => (None, Step::fail(fail)),
                    // (part of it is down already: the stream may only be resting, so it's tried again rather than given up)
                    Some(Ok(None)) if row.done > 0 => (None, Step::fail(Fail::Dropped(rules::NOT_ANSWERING.into()))),
                    Some(Ok(None)) => (None, Step::fail(Fail::Cant(rules::CANT.into()))),
                    Some(Ok(Some(found))) => {
                        if crate::hls::written(&found.playlist).locked && found.youtube.is_none() && found.file.is_none() {
                            return (None, Step::fail(Fail::Cant(rules::CANT.into())));
                        }
                        let name = match (&found.youtube, &found.level, &found.file) {
                            (Some(y), _, _) => crate::quality::level_name(crate::quality::nominal(y.width, y.height)),
                            (None, Some(l), _) => l.name(),
                            (None, None, Some(f)) if !f.quality.is_empty() => crate::quality::quality_name(&f.quality),
                            _ => String::new(),
                        };
                        if !name.is_empty() {
                            row.label = name;
                        }
                        let (w, next) = Weight::start(&row.title_id, &found, &shared.rates);
                        self.weighed(found, w, next, row)
                    }
                }
            }
            (Phase::Weigh(found, mut w), Tag::Weigh(i)) => {
                let next = w.answer(i, reply, &mut shared.rates);
                self.weighed(found, w, next, row)
            }
            (Phase::Have(mut plan), Tag::Have(t)) => {
                have(&mut plan, t, reply);
                plan.have_left -= 1;
                if plan.have_left > 0 {
                    return (Some(Phase::Have(plan)), Step::default());
                }
                resume(&mut plan);
                let down = down(&plan);
                row.done = down;
                row.size = row.size.max(plan.estimate).max(down);
                (Some(Phase::Room(plan)), Step::io(vec![(Tag::Room, Io::Disk(DiskOp::Free))]))
            }
            (Phase::Room(plan), Tag::Room) => {
                let free = disk(&reply).map(|d| d.bytes);
                shared.free = free.or(shared.free);
                if free.is_some_and(|f| !rules::fits(f, plan.estimate.saturating_sub(down(&plan)))) {
                    return (None, Step::fail(Fail::Full));
                }
                self.shown = cx.clock.now;
                self.roomed = cx.clock.now;
                self.next_or_captions(plan, row, cx)
            }
            (Phase::Run(mut plan), Tag::Part(i)) => {
                plan.out -= 1;
                let ok = match &reply {
                    Reply::Saved { status, bytes } if (200..300).contains(status) && *bytes > 0 => Some(*bytes),
                    // (a stretch past the end: it was all there)
                    Reply::Saved { status: 416, .. } if plan.form == Form::Stretches => Some(0),
                    _ => None,
                };
                match (ok, &reply) {
                    (Some(bytes), _) => {
                        let mut io = landed(&mut plan, i, bytes);
                        io.extend(self.tick(&mut plan, row, cx));
                        self.go_on(plan, io, row, cx)
                    }
                    // (turned away for asking too much: it waits, longer each time, rather than failing the download)
                    (None, Reply::Saved { status: 429, .. }) if plan.found.referer.is_none() => {
                        let wait = plan.waits.get(&i).map_or(BUSY_WAIT_MS, |w| w * 2);
                        if wait > BUSY_WAIT_MOST_MS {
                            return (None, Step::fail(Fail::Dropped(rules::DROPPED.into())));
                        }
                        plan.waits.insert(i, wait);
                        plan.out += 1;
                        (Some(Phase::Run(plan)), Step::io(vec![(Tag::Again(i), Io::Wait(wait))]))
                    }
                    (None, Reply::Saved { status, .. }) => {
                        (None, Step::fail(Fail::Dropped(format!("The stream answered {status}."))))
                    }
                    _ => (None, Step::fail(Fail::Dropped(rules::DROPPED.into()))),
                }
            }
            (Phase::Run(plan), Tag::Again(i)) => {
                let io = vec![(Tag::Part(i), fetch(&plan, i))];
                (Some(Phase::Run(plan)), Step::io(io))
            }
            (Phase::Run(mut plan), Tag::Append(t)) => {
                let Some(d) = disk(&reply).filter(|d| d.ok) else {
                    return (None, Step::fail(Fail::Cant(format!("{} {}", rules::CANT_SAVE, disk(&reply).map(|d| d.message.clone()).unwrap_or_default()))));
                };
                let track = &mut plan.tracks[t];
                track.appending = false;
                track.joined += 1;
                track.bytes = d.bytes;
                // (where it got to, kept with every part added: a round cut short carries on from there)
                let mark = track.mark.clone().map(|path| DiskOp::Write { path, text: format!("{}:{}", track.joined, track.bytes) });
                let mut io: Vec<(Tag, Io)> = mark.map(|op| (Tag::Mark, Io::Disk(op))).into_iter().collect();
                io.extend(append_next(&mut plan, t));
                io.extend(self.tick(&mut plan, row, cx));
                self.go_on(plan, io, row, cx)
            }
            (Phase::Run(mut plan), Tag::Room) => {
                let free = disk(&reply).map(|d| d.bytes);
                shared.free = free.or(shared.free);
                if free.is_some_and(|f| !rules::fits(f, rules::ROOM_WHILE_GOING)) {
                    return (None, Step::fail(Fail::Full));
                }
                let io = self.tick(&mut plan, row, cx);
                self.go_on(plan, io, row, cx)
            }
            (Phase::Captions(mut plan), Tag::Captions(n)) => {
                let (io, vtt) = captions_answer(&plan, n, reply, row, cx);
                if !io.is_empty() {
                    return (Some(Phase::Captions(plan)), Step::io(io));
                }
                plan.vtt = vtt.filter(|v| !v.trim().is_empty());
                self.finish(plan, row)
            }
            (Phase::Finish(mut plan), Tag::Finish) => {
                if let Some((_, must)) = plan.finish.pop_front() {
                    if must && !disk(&reply).is_some_and(|d| d.ok) {
                        let why = disk(&reply).map(|d| d.message.clone()).unwrap_or_default();
                        return (None, Step::fail(Fail::Cant(format!("{} {why}", rules::CANT_SAVE).trim().to_string())));
                    }
                }
                match plan.finish.front() {
                    Some((op, _)) => {
                        let io = vec![(Tag::Finish, Io::Disk(op.clone()))];
                        (Some(Phase::Finish(plan)), Step::io(io))
                    }
                    None => (None, Step { io: vec![], end: Some(End::Done { total: down(&plan) }) }),
                }
            }
            (phase, _) => (Some(phase), Step::default()),
        }
    }

    fn weighed(&mut self, found: Found, w: Weight, next: Next, row: &mut Row) -> (Option<Phase>, Step) {
        match next {
            Next::Ask(io) => (Some(Phase::Weigh(found, w)), Step::io(io.into_iter().map(|(i, io)| (Tag::Weigh(i), io)).collect())),
            Next::Bytes(estimate) => {
                let plan = plan(found, self.form, estimate, row);
                let io = have_asks(&plan, row);
                let left = io.iter().filter(|(t, _)| matches!(t, Tag::Have(_))).count();
                let plan = Plan { have_left: left, ..plan };
                if left == 0 {
                    return (Some(Phase::Room(plan)), Step::io(vec![(Tag::Room, Io::Disk(DiskOp::Free))]));
                }
                (Some(Phase::Have(plan)), Step::io(io))
            }
        }
    }

    /// Its progress shown and its room checked now and then.
    fn tick(&mut self, plan: &mut Plan, row: &mut Row, cx: &Context) -> Vec<(Tag, Io)> {
        let now = cx.clock.now;
        let mut io = vec![];
        let all_in = plan.next >= plan.parts.len() && plan.out == 0;
        if now - self.shown >= TICK || all_in {
            self.shown = now;
            let bytes = down(plan);
            row.done = bytes;
            row.size = match plan.form {
                // (the estimate for the first few pieces; past them, what the pieces so far weigh on average, times
                // all of them, which closes in on the real size)
                Form::Joined => {
                    let (n, joined) = plan.tracks.iter().fold((0, 0), |(n, j), t| (n + t.count, j + t.joined));
                    let whole = if plan.estimate > 0 && joined * 20 < n || joined == 0 { plan.estimate } else { bytes * n as u64 / joined as u64 };
                    whole.max(bytes)
                }
                Form::Stretches => plan.estimate.max(bytes),
            };
        }
        if now - self.roomed >= ROOM_EVERY {
            self.roomed = now;
            io.push((Tag::Room, Io::Disk(DiskOp::Free)));
        }
        io
    }

    fn go_on(&mut self, plan: Plan, mut io: Vec<(Tag, Io)>, row: &mut Row, cx: &Context) -> (Option<Phase>, Step) {
        let finished = plan.next >= plan.parts.len() && plan.out == 0 && plan.tracks.iter().all(Track::finished);
        if !finished {
            return (Some(Phase::Run(plan)), Step::io(io));
        }
        let (phase, step) = self.captions(plan, row, cx);
        io.extend(step.io);
        (phase, Step { io, end: step.end })
    }

    fn next_or_captions(&mut self, plan: Plan, row: &mut Row, cx: &Context) -> (Option<Phase>, Step) {
        self.go_on(plan, vec![], row, cx)
    }

    fn captions(&mut self, plan: Plan, row: &Row, cx: &Context) -> (Option<Phase>, Step) {
        let f = &plan.found;
        let timed = |mut r: Request| {
            r.timeout_ms = CAPTIONS_MS;
            r
        };
        let ask = if let Some(y) = &f.youtube {
            y.captions.as_ref().map(|u| probe::source_request(u, None, vec![Header::new("User-Agent", probe::YOUTUBE_AGENT)]))
        } else if let Some(imdb) = row.imdb.as_deref().filter(|_| row.copy.starts_with("dulo|")) {
            let mut body = probe::ids(imdb, row.season, row.episode);
            let length = crate::hls::written(&f.playlist).length();
            if length > 60. {
                body["length"] = json!(length);
            }
            body["quick"] = json!(true);
            Some(probe::server_post(cx, "/api/cinema/subtitles", &body))
        } else if let Some(stream) = row.copy.strip_prefix("ary|") {
            let path = format!("/api/ary/captions?stream={}", url::form_urlencoded::byte_serialize(stream.as_bytes()).collect::<String>());
            Some(crate::api::get(&cx.base, &cx.who(), &path, None))
        } else {
            f.subtitles.as_ref().map(|u| probe::source_request(u, f.referer.as_deref(), vec![]))
        };
        match ask {
            Some(r) => (Some(Phase::Captions(plan)), Step::io(vec![(Tag::Captions(0), Io::Ask(timed(r)))])),
            None => self.finish(plan, row),
        }
    }

    fn finish(&mut self, mut plan: Plan, row: &Row) -> (Option<Phase>, Step) {
        let key = &row.key;
        let mut ops: VecDeque<(DiskOp, bool)> = VecDeque::new();
        if let Some(vtt) = plan.vtt.take() {
            ops.push_back((DiskOp::Write { path: file(key, "vtt"), text: vtt }, false));
        }
        match plan.form {
            Form::Joined | Form::Stretches => {
                if plan.tracks.len() > 1 {
                    ops.push_back((DiskOp::Rename { from: file(key, "sound.part"), to: file(key, "sound") }, true));
                }
                ops.push_back((DiskOp::Rename { from: file(key, "part"), to: file(key, "video") }, true));
                ops.push_back((DiskOp::Remove { paths: vec![file(key, "at"), file(key, "sound.at"), file(key, "got")] }, false));
            }
        }
        let first = ops.front().map(|(op, _)| op.clone());
        plan.finish = ops;
        match first {
            Some(op) => (Some(Phase::Finish(plan)), Step::io(vec![(Tag::Finish, Io::Disk(op))])),
            None => (None, Step { io: vec![], end: Some(End::Done { total: down(&plan) }) }),
        }
    }
}

fn disk(reply: &Reply) -> Option<&DiskReply> {
    match reply {
        Reply::Disk { disk } => Some(disk),
        _ => None,
    }
}

/// What's down, all told.
fn down(plan: &Plan) -> u64 {
    plan.tracks.iter().map(|t| t.bytes).sum()
}

fn plan(found: Found, form: Form, estimate: u64, row: &Row) -> Plan {
    let key = &row.key;
    let mut parts = vec![];
    let mut tracks = vec![];
    let files: Vec<(String, u64, &str)> = match (&found.youtube, &found.file) {
        (Some(y), _) => vec![(y.video.clone(), y.video_size, "part"), (y.sound.clone(), y.sound_size, "sound.part")],
        (None, Some(f)) => vec![(f.url.clone(), f.size, "part")],
        _ => vec![],
    };
    let stretch = if found.file.is_some() { FILE_STRETCH } else { STRETCH };
    if !files.is_empty() {
        for (t, (url, size, into)) in files.into_iter().enumerate() {
            let ranges: Vec<(u64, u64)> = if size == 0 {
                vec![(0, u64::MAX)]
            } else {
                // (a film's last one open-ended: a size a little off still comes down whole)
                let n = size.div_ceil(stretch);
                let last = |i: u64| if found.file.is_some() { u64::MAX } else { size - 1 }.min(if i + 1 == n { u64::MAX } else { (i + 1) * stretch - 1 });
                (0..n).map(|i| (i * stretch, last(i))).collect()
            };
            for (n, range) in ranges.iter().enumerate() {
                parts.push(Part { url: url.clone(), range: Some(*range), file: format!("{}/{t}_{n}", file(key, "got")), track: t, n });
            }
            tracks.push(Track { into: Some(file(key, into)), count: ranges.len(), sized: size > 0, ..Default::default() });
        }
    } else {
        for (t, (at, playlist)) in [(found.at.clone(), found.playlist.clone())].into_iter().enumerate() {
            let media = crate::hls::media(&playlist, &at);
            // (the init piece first when the pieces are fMP4: joined, they're one playable file)
            let mut urls: Vec<(Option<usize>, String)> = media.init.map(|u| (None, u)).into_iter().collect();
            urls.extend(media.pieces.into_iter().enumerate().map(|(n, p)| (Some(n), p.url)));
            let count = urls.len();
            for (n, (_, url)) in urls.into_iter().enumerate() {
                parts.push(Part { url, range: None, file: format!("{}/{t}_{n}", file(key, "got")), track: t, n });
            }
            let (into, mark) = if t == 0 { (file(key, "part"), file(key, "at")) } else { (file(key, "sound.part"), file(key, "sound.at")) };
            tracks.push(Track { into: Some(into), mark: Some(mark), count, ..Default::default() });
        }
    }
    Plan { found, form, estimate, stretch, parts, tracks, next: 0, out: 0, waits: BTreeMap::new(), have_left: 0, finish: VecDeque::new(), vtt: None }
}

/// What's asked of the device before a transfer starts: what's down already.
fn have_asks(plan: &Plan, row: &Row) -> Vec<(Tag, Io)> {
    let key = &row.key;
    match plan.form {
        Form::Joined => plan
            .tracks
            .iter()
            .enumerate()
            .flat_map(|(t, track)| {
                let into = track.into.clone().unwrap_or_default();
                let mark = track.mark.clone().unwrap_or_default();
                [(Tag::Have(t * 2), Io::Disk(DiskOp::Read { path: mark })), (Tag::Have(t * 2 + 1), Io::Disk(DiskOp::Sizes { paths: vec![into] }))]
            })
            .collect(),
        Form::Stretches => vec![(Tag::Have(0), Io::Disk(DiskOp::Sizes { paths: vec![file(key, "part"), file(key, "sound.part")] }))],
    }
}

/// What the device said is down already.
fn have(plan: &mut Plan, t: usize, reply: Reply) {
    let Reply::Disk { disk } = reply else { return };
    match plan.form {
        Form::Joined => {
            let track = &mut plan.tracks[t / 2];
            if t.is_multiple_of(2) {
                let kept = disk.text.as_deref().and_then(|s| {
                    let (a, b) = s.trim().split_once(':')?;
                    Some((a.parse::<usize>().ok()?, b.parse::<u64>().ok()?))
                });
                (track.joined, track.bytes) = kept.unwrap_or((0, 0));
            } else {
                track.on_disk = disk.entries.first().map_or(0, |e| e.bytes);
            }
        }
        Form::Stretches => {
            for (track, e) in plan.tracks.iter_mut().zip(&disk.entries) {
                track.on_disk = e.bytes;
            }
        }
    }
}

/// Where a joined download carries on from: what was kept of it, with what's past the last whole part dropped; a
/// stretch's from its whole stretches (one whose size isn't known starts again).
fn resume(plan: &mut Plan) {
    for t in 0..plan.tracks.len() {
        let track = &mut plan.tracks[t];
        match plan.form {
            Form::Joined if track.joined > track.count || track.bytes > track.on_disk => (track.joined, track.bytes) = (0, 0),
            Form::Joined => {}
            Form::Stretches if track.sized => {
                track.joined = ((track.on_disk / plan.stretch) as usize).min(track.count);
                track.bytes = (track.joined as u64 * plan.stretch).min(track.on_disk);
            }
            Form::Stretches => (track.joined, track.bytes) = (0, 0),
        }
        let joined = track.joined;
        plan.parts.retain(|p| p.track != t || p.n >= joined);
    }
}

fn fetch(plan: &Plan, i: usize) -> Io {
    let part = &plan.parts[i];
    let mut extra = vec![];
    if let Some((from, to)) = part.range {
        extra.push(Header::new("Range", &if to == u64::MAX { format!("bytes={from}-") } else { format!("bytes={from}-{to}") }));
        extra.push(Header::new("Accept-Encoding", "identity"));
        extra.push(Header::new("User-Agent", probe::YOUTUBE_AGENT));
    }
    let mut request = probe::source_request(&part.url, plan.found.referer.as_deref(), vec![]);
    if !extra.is_empty() {
        request.headers.retain(|h| !h.name.eq_ignore_ascii_case("User-Agent"));
        request.headers.extend(extra);
    }
    Io::Fetch(request, part.file.clone())
}

/// A part is down: added to its file once those before it are.
fn landed(plan: &mut Plan, i: usize, bytes: u64) -> Vec<(Tag, Io)> {
    let (t, n, file) = (plan.parts[i].track, plan.parts[i].n, plan.parts[i].file.clone());
    plan.tracks[t].landed.insert(n, (file, bytes));
    append_next(plan, t)
}

/// The next part of a joined track added to its file, when it's down and nothing else is being added.
fn append_next(plan: &mut Plan, t: usize) -> Vec<(Tag, Io)> {
    let track = &mut plan.tracks[t];
    if track.appending {
        return vec![];
    }
    let Some((from, bytes)) = track.landed.remove(&track.joined) else { return vec![] };
    if bytes == 0 {
        // (a stretch past the end: nothing to add, and nothing after it)
        track.ended = true;
        return vec![];
    }
    track.appending = true;
    let to = track.into.clone().unwrap_or_default();
    vec![(Tag::Append(t), Io::Disk(DiskOp::Append { from, to, at: track.bytes }))]
}

/// The captions' answer: another ask (dulo's full sync while the quick ones are lined up), or the text to keep.
fn captions_answer(plan: &Plan, n: u8, reply: Reply, row: &Row, cx: &Context) -> (Vec<(Tag, Io)>, Option<String>) {
    let Reply::Response { status: 200..=299, body, .. } = reply else { return (vec![], None) };
    let first_vtt = |body: &str| -> Option<String> {
        let v: Value = serde_json::from_str(body).ok()?;
        v["subtitles"].as_array()?.first()?["vtt"].as_str().map(String::from)
    };
    if plan.found.youtube.is_some() || (plan.found.referer.is_some() && !row.copy.starts_with("ary|")) {
        return (vec![], Some(body));
    }
    if row.copy.starts_with("dulo|") && n == 0 {
        let v: Value = serde_json::from_str(&body).unwrap_or_default();
        if v["syncing"] == json!(true) {
            let mut again = probe::ids(row.imdb.as_deref().unwrap_or_default(), row.season, row.episode);
            let length = crate::hls::written(&plan.found.playlist).length();
            if length > 60. {
                again["length"] = json!(length);
            }
            again["quick"] = json!(false);
            let mut r = probe::server_post(cx, "/api/cinema/subtitles", &again);
            r.timeout_ms = CAPTIONS_MS;
            return (vec![(Tag::Captions(1), Io::Ask(r))], None);
        }
    }
    (vec![], first_vtt(&body))
}

// ---------- a chapter ----------

#[derive(Debug, Default)]
enum ChapterPhase {
    #[default]
    Pages,
    Have,
    Room,
    Run,
    Finish,
}

/// A manhwa chapter coming down, into its folder ([`rules::chapter_folder`]).
#[derive(Debug, Default)]
pub struct ChapterJob {
    phase: ChapterPhase,
    pages: Vec<String>,
    have: Vec<bool>,
    queue: VecDeque<usize>,
    pub out: usize,
    finish_left: usize,
    roomed: f64,
}

/// A chapter's folder, as a path.
pub fn chapter_dir(path: &str) -> String {
    format!("manhwa/{}", rules::chapter_folder(path))
}

impl ChapterJob {
    pub fn start(&mut self, row: &Row, cx: &Context) -> Step {
        *self = ChapterJob::default();
        let r = crate::api::get(&cx.base, &cx.who(), &crate::manhwa::pages_path(&row.key), None);
        Step::io(vec![(Tag::Pages, Io::Ask(r))])
    }

    pub fn pump(&mut self, row: &Row) -> Vec<(Tag, Io)> {
        if !matches!(self.phase, ChapterPhase::Run) {
            return vec![];
        }
        let dir = chapter_dir(&row.key);
        let mut io = vec![];
        while self.out < PAGES_AT_ONCE {
            let Some(i) = self.queue.pop_front() else { break };
            let request = crate::manhwa::page_request(&self.pages[i], vec![]);
            io.push((Tag::Part(i), Io::Fetch(request, format!("{dir}/{}", rules::page_part(i)))));
            self.out += 1;
        }
        io
    }

    pub fn answer(&mut self, tag: Tag, reply: Reply, row: &mut Row, cx: &Context, shared: &mut Shared) -> Step {
        let dir = chapter_dir(&row.key);
        match (&self.phase, tag) {
            (ChapterPhase::Pages, Tag::Pages) => match crate::api::outcome(reply, cx.online) {
                crate::api::Outcome::Body { body, .. } => {
                    let Ok(pages) = crate::manhwa::parse_pages(&body) else { return Step::fail(Fail::Cant(rules::CHAPTER_NOT_OUT.into())) };
                    if pages.is_empty() {
                        return Step::fail(Fail::Cant(rules::CHAPTER_NOT_OUT.into()));
                    }
                    self.have = vec![false; pages.len()];
                    let paths = (0..pages.len()).map(|i| format!("{dir}/{}", rules::page_file(i))).collect();
                    self.pages = pages;
                    self.phase = ChapterPhase::Have;
                    Step::io(vec![(Tag::Have(0), Io::Disk(DiskOp::Sizes { paths }))])
                }
                crate::api::Outcome::Failed { failure } if failure.status() != 0 && failure.status() != 401 => {
                    Step::fail(Fail::Cant(rules::CHAPTER_NOT_OUT.into()))
                }
                _ => Step::fail(Fail::Dropped(rules::DROPPED.into())),
            },
            (ChapterPhase::Have, Tag::Have(_)) => {
                if let Reply::Disk { disk } = reply {
                    for (i, e) in disk.entries.iter().enumerate().take(self.have.len()) {
                        self.have[i] = e.bytes > 0;
                    }
                }
                let done = self.have.iter().filter(|h| **h).count() as u64;
                let total = self.pages.len() as u64;
                row.done = done;
                row.size = total;
                self.queue = (0..self.pages.len()).filter(|i| !self.have[*i]).collect();
                self.phase = ChapterPhase::Room;
                Step::io(vec![(Tag::Room, Io::Disk(DiskOp::Free))])
            }
            (ChapterPhase::Room, Tag::Room) => {
                let free = disk(&reply).map(|d| d.bytes);
                shared.free = free.or(shared.free);
                if free.is_some_and(|f| !rules::fits(f, rules::chapter_bytes_left(row.done, row.size))) {
                    return Step::fail(Fail::Full);
                }
                self.roomed = cx.clock.now;
                self.phase = ChapterPhase::Run;
                self.more(row)
            }
            (ChapterPhase::Run, Tag::Part(i)) => {
                self.out -= 1;
                match reply {
                    Reply::Saved { status: 200..=299, bytes, .. } if bytes > 0 => {
                        Step::io(vec![(Tag::Check(i), Io::Disk(DiskOp::Picture { path: format!("{dir}/{}", rules::page_part(i)) }))])
                    }
                    Reply::Saved { status: 401 | 403, .. } => Step::fail(Fail::Cant(SIGN_IN.into())),
                    // (a page that isn't there is most often the connection: tried again, never a page kept broken)
                    Reply::Saved { .. } => Step::fail(Fail::Dropped(rules::PAGE_MISSING.into())),
                    _ => Step::fail(Fail::Dropped(rules::DROPPED.into())),
                }
            }
            (ChapterPhase::Run, Tag::Check(i)) => {
                if !disk(&reply).is_some_and(|d| d.ok) {
                    return Step::fail(Fail::Dropped(rules::PAGE_MISSING.into()));
                }
                let from = format!("{dir}/{}", rules::page_part(i));
                Step::io(vec![(Tag::Keep(i), Io::Disk(DiskOp::Rename { from, to: format!("{dir}/{}", rules::page_file(i)) }))])
            }
            (ChapterPhase::Run, Tag::Keep(i)) => {
                if !disk(&reply).is_some_and(|d| d.ok) {
                    return Step::fail(Fail::Cant(rules::CANT_SAVE.into()));
                }
                self.have[i] = true;
                row.done = self.have.iter().filter(|h| **h).count() as u64;
                let mut step = self.more(row);
                if cx.clock.now - self.roomed >= ROOM_EVERY {
                    self.roomed = cx.clock.now;
                    step.io.push((Tag::Room, Io::Disk(DiskOp::Free)));
                }
                step
            }
            (ChapterPhase::Run, Tag::Room) => {
                let free = disk(&reply).map(|d| d.bytes);
                shared.free = free.or(shared.free);
                if free.is_some_and(|f| !rules::fits(f, rules::ROOM_WHILE_GOING)) {
                    return Step::fail(Fail::Full);
                }
                Step::default()
            }
            (ChapterPhase::Finish, Tag::Finish) => {
                if !disk(&reply).is_some_and(|d| d.ok) {
                    return Step::fail(Fail::Cant(rules::CANT_SAVE.into()));
                }
                self.finish_left -= 1;
                if self.finish_left > 0 {
                    return Step::default();
                }
                Step::io(vec![(Tag::Bytes, Io::Disk(DiskOp::Listing { path: dir }))])
            }
            (ChapterPhase::Finish, Tag::Bytes) => {
                let bytes = disk(&reply).map_or(0, |d| d.entries.iter().map(|e| e.bytes).sum());
                row.bytes = bytes;
                Step { io: vec![], end: Some(End::Done { total: row.size }) }
            }
            _ => Step::default(),
        }
    }

    fn more(&mut self, row: &Row) -> Step {
        let io = self.pump(row);
        if !io.is_empty() || self.out > 0 || !self.queue.is_empty() {
            return Step::io(io);
        }
        let dir = chapter_dir(&row.key);
        self.phase = ChapterPhase::Finish;
        self.finish_left = 2;
        Step::io(vec![
            (Tag::Finish, Io::Disk(DiskOp::Write { path: format!("{dir}/{}", rules::COMPLETE), text: self.pages.len().to_string() })),
            (Tag::Finish, Io::Disk(DiskOp::Write { path: format!("{dir}/{}", rules::KEPT), text: "1".into() })),
        ])
    }
}

/// A download's transfer.
#[derive(Debug)]
pub enum Job {
    Video(Box<VideoJob>),
    Chapter(ChapterJob),
}

impl Job {
    pub fn new(row: &Row, cx: &Context) -> Job {
        match row.kind {
            Kind::Video => Job::Video(Box::new(VideoJob::new(row, cx))),
            Kind::Chapter => Job::Chapter(ChapterJob::default()),
        }
    }

    pub fn start(&mut self, row: &Row, plus: bool, cx: &Context, shared: &Shared) -> Step {
        match self {
            Job::Video(v) => v.start(row, plus, cx, shared),
            Job::Chapter(c) => c.start(row, cx),
        }
    }

    pub fn answer(&mut self, tag: Tag, reply: Reply, row: &mut Row, cx: &Context, shared: &mut Shared) -> Step {
        match self {
            Job::Video(v) => v.answer(tag, reply, row, cx, shared),
            Job::Chapter(c) => c.answer(tag, reply, row, cx, shared),
        }
    }
}
