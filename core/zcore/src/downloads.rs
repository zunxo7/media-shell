//! Downloads, of every kind an app keeps for offline: a film or an episode (its video's files) and a manhwa chapter (its
//! pages' images). One model for both: where a download stands and how that changes, which start next and how many at
//! once, when a dropped one is tried again, how far along it is, where its files go, and how the Downloads page groups
//! and words it. The apps fetch and keep the files.
//!
//! A new kind is a [`Kind`] (its name and how many run at once), its words, and the fetch in each app.

use std::collections::HashMap;

/// Where a download stands. Kept as its upper-case name ("QUEUED"), as both apps' lists always have.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum State {
    #[default]
    Queued,
    Running,
    Paused,
    Failed,
    Done,
}

impl State {
    /// A kept state; anything unknown waits its turn.
    pub fn parse(text: &str) -> State {
        match text {
            "RUNNING" => State::Running,
            "PAUSED" => State::Paused,
            "FAILED" => State::Failed,
            "DONE" => State::Done,
            _ => State::Queued,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            State::Queued => "QUEUED",
            State::Running => "RUNNING",
            State::Paused => "PAUSED",
            State::Failed => "FAILED",
            State::Done => "DONE",
        }
    }

    /// Read back when the app starts, or its connection went: one that was going waits its turn again.
    pub fn restored(self) -> State {
        if self == State::Running {
            State::Queued
        } else {
            self
        }
    }

    /// Paused: one already down stays down.
    pub fn paused(self) -> State {
        if self == State::Done {
            self
        } else {
            State::Paused
        }
    }

    /// Carried on (paused) or tried again (failed): back in the queue. One going, or down, stays as it is.
    pub fn resumed(self) -> State {
        match self {
            State::Paused | State::Failed => State::Queued,
            _ => self,
        }
    }

    /// Coming down or waiting to.
    pub fn going(self) -> bool {
        matches!(self, State::Queued | State::Running)
    }

    /// How a transfer that stopped leaves it: down; or, when it failed, failed, unless downloads were switched off
    /// under it (`stopped`: the app closing, the connection it may use gone), when it waits for them again.
    pub fn ended(ok: bool, stopped: bool) -> State {
        match (ok, stopped) {
            (true, _) => State::Done,
            (false, true) => State::Queued,
            (false, false) => State::Failed,
        }
    }
}

/// What a download is. Kept as its name ("video", "chapter"); a list from before chapters is all videos.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A film or an episode: its picture (and sound, captions) as files.
    #[default]
    Video,
    /// A manhwa chapter: its pages as image files in a folder of their own.
    Chapter,
}

impl Kind {
    pub fn parse(text: &str) -> Kind {
        if text == "chapter" {
            Kind::Chapter
        } else {
            Kind::Video
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Video => "video",
            Kind::Chapter => "chapter",
        }
    }

    /// How many of this kind come down at once: three videos (each already many pieces at a time), two chapters.
    pub fn at_once(self) -> usize {
        match self {
            Kind::Video => 3,
            Kind::Chapter => 2,
        }
    }
}

/// A download as the queue sees it: its kind, where it stands, and whether a transfer of it is running in the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub kind: Kind,
    pub state: State,
    pub busy: bool,
}

/// The waiting downloads to start now, in the order they were added, up to each kind's [`Kind::at_once`] with what's
/// busy counted in (as indices into `slots`).
pub fn to_start(slots: &[Slot]) -> Vec<usize> {
    let mut busy: HashMap<Kind, usize> = HashMap::new();
    for s in slots.iter().filter(|s| s.busy) {
        *busy.entry(s.kind).or_default() += 1;
    }
    let mut start = Vec::new();
    for (i, s) in slots.iter().enumerate() {
        let running = busy.entry(s.kind).or_default();
        if s.state == State::Queued && !s.busy && *running < s.kind.at_once() {
            *running += 1;
            start.push(i);
        }
    }
    start
}

/// A dropped connection is tried again this many times in a row; the count starts afresh whenever a try got further.
pub const TRIES: u32 = 6;

/// How long to wait before try `tries` (1 the first time again) after a dropped connection, in ms, a little longer each
/// time; None once it's tried [`TRIES`] times: it failed.
pub fn retry_wait(tries: u32) -> Option<u64> {
    (1..=TRIES).contains(&tries).then(|| 2500 * u64::from(tries))
}

/// Left free on the device whatever downloads want.
pub const SPARE: u64 = 100_000_000;
/// What must still be free, besides [`SPARE`], while a download comes down (checked as it goes).
pub const ROOM_WHILE_GOING: u64 = 64_000_000;

