#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod core_screen;
mod data;
mod disk;
mod ds;
mod downloads;
mod gfx;
mod icons;
mod images;
mod kit;
mod manhwa;
mod mpv;
mod net;
mod pages;
mod platform;
mod playback;
mod rank_digits;
mod setup;
mod profile_settings;
mod relay;
mod theme;
mod tokens;
mod ui;
mod player;
mod youtube;

use kit::Kit;
use platform::{install, key, Callbacks, Compositor, Event, Opening, Shell, Wake, Window, WindowCommand};
use std::cell::RefCell;
use std::future::Future;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, OnceLock,
};

/// The bootstrapper's size.
const BOOT_W: f32 = 340.;
const BOOT_H: f32 = 300.;

type Change = Box<dyn FnOnce(&mut App) + Send>;
static UI: OnceLock<(mpsc::Sender<Change>, platform::Waker)> = OnceLock::new();
static UPDATE_POSTED: AtomicBool = AtomicBool::new(false);

/// The app's window, held back while the update check answers, shown maximised now, its splash from the start.
pub(crate) fn show_opened(app: &mut App) {
    if !app.window.show_held() {
        return;
    }
    app.resize();
    app.pages.start();
    app.kit.dirty = true;
}
/// The thin strip along the top: the window's buttons on its right (on a Mac, the OS's own on its left), the rest of it
/// dragging the window; the pages start under it.
pub const WINDOW_STRIP: f32 = 32.;
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
thread_local! { static APP: RefCell<Option<App>> = const { RefCell::new(None) }; }
thread_local! { static QUIET: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

/// Called from a change sent with `ui`: it changed nothing on screen, so no frame is drawn for it (the player's status).
fn quiet() {
    QUIET.with(|q| q.set(true));
}

fn spawn<F>(task: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("network runtime")
        })
        .spawn(task)
}

fn ui(f: impl FnOnce(&mut App) + Send + 'static) {
    if let Some((tx, waker)) = UI.get() {
        if tx.send(Box::new(f)).is_ok() && !UPDATE_POSTED.swap(true, Ordering::AcqRel) && !waker.wake() {
            UPDATE_POSTED.store(false, Ordering::Release);
        }
    }
}

struct App {
    kit: Kit,
    window: Window,
    inbox: mpsc::Receiver<Change>,
    pages: pages::Pages,
    /// The app from its first frame to a profile in use (`zcore::session`): it outlives the pages, which start afresh
    /// for each profile.
    session: core_screen::CoreScreen<zcore::session::Session>,
    /// The app's updater (`zcore::update`), for the whole run.
    updater: core_screen::CoreScreen<zcore::update::Updater>,
    player: Option<player::Player>,
    /// The player's decisions (`zcore::play`): one for the app's whole run, so what a title page got ready is what
    /// Play plays and the last save goes out after the player's gone.
    play: core_screen::CoreScreen<zcore::play::Play>,
    /// Run as its own setup (the app's files packed onto its end): the install screen, not the app.
    setup: Option<setup::Setup>,
    fullscreen: Option<platform::Restore>,
    high_surrogate: Option<u16>,
    exit_save: Option<tokio::task::JoinHandle<()>>,
    frame_pending: bool,
    /// Minimised: nothing drawn (what's changed waits, marked, for the window to come back), no picture's frames played.
    minimised: bool,
}

impl App {
    /// The server no longer knows this PC.
    fn signed_out(&mut self) {
        self.session.send(zcore::session::Event::SignedOut);
    }

    /// Signed out: nothing of the account kept, and the pages start afresh at signing in.
    fn forget_account(&mut self) {
        self.close_player();
        self.pages.sign_out();
    }

    fn close_player(&mut self) {
        // (a show played from its own page: that page comes back showing the episode it was on last)
        if self.player.is_some() {
            self.pages.player_closed(self.play.view().episode_id.as_deref());
        }
        if self.player.take().is_some() {
            self.play.send(zcore::play::Event::Close);
            self.kit.video_brush = None;
        }
        if self.fullscreen.is_some() {
            self.toggle_fullscreen();
        }
    }

