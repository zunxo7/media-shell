//! zunflix's own setup screen: the bootstrapper's small window installing the app (the zf splash, one bar, then
//! zunflix opens), playing an install to look at (`--preview-setup`), or taking zunflix away (`--uninstall`). How the
//! files get on and off the computer is the OS's (`platform::install`).
use crate::kit::{col, El};
use crate::platform::{install, Shell};
use crate::tokens as t;
use crate::{ui as post, App};
use std::path::Path;
use std::time::Instant;

// ---------- installing ----------
pub use zcore::setup::Phase;
use zcore::setup as words;

pub struct Setup {
    pub phase: Phase,
    pub share: f32,
    pub error: String,
    pub opened: Instant,
    /// Taking zunflix off this PC (the uninstaller), not putting it on.
    pub removing: bool,
    /// Files taken away so far, of how many.
    pub files: (usize, usize),
    /// Only showing how it looks (`--preview-setup`): an install played over and over, nothing touched.
    pub preview: bool,
    /// Uninstalling with downloaded films and episodes on this PC: how many bytes (a box to delete them too).
    pub kept: u64,
    /// That box ticked: the downloads go too, at the end.
    pub delete_downloads: bool,
}

impl Setup {
    pub fn new() -> Setup {
        Setup { phase: Phase::Ready, share: 0., error: String::new(), opened: Instant::now(), removing: false, files: (0, 0), preview: false, kept: 0, delete_downloads: false }
    }
    /// The bootstrapper to look at: an install played over and over (`--preview-setup`).
    pub fn preview() -> Setup {
        Setup { preview: true, ..Setup::new() }
    }
    /// The uninstaller: the same small window. Asked from Settings it's already been said yes to.
    pub fn uninstaller() -> Setup {
        Setup { removing: true, kept: folder_bytes(&install::data_dir().join("Downloads")), ..Setup::new() }
    }

    /// The bootstrapper: zunflix's icon, what it's doing, how far (megabytes and a bar); it starts on its own,
    /// nothing to press, then zunflix opens. The same small window an update shows.
    pub fn view(&self, app: &App) -> El {
        if self.removing {
            return self.uninstall_view(app);
        }
        let total = if self.preview { 14.2e6 } else { install::size() as f32 };
        // (an update's last part: the same window, title and bar the app showed downloading it, carried on from where it
        // got to: one update, not a second one)
        let said = words::installing(self.phase, install::updating().is_some(), self.share, f64::from(total));
        let mut v = crate::ui::bootstrap_view(app, &said.title, &said.detail, said.share);
        if !self.error.is_empty() {
            v = v.kid(col().at(24., f32::NAN, 24., 22.).items_center().gap(12.)
                .kid(crate::ds::error_text(&self.error, true))
                .kid(crate::ds::button(app, "setup-retry", crate::ds::Kind::Primary, crate::ds::Size::S, None, words::TRY_AGAIN, start)));
        }
        v
    }
}

impl Setup {
    /// The uninstaller, as the bootstrapper: asked first (opened from Windows' installed apps), then what it's doing
    /// and how far, then done.
    fn uninstall_view(&self, app: &App) -> El {
        // (one panel, asking then doing: the words change in place, the bar fades in where its room was kept, and the
        // downloads box and the buttons fade away; the downloads are decided once Uninstall is pressed)
        let asking = self.phase == Phase::Ready;
        let said = words::uninstalling(self.phase, self.files, self.share);
        let mut v = crate::ui::bootstrap_view_over(app, &said.title, &said.detail, said.share, 0.);
        let fade = |e: El| { let e = e.opacity(if asking { 1. } else { 0. }).ease(180); if asking { e } else { e.decorative() } };
        if self.kept > 0 {
            let on = self.delete_downloads;
            let hot = app.kit.hover_within(crate::kit::k("uninstall-delete-downloads"));
            let tick = col().size(18., 18.).radius(5.).center().no_shrink()
                .bg(if on { t::C_WHITE } else if hot { crate::ui::w(0.1) } else { 0 }).border(if on { 0. } else { 1.5 }, crate::ui::w(if hot { 0.5 } else { 0.32 }))
                .maybe(on.then(|| crate::kit::icon("check", 13., t::C_BLACK)));
            v = v.kid(fade(crate::kit::row().key("uninstall-box-row").at(0., f32::NAN, 0., 78.).center().kid(crate::ui::tap(
                crate::kit::row().key("uninstall-delete-downloads").items_center().gap(9.).kid(tick)
                    .kid(crate::ui::words(words::delete_downloads(self.kept), crate::gfx::Font::Medium, 13., crate::ui::w(t::W75))),
                t::PRESS_CHIP,
                |a| if let Some(s) = a.setup.as_mut() { if s.phase == Phase::Ready { s.delete_downloads = !s.delete_downloads; a.kit.dirty = true; } },
            ))));
        }
        v.kid(fade(crate::kit::row().key("uninstall-buttons").at(24., f32::NAN, 24., 22.).gap(10.)
            .kid(crate::ds::button(app, "uninstall-cancel", crate::ds::Kind::Ghost, crate::ds::Size::M, None, words::CANCEL, |a| {
                if a.setup.as_ref().is_some_and(|s| s.phase != Phase::Ready) { return; }
                a.window.close();
            }).grow())
            .kid(crate::ds::button(app, "uninstall-go", crate::ds::Kind::Primary, crate::ds::Size::M, None, words::UNINSTALL, start).grow())))
    }
}