/// Whether `needed` more bytes fit in `free` with [`SPARE`] left over.
pub fn fits(free: u64, needed: u64) -> bool {
    free >= needed.saturating_add(SPARE)
}

/// How far along, 0 to 1 (0 while its size isn't known).
pub fn share(done: u64, size: u64) -> f32 {
    if size == 0 {
        0.
    } else {
        (done as f64 / size as f64).clamp(0., 1.) as f32
    }
}

// ---------- each profile's own ----------

/// Whether a download is this profile's: one listing no profiles was saved before profiles had their own, so it's
/// everyone's.
pub fn owned(profiles: Option<&[String]>, me: &str) -> bool {
    profiles.is_none_or(|list| list.iter().any(|p| p == me))
}

/// The profiles of a download this profile also wants, or None when nothing changes (already its, or everyone's).
pub fn shared_with(profiles: Option<&[String]>, me: &str) -> Option<Vec<String>> {
    let list = profiles?;
    (!me.is_empty() && !list.iter().any(|p| p == me)).then(|| list.iter().cloned().chain([me.to_string()]).collect())
}

/// What deleting does: Some(the profiles left) when others still have it (it stays for them, files and all); None
/// when it goes for good.
pub fn after_delete(profiles: Option<&[String]>, me: &str) -> Option<Vec<String>> {
    let others: Vec<String> = profiles?.iter().filter(|p| *p != me).cloned().collect();
    (!others.is_empty()).then_some(others)
}

// ---------- where a video comes from ----------

/// What a video download needs to know of its title and episode to say where it comes from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Source<'a> {
    /// The title's source ("cinema", "anime", "youtube"; else a channel's ARY stream).
    pub source: &'a str,
    pub title_id: &'a str,
    pub has_imdb: bool,
    /// None for a film.
    pub episode_id: Option<&'a str>,
    /// An anime episode's own show (its season's id), when it has one.
    pub season_id: Option<&'a str>,
    /// The episode's own stream, else the title's.
    pub stream: Option<&'a str>,
    /// The profile's anime audio ("sub", "dub").
    pub anime_audio: &'a str,
}

/// Where a film's or an episode's download comes from, as kept with it: "yt|id", "anime|episode|show|audio",
/// "dulo|title|episode" (Movies & TV: Real-Debrid's file, the tag kept from when it was dulo) or "ary|stream". None
/// when it can't be downloaded.
pub fn copy_of(s: &Source) -> Option<String> {
    let bare = |id: &str, prefix: &str| id.strip_prefix(prefix).unwrap_or(id).to_string();
    match (s.source, s.episode_id) {
        ("youtube", Some(e)) => Some(format!("yt|{}", bare(e, "yt-"))),
        ("anime", Some(e)) => {
            let show = bare(s.season_id.unwrap_or(s.title_id), "anime-");
            Some(format!("anime|{}|{show}|{}", bare(e, "anime-"), s.anime_audio))
        }
        ("youtube" | "anime", None) => None,
        ("cinema", e) => s.has_imdb.then(|| format!("dulo|{}|{}", s.title_id, e.unwrap_or(""))),
        _ => s.stream.filter(|st| st.to_ascii_lowercase().contains("m3u8")).map(|st| format!("ary|{st}")),
    }
}

/// What a download from before levels were named says it is: its "Standard" or "High" becomes the level it got
/// ("720p").
pub fn migrated_label(label: &str, quality: &str) -> String {
    if (label == "Standard" || label == "High") && quality.ends_with('p') {
        quality.to_string()
    } else {
        label.to_string()
    }
}

// ---------- files ----------

/// A video download's file: its key made safe as a name (letters, digits and `-` kept, the rest `_`), then `ext`
/// ("video", "part", "sound", "vtt"...), as every version of both apps has named them.
pub fn file_name(key: &str, ext: &str) -> String {
    let name: String = key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    format!("{name}.{ext}")
}

