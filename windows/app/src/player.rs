//! The player's shell on the desktop: mpv on its own thread plays what the shared core says (`zcore::play`) and tells it
//! where it is; the window draws the core's view. What's left here is the engine, the drawing and the desktop's own
//! extras (the volume, full screen, the jumps' light, the pointer's tips and previews).
use crate::gfx::{Font, TextStyle};
use crate::kit::{col, el, icon, k, row, text, Anim, El, Fill};
use crate::mpv::{self, Mpv};
use crate::tokens as t;
use crate::ui::*;
use crate::{downloads, net, spawn, ui, App};
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use crate::platform::{key, video, Compositor};
use zcore::play::{self as play, Command as Order, Event, Panel};
use zcore::screen::Effect;

enum Command {
    Open(Box<play::Media>, f64, bool, f64, Option<u64>, u64),
    Set(&'static str, String),
    Seek(f64),
    Resize(i32, i32),
}

/// The way to the mpv thread: commands, each with a nudge out of its wait (mpv_wakeup is safe from any thread). Let
/// go, the thread's nudged to find it gone and close at once.
struct Link {
    tx: Option<mpsc::Sender<Command>>,
    wake: Arc<Mutex<Option<Waker>>>,
}
/// The running mpv's handle and its mpv_wakeup (cleared by the thread before the handle goes).
struct Waker(usize, unsafe extern "C" fn(mpv::Handle));
impl Link {
    fn send(&self, c: Command) {
        if self.tx.as_ref().is_some_and(|tx| tx.send(c).is_ok()) {
            self.nudge();
        }
    }
    fn nudge(&self) {
        if let Some(Waker(h, wakeup)) = self.wake.lock().unwrap().as_ref() {
            unsafe { wakeup(*h as mpv::Handle) };
        }
    }
}
impl Drop for Link {
    fn drop(&mut self) {
        self.tx = None;
        self.nudge();
    }
}
/// Clears the waker as the thread ends (dropped before the mpv it points at).
struct Unwake(Arc<Mutex<Option<Waker>>>);
impl Drop for Unwake {
    fn drop(&mut self) {
        *self.0.lock().unwrap() = None;
    }
}

/// A frame in `ms` (a view whose look turns by itself as time passes: the volume's bar closing, a jump's light fading).
fn redraw_in(ms: u64) {
    spawn(async move {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        ui(|_| {});
    });
}

/// The core's run of jumps, as it was seen last.
#[derive(Clone)]
struct Jumped {
    side: i8,
    total: u32,
    count: u32,
    /// The core's words for it ("+30").
    label: String,
    at: Instant,
    /// The core still has the run going.
    live: bool,
}

pub struct Player {
    id: u64,
    /// The engine's load: what mpv says about an older one is dropped.
    load: u64,
    tx: Link,
    /// mpv has what it was given open: the picture can show.
    loaded: bool,
    feedback: Option<(&'static str, Instant)>,
    volume_hint: Option<Instant>,
    /// When the controls last hid (they fade out over 300 ms) and whether they were up when last drawn: seen as it's
    /// drawn, since a draw can come before the next tick, which put them away at once and then faded them out again.
    hidden_at: std::cell::Cell<Option<Instant>>,
    drawn_up: std::cell::Cell<bool>,
    /// Whether they were up at the tick before.
    controls_seen: bool,
    /// The panel that just closed, playing its way out, and the one open at the tick before.
    panel_closed: Option<(Panel, Instant)>,
    panel_seen: Option<Panel>,
    /// Panels opened so far: each closing copy keyed by its opening, so it plays once, from the first frame.
    panel_opens: u32,
    /// The core's run of jumps as it was last seen (kept a while after it ends, to fade out): the side, the seconds
    /// so far, how many, and when it last grew.
    jump: Option<Jumped>,
    /// When the picture was last clicked: a second click soon after is a double click.
    last_click: Option<Instant>,
    click_token: u64,
    /// The volume's bar being dragged (it stays out until let go).
    volume_dragging: bool,
    scrub: Option<f64>,
    volume: i64,
    muted: bool,
    /// The screen is held awake (while it plays: the core's `awake`).
    awake: bool,
    aspect: f64,
    /// When it opened: each play or pause flash is keyed by when it came.
    opened: Instant,
}

/// The profile's settings, as the player follows them.
fn settings() -> play::Settings {
    use crate::profile_settings as s;
    play::Settings {
        profile: s::current_id(),
        quality: s::quality(),
        captions: s::cc_on(),
        caption_size: s::cc_size(),
        autoplay: s::autoplay_next(),
        dramas_audio: s::dramas_audio(),
        anime_audio: s::anime_audio(),
        plus: crate::data::plus(),
    }
}

/// A title page wants it ready before Play: the core finds and mints what Play would play first.
pub fn warm(title: String, episode: Option<String>) {
    ui(move |app| app.play.send(Event::Warm { title, episode, settings: settings() }));
}

/// The core's navigation says a title plays here: the player opens (once for each play it says).
pub fn follow_nav(app: &mut App, playing: Option<&zcore::nav::Playing>) {
    thread_local!(static SEEN: std::cell::RefCell<Option<zcore::nav::Playing>> = const { std::cell::RefCell::new(None) });
    let fresh = SEEN.with(|seen| {
        let mut seen = seen.borrow_mut();
        let fresh = playing.is_some() && app.player.is_none() && *seen != playing.cloned();
        *seen = playing.cloned();
        fresh
    });
    if let (true, Some(p)) = (fresh, playing) {
        open(app, p.show.clone(), p.episode.clone(), p.local);
    }
}

/// Opens the player on a title: the core loads it, picks the episode (none: where the show was left), and tells the
/// engine what to play.
fn open(app: &mut App, title: String, episode: Option<String>, local: bool) {
    static IDS: AtomicU64 = AtomicU64::new(0);
    let id = IDS.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel();
    let wake = Arc::new(Mutex::new(None));
    let tx = Link { tx: Some(tx), wake: wake.clone() };
    let scale = app.kit.g.scale;
    let (w, h) = ((app.kit.width * scale) as i32, (app.kit.height * scale) as i32);
    let volume = net::pref_i("volume", 100).clamp(0, 100);
    let muted = net::pref_b("muted", false);
    std::thread::spawn(move || {
        if let Err(fault) = drive(id, rx, wake, w, h, volume, muted) {
            net::report_error("error", "PlaybackError", &fault, "", "player.play");
            // (the engine never started: the player says so, with Try again, instead of waiting on it for ever)
            ui(move |app| if app.player.as_ref().is_some_and(|p| p.id == id) {
                app.play.send(Event::Failed { failure: play::Failure::Other, message: fault });
            });
        }
    });
    let aspect = f64::from(app.kit.width / app.kit.height.max(1.));
    app.play.send(Event::Open {
        title,
        episode,
        local,
        settings: settings(),
        // (mpv doesn't pick its own level: Auto is the core's, stepping down when it stalls)
        adaptive: false,
        screen: aspect,
    });
    app.player = Some(Player {
        id,
        load: 0,
        tx,
        loaded: false,
        feedback: None,
        volume_hint: None,
        hidden_at: std::cell::Cell::new(None),
        drawn_up: std::cell::Cell::new(true),
        controls_seen: true,
        panel_closed: None,
        panel_seen: None,
        panel_opens: 0,
        jump: None,
        last_click: None,
        click_token: 0,
        volume_dragging: false,
        scrub: None,
        volume,
        muted,
        awake: false,
        aspect,
        opened: Instant::now(),
    });
    app.kit.focus = 0;
    app.kit.dirty = true;
}

/// What only the player's page does for the core: the engine, the copy on this PC, the profile's settings, closing.
pub fn command(app: &mut App, effect: Effect) {
    let Effect::Player { order } = effect else { return };
    match order {
        Order::FindLocal { title, episode, offline } => {
            let copy = downloads::playback(&title, episode.as_deref()).map(|p| {
                let key = episode.clone().unwrap_or_else(|| title.clone());
                play::LocalCopy {
                    url: p.uri,
                    sound: p.sound,
                    captions: p.captions.and_then(|file| std::fs::read_to_string(file).ok()),
                    spot: downloads::spot(&key),
                    ..play::LocalCopy::default()
                }
            });
            // (offline: the title is what the downloads there are say it is, `zcore::queue::offline_title`)
            app.play.send(Event::Local { title, episode, copy, offline: offline.then(downloads::items) });
        }
        Order::Load { mut media, start, paused, speed, level, .. } => {
            let Some(p) = app.player.as_mut() else { return };
            // (a remote file through the relay: its start and end fetched at once, mpv's opening trips answered locally)
            if media.relay {
                if let Some(local) = crate::relay::local(&media.url, &media.agent, start) {
                    media.url = local;
                }
            }
            p.load += 1;
            p.loaded = false;
            p.tx.send(Command::Open(Box::new(media), start, paused, speed, level.map(|l| l.bandwidth), p.load));
        }
        Order::Pause { paused } => set(app, "pause", yes(paused)),
        Order::Seek { to } => {
            if let Some(p) = app.player.as_ref() {
                p.tx.send(Command::Seek(to));
            }
        }
        Order::Speed { speed } => set(app, "speed", speed.to_string()),
        Order::Level { level, .. } => set(app, "hls-bitrate", level.map_or_else(|| "max".into(), |l| l.bandwidth.to_string())),
        Order::Track { id } => set(app, "aid", id),
        Order::Fit { fill } => {
            set(app, "panscan", if fill { "1" } else { "0" });
        }
        // (a remote file read ahead by the relay: its start and end, so Play opens it at once)
        Order::Preload { media, start, .. } => {
            if media.relay {
                let _ = crate::relay::local(&media.url, &media.agent, start);
            }
        }
        Order::Drop { key } => crate::relay::drop_url(&key),
        // (mpv doesn't adapt by itself: the core moves Auto between levels, so it's never told to)
        Order::Adapt => {}
        Order::Setting { group, key, value } => {
            crate::profile_settings::put(&group, &key, value);
        }
        Order::Spot { key, at } => downloads::remember(&key, at),
        // (the core tells the screens showing spots itself)
        Order::Watched => {}
        Order::Close => app.close_player(),
    }
}

/// The last save as the player closes, sent before the app's gone: done when the core says it's in.
static LEAVING: Mutex<Option<tokio::sync::oneshot::Sender<()>>> = Mutex::new(None);

/// The core's last save is in (from the core's thread: the window may already be gone).
pub fn watched() {
    if let Some(done) = LEAVING.lock().unwrap().take() {
        let _ = done.send(());
    }
}

/// The app closing with the player open: it closes, and what's returned ends once its last save is in.
pub fn leaving(app: &mut App) -> Option<tokio::task::JoinHandle<()>> {
    app.player.as_ref()?;
    let (done, saved) = tokio::sync::oneshot::channel();
    *LEAVING.lock().unwrap() = Some(done);
    app.close_player();
    Some(spawn(async move {
        let _ = saved.await;
    }))
}

impl Player {
    pub fn resize(&self, w: i32, h: i32) {
        self.tx.send(Command::Resize(w, h));
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // (a player going away on the UI's thread lets the display sleep again)
        if self.awake {
            video::keep_awake(false);
        }
    }
}

fn set(app: &App, key: &'static str, value: impl Into<String>) {
    if let Some(p) = app.player.as_ref() {
        p.tx.send(Command::Set(key, value.into()));
    }
}

#[allow(clippy::too_many_arguments)]
fn drive(
    id: u64,
    rx: mpsc::Receiver<Command>,
    wake: Arc<Mutex<Option<Waker>>>,
    w: i32,
    h: i32,
    volume: i64,
    muted: bool,
) -> Result<(), String> {
    let m = Mpv::new().map_err(|e| play::engine_missing(&e.to_string()))?;
    for &(key, value) in video::OPTIONS {
        m.opt(key, value);
    }
    for (key, value) in [
        ("hwdec", "auto-safe"),
        ("idle", "yes"),
        ("config", "no"),
        ("load-scripts", "no"),
        ("ytdl", "no"),
        ("osc", "no"),
        ("osd-level", "0"),
        ("terminal", "no"),
        ("input-default-bindings", "no"),
        ("input-vo-keyboard", "no"),
        ("input-cursor", "no"),
        ("sid", "no"),
        ("sub-auto", "no"),
        ("alang", "eng,en"),
        ("audio-client-name", "zunflix"),
        ("volume-max", "100"),
        ("cache", "yes"),
        ("cache-secs", "120"),
        ("demuxer-max-bytes", "400MiB"),
        ("demuxer-max-back-bytes", "100MiB"),
        ("network-timeout", "20"),
        ("hr-seek", "yes"),
    ] {
        m.opt(key, value);
    }
    m.opt("volume", &volume.to_string());
    m.opt("mute", yes(muted));
    let result = unsafe { (m.api.mpv_initialize)(m.h) };
    if result < 0 {
        return Err(m.err(result));
    }
    // Declared after `m`, so it drops first: render.h wants the picture's producer gone before mpv is.
    let mut output = video::Output::new(&m, w, h).map_err(|e| e.to_string())?;
    *wake.lock().unwrap() = Some(Waker(m.h as usize, m.api.mpv_wakeup));
    let _unwake = Unwake(wake);
    let mut last = Instant::now();
    let mut last_surface = Instant::now();
    let mut generation = 0;
    let mut loaded = false;
    let mut replacing = false;
    let mut aspect = 0.;
    let tick = Duration::from_millis(zcore::play::TICK_MS.into());
    // (asleep in mpv's own wait until something happens: an event, a command's nudge, or the next status due)
    loop {
        let command = match rx.try_recv() {
            Ok(command) => Some(command),
            Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            Err(mpsc::TryRecvError::Empty) => None,
        };
        let busy = command.is_some();
        match command {
            Some(Command::Open(media, from, pause, speed, bandwidth, token)) => {
                generation = token;
                loaded = false;
                replacing = true;
                let url = media.url.clone();
                // (mpv has one list of headers: only this video's, its Referer and, from zunflix, the sign-in)
                let mut headers = Vec::new();
                if let Some(referer) = &media.referer {
                    headers.push(format!("Referer: {}", referer.value));
                }
                if let Some(cookie) = media.cookies.then(|| net::cookie_for(&media.url)).flatten() {
                    headers.push(format!("Cookie: {cookie}"));
                }
                let headers =
                    headers.iter().map(|h| h.replace('\\', "\\\\").replace(',', "\\,")).collect::<Vec<_>>().join(",");
                m.set("http-header-fields", &headers);
                m.set("user-agent", &media.agent);
                m.set("start", if from > 0. { format!("{from:.3}") } else { "none".into() }.as_str());
                m.set("audio-files", media.sound.as_deref().unwrap_or(""));
                m.set("alang", &media.track_names);
                m.set("pause", yes(pause));
                m.set("speed", &speed.to_string());
                m.set("hls-bitrate", &bandwidth.map(|n| n.to_string()).unwrap_or_else(|| "max".into()));
                let result = m.cmd(&["loadfile", &url, "replace"]);
                if result < 0 {
                    let (failure, message) = (play::failure_of_mpv(result), m.err(result));
                    event(id, generation, move |app| app.play.send(Event::Failed { failure, message }));
                }
            }
            Some(Command::Set(key, value)) => {
                m.set(key, &value);
            }
            Some(Command::Seek(to)) => {
                m.cmd(&["seek", &format!("{to:.3}"), "absolute+exact"]);
            }
            Some(Command::Resize(w, h)) => {
                output.resize(&m, w, h);
            }
            None => {}
        }
        let mut report_now = false;
        // (more commands waiting: none of the wait; loaded, until the next status; otherwise until something comes)
        let mut timeout = if busy { 0. } else if loaded { tick.as_secs_f64() - last.elapsed().as_secs_f64() } else { 1. }.max(0.);
        loop {
            let ev = unsafe { &*(m.api.mpv_wait_event)(m.h, timeout) };
            timeout = 0.;
            match ev.id {
                0 => break,
                6 => replacing = false,
                mpv::EV_SHUTDOWN => return Ok(()),
                mpv::EV_FILE_LOADED => {
                    loaded = true;
                    report_now = true;
                    let sounds = tracks(&m);
                    event(id, generation, move |app| {
                        if let Some(p) = app.player.as_mut() {
                            p.loaded = true;
                        }
                        app.play.send(Event::Sounds { sounds });
                    });
                }
                mpv::EV_VIDEO_RECONFIG | mpv::EV_PLAYBACK_RESTART => report_now = loaded,
                mpv::EV_END_FILE if !ev.data.is_null() => {
                    let end = unsafe { &*ev.data.cast::<mpv::EndFile>() };
                    if end.reason == 4 && !replacing {
                        loaded = false;
                        let (failure, message) = (play::failure_of_mpv(end.error), m.err(end.error));
                        event(id, generation, move |app| app.play.send(Event::Failed { failure, message }));
                    } else if end.reason == 0 && !replacing {
                        loaded = false;
                        event(id, generation, |app| app.play.send(Event::Ended));
                    }
                }
                _ => {}
            }
        }
        if loaded && (report_now || last.elapsed() >= tick) {
            last = Instant::now();
            // (the picture's surface looked for as it's made or remade; once a second besides, in case it changed quietly)
            let surface_due = report_now || last_surface.elapsed() >= Duration::from_secs(1);
            if surface_due {
                last_surface = Instant::now();
            }
            if let Some(surface) = output.surface(&m, surface_due) {
                ui(move |app| {
                    if app.player.as_ref().is_some_and(|p| p.id == id) {
                        if let Ok(b) = app.kit.g.os.video_brush(surface) {
                            app.kit.video_brush = Some(b);
                        }
                    }
                });
            }
            let number = |key| m.get(key).and_then(|s| s.parse().ok()).unwrap_or(0.);
            let status = play::Status {
                at: number("time-pos"),
                total: number("duration"),
                buffered: number("demuxer-cache-time"),
                paused: m.get("pause").as_deref() == Some("yes"),
                buffering: m.get("paused-for-cache").as_deref() == Some("yes"),
                throughput: number("demuxer-cache-state/raw-input-rate") * 8.,
            };
            let shape = number("video-params/aspect");
            let changed = (shape - aspect).abs() > 1e-3;
            aspect = shape;
            event(id, generation, move |app| {
                app.play.send(Event::Status { status });
                if changed {
                    app.play.send(Event::Picture { aspect: shape });
                }
                // (what's drawn changes when the core's view comes back)
                crate::quiet();
            });
        }
    }
}

fn tracks(m: &Mpv) -> Vec<play::Sound> {
    let n: usize = m.get("track-list/count").and_then(|n| n.parse().ok()).unwrap_or_default();
    (0..n)
        .filter_map(|i| {
            let get = |k| m.get(&format!("track-list/{i}/{k}"));
            if get("type").as_deref() != Some("audio") {
                return None;
            }
            Some(play::Sound {
                id: get("id")?,
                label: play::sound_label(get("title").as_deref(), get("lang").as_deref(), i + 1),
                selected: get("selected").as_deref() == Some("yes"),
                lang: get("lang"),
            })
        })
        .collect()
}

fn event(id: u64, load: u64, f: impl FnOnce(&mut App) + Send + 'static) {
    ui(move |app| {
        if app.player.as_ref().is_some_and(|p| p.id == id && p.load == load) {
            f(app);
        }
    });
}

fn yes(v: bool) -> &'static str {
    if v {
        "yes"
    } else {
        "no"
    }
}

impl Player {
    fn set_volume(&mut self, v: i64) {
        self.volume_hint = Some(Instant::now());
        redraw_in(900);
        self.volume = v.clamp(0, 100);
        self.tx.send(Command::Set("volume", self.volume.to_string()));
        net::pref_set("volume", json!(self.volume));
        if self.muted && self.volume > 0 {
            self.mute();
        }
    }