/// Everything in a folder, in bytes.
fn folder_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir).map(|entries| entries.flatten().map(|e| {
        let p = e.path();
        if p.is_dir() { folder_bytes(&p) } else { e.metadata().map(|m| m.len()).unwrap_or(0) }
    }).sum()).unwrap_or(0)
}


pub fn start(app: &mut App) {
    let Some(s) = app.setup.as_mut() else { return };
    if s.phase != Phase::Ready { return; }
    if s.preview && s.removing {
        // (an uninstall played: files across in about 3 s, then removed with 2.8 GB of downloads kept; nothing touched)
        s.phase = Phase::Installing;
        std::thread::spawn(|| {
            for i in 0..=140 {
                std::thread::sleep(std::time::Duration::from_millis(30));
                post(move |a| if let Some(s) = a.setup.as_mut() { s.files = (i, 140); s.share = i as f32 / 140.; });
            }
            post(|a| if let Some(s) = a.setup.as_mut() { s.phase = Phase::Done; });
            std::thread::sleep(std::time::Duration::from_millis(2200));
            post(|a| if let Some(s) = a.setup.as_mut() { s.phase = Phase::Ready; s.share = 0.; s.files = (0, 0); });
        });
        return;
    }
    if s.preview {
        // (an install played: the bar across in about 5 s, a moment done, then again; nothing on disk touched)
        s.phase = Phase::Installing;
        std::thread::spawn(|| loop {
            for i in 0..=100 {
                std::thread::sleep(std::time::Duration::from_millis(50));
                post(move |a| if let Some(s) = a.setup.as_mut() { s.phase = Phase::Installing; s.share = i as f32 / 100.; });
            }
            post(|a| if let Some(s) = a.setup.as_mut() { s.phase = Phase::Done; });
            std::thread::sleep(std::time::Duration::from_millis(1800));
        });
        return;
    }
    if s.removing {
        s.phase = Phase::Installing;
        std::thread::spawn(|| {
            install::uninstall(|n, all| post(move |a| if let Some(s) = a.setup.as_mut() {
                s.files = (n, all);
                s.share = words::files_share(n, all);
            }));
            post(move |a| {
                if let Some(s) = a.setup.as_mut() { s.phase = Phase::Done; }
                // (a moment on Removed, and never under 4 s in all, so the downloads box can be ticked; ticked, the
                // downloads go now, then it closes)
                let wait = words::removed_wait_ms(a.setup.as_ref().map_or(0, |s| s.opened.elapsed().as_millis() as u64));
                crate::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
                    post(|a| {
                        if a.setup.as_ref().is_some_and(|s| s.delete_downloads) { let _ = std::fs::remove_dir_all(install::data_dir()); }
                        install::clean_up_after_uninstall();
                        a.window.close();
                    });
                });
            });
        });
        return;
    }
    s.phase = Phase::Installing;
    s.error.clear();
    std::thread::spawn(move || {
        // (the bar never fills faster than across in `words::MIN_SHOW_MS`, so a quick install still shows itself: the real share
        // kept here, what's shown fed every 30 ms by a ticker, and zunflix opened once it's shown full)
        let real = std::sync::Arc::new(std::sync::Mutex::new(0f32));
        let began = Instant::now();
        let shown_real = real.clone();
        let ticker = std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(30));
            let r = *shown_real.lock().unwrap();
            let shown = words::shown_share(r, began.elapsed().as_millis() as f32);
            post(move |a| if let Some(s) = a.setup.as_mut() { s.share = shown });
            if shown >= 1. || r < 0. { break; }
        });
        let result = install::install(install::updating(), |share| *real.lock().unwrap() = share);
        // (failed: the ticker stops where it is; done: it runs on to full)
        if result.is_err() { *real.lock().unwrap() = -1.; } else { *real.lock().unwrap() = 1.; }
        let _ = ticker.join();
        post(move |a| {
            let Some(s) = a.setup.as_mut() else { return };
            match result {
                Ok(exe) => {
                    s.phase = Phase::Done;
                    crate::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(words::OPEN_AFTER_MS)).await;
                        let _ = std::process::Command::new(&exe).spawn();
                        post(|a| a.window.close());
                    });
                }
                Err(e) => {
                    s.phase = Phase::Ready;
                    s.error = e;
                }
            }
        });
    });
}