/// A chapter's folder (in the app's manhwa folder): the SHA-256 of its path, as hex.
pub fn chapter_folder(path: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(path.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// A chapter's page `index` (from 0) in its folder.
pub fn page_file(index: usize) -> String {
    format!("{index}.img")
}

/// A page still coming down, renamed to [`page_file`] once it's whole.
pub fn page_part(index: usize) -> String {
    format!("{index}.part")
}

/// Written in a chapter's folder once every page is there: how many there are.
pub const COMPLETE: &str = "complete";
/// Written in a chapter's folder when it was downloaded on purpose: only those are in Downloads (one just read isn't).
pub const KEPT: &str = "kept";

/// How many pages a chapter's [`COMPLETE`] says it has; None when it isn't a count.
pub fn saved_pages(complete: &str) -> Option<usize> {
    complete.trim().parse().ok().filter(|n| *n > 0)
}

/// About what's still to come of a chapter (a quarter MB a page; 12 MB while its pages aren't known).
pub fn chapter_bytes_left(done: u64, total: u64) -> u64 {
    if total == 0 {
        12_000_000
    } else {
        total.saturating_sub(done).max(1) * 250_000
    }
}

// ---------- how fast ----------

/// How fast each download comes down, by its key: bytes a second, eased over the last few seconds.
#[derive(Clone, Debug, Default)]
pub struct Speeds(HashMap<String, (u64, f64, f64)>);

impl Speeds {
    /// `done` bytes of `key` are down at `now` (ms).
    pub fn seen(&mut self, key: &str, done: u64, now: f64) {
        let rate = match self.0.get(key) {
            Some(&(before, at, rate)) if now > at && done >= before => {
                let fresh = (done - before) as f64 * 1000. / (now - at);
                if rate > 0. {
                    rate * 0.6 + fresh * 0.4
                } else {
                    fresh
                }
            }
            _ => 0.,
        };
        self.0.insert(key.to_string(), (done, now, rate));
    }

    /// How fast `key` comes down at `now`; None when it isn't. Pieces land in order, so one slow piece holds the next
    /// back a few seconds: the last speed stays (easing down as the wait goes on) for 15 s rather than blinking out.
    pub fn now(&self, key: &str, now: f64) -> Option<f64> {
        let &(_, at, rate) = self.0.get(key)?;
        let quiet = now - at;
        (quiet < 15_000. && rate > 0.).then(|| if quiet < 3000. { rate } else { rate * 3000. / quiet })
    }
}

// ---------- words ----------

/// The room left for downloads, as Downloads shows it.
pub fn room_words(free: u64) -> String {
    format!("{} free on this device", size_words(free))
}

/// A size as Downloads says it: "1.2 GB", or under a gigabyte "350 MB".
pub fn size_words(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1e9)
    } else {
        format!("{:.0} MB", bytes as f64 / 1e6)
    }
}

/// How fast, in words: "12.4 MB/s", or under a megabyte a second "450 KB/s".
pub fn rate_words(bytes_per_second: f64) -> String {
    if bytes_per_second >= 1e6 {
        format!("{:.1} MB/s", bytes_per_second / 1e6)
    } else {
        format!("{:.0} KB/s", bytes_per_second / 1e3)
    }
}

pub const CANT: &str = "This one can't be downloaded right now.";
pub const FULL: &str = "Not enough space on this device.";
pub const DROPPED: &str = "The connection dropped.";
pub const NOT_ANSWERING: &str = "The stream isn't answering right now.";
pub const CANT_SAVE: &str = "Couldn't save the download.";
pub const CHAPTER_NOT_OUT: &str = "This chapter isn't available yet.";
pub const PAGE_MISSING: &str = "A page of this chapter didn't load.";

/// Why a download didn't finish, in words a viewer can act on: a reason the apps give as it is, else the connection.
pub fn why(note: &str) -> String {
    const SAID: [&str; 4] = [CANT, FULL, DROPPED, NOT_ANSWERING];
    if note.is_empty() {
        "Didn't finish".into()
    } else if SAID.contains(&note) {
        note.into()
    } else if note.starts_with(CANT_SAVE) {
        CANT_SAVE.into()
    } else if note.starts_with("Not enough space") {
        FULL.into()
    } else {
        DROPPED.into()
    }
}

/// Why a download waits: its turn, or (`online` false) the connection downloads may use: Wi-Fi only (`wifi_only`), or
/// any.
pub fn waiting(online: bool, wifi_only: bool) -> String {
    match (online, wifi_only) {
        (true, _) => "Waiting".into(),
        (false, true) => "Waiting for Wi-Fi".into(),
        (false, false) => "Waiting for a connection".into(),
    }
}

/// A download as its line on Downloads words it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line<'a> {
    pub kind: Kind,
    pub state: Option<State>,
    /// A video's bytes (what it will weigh, what's down); a chapter's pages while it comes down.
    pub size: u64,
    pub done: u64,
    /// A finished chapter's bytes on the device.
    pub bytes: u64,
    /// A video's level, "1080p".
    pub label: &'a str,
    pub note: &'a str,
    pub speed: Option<f64>,
    pub online: bool,
    pub wifi_only: bool,
    /// The speed on a line of its own (a phone's row has no room beside the sizes).
    pub stacked: bool,
}