    fn mute(&mut self) {
        self.muted = !self.muted;
        self.tx.send(Command::Set("mute", yes(self.muted).into()));
        net::pref_set("muted", json!(self.muted));
    }

    /// The volume's bar out: under the pointer, while dragged, or for a moment as the keys or the wheel change it.
    fn volume_out(&self, app: &App) -> bool {
        app.kit.hover_within(k("p-volume-wrap"))
            || self.volume_dragging
            || self.volume_hint.is_some_and(|at| at.elapsed().as_millis() < t::D_LEVEL.into())
    }
}

/// Play or pause, with its sign flashing in the middle.
fn toggle(app: &mut App) {
    let paused = !app.play.view().paused;
    if let Some(p) = app.player.as_mut() {
        p.feedback = Some((if paused { "pause" } else { "play" }, Instant::now()));
    }
    app.play.send(Event::Toggle);
}

fn send(event: Event) -> impl Fn(&mut App) + 'static {
    move |a: &mut App| a.play.send(event.clone())
}

impl Player {
    /// The choices panels: subtitles (on or off, their size) and settings (quality, sound, picture), each choice a chip.
    fn choices(&self, app: &App, cc: bool) -> El {
        let v = app.play.view();
        let words_ = play::words();
        let (groups, panel) = if cc { (&v.subtitles, Panel::Subtitles) } else { (&v.settings, Panel::Settings) };
        let mut c = col().pad4(t::SPACE_16, t::SPACE_16, t::SPACE_10, t::SPACE_14);
        if groups.is_empty() {
            let none = if cc { words_.no_subtitles } else { words_.nothing_to_change };
            c = c.kid(line(none, t::T_PLAYER_ANSWER, Font::Medium, w(t::W80)));
        }
        for (i, group) in groups.iter().enumerate() {
            c = c.kid(
                text(&group.title, ts(t::T_PLAYER_PANEL_HEAD, Font::Bold, t::C_WHITE, 1))
                    .margin(0., if i > 0 { t::SPACE_14 } else { 0. }, 0., t::SPACE_8),
            );
            let mut flow = row().fill_w().wrap().gap(t::SPACE_6).items_start();
            for (j, choice) in group.choices.iter().enumerate() {
                let pick = Event::Pick { panel, group: i as u32, choice: j as u32 };
                let key = format!("pick-{}-{j}", group.title);
                flow = flow.kid(crate::ds::badged_chip(app, &key, &choice.label, choice.badge.as_deref(), choice.on, move |a| {
                    a.play.send(pick.clone());
                    a.kit.dirty = true;
                }));
            }
            c = c.kid(flow);
        }
        panel_of(col().key(if cc { "panel-cc" } else { "panel-settings" }).w(if cc { t::K_PLAYER_CC_PANEL_WIDTH } else { t::K_PLAYER_PANEL_WIDTH }))
            .kid(c.key("panel-scroll").scroll_y().max_h(t::K_PLAYER_PANEL_HEIGHT))
    }