    fn toggle_fullscreen(&mut self) {
        // (the OS's own animations held off for the switch: leaving full screen restores the window maximised, and its
        // maximise animation played, the picture shrinking and growing back; back on a moment later)
        self.window.set_transitions(false);
        spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            ui(move |a| a.window.set_transitions(true));
        });
        match self.fullscreen.take() {
            Some(restore) => self.window.leave_full_screen(&restore),
            None => self.fullscreen = self.window.enter_full_screen(),
        }
        // (resizing the window may deliver its resize while this state is borrowed: read again here)
        self.resize();
    }

    fn resize(&mut self) {
        let (w, h) = self.window.client_size();
        let scale = self.window.scale();
        if scale <= 0. || w <= 0 || h <= 0 {
            return;
        }
        self.kit.scale_to_dpi(scale);
        pages::set_scale(scale);
        self.kit.width = w as f32 / scale;
        self.kit.height = h as f32 / scale;
        self.kit.dirty = true;
        if let Some(p) = &self.player {
            p.resize(w, h);
        }
    }

    /// The window's buttons, as Windows 11 has them (Spotify's, Discord's): minimise, maximise or restore, close, flat,
    /// 46 by 32 in the top right, on the thin strip the pages start under (nothing of theirs covered); a soft light
    /// under the pointer, close turning Windows' red.
    fn window_buttons(&self) -> kit::El {
        use kit::{el, k, row};
        let zoomed = self.window.is_maximised();
        let button = |key: &str, glyph: &'static str, close: bool, c: WindowCommand| {
            let hot = self.kit.hover_within(k(key));
            let mut b = el().key(key).size(crate::tokens::K_WINDOW_CAPTION_BUTTON, WINDOW_STRIP)
                .bg(if hot { if close { 0xFFC42B1C } else { ui::w(0.1) } } else { 0 });
            // (its sign drawn in its own picture, centred in it exactly)
            b.look.icon = Some((glyph, crate::tokens::K_WINDOW_CAPTION_ICON, if hot { crate::tokens::C_WHITE } else { ui::w(0.85) }));
            // (each asked of the OS, handled after this click: resizing the window inside the click left the app laying
            // out for the old size)
            ui::tap(b, 1., move |a| a.window.command(c))
        };
        row().key("window-buttons").at(f32::NAN, 0., 0., f32::NAN).hover()
            .kid(button("window-minimise", "win_minus", false, WindowCommand::Minimise))
            .kid(button("window-maximise", if zoomed { "win_copy" } else { "win_square" }, false, if zoomed { WindowCommand::Restore } else { WindowCommand::Maximise }))
            .kid(button("window-close", "win_x", true, WindowCommand::Close))
    }

    fn redraw(&mut self) {
        if !self.kit.dirty {
            // (only pictures come in: put on where they're waiting, nothing built again)
            if self.kit.has_uploads() {
                if let Err(e) = self.kit.apply_uploads() {
                    eprintln!("Drawing zunflix: {e}");
                    self.kit.dirty = true;
                }
            }
            if !self.kit.dirty {
                return;
            }
        }
        // (laid out every frame: each frame in time with the screen's refresh, never the timer's uneven beat)
        let every_frame = self.setup.is_none() && self.player.is_none() && self.pages.animating();
        if every_frame {
            self.window.wait_for_refresh();
        }
        let tree = match (&self.setup, &self.player) {
            (Some(setup), _) => setup.view(self),
            (None, Some(player)) => player.view(self),
            (None, None) => self.pages.view(self),
        };
        // (not over the splash: nothing but the name while it plays; none where the OS shows its own)
        let tree = if Window::OWN_BUTTONS && self.setup.is_none() && self.fullscreen.is_none() && !self.pages.checking {
            kit::col().fill().kid(tree).kid(self.window_buttons())
        } else {
            tree
        };
        let committed = self.kit.commit(tree);
        if committed.is_ok() && self.player.is_none() && self.setup.is_none() {
            self.pages.reveal_laid_out(&mut self.kit);
        }
        if let Err(e) = committed {
            if e.device_lost() && self.kit.g.os.replace_device().is_ok() {
                self.kit.clear_visuals();
            } else {
                eprintln!("Drawing zunflix: {e}");
                // (kept, to be read when something doesn't draw as it should)
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(net::root().join("errors.txt")) {
                    let _ = writeln!(f, "{} drawing: {e}", net::now_ms());
                }
            }
        }
        // (the pointer's hover looked at again over what's now drawn: a sheet that just opened un-lights what's under it)
        if self.kit.refresh_hover() {
            self.kit.dirty = true;
        }
        // (asked again after the layout: something that started moving while it was laid out (a delete growing into its
        // pill) has its next frames too)
        if every_frame || (self.setup.is_none() && self.player.is_none() && self.pages.animating()) {
            self.kit.dirty = true;
        }
    }

    fn edit(&mut self, vk: u16, ch: Option<char>) -> bool {
        let Some((change, submit)) = self.kit.field_spec() else {
            return false;
        };
        if vk == key::RETURN {
            if let Some(f) = submit {
                f(self);
            }
            return true;
        }
        let window = self.window;
        let Some(edit) = self.kit.focused() else {
            return false;
        };
        let mut text: Vec<_> = edit.text.encode_utf16().collect();
        let ctrl = platform::ctrl_down();
        let shift = platform::shift_down();
        let pos = edit.caret.min(text.len());
        let anchor = edit.anchor.min(text.len());
        let (lo, hi) = (pos.min(anchor), pos.max(anchor));
        let previous = |i: usize| {
            let prev = i.saturating_sub(1);
            if prev > 0 && (0xDC00..=0xDFFF).contains(&text[prev]) {
                prev - 1
            } else {
                prev
            }
        };
        let next = |i: usize| {
            if i < text.len() && (0xD800..=0xDBFF).contains(&text[i]) {
                (i + 2).min(text.len())
            } else {
                (i + 1).min(text.len())
            }
        };
        let mut range = lo..hi;
        let mut insert = Vec::new();
        let mut replace = false;

        match vk {
            key::A if ctrl => {
                edit.anchor = 0;
                edit.caret = text.len();
            }
            key::C | key::X if ctrl => {
                if lo != hi {
                    window.set_clipboard(&text[lo..hi]);
                }
                replace = vk == key::X;
            }
            key::V if ctrl => {
                if let Some(paste) = window.clipboard() {
                    insert = paste.into_iter().filter(|c| *c >= 32).take(4096).collect();
                    replace = true;
                }
            }
            key::HOME | key::END | key::LEFT | key::RIGHT => {
                let caret = if vk == key::HOME {
                    0
                } else if vk == key::END {
                    text.len()
                } else if vk == key::LEFT {
                    if !shift && lo != hi {
                        lo
                    } else {
                        previous(pos)
                    }
                } else if !shift && lo != hi {
                    hi
                } else {
                    next(pos)
                };
                edit.caret = caret;
                if !shift {
                    edit.anchor = caret;
                }
            }
            key::BACK | key::DELETE => {
                if lo == hi {
                    range = if vk == key::BACK {
                        previous(pos)..pos
                    } else {
                        pos..next(pos)
                    };
                }
                replace = true;
            }
            _ => {
                if let Some(ch) = ch.filter(|c| !c.is_control()) {
                    insert.extend_from_slice(ch.encode_utf16(&mut [0; 2]));
                    replace = true;
                }
            }
        }
        if replace {
            let caret = range.start + insert.len();
            text.splice(range, insert);
            edit.text = String::from_utf16_lossy(&text);
            edit.caret = caret;
            edit.anchor = caret;
            if let Some(f) = change {
                f(self);
            }
        }
        self.kit.dirty = true;
        true
    }

    /// What the window says, done: false leaves it to the OS.
    fn handle(&mut self, event: Event) -> bool {
        let s = self.kit.g.scale;
        match event {
            Event::Wake => {
                UPDATE_POSTED.store(false, Ordering::Release);
                while let Ok(change) = self.inbox.try_recv() {
                    change(self);
                    if !QUIET.with(|q| q.replace(false)) {
                        self.kit.dirty = true;
                        self.pages.changes += 1;
                    }
                }
                // (the core's view changed what's open: a menu or sheet's way out starts in this very frame)
                self.pages.follow_layers();
            }
            Event::Resized { minimised } => {
                if minimised != self.minimised {
                    core_screen::app(zcore::screen::AppEvent::Lifecycle { foreground: !minimised });
                }
                self.minimised = minimised;
                self.kit.hold_pictures(self.minimised);
                self.resize();
            }
            Event::Frame => {
                self.window.stop_frame_timer();
                self.frame_pending = false;
                if !self.minimised {
                    self.redraw();
                    // (the sign-in card folding: the next frame at once, not on the next tick)
                    if self.pages.folding() {
                        self.kit.dirty = true;
                    }
                }
            }
            Event::Tick => {
                self.kit.tick();
                pages::Pages::follow_updater(self);
                // (the core's say (maintenance, Home's showing, the layers) is followed under the player too)
                pages::Pages::follow_session(self);
                pages::Pages::follow_nav(self);
                if self.player.is_some() {
                    player::tick(self);
                    self.pages.follow_core(&mut self.kit);
                } else {
                    self.pages.tick(&mut self.kit);
                }
            }
            Event::PointerMoved { x, y } => {
                let (x, y) = (x / s, y / s);
                // (Windows sends a move where the pointer already is as the cursor's hidden: not the viewer stirring)
                let stirred = self.kit.pointer != (x, y);
                self.kit.moved(x, y);
                if let Some((drag, at)) = self.kit.dragged(x, y) {
                    drag(self, at, false);
                    self.pages.changes += 1;
                }
                if stirred {
                    player::activity(self);
                }
            }
            Event::PointerLeft => {
                self.kit.moved(-1., -1.);
            }
            Event::PointerDown { x, y } => {
                self.pages.changes += 1;
                if let Some((drag, at)) = self.kit.down(x / s, y / s) {
                    drag(self, at, false);
                }
            }
            Event::PointerUp { x, y } => {
                self.pages.changes += 1;
                let (click, drag) = self.kit.up(x / s, y / s);
                if let Some((drag, at)) = drag {
                    drag(self, at, true);
                }
                // (a "click again to delete" waiting: a click anywhere else lets it go; the button itself deletes, and
                // another delete asks in its place, so only a click that left it as it was puts it away)
                // (the core's navigation lets it go first; the button asking still reads it as it was, and asks again after)
                if self.pages.nav.view().asking.is_some() {
                    self.pages.nav.send(zcore::nav::Event::Unask);
                }
                if let Some(click) = click {
                    click(self);
                }
                self.kit.dirty = true;
            }
            Event::RightUp { x, y } if self.player.is_none() => {
                if let Some(context) = self.kit.context_at(x / s, y / s) {
                    self.pages.changes += 1;
                    context(self);
                    self.kit.dirty = true;
                }
            }
            Event::RightUp { .. } => return false,
            Event::Wheel { x, y, delta, horizontal, ctrl } => {
                let (x, y) = (x / s, y / s);
                // (the pointer known again where it is: coming back from another window, nothing had moved it yet)
                self.kit.moved(x, y);
                self.pages.scrolled();
                // (Ctrl with the wheel zooms the reader, as a browser's page)
                if horizontal || !(player::wheel(self, delta) || ctrl && pages::Pages::reader_zoom_wheel(self, delta, y)) {
                    let _ = self.kit.wheel(delta, x, y, horizontal);
                }
            }
            Event::Char(u) => {
                self.pages.changes += 1;
                if (0xD800..=0xDBFF).contains(&u) {
                    self.high_surrogate = Some(u);
                } else {
                    let high = self.high_surrogate.take();
                    let ch = if (0xDC00..=0xDFFF).contains(&u) {
                        high.and_then(|h| char::decode_utf16([h, u]).next().and_then(|c| c.ok()))
                    } else {
                        char::from_u32(u as u32)
                    };
                    self.edit(0, ch);
                }
            }
            Event::Key(vk) => {
                self.pages.changes += 1;
                if vk == key::TAB && self.player.is_none() {
                    self.kit.tab(platform::shift_down());
                } else if vk == key::ESCAPE {
                    if player::dismiss_panel(self) {
                    } else if self.fullscreen.is_some() {
                        self.toggle_fullscreen();
                    } else if self.player.is_some() {
                        self.close_player();
                    } else {
                        pages::Pages::go_back(self);
                    }
                    self.kit.dirty = true;
                } else if !self.edit(vk, None) && !pages::Pages::reader_keydown(self, vk) && !pages::Pages::your_tv_keydown(self, vk) {
                    player::key(self, vk);
                }
            }
            Event::Close => {
                self.exit_save = player::leaving(self);
                return false;
            }
        }
        true
    }
}