/// What a download is doing, in a few words: how far and how fast while it comes down, why it waits, why it failed,
/// what it is once down ("1080p · 1.2 GB", a chapter's size). "" for a chapter not downloaded.
pub fn line(l: &Line) -> String {
    let Some(state) = l.state else { return String::new() };
    match (state, l.kind) {
        (State::Done, Kind::Video) => [l.label.to_string(), size_words(l.size)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · "),
        (State::Done, Kind::Chapter) => if l.bytes > 0 { size_words(l.bytes) } else { "Downloaded".into() },
        (State::Running, Kind::Video) if l.size > 0 && l.done > 0 => {
            let speed = l.speed.map(|r| format!("{}{}", if l.stacked { "\n" } else { " · " }, rate_words(r))).unwrap_or_default();
            format!("{} of {}{speed}", size_words(l.done), size_words(l.size))
        }
        (State::Running, Kind::Chapter) if l.size > 0 => format!("{} of {} pages", l.done, l.size),
        (State::Running, _) => "Starting".into(),
        (State::Queued, _) => waiting(l.online, l.wifi_only),
        (State::Paused, _) => "Paused".into(),
        (State::Failed, Kind::Video) => why(l.note),
        // (a chapter's reasons are its source's own words: "This chapter needs a sign-in.")
        (State::Failed, Kind::Chapter) => if l.note.is_empty() { "Didn't finish".into() } else { l.note.to_string() },
    }
}

/// A show's stacked entry on Downloads, its episodes as `lines` (each one's state, size, what's down, level).
pub fn show_line(rows: &[(State, u64, u64)], levels: &[String], speed: Option<f64>, online: bool, wifi_only: bool, stacked: bool) -> String {
    let n = rows.len();
    let finished = rows.iter().filter(|r| r.0 == State::Done).count();
    let episodes = if n == 1 { "1 episode".to_string() } else { format!("{n} episodes") };
    let size: u64 = rows.iter().map(|r| r.1).sum();
    if finished == n {
        return [level_words(levels), size_words(size)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
    }
    if !online {
        return format!("{finished} of {episodes} · {}", waiting(false, wifi_only).to_lowercase().replace("wi-fi", "Wi-Fi"));
    }
    let done: u64 = rows.iter().map(|r| if r.0 == State::Done { r.1 } else { r.2 }).sum();
    let speed = speed.filter(|r| *r > 0.).map(|r| format!(" · {}", rate_words(r))).unwrap_or_default();
    format!("{finished} of {episodes}{}{} of {}{speed}", if stacked { "\n" } else { " · " }, size_words(done), size_words(size))
}

/// The levels a show's episodes came down at, lowest first (`crate::quality::name_rank`): "1080p", "1080p/4K" for
/// two, "Mixed" past that.
pub fn level_words(levels: &[String]) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for l in levels.iter().map(String::as_str).filter(|l| !l.is_empty()) {
        if !seen.contains(&l) {
            seen.push(l);
        }
    }
    seen.sort_by_key(|l| crate::quality::name_rank(l));
    if seen.len() > 2 {
        "Mixed".into()
    } else {
        seen.join("/")
    }
}

/// What a control does when pressed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Act {
    /// Not added yet: download it (them).
    #[default]
    Add,
    Pause,
    /// Failed: try again.
    Retry,
    Resume,
    /// Down: open it (Downloads, the reader).
    Open,
}

/// What a tap on one download's control does, everywhere: nothing yet, start it; going (or waiting), pause; paused,
/// carry on; failed, try again; down, open it.
pub fn tap(state: Option<State>) -> Act {
    match state {
        None => Act::Add,
        Some(State::Queued | State::Running) => Act::Pause,
        Some(State::Paused) => Act::Resume,
        Some(State::Failed) => Act::Retry,
        Some(State::Done) => Act::Open,
    }
}

/// Whether a show's Download all takes this episode: its episodes, not its extras (season 0).
pub fn in_download_all(season: Option<i64>) -> bool {
    season.unwrap_or(1) > 0
}

