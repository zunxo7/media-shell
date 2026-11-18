//! The shared core (decision 0008) on its own thread: the root and every screen on it live there, never on the
//! window's thread. A page draws its [`CoreScreen::view`] and gives the core its events with [`CoreScreen::send`]; the
//! effects the core asks for run here with this app's own I/O, their answers go back to the core's thread, and each
//! new view is posted to the window.
use crate::{data, net, spawn, ui, App};
use std::collections::HashMap;
use std::sync::{mpsc, Arc, OnceLock};
use std::time::Duration;
use zcore::root::{Order, Root, ScreenId, Turn};
use zcore::screen::{AppEvent, Clock, Device, Effect, Platform, Reply, Screen};
use zcore::store::{Need, PageReply};

type Job = Box<dyn FnOnce(&mut Core) + Send>;

/// The root and what's under way for it, owned by the core's thread.
struct Core {
    root: Root,
    screens: HashMap<ScreenId, Hooks>,
    /// Pages the app's own code waits for, by ticket.
    tickets: HashMap<u64, tokio::sync::oneshot::Sender<PageReply>>,
    next_ticket: u64,
    /// Requests and timers under way, by effect id, so a Cancel can stop them.
    running: HashMap<u32, tokio::task::AbortHandle>,
}

/// How a live screen's view reaches the window, and where its page-level effects (play, open a page) go.
struct Hooks {
    draw: Box<dyn Fn(&Root) + Send>,
    outside: fn(&mut App, Effect),
}

/// Something for the core's thread to do, in order; the thread starts the first time.
fn on(job: impl FnOnce(&mut Core) + Send + 'static) {
    static CORE: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
    let jobs = CORE.get_or_init(|| {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let _ = std::thread::Builder::new().name("core".into()).spawn(move || {
            let root = Root::new(platform(), net::base().into(), clock());
            let mut core = Core { root, screens: HashMap::new(), tickets: HashMap::new(), next_ticket: 1, running: HashMap::new() };
            while let Ok(job) = inbox.recv() {
                job(&mut core);
            }
        });
        jobs
    });
    let _ = jobs.send(Box::new(job));
}

/// The server page at `path`, kept under `key`, from the core's one store (`zcore::store`): read from this PC and
/// asked of the server once however many want it, by the cache rules. The core's failure when there's no copy at all.
pub async fn page(path: &str, key: &str, need: Need) -> net::Res<String> {
    let (answer, waiting) = tokio::sync::oneshot::channel();
    let (path, key) = (path.to_string(), key.to_string());
    on(move |core| {
        let ticket = core.next_ticket;
        core.next_ticket += 1;
        core.tickets.insert(ticket, answer);
        let turn = core.root.page(&path, &key, need, ticket, clock());
        core.run(turn);
    });
    let unreachable = || net::ApiError { message: zcore::api::UNREACHABLE.into(), status: 0 };
    let page = waiting.await.map_err(|_| unreachable())?;
    match (page.body, page.failure) {
        (Some(body), _) => Ok(body),
        (None, Some(failure)) => Err(net::ApiError { message: failure.words.clone(), status: failure.status() }),
        (None, None) => Err(unreachable()),
    }
}

/// Every page read again when next wanted (signed out).
pub fn forget_pages() {
    on(|core| core.root.forget_pages());
}

/// Something happened to the whole app (minimised, the network): every screen is told.
pub fn app(event: AppEvent) {
    on(move |core| {
        let turn = core.root.app(event, clock());
        core.run(turn);
    });
}

impl Core {
    fn run(&mut self, turn: Turn) {
        for screen in &turn.changed {
            if let Some(hooks) = self.screens.get(screen) {
                (hooks.draw)(&self.root);
            }
        }
        for Order { screen, effect } in turn.orders {
            self.carry(screen, effect);
        }
        // (the core's screens hear of changed pages themselves)
        for delivery in turn.delivered {
            if let Some(answer) = self.tickets.remove(&delivery.ticket) {
                let _ = answer.send(delivery.page);
            }
        }
    }