/// The window's events, to the app when it isn't busy (an event that comes while it's handling another is the OS's).
fn on_event(event: Event) -> bool {
    APP.with(|slot| {
        let Ok(mut slot) = slot.try_borrow_mut() else {
            return false;
        };
        slot.as_mut().is_some_and(|app| app.handle(event))
    })
}

/// The strip along the top drags the window, wherever nothing clickable is.
fn drags_window(x: f32, y: f32) -> bool {
    y < WINDOW_STRIP && APP.with(|slot| slot.try_borrow().ok().and_then(|s| s.as_ref().map(|app| app.kit.hot_at(x, y)))) == Some(false)
}

/// After each of the window's messages: anything changed is drawn on the frame timer (or now, without one).
fn after_message() {
    APP.with(|slot| {
        if let Some(app) = slot.borrow_mut().as_mut() {
            if (app.kit.dirty || app.kit.has_uploads()) && !app.frame_pending && !app.minimised {
                app.frame_pending = app.window.start_frame_timer();
                if !app.frame_pending {
                    app.redraw();
                }
            }
        }
    });
}

fn run() -> platform::Result<()> {
    // zunflix's own setup: making one (the build), taking the app away, or being one
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--make-setup") {
        return install::make(&args[i + 1], &args[i + 2]).map_err(|e| platform::Error::new(e.to_string()));
    }
    // (taking zunflix away: the uninstaller, in the bootstrapper's window; run from zunflix's own folder it first
    // hands over to a copy of itself in Temp, which can take that folder away)
    let uninstalling = args.iter().any(|a| a == "--uninstall");
    if uninstalling && install::relaunch_from_temp(&args) {
        return Ok(());
    }
    // (`--preview-setup`: the bootstrapper's window playing an install that touches nothing, to look at it)
    let preview = args.iter().any(|a| a == "--preview-setup");
    let preview_removing = args.iter().any(|a| a == "--preview-uninstall");
    let installing = install::is_setup() || uninstalling || preview || preview_removing;
    if !installing && install::place() {
        return Ok(());
    }
    if install::is_setup() && args.iter().any(|a| a == "/S") {
        let pid = args.iter().find_map(|a| a.strip_prefix("/UPDATE-PID=")).and_then(|p| p.parse().ok());
        install::install_silently(pid);
        return Ok(());
    }
    let callbacks = Callbacks { event: on_event, drags: drags_window, idle: after_message };
    let Some(window) = Window::open(Opening { setup: installing, uninstalling, small: (BOOT_W, BOOT_H) }, callbacks)? else {
        return Ok(());
    };
    let local = install::assets_dir().join("fonts");
    let fonts = if installing {
        install::own_assets().unwrap_or(local)
    } else if local.is_dir() {
        local
    } else {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts")
    };
    window.set_icon(&fonts.join("../icon.ico"));
    let kit = Kit::new(&window, 1., 1180., 780., &fonts)?;
    let (tx, inbox) = mpsc::channel();
    let _ = UI.set((tx, window.waker()));
    // (before the session: it hears of the account from the first)
    profile_settings::start();
    downloads::start();
    let mut app = App {
        kit,
        window,
        inbox,
        pages: pages::Pages::default(),
        session: core_screen::CoreScreen::new(|a| &mut a.session, |_, _| {}),
        updater: core_screen::CoreScreen::new(|a| &mut a.updater, |_, _| {}),
        player: None,
        play: core_screen::CoreScreen::new(|a| &mut a.play, player::command),
        setup: installing.then(|| if uninstalling { setup::Setup::uninstaller() } else if preview { setup::Setup::preview() } else if preview_removing { setup::Setup { removing: true, kept: 2_800_000_000, ..setup::Setup::preview() } } else { setup::Setup::new() }),
        fullscreen: None,
        high_surrogate: None,
        exit_save: None,
        frame_pending: false,
        minimised: false,
    };
    app.resize();
    app.redraw();
    APP.with(|slot| *slot.borrow_mut() = Some(app));
    // (it opens maximised, filling the screen; the setup at its own size)
    if installing {
        window.show();
    } else {
        window.hold_hidden();
    }
    // then, maximised, its size read again and the splash begun: it plays at the size it's seen at, never moved
    // part-way through by the window growing
    APP.with(|slot| if let Some(app) = slot.borrow_mut().as_mut() {
        app.resize();
        if installing {
            // (the first install is the bootstrapper: the splash from now, installing on its own)
            if let Some(s) = app.setup.as_mut() { s.opened = std::time::Instant::now(); }
            // (the uninstaller waits on its own confirm screen: Uninstall starts it)
            if !app.setup.as_ref().is_some_and(|s| s.removing) {
                setup::start(app);
            }
        } else {
            // (held back until the update check answers: the splash starts as the window shows, show_opened)
            if !window.is_held_hidden() {
                app.pages.start();
            }
            app.session.send(zcore::session::Event::Started);
            app.updater.send(zcore::update::Event::Opened);
        }
        app.kit.dirty = true;
        app.redraw();
    });
    window.to_front();
    window.start_ticks();
    window.run();
    let save = APP.with(|slot| {
        slot.borrow_mut()
            .take()
            .and_then(|mut app| app.exit_save.take())
    });
    if let (Some(save), Some(runtime)) = (save, RUNTIME.get()) {
        runtime.block_on(async {
            let _ = tokio::time::timeout(std::time::Duration::from_millis(zcore::cache::EXIT_SAVE_WAIT_MS), save).await;
        });
    }
    // (settings changed in the last moment, written before the app's gone)
    net::flush_files();
    Ok(())
}

fn main() {
    // (a panic leaves its story in crash.txt beside the app's data, to be read when something goes wrong)
    std::panic::set_hook(Box::new(|info| {
        let _ = std::fs::write(net::root().join("crash.txt"), format!("{info}

{}", std::backtrace::Backtrace::force_capture()));
    }));
    if let Err(e) = run() {
        platform::alert(&e.to_string());
    }
}