/// A button for several downloads at once (a show's or a series' Download all; with `one`, a film's Download): what
/// it says and does, by the same rule as one's control. `items`: each one's state (None: not added) and how far along.
pub fn batch(items: &[(Option<State>, f32)], one: bool) -> (Act, String) {
    let n = items.len();
    let done = items.iter().filter(|i| i.0 == Some(State::Done)).count();
    let any = |s: &[State]| items.iter().any(|i| i.0.is_some_and(|x| s.contains(&x)));
    let far = if one {
        format!("{}%", (items.iter().map(|i| i.1).sum::<f32>() / n.max(1) as f32 * 100.) as i32)
    } else {
        format!("{done} of {n}")
    };
    if items.iter().any(|i| i.0.is_none()) {
        (Act::Add, if one { "Download" } else { "Download all" }.into())
    } else if done == n {
        (Act::Open, "Downloaded".into())
    } else if any(&[State::Queued, State::Running]) {
        (Act::Pause, format!("Pause · {far}"))
    } else if any(&[State::Failed]) {
        (Act::Retry, "Try again".into())
    } else if any(&[State::Paused]) {
        (Act::Resume, format!("Resume · {far}"))
    } else {
        (Act::Open, far)
    }
}

/// Which of a batch a press of its button acts on (as indices): those not added, those going, failed or paused, as
/// [`batch`] says.
pub fn batch_targets(items: &[Option<State>], act: Act) -> Vec<usize> {
    let wanted = |s: Option<State>| match act {
        Act::Add => s.is_none(),
        Act::Pause => s.is_some_and(State::going),
        Act::Retry => s == Some(State::Failed),
        Act::Resume => s == Some(State::Paused),
        Act::Open => false,
    };
    items.iter().enumerate().filter(|(_, s)| wanted(**s)).map(|(i, _)| i).collect()
}

/// The chip on a Download or Download all with what it'll take: `whole`, what all `n` weigh, less the share of those
/// already there; only while it adds (`targets` of them). "~" unless it's one file whose size is `exact` (a Real-Debrid
/// film's), not weighed from samples.
pub fn size_chip(act: Act, whole: Option<u64>, targets: usize, n: usize, exact: bool) -> Option<String> {
    let about = if exact && n == 1 { "" } else { "~" };
    whole.filter(|_| act == Act::Add && n > 0).map(|bytes| format!("{about}{}", size_words(bytes * targets as u64 / n as u64)))
}

// ---------- the Downloads page ----------

/// A download as Downloads lists it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listed<'a> {
    pub kind: Kind,
    /// What it's listed under: a video's title id, a chapter's series path.
    pub group: &'a str,
    /// Its title's or series' name (what a search looks through).
    pub name: &'a str,
    pub season: Option<i64>,
    /// An episode's number; a chapter's path (its number read from its end).
    pub episode: Option<i64>,
    pub chapter: &'a str,
}

/// Downloads' entries, as indices into `items`: every film and show as it was first added (a show's episodes in order),
/// then every manhwa by name (its chapters in order). Only those whose name has `query` in it, when there's one.
pub fn grouped(items: &[Listed], query: &str) -> Vec<Vec<usize>> {
    let query = crate::search::fold(query.trim());
    let mut groups: Vec<(Kind, &str, Vec<usize>)> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if !query.is_empty() && !crate::search::fold(item.name).contains(&query) {
            continue;
        }
        match groups.iter_mut().find(|g| g.0 == item.kind && g.1 == item.group) {
            Some(g) => g.2.push(i),
            None => groups.push((item.kind, item.group, vec![i])),
        }
    }
    for g in &mut groups {
        match g.0 {
            Kind::Video => g.2.sort_by_key(|&i| (items[i].season.unwrap_or_default(), items[i].episode.unwrap_or_default())),
            Kind::Chapter => g.2.sort_by(|&a, &b| chapter_number(items[a].chapter).total_cmp(&chapter_number(items[b].chapter))),
        }
    }
    let name = |g: &(Kind, &str, Vec<usize>)| items[g.2[0]].name.to_lowercase();
    let (videos, mut chapters): (Vec<_>, Vec<_>) = groups.into_iter().partition(|g| g.0 == Kind::Video);
    chapters.sort_by_key(name);
    videos.into_iter().chain(chapters).map(|g| g.2).collect()
}