    pub fn view(&self, app: &App) -> El {
        let v = app.play.view();
        let words_ = play::words();
        let (width, height) = (app.kit.width, app.kit.height);
        let pad = t::SPACE_24;
        let mut root = el().fill().bg(t::C_BLACK);
        let has_picture = self.loaded && app.kit.video_brush.is_some();
        let playing = v.started && has_picture;
        if has_picture {
            root = root.kid(el().key("video").cover().video().click(tapped).sink(1.));
            if let Some(last) = root.kids.last_mut() {
                last.hand = false;
            }
        }
        // a jump (its button or an arrow key): a soft white light from that side, the jump button turning once, the seconds beside it
        if let Some(Jumped { side, total, live: fresh, label, .. }) = self.jump.clone() {
            let left = side < 0;
            let light = el()
                .key(&format!("jump-light-{side}"))
                .at(if left { 0. } else { f32::NAN }, 0., if left { f32::NAN } else { 0. }, 0.)
                .pct_w(0.45)
                .decorative()
                .paint(Fill::Radial(if left { 0. } else { 1. }, 0.5, 1.6, vec![(0., w(0.16 + 0.12)), (0.5, w(0.05)), (1., 0)]))
                .anim(if fresh { Anim::Fade(160) } else { Anim::FadeOut(380) });
            let burst = row()
                .key(&format!("jump-burst-{side}-{total}"))
                .items_center()
                .decorative()
                .kid(jump_icon(side > 0).anim(Anim::Spin(if side > 0 { 1. } else { -1. })))
                .kid(
                    line(label, t::T_PLAYER_BURST, Font::Bold, t::C_WHITE)
                        .key(&format!("jump-n-{total}"))
                        .anim(Anim::Enter(0., 9., 220))
                        .margin(t::SPACE_10, 0., t::SPACE_10, 0.),
                );
            let (on_left, on_right) = if left { (burst, el()) } else { (el(), burst) };
            let lane = row()
                .cover()
                .center()
                .decorative()
                .gap(t::SPACE_40)
                .kid(el().size(t::S_JUMP_BUTTON + t::K_PLAYER_BURST_COUNT, t::S_JUMP_BUTTON).kid(on_left))
                .kid(el().size(t::K_PLAYER_BURST_MIDDLE, t::K_PLAYER_BURST_MIDDLE))
                .kid(el().size(t::S_JUMP_BUTTON + t::K_PLAYER_BURST_COUNT, t::S_JUMP_BUTTON).kid(on_right));
            root = root.kid(
                el().key(&format!("jump-{side}"))
                    .cover()
                    .decorative()
                    .kid(light)
                    .kid(lane.anim(if fresh { Anim::None } else { Anim::FadeOut(380) })),
            );
        }
        if !v.caption.is_empty() {
            root = root.kid(caption_lines(&v.caption, height).key("captions").at(t::K_PLAYER_WORDS_SIDE, f32::NAN, t::K_PLAYER_WORDS_SIDE, if v.controls { t::K_PLAYER_CAPTION_RISE } else { t::K_PLAYER_HIDDEN_FOOT }));
        }
        if let Some(fault) = &v.fault {
            root = root.kid(
                col()
                    .key("fault")
                    .cover()
                    .center()
                    .pad(t::K_PLAYER_WORDS_SIDE)
                    // (Back's own room at the top: the words sit in the middle of what's under it, not of the window)
                    .kid(el().h(t::SPACE_14 + t::K_PLAYER_CONTROL).decorative())
                    .kid(text(fault, centred(ts(t::T_PLAYER_FAULT, Font::Sans, t::C_INK2, 4))).max_w(t::K_PLAYER_FAULT_WIDTH).anim(Anim::Enter(0., 8., 350)))
                    // (Try again, and Report: it tells the owner, its answer then showing under them)
                    .kid(
                        row().gap(t::SPACE_10).margin(0., t::K_PLAYER_STACK_GAP, 0., 0.).anim(Anim::Enter(0., 8., 350))
                            .kid(crate::ds::button(app, "retry", crate::ds::Kind::Primary, crate::ds::Size::M, Some("rotate_cw"), &words_.try_again, send(Event::Retry)))
                            .maybe(v.fault_report.then(|| crate::ds::button(app, "report-fault", crate::ds::Kind::Secondary, crate::ds::Size::M, Some("flag"), &words_.report_fault, send(Event::ReportFault)))),
                    )
                    .maybe(v.report.answer.as_ref().map(|a| line(a, t::T_PLAYER_ANSWER, Font::Medium, w(t::W80)).margin(0., t::SPACE_12, 0., 0.).anim(Anim::Fade(220))))
                    .kid(
                        tap(
                            col().key("fault-back").size(t::K_PLAYER_CONTROL, t::K_PLAYER_CONTROL).radius(t::K_PLAYER_CONTROL / 2.).bg(black(0.45)).center().kid(icon("arrow_left", t::K_CONTROL_BACK_ICON, t::C_WHITE)),
                            t::PRESS_ROUND,
                            |a| a.close_player(),
                        )
                        .at(pad, t::SPACE_14, f32::NAN, f32::NAN),
                    ),
            );
        }

        // The controls, flat over the picture: back, the name and the episode at the top left,
        // subtitles and settings at the top right; play and the 10-second jumps in the middle; the time and a rounded bar
        // at the foot. They fade in (160 ms) and out (300 ms).
        let show_controls = v.controls && playing;
        if self.drawn_up.replace(show_controls) && !show_controls {
            self.hidden_at.set(Some(Instant::now()));
        }
        let hiding = !show_controls && playing && self.hidden_at.get().is_some_and(|at| at.elapsed().as_millis() < 300);
        if show_controls || hiding {
            let layer = self
                .controls_layer(app, width, pad)
                .key(if hiding { "controls-out" } else { "controls" })
                .anim(if hiding { Anim::FadeOut(300) } else { Anim::Fade(160) });
            root = root.kid(if hiding { layer.decorative() } else { layer });
        }

        // Next episode, as Netflix's: bottom right from where the credits start (or the last 20 s), Watch credits
        // beside it; while it counts down (5 s) it fills. Or Skip intro / recap / credits where it sits.
        if let Some(pill) = v.pill.as_ref().filter(|_| playing) {
            let face_of = |count: f64, left: f64| {
                let mut face = row().key(if pill.next { "next-ep" } else { "skip" }).h(t::K_PLAYER_NEXT_PILL).pad_xy(t::SPACE_16, 0.).radius(t::K_PLAYER_NEXT_PILL / 2.).clip().items_center()
                    .bg(if count > 0. { t::C_NEXT_FILL } else { t::C_INK });
                if count > 0. {
                    face = face.kid(el().key("next-fill").cover().decorative().bg(t::C_INK).anim(Anim::FillX(count as f32, (left * 1000.) as i64)));
                }
                face.kid(icon("skip_forward", t::K_PLAYER_NEXT_ICON, t::C_DARK)).kid(line(&pill.label, t::T_PLAYER_PILL, Font::Medium, t::C_DARK).margin(t::K_PLAYER_NEXT_ICON_GAP, 0., 0., 0.))
            };
            let mut pills = row().key(if pill.next { "ending" } else { "skipping" }).at(f32::NAN, f32::NAN, pad, if v.controls { t::K_PLAYER_PILL_RISE } else { t::K_PLAYER_HIDDEN_FOOT })
                .gap(t::SPACE_10).items_center().anim(Anim::Enter(0., 8., 350));
            if pill.next {
                let credits = row().key("watch-credits").h(t::K_PLAYER_NEXT_PILL).pad_xy(t::SPACE_16, 0.).radius(t::K_PLAYER_NEXT_PILL / 2.).items_center().bg(black(0.55)).border(t::S_HAIRLINE, w(t::W25))
                    .kid(line(&words_.watch_credits, t::T_PLAYER_PILL, Font::Medium, t::C_WHITE));
                pills = pills.kid(tap(credits, t::PRESS_DEFAULT, send(Event::WatchCredits)));
            }
            root = root.kid(pills.kid(tap(face_of(pill.count, pill.left), t::PRESS_DEFAULT, send(Event::Pill))));
        }

        // getting it ready: black, as Netflix waits; a spinner; the name and what it's doing, until it really plays
        // (the Mac's video layer is there before its first frame)
        if !playing && v.fault.is_none() {
            let mut wait = el().key("waiting").cover().bg(0xFF000000);
            wait = wait.kid(
                col()
                    .cover()
                    .center()
                    .decorative()
                    .kid(spinner(t::K_PLAYER_SPINNER, t::C_INK))
                    .kid(text(&v.heading, centred(ts(t::T_PLAYER_WAIT_TITLE, Font::Bold, t::C_WHITE, 1))).max_w(width - t::K_PLAYER_WORDS_SIDE * 2.).margin(0., t::K_PLAYER_STACK_GAP, 0., 0.))
                    .kid(line(&v.status, t::T_PLAYER_LABEL, Font::Sans, t::C_INK2).margin(0., t::SPACE_8, 0., 0.)),
            );
            wait = wait.kid(crate::ds::back_button(app, "wait-back", |a| a.close_player()).at(pad, top_line(app), f32::NAN, f32::NAN));
            root = root.kid(wait);
        }

        // a panel eases in, and closing plays the same backwards; a click outside closes it
        // (each panel rises over its own button: its right edge at the button's, counted from the corner; Report drops
        // from the flag at the top right)
        let over = |from_corner: u32| pad + from_corner as f32 * (CONTROL + t::SPACE_4);
        let placed = |panel: Panel| match panel {
            Panel::Speed => self.speed_view(app).at(f32::NAN, f32::NAN, pad, PANEL_FOOT),
            Panel::Settings | Panel::Subtitles => self.choices(app, panel == Panel::Subtitles).at(f32::NAN, f32::NAN, over(1), PANEL_FOOT),
            Panel::Episodes => self.episodes_view(app).at(f32::NAN, f32::NAN, over(4), PANEL_FOOT),
            Panel::Report => self.report_view(app).at(f32::NAN, top_line(app) + crate::ds::BACK + t::SPACE_12, pad, f32::NAN),
        };
        if let Some(panel) = v.panel {
            let mut dismiss = el().key("panel-dismiss").cover().click(send(Event::Panel { panel: None }));
            dismiss.hand = false;
            root = root.kid(dismiss).kid(placed(panel).anim(Anim::Drop));
        } else {
            // (the very panel that was open, where it was: from the first frame after it closed, before the tick
            // has seen it close too)
            let closing = match self.panel_closed {
                Some((panel, at)) => (at.elapsed().as_millis() < PANEL_OUT_MS.into()).then_some(panel),
                None => self.panel_seen,
            };
            if let Some(panel) = closing {
                root = root.kid(placed(panel).key(&format!("panel-out-{}", self.panel_opens)).anim(Anim::DropOut).decorative());
            }
        }
        // play or pause just pressed: its sign in the middle, fading away (each press its own)
        if let Some((glyph, at)) = self.feedback.filter(|(_, at)| at.elapsed().as_millis() < t::D_FEEDBACK.into()) {
            root = root.kid(col().cover().center().decorative().kid(
                col().key(&format!("flash-{}", at.duration_since(self.opened).as_millis())).size(t::K_PLAYER_FLASH, t::K_PLAYER_FLASH).radius(t::K_PLAYER_FLASH / 2.).bg(black(0.5)).center()
                    .kid(icon(glyph, t::K_PLAYER_FLASH_ICON, t::C_WHITE).margin(if glyph == "play" { t::SPACE_4 } else { 0. }, 0., 0., 0.)).anim(Anim::Flash),
            ));
        }
        if let Some(notice) = &v.notice {
            root = root.kid(row().key("notice").at(pad, f32::NAN, pad, if v.controls { t::K_PLAYER_NOTICE_RISE } else { t::SPACE_40 }).center().kid(
                panel(row().pad_xy(t::K_PLAYER_NOTICE_PAD_X, t::SPACE_10).items_center().gap(t::SPACE_10), t::R_TOAST)
                    .kid(line(notice, t::T_PLAYER_TOAST, Font::Medium, t::C_WHITE))
                    .kid(crate::ds::button(app, "notice-close", crate::ds::Kind::Secondary, crate::ds::Size::S, None, &words_.dismiss, send(Event::Dismiss))),
            ));
        }
        root
    }