    fn carry(&mut self, screen: ScreenId, effect: Effect) {
        match effect {
            Effect::Request { id, request } => {
                let task = spawn(async move {
                    let reply = net::send(request).await;
                    on(move |core| core.answer(id, reply));
                });
                self.running.insert(id, task.abort_handle());
            }
            Effect::Fetch { id, request, file } => {
                let task = spawn(async move {
                    let reply = net::fetch(request, &file).await;
                    on(move |core| core.answer(id, reply));
                });
                self.running.insert(id, task.abort_handle());
            }
            Effect::Timer { id, ms } => {
                let task = spawn(async move {
                    tokio::time::sleep(Duration::from_millis(ms.into())).await;
                    on(move |core| core.answer(id, Reply::Fired));
                });
                self.running.insert(id, task.abort_handle());
            }
            Effect::Cancel { id } => {
                if let Some(task) = self.running.remove(&id) {
                    task.abort();
                }
            }
            Effect::ReadPref { id, key } => {
                let value = net::pref_get(&key);
                on(move |core| core.answer(id, Reply::Pref { value }));
            }
            Effect::WritePref { key, value } => net::pref_put(&key, value),
            Effect::Load { id, key } => {
                spawn(async move {
                    let reply = data::kept(&key);
                    on(move |core| core.answer(id, reply));
                });
            }
            Effect::Store { key, body, etag } => {
                spawn(async move { data::keep(&key, &body, etag.as_deref()) });
            }
            Effect::Report { place, message } => net::report_error("error", "core", &message, "", &place),
            Effect::Disk { id, op } => {
                spawn(async move {
                    let disk = tokio::task::spawn_blocking(move || crate::disk::run(op)).await.unwrap_or_default();
                    on(move |core| core.answer(id, Reply::Disk { disk }));
                });
            }
            Effect::SignedOut => ui(|app| app.signed_out()),
            // (the last profile's spots let go: this one's are read as they're wanted)
            Effect::Profile { .. } => data::progress_forget(),
            Effect::Forget => ui(|app| app.forget_account()),
            // (the root answers and tells these itself)
            Effect::Page { .. } | Effect::Tell { .. } => {}
            // (a PC is never the account's TV)
            Effect::Tv { .. } => {}
            outside @ (Effect::Play { .. } | Effect::Open { .. } | Effect::Warm { .. } | Effect::Player { .. }) => {
                // (the player's last save: the app may be closing, its window no longer answering)
                if let Effect::Player { order: zcore::play::Command::Watched } = &outside {
                    crate::player::watched();
                }
                if let Some(go) = self.screens.get(&screen).map(|hooks| hooks.outside) {
                    ui(move |app| go(app, outside));
                }
            }
        }
    }

    fn answer(&mut self, id: u32, reply: Reply) {
        self.running.remove(&id);
        let turn = self.root.answer(id, reply, clock());
        self.run(turn);
    }
}

/// A screen whose logic is in the core, as the window holds it: its latest view, and the way to its model on the
/// core's thread.
pub struct CoreScreen<S: Screen> {
    /// Set on the core's thread once the screen is open there; every later job finds it set, as jobs run in order.
    id: Arc<OnceLock<ScreenId>>,
    view: S::View,
}