/// A chapter's number from its path's end ("/comics/a/chapter/12.5": 12.5); 0 when it has none.
pub fn chapter_number(path: &str) -> f64 {
    path.rsplit('/').next().and_then(|n| n.parse().ok()).unwrap_or(0.)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_kept_and_changed() {
        for s in [State::Queued, State::Running, State::Paused, State::Failed, State::Done] {
            assert_eq!(State::parse(s.as_str()), s);
        }
        assert_eq!(State::parse("junk"), State::Queued);
        assert_eq!(State::Running.restored(), State::Queued);
        assert_eq!(State::Failed.restored(), State::Failed);
        assert_eq!(State::Done.paused(), State::Done);
        assert_eq!(State::Running.paused(), State::Paused);
        assert_eq!(State::Failed.resumed(), State::Queued);
        assert_eq!(State::Done.resumed(), State::Done);
        assert_eq!(State::Running.resumed(), State::Running);
        assert_eq!(State::Paused.resumed(), State::Queued);
        assert_eq!(State::ended(true, true), State::Done);
        assert_eq!(State::ended(false, true), State::Queued);
        assert_eq!(State::ended(false, false), State::Failed);
        assert_eq!(Kind::parse("chapter"), Kind::Chapter);
        assert_eq!(Kind::parse(""), Kind::Video);
    }

    #[test]
    fn each_kind_its_own_turns() {
        let slot = |kind, state, busy| Slot { kind, state, busy };
        let slots = [
            slot(Kind::Video, State::Running, true),
            slot(Kind::Video, State::Queued, false),
            slot(Kind::Chapter, State::Queued, false),
            slot(Kind::Video, State::Paused, false),
            slot(Kind::Video, State::Queued, false),
            slot(Kind::Video, State::Queued, false),
            slot(Kind::Chapter, State::Queued, false),
            slot(Kind::Chapter, State::Queued, false),
        ];
        // two more videos (three with the busy one), two chapters
        assert_eq!(to_start(&slots), vec![1, 2, 4, 6]);
        assert!(to_start(&[slot(Kind::Video, State::Queued, true)]).is_empty());
    }

    #[test]
    fn tried_again_longer_each_time_then_failed() {
        assert_eq!(retry_wait(1), Some(2500));
        assert_eq!(retry_wait(6), Some(15000));
        assert_eq!(retry_wait(7), None);
        assert_eq!(retry_wait(0), None);
        assert!(fits(200_000_000, 100_000_000));
        assert!(!fits(199_999_999, 100_000_000));
        assert_eq!(share(5, 10), 0.5);
        assert_eq!(share(5, 0), 0.);
        assert_eq!(share(20, 10), 1.);
    }

    #[test]
    fn each_profile_its_own() {
        let both = vec!["a".to_string(), "b".to_string()];
        assert!(owned(None, "a"));
        assert!(owned(Some(&both), "b"));
        assert!(!owned(Some(&both), "c"));
        assert_eq!(shared_with(Some(&both), "c"), Some(vec!["a".into(), "b".into(), "c".into()]));
        assert_eq!(shared_with(Some(&both), "a"), None);
        assert_eq!(shared_with(None, "a"), None);
        assert_eq!(after_delete(Some(&both), "a"), Some(vec!["b".into()]));
        assert_eq!(after_delete(Some(&["a".to_string()]), "a"), None);
        assert_eq!(after_delete(None, "a"), None);
    }

    #[test]
    fn where_videos_come_from() {
        let film = Source { source: "cinema", title_id: "tt1", has_imdb: true, anime_audio: "sub", ..Default::default() };
        assert_eq!(copy_of(&film).as_deref(), Some("dulo|tt1|"));
        assert_eq!(copy_of(&Source { has_imdb: false, ..film.clone() }), None);
        let anime = Source { source: "anime", title_id: "anime-9", episode_id: Some("anime-12"), anime_audio: "dub", ..Default::default() };
        assert_eq!(copy_of(&anime).as_deref(), Some("anime|12|9|dub"));
        assert_eq!(copy_of(&Source { season_id: Some("anime-10"), ..anime.clone() }).as_deref(), Some("anime|12|10|dub"));
        assert_eq!(copy_of(&Source { episode_id: None, ..anime }), None);
        let yt = Source { source: "youtube", episode_id: Some("yt-abc"), ..Default::default() };
        assert_eq!(copy_of(&yt).as_deref(), Some("yt|abc"));
        let ary = Source { source: "ary", stream: Some("https://x/a.M3U8"), ..Default::default() };
        assert_eq!(copy_of(&ary).as_deref(), Some("ary|https://x/a.M3U8"));
        assert_eq!(copy_of(&Source { stream: Some("https://x/a.mp4"), ..ary }), None);
        assert_eq!(migrated_label("High", "1080p"), "1080p");
        assert_eq!(migrated_label("High", "best"), "High");
        assert_eq!(migrated_label("4K", "best"), "4K");
    }

    #[test]
    fn files_named_as_ever() {
        assert_eq!(file_name("tt1:2/x", "video"), "tt1_2_x.video");
        assert_eq!(file_name("a-1", "sound.part"), "a-1.sound.part");
        // (Android's chapter folders from before: the same names)
        assert_eq!(chapter_folder("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!((page_file(3).as_str(), page_part(3).as_str()), ("3.img", "3.part"));
        assert_eq!(saved_pages(" 40\n"), Some(40));
        assert_eq!(saved_pages("0"), None);
        assert_eq!(saved_pages("x"), None);
        assert_eq!(chapter_bytes_left(10, 40), 7_500_000);
        assert_eq!(chapter_bytes_left(40, 40), 250_000);
        assert_eq!(chapter_bytes_left(0, 0), 12_000_000);
    }

    #[test]
    fn speeds_eased_and_held() {
        let mut s = Speeds::default();
        s.seen("a", 0, 0.);
        assert_eq!(s.now("a", 0.), None);
        s.seen("a", 1_000_000, 1000.);
        assert_eq!(s.now("a", 1000.), Some(1_000_000.));
        s.seen("a", 1_000_000 + 2_000_000, 2000.);
        assert_eq!(s.now("a", 2000.), Some(1_000_000. * 0.6 + 2_000_000. * 0.4));
        assert_eq!(s.now("a", 2000. + 6000.).map(|r| r.round()), Some((1_400_000. * 0.5_f64).round()));
        assert_eq!(s.now("a", 2000. + 15_000.), None);
    }

    #[test]
    fn lines_in_words() {
        assert_eq!(size_words(1_234_000_000), "1.2 GB");
        assert_eq!(size_words(350_400_000), "350 MB");
        assert_eq!(rate_words(12_400_000.), "12.4 MB/s");
        assert_eq!(rate_words(450_000.), "450 KB/s");
        assert_eq!(why(""), "Didn't finish");
        assert_eq!(why(FULL), FULL);
        assert_eq!(why("Not enough space on this phone."), FULL);
        assert_eq!(why("Couldn't save the download. Access is denied"), CANT_SAVE);
        assert_eq!(why("error sending request"), DROPPED);
        let video = Line { kind: Kind::Video, state: Some(State::Running), size: 2_000_000_000, done: 500_000_000, label: "1080p", online: true, ..Default::default() };
        assert_eq!(line(&video), "500 MB of 2.0 GB");
        assert_eq!(line(&Line { speed: Some(2e6), ..video.clone() }), "500 MB of 2.0 GB · 2.0 MB/s");
        assert_eq!(line(&Line { speed: Some(2e6), stacked: true, ..video.clone() }), "500 MB of 2.0 GB\n2.0 MB/s");
        assert_eq!(line(&Line { done: 0, ..video.clone() }), "Starting");
        assert_eq!(line(&Line { state: Some(State::Done), ..video.clone() }), "1080p · 2.0 GB");
        assert_eq!(line(&Line { state: Some(State::Queued), online: false, wifi_only: true, ..video.clone() }), "Waiting for Wi-Fi");
        assert_eq!(line(&Line { state: Some(State::Queued), online: false, ..video.clone() }), "Waiting for a connection");
        assert_eq!(line(&Line { state: Some(State::Failed), note: CANT, ..video }), CANT);
        let chapter = Line { kind: Kind::Chapter, state: Some(State::Running), size: 40, done: 3, ..Default::default() };
        assert_eq!(line(&chapter), "3 of 40 pages");
        assert_eq!(line(&Line { size: 0, ..chapter.clone() }), "Starting");
        assert_eq!(line(&Line { state: Some(State::Done), bytes: 12_000_000, ..chapter.clone() }), "12 MB");
        assert_eq!(line(&Line { state: Some(State::Done), ..chapter.clone() }), "Downloaded");
        assert_eq!(line(&Line { state: Some(State::Failed), note: PAGE_MISSING, ..chapter.clone() }), PAGE_MISSING);
        assert_eq!(line(&Line { state: Some(State::Failed), note: "This chapter needs a sign-in.", ..chapter.clone() }), "This chapter needs a sign-in.");
        assert_eq!(line(&Line { state: Some(State::Failed), ..chapter.clone() }), "Didn't finish");
        assert_eq!(line(&Line { state: None, ..chapter }), "");
    }

    #[test]
    fn a_show_in_one_line() {
        let levels = vec!["4K".to_string(), "1080p".to_string(), "1080p".to_string()];
        let done = [(State::Done, 1_000_000_000, 0), (State::Done, 500_000_000, 0)];
        assert_eq!(show_line(&done, &levels, None, true, false, false), "1080p/4K · 1.5 GB");
        let going = [(State::Done, 1_000_000_000, 0), (State::Running, 1_000_000_000, 500_000_000)];
        assert_eq!(show_line(&going, &[], Some(1e6), true, false, false), "1 of 2 episodes · 1.5 GB of 2.0 GB · 1.0 MB/s");
        assert_eq!(show_line(&going, &[], None, true, false, true), "1 of 2 episodes\n1.5 GB of 2.0 GB");
        assert_eq!(show_line(&going, &[], None, false, true, false), "1 of 2 episodes · waiting for Wi-Fi");
        assert_eq!(level_words(&["720p".into(), "1080p".into(), "4K".into()]), "Mixed");
        assert_eq!(level_words(&[]), "");
    }

    #[test]
    fn one_rule_for_every_control() {
        assert_eq!(tap(None), Act::Add);
        assert_eq!(tap(Some(State::Queued)), Act::Pause);
        assert_eq!(tap(Some(State::Paused)), Act::Resume);
        assert_eq!(tap(Some(State::Failed)), Act::Retry);
        assert_eq!(tap(Some(State::Done)), Act::Open);
        assert_eq!(batch(&[(None, 0.), (Some(State::Done), 1.)], false), (Act::Add, "Download all".into()));
        assert_eq!(batch(&[(None, 0.)], true), (Act::Add, "Download".into()));
        assert_eq!(batch(&[(Some(State::Done), 1.), (Some(State::Running), 0.5)], false), (Act::Pause, "Pause · 1 of 2".into()));
        assert_eq!(batch(&[(Some(State::Running), 0.25)], true), (Act::Pause, "Pause · 25%".into()));
        assert_eq!(batch(&[(Some(State::Failed), 0.), (Some(State::Paused), 0.)], false), (Act::Retry, "Try again".into()));
        assert_eq!(batch(&[(Some(State::Done), 1.), (Some(State::Paused), 0.)], false), (Act::Resume, "Resume · 1 of 2".into()));
        assert_eq!(batch(&[(Some(State::Done), 1.)], false), (Act::Open, "Downloaded".into()));
        let states = [None, Some(State::Queued), Some(State::Running), Some(State::Failed), Some(State::Paused)];
        assert_eq!(batch_targets(&states, Act::Add), vec![0]);
        assert_eq!(batch_targets(&states, Act::Pause), vec![1, 2]);
        assert_eq!(batch_targets(&states, Act::Retry), vec![3]);
        assert_eq!(batch_targets(&states, Act::Resume), vec![4]);
        assert_eq!(size_chip(Act::Add, Some(4_000_000_000), 1, 2, true).as_deref(), Some("~2.0 GB"));
        assert_eq!(size_chip(Act::Add, Some(2_300_000_000), 1, 1, true).as_deref(), Some("2.3 GB"));
        assert_eq!(size_chip(Act::Add, Some(2_300_000_000), 1, 1, false).as_deref(), Some("~2.3 GB"));
        assert_eq!(size_chip(Act::Pause, Some(4_000_000_000), 1, 2, false), None);
        assert_eq!(size_chip(Act::Add, None, 1, 2, false), None);
    }

    #[test]
    fn downloads_listed() {
        let ep = |group, name, season, episode| Listed { kind: Kind::Video, group, name, season: Some(season), episode: Some(episode), chapter: "" };
        let ch = |group, name, chapter| Listed { kind: Kind::Chapter, group, name, chapter, ..Default::default() };
        let items = [
            ch("/comics/z", "Zeta", "/comics/z/chapter/2"),
            ep("show", "Show", 2, 1),
            ch("/comics/a", "Alpha", "/comics/a/chapter/10"),
            Listed { kind: Kind::Video, group: "film", name: "Film", ..Default::default() },
            ep("show", "Show", 1, 3),
            ch("/comics/a", "Alpha", "/comics/a/chapter/9.5"),
        ];
        assert_eq!(grouped(&items, ""), vec![vec![4, 1], vec![3], vec![5, 2], vec![0]]);
        assert_eq!(grouped(&items, " alp "), vec![vec![5, 2]]);
        assert_eq!(chapter_number("/comics/a/chapter/12.5"), 12.5);
        assert_eq!(chapter_number("/comics/a/prologue"), 0.);
    }
}