    /// Reporting a problem: the player's own panel (as Subtitles and Settings), dropping under the flag: what's wrong
    /// as chips, a few words if you like, Send; then what the server said.
    fn report_view(&self, app: &App) -> El {
        let v = app.play.view();
        let words_ = play::words();
        let inner = t::K_PLAYER_REPORT_INNER;
        let mut c = col().pad4(t::SPACE_16, t::SPACE_16, t::SPACE_16, t::SPACE_16)
            .kid(text(&words_.report, ts(t::T_PLAYER_PANEL_HEAD, Font::Bold, t::C_WHITE, 1)).margin(0., 0., 0., t::SPACE_8));
        if let Some(answer) = &v.report.answer {
            c = c.kid(line(answer, t::T_PLAYER_ANSWER, Font::Medium, w(t::W80)).margin(0., t::SPACE_6, 0., t::SPACE_6));
            return panel_of(col().key("panel-report").w(inner + t::SPACE_16 * 2.)).kid(c);
        }
        let mut flow = row().fill_w().wrap().gap(t::SPACE_6).items_start();
        for (i, reason) in v.report.reasons.iter().enumerate() {
            flow = flow.kid(crate::ds::chip(app, &format!("report-{i}"), &reason.label, reason.on, move |a| {
                a.play.send(Event::Reason { index: i as u32 });
                a.kit.dirty = true;
            }));
        }
        let send_note: crate::kit::Cb = std::rc::Rc::new(|a: &mut App| { let note = a.kit.text_of("report-note"); a.play.send(Event::Send { note }) });
        let again = send_note.clone();
        c = c.kid(flow)
            .maybe(v.report.details.then(|| crate::ui::glass_field(app, Field { key: "report-note", hint: &words_.note_hint, submit: Some(send_note), ..Default::default() }, inner, None).margin(0., t::SPACE_14, 0., 0.)))
            .kid(crate::ds::button(app, "report-send", crate::ds::Kind::Primary, crate::ds::Size::M, None, &words_.send, move |a| again(a))
                .fill_w().margin(0., t::SPACE_12, 0., 0.).opacity(if v.report.ready { 1. } else { 0.4 }).ease(160));
        panel_of(col().key("panel-report").w(inner + t::SPACE_16 * 2.)).kid(c)
    }