impl<S> CoreScreen<S>
where
    S: Screen + Default + Send + 'static,
    S::Event: Send + 'static,
    S::View: Default + Send + 'static,
{
    /// The screen opened on the core. `at` is where it lives in the app (its views are posted there); `outside` takes
    /// what only the app's pages can do (play, open a page).
    pub fn new(at: fn(&mut App) -> &mut CoreScreen<S>, outside: fn(&mut App, Effect)) -> Self {
        let id = Arc::new(OnceLock::new());
        let opened = Arc::clone(&id);
        on(move |core| {
            let screen = core.root.open(S::default());
            let _ = opened.set(screen);
            let draw = move |root: &Root| {
                if let Some(view) = root.view::<S>(screen) {
                    ui(move |app| at(app).view = view);
                }
            };
            draw(&core.root);
            core.screens.insert(screen, Hooks { draw: Box::new(draw), outside });
        });
        Self { id, view: S::View::default() }
    }

    /// A screen that lasts as long as the app, its views handed to `drawn` on the core's thread, for state the app
    /// reads from any thread. It asks for nothing only the pages can do.
    pub fn lasting(drawn: fn(S::View)) -> Self {
        let id = Arc::new(OnceLock::new());
        let opened = Arc::clone(&id);
        on(move |core| {
            let screen = core.root.open(S::default());
            let _ = opened.set(screen);
            let draw = move |root: &Root| {
                if let Some(view) = root.view::<S>(screen) {
                    drawn(view);
                }
            };
            draw(&core.root);
            core.screens.insert(screen, Hooks { draw: Box::new(draw), outside: |_, _| {} });
        });
        Self { id, view: S::View::default() }
    }

    /// Exactly what to draw.
    pub fn view(&self) -> &S::View {
        &self.view
    }

    /// Something happened: the core updates on its thread, and what it asks for is set going.
    pub fn send(&self, event: S::Event) {
        let id = Arc::clone(&self.id);
        on(move |core| {
            if let Some(&screen) = id.get() {
                let turn = core.root.update::<S>(screen, event, clock());
                core.run(turn);
            }
        });
    }
}

impl<S: Screen> Drop for CoreScreen<S> {
    /// The page holding it has gone (signed out, a new profile): its requests and timers end with it.
    fn drop(&mut self) {
        let id = Arc::clone(&self.id);
        on(move |core| {
            if let Some(&screen) = id.get() {
                core.screens.remove(&screen);
                let turn = core.root.close(screen, clock());
                core.run(turn);
            }
        });
    }
}

/// A screen that lives as long as the app, not on any page (the downloads queue): each new view goes to `show`, on
/// the core's thread.
pub struct Lasting<S: Screen> {
    id: Arc<OnceLock<ScreenId>>,
    screen: std::marker::PhantomData<fn(S)>,
}

impl<S> Lasting<S>
where
    S: Screen + Default + Send + 'static,
    S::Event: Send + 'static,
    S::View: Send + 'static,
{
    pub fn new(show: fn(S::View)) -> Self {
        let id = Arc::new(OnceLock::new());
        let opened = Arc::clone(&id);
        on(move |core| {
            let screen = core.root.open(S::default());
            let _ = opened.set(screen);
            let draw = move |root: &Root| {
                if let Some(view) = root.view::<S>(screen) {
                    show(view);
                }
            };
            draw(&core.root);
            core.screens.insert(screen, Hooks { draw: Box::new(draw), outside: |_, _| {} });
        });
        Self { id, screen: std::marker::PhantomData }
    }

    /// Something happened: the core updates on its thread, and what it asks for is set going.
    pub fn send(&self, event: S::Event) {
        let id = Arc::clone(&self.id);
        on(move |core| {
            if let Some(&screen) = id.get() {
                let turn = core.root.update::<S>(screen, event, clock());
                core.run(turn);
            }
        });
    }
}

/// This PC and app, as the core is told once.
fn platform() -> Platform {
    Platform {
        device: Device::Desktop,
        model: net::MODEL.into(),
        os: net::os_name().into(),
        version_name: net::VERSION.into(),
        version_code: net::VERSION_CODE,
        stream: net::stream_id().into(),
        // (mpv plays every codec, so every one is claimed: HDR too; a test run may name fewer, ZUNFLIX_CODECS)
        codecs: std::env::var("ZUNFLIX_CODECS")
            .map(|v| v.split(',').map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect())
            .unwrap_or_else(|_| crate::playback::CODECS.iter().map(|c| c.to_string()).collect()),
        hdr: std::env::var("ZUNFLIX_CODECS").map_or(true, |v| v.contains("hdr")),
    }
}

/// The time now and this PC's offset from UTC, as every call into the core carries.
fn clock() -> Clock {
    Clock { now: net::now_ms() as f64 / 1000., utc_offset: crate::platform::utc_offset_seconds() }
}