    /// This season's episodes, in the player's own panel: the one playing open (its picture, Now playing, what
    /// happens in it), the others a line each; a click plays one.
    fn episodes_view(&self, app: &App) -> El {
        let v = app.play.view();
        let words_ = play::words();
        let w_ = t::K_PLAYER_EPISODES_WIDTH;
        let mut rows = col().fill_w().gap(t::SPACE_2);
        for ep in &v.episodes {
            let key = format!("ep-pick-{}", ep.id);
            let hot = app.kit.hover_within(k(&key));
            let head = row().w(w_ - t::SPACE_14 * 2. - t::SPACE_12 * 2.).items_center().gap(t::SPACE_12)
                .kid(line(&ep.number, t::T_PLAYER_EPISODE, Font::Bold, if ep.playing { t::C_WHITE } else { w(t::W60) }).w(t::K_PLAYER_EPISODE_NUMBER).no_shrink())
                .kid(text(&ep.name, ts(t::T_PLAYER_EPISODE, Font::Bold, if ep.playing { t::C_WHITE } else { w(t::W85) }, 1)).w(w_ - t::SPACE_14 * 2. - t::SPACE_12 * 2. - t::K_PLAYER_EPISODE_NUMBER - t::SPACE_12));
            let mut item = col().key(&key).fill_w().pad4(t::SPACE_12, t::SPACE_10, t::SPACE_12, t::SPACE_10).radius(t::R_ROW)
                .bg(if ep.playing { w(0.08) } else if hot { w(0.05) } else { 0 }).kid(head);
            if ep.playing {
                let still = ep.picture.as_deref().map(net::img).unwrap_or_default();
                // (the picture across the item, Now playing on it; what happens under it, wrapped to the panel)
                let inner = w_ - t::SPACE_14 * 2. - t::SPACE_12 * 2.;
                let shot = picture(el().size(inner, (inner * t::K_CARD_WIDE_RATIO).round()).no_shrink(), &still, t::K_PLAYER_EPISODE_STILL_RADIUS)
                    .kid(col().cover().center().decorative().bg(black(0.4)).radius(t::K_PLAYER_EPISODE_STILL_RADIUS)
                        .kid(row().items_center().gap(t::SPACE_8).kid(icon("play", t::K_PLAYER_NOW_ICON, t::C_WHITE)).kid(line(&words_.now_playing, t::T_PLAYER_NOW_PLAYING, Font::Bold, t::C_WHITE))));
                let mut detail = col().w(inner).margin(0., t::SPACE_12, 0., t::SPACE_2).kid(shot);
                if let Some(about) = ep.about.as_deref() {
                    detail = detail.kid(text(about, ts(t::T_PLAYER_PANEL_NOTE, Font::Medium, w(t::W80), 3)).w(inner).margin(0., t::SPACE_10, 0., 0.));
                }
                item = item.kid(detail);
            }
            rows = rows.kid(if ep.playing { item } else { tap(item, t::PRESS_ROW, send(Event::Episode { id: ep.id.clone() })) });
        }
        let body = col().pad4(t::SPACE_14, t::SPACE_16, t::SPACE_14, t::SPACE_14)
            .kid(row().fill_w().items_center().margin(t::SPACE_4, 0., t::SPACE_4, t::SPACE_10)
                .kid(line(&v.name, t::T_PLAYER_PANEL_TITLE, Font::Bold, t::C_WHITE))
                .kid(el().grow())
                .maybe(v.season.as_ref().map(|l| line(l, t::T_PLAYER_PANEL_NOTE, Font::Medium, w(t::W60)))))
            .kid(col().key("episodes-scroll").fill_w().scroll_y().max_h(t::K_PLAYER_PANEL_HEIGHT).kid(rows));
        panel_of(col().key("panel-episodes").w(w_)).kid(body)
    }

    /// Speed, as Netflix's: a line with a stop for each, the chosen one ringed, its name under it.
    fn speed_view(&self, app: &App) -> El {
        let v = app.play.view();
        let words_ = play::words();
        let span = t::K_PLAYER_SPEED_SPAN;
        let step = span / v.speeds.len().max(1) as f32;
        let mut stops = el().w(span).h(t::K_PLAYER_SPEED_TRACK).kid(el().at(step / 2., (t::K_PLAYER_SPEED_STOP - t::K_PLAYER_SPEED_LINE) / 2., step / 2., f32::NAN).h(t::K_PLAYER_SPEED_LINE).radius(t::K_PLAYER_SPEED_LINE / 2.).bg(w(t::W25)).decorative());
        for (i, speed) in v.speeds.iter().enumerate() {
            let key = format!("speed-{i}");
            let hot = app.kit.hover_within(k(&key));
            let dot: El = if speed.on {
                col().size(t::K_PLAYER_SPEED_RING, t::K_PLAYER_SPEED_RING).radius(t::K_PLAYER_SPEED_RING / 2.).border(t::K_PLAYER_SPEED_RING_STROKE, t::C_WHITE).center()
                    .kid(el().size(t::K_PLAYER_SPEED_DOT, t::K_PLAYER_SPEED_DOT).radius(t::K_PLAYER_SPEED_DOT / 2.).bg(t::C_WHITE))
            } else {
                let d = if hot { t::K_PLAYER_SPEED_STOP_FOCUSED } else { t::K_PLAYER_SPEED_STOP_DOT };
                el().size(d, d).radius(t::K_PLAYER_SPEED_STOP_FOCUSED / 2.).bg(if hot { t::C_WHITE } else { w(t::W80) })
            };
            stops = stops.kid(tap(
                col().key(&key).at(step * i as f32, 0., f32::NAN, f32::NAN).w(step).h(t::K_PLAYER_SPEED_TRACK).items_center()
                    .kid(col().h(t::K_PLAYER_SPEED_STOP).center().kid(dot))
                    .kid(text(&speed.label, centred(ts(t::T_PLAYER_LABEL, if speed.on { Font::Bold } else { Font::Medium }, if speed.on { t::C_WHITE } else { w(t::W80) }, 1))).w(step)),
                t::PRESS_CHIP,
                move |a| {
                    a.play.send(Event::Speed { index: i as u32 });
                    a.kit.dirty = true;
                },
            ));
        }
        panel_of(col().key("panel-speed").pad4(t::SPACE_20, t::K_PLAYER_SPEED_PAD_Y, t::SPACE_20, t::K_PLAYER_SPEED_PAD_Y)).kid(line(&words_.speed, t::T_PLAYER_PANEL_TITLE, Font::Bold, t::C_WHITE).margin(0., 0., 0., t::SPACE_10)).kid(stops)
    }

    fn controls_layer(&self, app: &App, width: f32, pad: f32) -> El {
        let v = app.play.view();
        let words_ = play::words();
        let mut layer = el()
            .cover()
            .kid(el().at(0., 0., 0., f32::NAN).h(t::K_PLAYER_SCRIM_TOP).decorative().paint(Fill::Linear(0., 0., 0., 1., vec![(0., black(t::SCRIM_PLAYER_TOP)), (1., 0)])))
            .kid(el().at(0., f32::NAN, 0., 0.).h(t::K_PLAYER_SCRIM_FOOT).decorative().paint(Fill::Linear(0., 0., 0., 1., vec![(0., 0), (1., black(t::SCRIM_PLAYER_BOTTOM))])));
        // (under the pointer, a control says what it does in a small label over it; the one in the corner keeps its
        // label inside the window)
        let fullscreen = app.fullscreen.is_some();
        let tip_of = |key: &str| -> String {
            match key {
                "p-play" => if v.paused { words_.play.clone() } else { words_.pause.clone() },
                "p-back10" => words_.back_10.clone(),
                "p-fwd10" => words_.forward_10.clone(),
                "p-cc" => words_.subtitles.clone(),
                "p-next" => words_.next_episode.clone(),
                "p-report" => words_.report.clone(),
                "p-speed" => words_.speed.clone(),
                "p-settings" => words_.settings.clone(),
                "p-full" => if fullscreen { words_.leave_full_screen.clone() } else { words_.full_screen.clone() },
                // (none on the volume: its own bar rises there)
                _ => String::new(),
            }
        };
        let tipped = |key: &str, control: El| -> El {
            let label = tip_of(key);
            let hot = app.kit.hover_within(k(key)) && !label.is_empty();
            let corner = key == "p-full" || key == "p-report";
            // (the flag's at the top: its label under it)
            let below = key == "p-report";
            let size = if below { t::K_PLAYER_REPORT_TIP_TOP } else { CONTROL };
            let x = if corner { size - t::K_PLAYER_TIP_WIDTH } else { (size - t::K_PLAYER_TIP_WIDTH) / 2. };
            let tip = row()
                .w(t::K_PLAYER_TIP_WIDTH)
                .at(x, if below { size + t::SPACE_8 } else { f32::NAN }, f32::NAN, if below { f32::NAN } else { CONTROL + t::SPACE_8 })
                .decorative()
                .kid(el().grow())
                .kid(row().pad_xy(t::SPACE_10, t::K_PLAYER_TIP_PAD_Y).radius(t::R_SMALL).bg(t::C_TOOLTIP).kid(line(label, t::T_PLAYER_TIP, Font::Medium, t::C_WHITE)).anim(Anim::Fade(140)))
                .maybe((!corner).then(|| el().grow()));
            el().kid(control).maybe(hot.then_some(tip))
        };
        let button = |key: &str, glyph: &'static str, lit: bool, f: Box<dyn Fn(&mut App)>| {
            let hot = app.kit.hover_within(k(key));
            let face = col().key(key).size(CONTROL, CONTROL).center().radius(CONTROL / 2.).bg(if lit { t::C_WHITE } else if hot { w(0.12) } else { 0 })
                .kid(icon(glyph, CONTROL_ICON, if lit { t::C_BLACK } else { t::C_WHITE }));
            tipped(key, tap(face, t::PRESS_ROUND, move |a| f(a)))
        };
        let panel_button = |key: &str, glyph: &'static str, which: Panel| {
            let open = v.panel == Some(which);
            button(key, glyph, open, Box::new(move |a: &mut App| a.play.send(Event::Panel { panel: (!open).then_some(which) })))
        };
        let volume = if self.muted { 0 } else { self.volume };
        // (as Netflix's: the top Back, and Report at its right, on one line under the window's strip (at the very top in
        // full screen, where there's no strip); the rest along the foot)
        let top_y = top_line(app);
        let reporting = v.panel == Some(Panel::Report);
        layer = layer.kid(row().at(pad, top_y, pad, f32::NAN).items_center()
            .kid(crate::ds::back_button(app, "p-back", |a| a.close_player()))
            .kid(el().grow())
            // (the flag drawn as Back is: the same circle, grey, its sign the same size; white while its panel's open)
            .kid({
                let hot = app.kit.hover_within(k("p-report"));
                let side = crate::ds::BACK;
                let face = col().key("p-report").size(side, side).no_shrink().radius(side / 2.).center()
                    .bg(if reporting { t::C_WHITE } else { w(if hot { 0.2 } else { 0.11 }) }).ease(160)
                    .kid(icon("flag", t::K_PLAYER_REPORT_ICON, if reporting { t::C_BLACK } else { t::C_WHITE }));
                tipped("p-report", tap(face, t::PRESS_ROUND, move |a| {
                    a.kit.set_text("report-note", "");
                    a.play.send(Event::Panel { panel: (!reporting).then_some(Panel::Report) });
                }))
            }));
        // the volume: its button, and over it a bar that rises under the pointer (dragged up and down, it sets the
        // level), out for a moment too as the keys or the wheel change it
        let sliding = self.volume_out(app);
        let level = volume as f32 / 100.;
        let track = t::K_PLAYER_VOLUME_TRACK;
        let knob_d = t::K_PLAYER_VOLUME_KNOB;
        let slider = el().key("p-volume-bar").w(t::K_PLAYER_VOLUME_WIDTH).h(track)
            .drag_up(|a, fraction, done| {
                let Some(p) = a.player.as_mut() else { return };
                if p.muted { p.mute(); }
                p.set_volume((fraction * 100.).round() as i64);
                p.volume_hint = None;
                p.volume_dragging = !done;
                a.kit.dirty = true;
            })
            .kid(el().at((t::K_PLAYER_VOLUME_WIDTH - t::K_PLAYER_LEVEL_TRACK) / 2., 0., f32::NAN, 0.).w(t::K_PLAYER_LEVEL_TRACK).radius(t::K_PLAYER_LEVEL_TRACK / 2.).clip().bg(w(t::W25)).decorative()
                .kid(el().at(0., f32::NAN, 0., 0.).pct_h(level).bg(t::C_WHITE)))
            .kid(el().at((t::K_PLAYER_VOLUME_WIDTH - knob_d) / 2., (track * (1. - level) - knob_d / 2.).clamp(0., track - knob_d), f32::NAN, f32::NAN).size(knob_d, knob_d).radius(knob_d / 2.).bg(t::C_WHITE).decorative());
        // (solid: nothing behind shows through it)
        let pop_w = t::K_PLAYER_VOLUME_WIDTH + t::SPACE_8 * 2.;
        let rising = col().key("p-volume-pop").w(pop_w).pad4(t::SPACE_8, t::SPACE_12, t::SPACE_8, t::SPACE_12).items_center().radius(pop_w / 2.).bg(0xF2161618).kid(slider)
            .at((CONTROL - pop_w) / 2., f32::NAN, f32::NAN, CONTROL + t::SPACE_4).opacity(if sliding { 1. } else { 0. }).ease(160);
        let volume_row = el().key("p-volume-wrap").hover()
            .kid(button("p-volume", if volume == 0 { "volume_x" } else if volume < 50 { "volume_1" } else { "volume_2" }, false,
                Box::new(|a| if let Some(p) = a.player.as_mut() { p.mute() })))
            .maybe(sliding.then_some(rising));
        let jumping = |side: i8| v.burst.is_some_and(|b| b.ahead == (side > 0));
        let play_face = {
            let hot = app.kit.hover_within(k("p-play"));
            col().key("p-play").size(CONTROL, CONTROL).center().radius(CONTROL / 2.).bg(if hot { w(0.12) } else { 0 })
                .kid(icon(if v.paused { "play" } else { "pause" }, CONTROL_ICON, t::C_WHITE).margin(if v.paused { t::SPACE_2 } else { 0. }, 0., 0., 0.))
        };
        let left = row().items_center().gap(t::SPACE_4)
            .kid(tipped("p-play", tap(play_face, t::PRESS_ROUND, toggle)))
            .kid(tipped("p-back10", tap(jump_icon(false).key("p-back10").opacity(if jumping(-1) { 0. } else { 1. }), t::PRESS_JUMP, |a| jump(a, -1))))
            .kid(tipped("p-fwd10", tap(jump_icon(true).key("p-fwd10").opacity(if jumping(1) { 0. } else { 1. }), t::PRESS_JUMP, |a| jump(a, 1))))
            .kid(volume_row);
        // (the name in the middle of the row, as Netflix has it: the show in bold, then where in it)
        let name = row().items_center().gap(t::SPACE_10)
            .kid(line(&v.name, t::T_PLAYER_TV_NAME, Font::Bold, t::C_WHITE))
            .maybe((!v.place.is_empty()).then(|| line(&v.place, t::T_PLAYER_TV_WHERE, Font::Medium, w(t::W85))));
        let mut tools = row().items_center();
        if v.has_captions {
            // (its own panel, English or Off, as the phone and the TV have it)
            tools = tools.kid(panel_button("p-cc", "captions", Panel::Subtitles).margin(t::SPACE_4, 0., 0., 0.));
        }
        // (a show: the next episode)
        if v.has_next {
            tools = tools.kid(button("p-next", "skip_forward", false, Box::new(|a| a.play.send(Event::Next))).margin(t::SPACE_4, 0., 0., 0.));
        }
        // (speed its own, as Netflix's: a gauge with its own panel)
        tools = tools.kid(panel_button("p-speed", "gauge", Panel::Speed).margin(t::SPACE_4, 0., 0., 0.));
        tools = tools.kid(panel_button("p-settings", "settings", Panel::Settings).margin(t::SPACE_4, 0., 0., 0.));
        tools = tools.kid(button("p-full", if fullscreen { "minimize" } else { "maximize" }, false, Box::new(|a| a.toggle_fullscreen())).margin(t::SPACE_4, 0., 0., 0.));
        let tools = el().fill_w().h(CONTROL)
            .kid(row().at(0., 0., f32::NAN, f32::NAN).kid(left))
            .kid(row().at(0., 0., 0., 0.).center().decorative().kid(name))
            .kid(row().at(f32::NAN, 0., 0., f32::NAN).kid(tools));
        // (waiting: a spinner in the middle, nothing else there)
        if v.buffering {
            layer = layer.kid(col().cover().center().decorative().kid(spinner(t::K_PLAYER_BUFFERING_SPINNER, t::C_INK)));
        }

        // the bar: rounded, the loaded part lighter; thicker under the pointer while dragged; what's left beside it
        let total = v.total;
        let at = self.scrub.unwrap_or(v.time);
        let share = |n: f64| if total > 0. { (n / total).clamp(0., 1.) as f32 } else { 0. };
        let dragging = self.scrub.is_some();
        let thick = if dragging { t::S_SCRUB_DRAG } else { t::S_SCRUB };
        let knob = if dragging { t::S_KNOB_DRAG } else { t::S_KNOB };
        let left_w = t::K_PLAYER_CLOCK;
        let bar_w = width - pad * 2. - left_w - t::SPACE_14;
        let played = share(at);
        let loaded = share(v.buffered).max(played);
        let knob_x = (bar_w * played - knob / 2.).clamp(0., (bar_w - knob).max(0.));
        let bar = el().key("player-seek").fill_w().h(t::K_PLAYER_SEEK_ROW)
            .drag(move |a, fraction, done| {
                let to = total * fraction as f64;
                if let Some(p) = a.player.as_mut() {
                    p.scrub = (!done).then_some(to);
                }
                a.play.send(if done { Event::Seek { to } } else { Event::Poke });
                a.kit.dirty = true;
            })
            .kid(el().at(0., (t::K_PLAYER_SEEK_ROW - thick) / 2., 0., f32::NAN).h(thick).radius(thick / 2.).clip().bg(w(t::W20)).decorative()
                .kid(el().at(0., 0., f32::NAN, 0.).pct_w(loaded).radius(thick / 2.).bg(w(t::W25)))
                .kid(el().at(0., 0., f32::NAN, 0.).pct_w(played).radius(thick / 2.).bg(crate::theme::accent())))
            .kid(el().at(knob_x, (t::K_PLAYER_SEEK_ROW - knob) / 2., f32::NAN, f32::NAN).size(knob, knob).radius(knob / 2.).bg(crate::theme::accent()).decorative());
        let left_now = if dragging { zcore::player::clock((total - at).max(0.)) } else { v.left.clone() };
        let seek = row().fill_w().items_center()
            .kid(bar.w(bar_w).no_shrink())
            .kid(text(left_now, TextStyle { centre: true, ..ts(t::T_PLAYER_CLOCK, Font::Medium, t::C_WHITE, 1) }).w(left_w).margin(t::SPACE_14, 0., 0., 0.));
        // (only the volume's bar steps the progress bar aside; the panels sit over it)
        let quiet = sliding;
        // (stepped aside, it doesn't answer the pointer either: no time over it while a panel's out)
        let seek = if quiet { seek.decorative() } else { seek };
        layer = layer.kid(col().at(pad, f32::NAN, pad, t::SPACE_12).gap(t::SPACE_4).kid(seek.opacity(if quiet { 0. } else { 1. }).ease(160)).kid(tools));

        // (a laptop: over the bar, a picture of that moment and its time)
        let over_bar = (app.kit.hovered() == k("player-seek") && !quiet) || dragging;
        let hover_at = (over_bar && total > 0.).then(|| total * ((app.kit.pointer.0 - pad) / bar_w).clamp(0., 1.) as f64);
        if let Some(moment) = self.scrub.or(hover_at) {
            // (a picture of that moment when there is one; otherwise just the time, in a small pill its own size)
            let thumb = v.preview.as_ref().and_then(|p| zcore::play::preview::thumb(p, moment));
            let tip_w = if thumb.is_some() { t::K_PLAYER_THUMB_W + t::K_PLAYER_THUMB_PAD * 2. } else { t::K_PLAYER_TIME_TIP };
            let x = (app.kit.pointer.0 - tip_w / 2.).clamp(pad, (width - pad - tip_w).max(pad));
            let mut tip = col().w(tip_w).items_center().pad(t::K_PLAYER_THUMB_PAD);
            if let Some(thumb) = thumb {
                tip = tip.kid(el().size(t::K_PLAYER_THUMB_W, t::K_PLAYER_THUMB_H).radius(t::R_STILL - t::K_PLAYER_THUMB_PAD).clip().image(thumb.url).crop(thumb.x, thumb.y, thumb.w, thumb.h).contain());
            }
            tip = tip.kid(row().center().h(t::K_PLAYER_TIME_ROW).kid(line(zcore::player::clock(moment), t::T_PLAYER_CLOCK, Font::Medium, t::C_WHITE)));
            layer = layer.kid(panel(tip, t::R_STILL).decorative().at(x, f32::NAN, f32::NAN, t::K_PLAYER_THUMB_RISE));
        }
        layer
    }
}

/// A panel of the player's glass over the picture.
fn panel_of(base: El) -> El {
    panel(base, t::R_PLAYER_PANEL)
}

/// The controls along the foot: every one the same size, every sign the same size.
const CONTROL: f32 = t::K_PLAYER_TV_CONTROL;
/// How far up the panels sit: over the bar and the time.
const PANEL_FOOT: f32 = t::K_PLAYER_PANEL_FOOT;
/// How long a closing panel is kept: its 240 ms played out, and a frame or two after.
const PANEL_OUT_MS: u16 = 280;

/// Where the player's top line sits: under the window's strip, or at the very top in full screen (no strip there).
fn top_line(app: &App) -> f32 {
    if app.fullscreen.is_some() {
        t::SPACE_12
    } else {
        crate::WINDOW_STRIP + t::SPACE_6
    }
}
const CONTROL_ICON: f32 = t::K_PLAYER_TV_ICON;

/// A 10-second jump: the round arrow with 10 in it, the same size as the others.
fn jump_icon(ahead: bool) -> El {
    col().size(CONTROL, CONTROL).center()
        .kid(icon(if ahead { "rotate_cw" } else { "rotate_ccw" }, CONTROL_ICON, t::C_WHITE))
        .kid(text("10", centred(ts(t::T_PLAYER_JUMP, Font::Bold, t::C_WHITE, 1))).w(CONTROL).at(0., CONTROL / 2. - t::K_PLAYER_JUMP_DIGIT_RISE, f32::NAN, f32::NAN))
}

/// A jump, back or ahead: the core moves, and counts the run of them for the burst ([`tick`] takes it up).
fn jump(app: &mut App, side: i8) {
    app.play.send(Event::Jump { ahead: side > 0, held: false });
    app.kit.dirty = true;
}

/// A click on the picture plays or pauses; a double click goes in or out of full screen, as desktop players do.
fn tapped(app: &mut App) {
    let Some(p) = app.player.as_mut() else { return };
    if p.last_click.is_some_and(|at| at.elapsed().as_millis() < t::D_DOUBLE_TAP.into()) {
        p.click_token += 1;
        p.last_click = None;
        app.toggle_fullscreen();
        return;
    }
    p.last_click = Some(Instant::now());
    // (wait a moment for a second click before playing or pausing, so a double click doesn't pause too)
    p.click_token += 1;
    let (token, id) = (p.click_token, p.id);
    spawn(async move {
        tokio::time::sleep(Duration::from_millis(t::D_DOUBLE_TAP.into())).await;
        ui(move |a| {
            if a.player.as_ref().is_some_and(|p| p.id == id && p.click_token == token) {
                toggle(a);
            }
        });
    });
}

/// The pointer moved: the controls come up (told to the core at most every so often: it counts from the last).
pub fn activity(app: &mut App) {
    // (the core keeps the controls up and counts their wait from the last, and ignores a move soon after one)
    if app.player.is_none() {
        return;
    }
    app.play.send(Event::Poke);
    if app.kit.hovered() == crate::kit::k("player-seek") {
        app.kit.dirty = true;
    }
}

/// The window's timer: what this shell animates on its own, and what it noticed of the core's view.
pub fn tick(app: &mut App) {
    let v = app.play.view();
    let (controls, panel, burst, awake) = (v.controls, v.panel, v.burst, v.awake);
    let label = v.burst_label.clone();
    let aspect = f64::from(app.kit.width / app.kit.height.max(1.));
    let Some(p) = app.player.as_mut() else { return };
    let mut dirty = false;
    // (the display may not sleep while it plays, nor the PC: the core says when)
    if p.awake != awake {
        p.awake = awake;
        video::keep_awake(awake);
    }
    if p.controls_seen && !controls {
        if p.hidden_at.get().is_none() {
            p.hidden_at.set(Some(Instant::now()));
        }
        dirty = true;
        if panel.is_none() {
            crate::platform::set_cursor(crate::platform::Cursor::None);
        }
    }
    p.controls_seen = controls;
    match (p.panel_seen, panel) {
        (Some(was), None) => {
            p.panel_closed = Some((was, Instant::now()));
            dirty = true;
        }
        (seen, Some(now)) => {
            p.panel_closed = None;
            if seen != Some(now) {
                p.panel_opens += 1;
            }
        }
        _ => {}
    }
    p.panel_seen = panel;
    if (aspect - p.aspect).abs() > 1e-3 {
        p.aspect = aspect;
        app.play.send(Event::Screen { aspect });
    }
    let Some(p) = app.player.as_mut() else { return };
    let gone = |at: &Option<Instant>, ms: u64| at.is_some_and(|at| at.elapsed() > Duration::from_millis(ms));
    if gone(&p.hidden_at.get(), 350) {
        p.hidden_at.set(None);
        dirty = true;
    }
    if p.panel_closed.is_some_and(|(_, at)| at.elapsed() > Duration::from_millis(PANEL_OUT_MS.into())) {
        p.panel_closed = None;
        dirty = true;
    }
    // (the core's run of jumps, kept here a while after it ends so its light fades out)
    match burst {
        Some(b) => {
            let side = if b.ahead { 1 } else { -1 };
            if !p.jump.as_ref().is_some_and(|j| j.live && (j.side, j.total, j.count) == (side, b.seconds, b.count)) {
                p.jump = Some(Jumped { side, total: b.seconds, count: b.count, label, at: Instant::now(), live: true });
                dirty = true;
            }
        }
        None => {
            if let Some(j) = p.jump.as_mut().filter(|j| j.live) {
                j.live = false;
                dirty = true;
            }
        }
    }
    if p.jump.as_ref().is_some_and(|j| j.at.elapsed() > Duration::from_millis(t::D_BURST_GONE.into())) {
        p.jump = None;
        dirty = true;
    }
    if gone(&p.volume_hint, 1500) {
        p.volume_hint = None;
        dirty = true;
    }
    if p.feedback.is_some_and(|(_, at)| at.elapsed() > Duration::from_millis(t::D_FEEDBACK.into())) {
        p.feedback = None;
        dirty = true;
    }
    if dirty {
        app.kit.dirty = true;
    }
}

pub fn key(app: &mut App, vk: u16) {
    activity(app);
    if app.player.is_none() {
        return;
    }
    match vk {
        key::SPACE | key::K => toggle(app),
        key::LEFT | key::J => return jump(app, -1),
        key::RIGHT | key::L => return jump(app, 1),
        key::UP | key::DOWN => {
            if let Some(p) = app.player.as_mut() {
                p.set_volume(p.volume + if vk == key::UP { 5 } else { -5 });
            }
        }
        key::C => {
            let on = !app.play.view().captions_on;
            app.play.send(Event::Captions { on });
        }
        key::M => {
            if let Some(p) = app.player.as_mut() {
                p.mute();
            }
        }
        key::N => app.play.send(Event::Next),
        key::F | key::F11 => app.toggle_fullscreen(),
        _ => return,
    }
    app.kit.dirty = true;
}

/// The wheel over the picture: the volume, a step a notch.
pub fn wheel(app: &mut App, delta: f32) -> bool {
    if app.play.view().panel.is_some() {
        return false;
    }
    let Some(p) = app.player.as_mut() else { return false };
    let step = (delta / 120. * 5.).round() as i64;
    p.set_volume(p.volume + step);
    app.kit.dirty = true;
    true
}

/// Esc: an open panel closes; true if one was.
pub fn dismiss_panel(app: &mut App) -> bool {
    if app.player.is_none() || app.play.view().panel.is_none() {
        return false;
    }
    app.play.send(Event::Panel { panel: None });
    true
}

/// Captions as this profile has them: their size (and the window's), their ink, and a box as dark as it's set (below
/// `zcore::captions::EDGED_BELOW`, no box: an edge round the letters). Settings' preview draws them with this too, so
/// it's exactly what the player shows.
pub fn caption_lines(words_: &str, height: f32) -> El {
    caption_lines_as(words_, caption_size(&crate::profile_settings::cc_size(), height), &crate::profile_settings::cc_color(), crate::profile_settings::cc_background(), 1.)
}

/// The captions' letter size in a player `height` tall, at a size setting (`zcore::captions::size`).
pub fn caption_size(setting: &str, height: f32) -> f32 {
    zcore::captions::size(t::T_CAPTION.0.into(), setting, height.into()) as f32
}

/// `k`: everything (letters, box padding, corners, the outline) shrunk by this much: Settings' preview, an exact
/// miniature of the player at the window's size.
pub fn caption_lines_as(words_: &str, size: f32, color: &str, background: f32, k: f32) -> El {
    let ink = zcore::captions::ink(color);
    let edged = zcore::captions::edged(background.into());
    let ground = if edged { 0 } else { black(background) };
    // (square boxes, never rounded; a few pixels between lines so their boxes never touch)
    col().items_center().gap((t::SPACE_4 * k).max(t::K_PLAYER_CAPTION_GAP_MIN)).decorative().kids(
        words_
            .lines()
            .map(|l| {
                let size = size * k;
                let mut words = text(l, centred(TextStyle { line: size * t::T_CAPTION.1 / t::T_CAPTION.0, ..st(Font::Medium, size, ink, 2) }));
                if edged {
                    words = words.outline(zcore::captions::edge((size / k).into()) as f32 * k, 0xFF000000);
                }
                row().pad(t::K_PLAYER_CAPTION_PAD * k).bg(ground).kid(words)
            })
            .collect(),
    )
}

